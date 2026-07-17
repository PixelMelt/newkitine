mod db;
mod policy;
mod state;

pub use state::Behavior;

use std::sync::Arc;

use crate::types::{FilterLevel, Restriction};

use db::has_downloaded_from;

use super::peer_history::{load_verdicts, set_user_verdict};
use policy::{
    CONTRADICTION_MIN_FILES, PRESET_STATS, QUEUE_FLOOD, SEARCH_FLOOD, Verdict, restriction_for,
    restriction_str,
};
use state::{Check, Peer, prune, touch};

use super::db::fatal;
use super::state::{App, now};

fn policy(app: &App) -> (FilterLevel, String) {
    app.settings.behavior_policy()
}

fn is_self(app: &App, username: &str) -> bool {
    app.projection.read().session.status().username == username
}

async fn exempt(app: &Arc<App>, username: &str) -> bool {
    if app.projection.read().users.is_buddy(username) {
        return true;
    }
    has_downloaded_from(&app.db, username).await
}

async fn sync(app: &Arc<App>, username: &str) {
    let (level, denied_message) = policy(app);
    let (verdict, evidence) = {
        let peers = app.behavior.peers.lock().unwrap();
        let peer = &peers[username];
        (peer.verdict, peer.evidence.clone())
    };
    let restriction = restriction_for(level, verdict, &denied_message);
    let restriction_label = restriction_str(&restriction);
    set_user_verdict(
        &app.db,
        username,
        verdict.as_str(),
        &evidence.join(","),
        restriction_label,
        now(),
    )
    .await
    .unwrap_or_else(|error| fatal(error));
    app.client.set_user_restriction(username, restriction).await;
}

fn mark_verified(app: &App, username: &str) {
    let mut peers = app.behavior.peers.lock().unwrap();
    let peer = touch(&mut peers, username, now());
    peer.verdict = Verdict::Verified;
    peer.evidence.clear();
    peer.searches.clear();
    peer.queue_requests.clear();
    peer.check = Check::Idle;
}

async fn convict(app: &Arc<App>, username: &str, verdict: Verdict, evidence: &str) {
    if exempt(app, username).await {
        mark_verified(app, username);
        sync(app, username).await;
        return;
    }
    if app.projection.read().users.is_buddy(username) {
        mark_verified(app, username);
    } else {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        if peer.verdict < verdict {
            peer.verdict = verdict;
        }
        if !peer.evidence.iter().any(|entry| entry == evidence) {
            peer.evidence.push(evidence.to_owned());
        }
        peer.check = Check::Idle;
    }
    sync(app, username).await;
}

pub async fn search_seen(app: &Arc<App>, username: &str) {
    let _transition = app.behavior.transition.lock().await;
    if is_self(app, username) {
        return;
    }
    let flooded = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let timestamp = now();
        let peer = touch(&mut peers, username, timestamp);
        peer.searches.push_back(timestamp);
        prune(&mut peer.searches, timestamp);
        peer.searches.len() > SEARCH_FLOOD && peer.verdict < Verdict::Suspect
    };
    if flooded {
        convict(app, username, Verdict::Suspect, "search-flood").await;
    }
}

pub async fn queue_request(app: &Arc<App>, username: &str) {
    let _transition = app.behavior.transition.lock().await;
    if is_self(app, username) {
        return;
    }
    let (level, _) = policy(app);
    enum Action {
        None,
        Flood,
        Probe { hold: bool },
    }
    let action = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let timestamp = now();
        let peer = touch(&mut peers, username, timestamp);
        peer.queue_requests.push_back(timestamp);
        prune(&mut peer.queue_requests, timestamp);
        if peer.queue_requests.len() > QUEUE_FLOOD && peer.verdict < Verdict::Suspect {
            Action::Flood
        } else if peer.verdict == Verdict::Clean
            && peer.check == Check::Idle
            && peer.stats.is_none()
            && level != FilterLevel::Open
        {
            peer.check = Check::AwaitingStats;
            Action::Probe {
                hold: level == FilterLevel::Strict,
            }
        } else {
            Action::None
        }
    };
    match action {
        Action::Flood => convict(app, username, Verdict::Suspect, "queue-flood").await,
        Action::Probe { hold } => {
            if exempt(app, username).await {
                let mut peers = app.behavior.peers.lock().unwrap();
                let peer = touch(&mut peers, username, now());
                peer.check = Check::Idle;
                peer.verdict = Verdict::Verified;
                return;
            }
            if hold {
                app.client
                    .set_user_restriction(username, Restriction::Hold)
                    .await;
            }
            app.client.request_user_stats(username).await;
        }
        Action::None => {}
    }
}

pub async fn stats_received(app: &Arc<App>, username: &str, files: u32, dirs: u32) {
    let _transition = app.behavior.transition.lock().await;
    if is_self(app, username) {
        return;
    }
    enum Action {
        None,
        Preset,
        BrowseVerify,
        Passed,
    }
    let action = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        peer.stats = Some((files, dirs));
        if PRESET_STATS.contains(&(files, dirs)) && peer.verdict != Verdict::Leech {
            peer.check = Check::Idle;
            Action::Preset
        } else if peer.check == Check::AwaitingStats && files == 0 {
            peer.check = Check::AwaitingBrowse;
            Action::BrowseVerify
        } else if peer.check == Check::AwaitingStats {
            peer.check = Check::Idle;
            if peer.verdict == Verdict::Clean {
                peer.verdict = Verdict::Verified;
            }
            Action::Passed
        } else {
            Action::None
        }
    };
    match action {
        Action::Preset => {
            convict(
                app,
                username,
                Verdict::Leech,
                &format!("preset-stats:{files}/{dirs}"),
            )
            .await;
        }
        Action::BrowseVerify => app.client.browse_user(username).await,
        Action::Passed => sync(app, username).await,
        Action::None => {}
    }
}

pub async fn browse_received(app: &Arc<App>, username: &str, file_count: u32) {
    let _transition = app.behavior.transition.lock().await;
    if is_self(app, username) {
        return;
    }
    enum Action {
        None,
        Contradiction(u32),
        ZeroShare,
        Passed,
    }
    let action = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        let checking = peer.check == Check::AwaitingBrowse;
        peer.check = Check::Idle;
        let stats_files = peer.stats.map(|(files, _)| files);
        if file_count == 0
            && let Some(files) = stats_files
            && files >= CONTRADICTION_MIN_FILES
            && peer.verdict != Verdict::Leech
        {
            Action::Contradiction(files)
        } else if file_count == 0
            && (checking || stats_files == Some(0))
            && peer.verdict < Verdict::Suspect
        {
            Action::ZeroShare
        } else if checking && file_count > 0 {
            if peer.verdict == Verdict::Clean {
                peer.verdict = Verdict::Verified;
            }
            Action::Passed
        } else {
            Action::None
        }
    };
    match action {
        Action::Contradiction(stats_files) => {
            convict(
                app,
                username,
                Verdict::Leech,
                &format!("browse-contradicts-stats:{stats_files}/0"),
            )
            .await;
        }
        Action::ZeroShare => convict(app, username, Verdict::Suspect, "zero-share").await,
        Action::Passed => sync(app, username).await,
        Action::None => {}
    }
}

pub async fn apply_level(app: &Arc<App>) {
    let _transition = app.behavior.transition.lock().await;
    let usernames: Vec<String> = {
        let peers = app.behavior.peers.lock().unwrap();
        peers
            .iter()
            .filter(|(_, peer)| peer.verdict != Verdict::Clean || peer.check != Check::Idle)
            .map(|(username, _)| username.clone())
            .collect()
    };
    for username in usernames {
        sync(app, &username).await;
    }
}

pub async fn buddy_added(app: &Arc<App>, username: &str) {
    let _transition = app.behavior.transition.lock().await;
    {
        let peers = app.behavior.peers.lock().unwrap();
        let Some(peer) = peers.get(username) else {
            return;
        };
        if peer.verdict < Verdict::Suspect {
            return;
        }
    }
    mark_verified(app, username);
    sync(app, username).await;
}

pub async fn load(app: &Arc<App>) {
    let _transition = app.behavior.transition.lock().await;
    let (level, denied_message) = policy(app);
    for (username, verdict, evidence) in load_verdicts(&app.db).await {
        let mut verdict = Verdict::from_str(&verdict);
        let mut evidence: Vec<String> = evidence
            .split(',')
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect();
        if verdict >= Verdict::Suspect && exempt(app, &username).await {
            verdict = Verdict::Verified;
            evidence.clear();
            set_user_verdict(&app.db, &username, verdict.as_str(), "", "none", now())
                .await
                .unwrap_or_else(|error| fatal(error));
        }
        {
            let mut peers = app.behavior.peers.lock().unwrap();
            peers.insert(
                username.clone(),
                Peer {
                    verdict,
                    evidence,
                    last_activity: now(),
                    ..Default::default()
                },
            );
        }
        let restriction = restriction_for(level, verdict, &denied_message);
        if restriction != Restriction::None {
            app.client
                .set_user_restriction(&username, restriction)
                .await;
        }
    }
}
