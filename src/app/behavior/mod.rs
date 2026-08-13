mod db;
mod policy;
mod state;

pub use state::Behavior;

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use tracing::info;

use crate::types::{DenialMessages, FilterLevel, Restriction};

use db::{downloaded_from_any, has_downloaded_from, repeat_download_users, search_scrape_users};

use super::peer_history::{clear_user_verdict, load_verdicts, set_user_verdict};
use policy::{CONTRADICTION_MIN_FILES, PRESET_STATS, SWEEP_SECS, Verdict, restriction_for};
use state::{Check, Peer, touch};

use super::db::fatal;
use super::state::{App, now};

fn policy(app: &App) -> (FilterLevel, DenialMessages) {
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
    let (level, messages) = policy(app);
    let (verdict, evidence) = {
        let peers = app.behavior.peers.lock().unwrap();
        let peer = &peers[username];
        (peer.verdict, peer.evidence.clone())
    };
    let restriction = restriction_for(level, verdict, &messages);
    let timestamp = now();
    set_user_verdict(
        &app.db,
        username,
        verdict.as_str(),
        &evidence.join(","),
        restriction.as_str(),
        timestamp,
        (verdict >= Verdict::Leech).then_some(timestamp),
    )
    .await
    .unwrap_or_else(|error| fatal(error));
    app.client.set_user_restriction(username, restriction).await;
}

fn mark_verified(app: &App, username: &str) {
    let mut peers = app.behavior.peers.lock().unwrap();
    let peer = touch(&mut peers, username, now());
    peer.verdict = Verdict::Verified;
    peer.check = Check::Idle;
}

async fn convict(app: &Arc<App>, username: &str, verdict: Verdict, evidence: &str, exempt: bool) {
    if exempt {
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

pub async fn sweep_loop(app: Arc<App>) {
    let mut ticks = tokio::time::interval(std::time::Duration::from_secs(SWEEP_SECS));
    loop {
        ticks.tick().await;
        sweep(&app).await;
    }
}

async fn sweep(app: &Arc<App>) {
    let (mut candidates, repeaters) = tokio::join!(
        search_scrape_users(&app.db),
        repeat_download_users(&app.db, now()),
    );
    candidates.extend(repeaters);
    if candidates.is_empty() {
        return;
    }
    info!(candidates = candidates.len(), "behaviour sweep candidates");

    let usernames: Vec<String> = candidates
        .iter()
        .map(|(username, _)| username.clone())
        .collect();
    let downloaded = downloaded_from_any(&app.db, &usernames).await;

    for (username, evidence) in candidates {
        let _transition = app.behavior.transition.lock().await;
        if is_self(app, &username) {
            continue;
        }
        let exempt =
            downloaded.contains(&username) || app.projection.read().users.is_buddy(&username);
        convict(app, &username, Verdict::Abusive, &evidence, exempt).await;
    }
}

pub async fn queue_request(app: &Arc<App>, username: &str) {
    let _transition = app.behavior.transition.lock().await;
    if is_self(app, username) {
        return;
    }
    let (level, _) = policy(app);
    let probe = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        let probe = peer.verdict == Verdict::Clean
            && peer.check == Check::Idle
            && peer.stats.is_none()
            && level == FilterLevel::Strict;
        if probe {
            peer.check = Check::AwaitingStats;
        }
        probe
    };
    if !probe {
        return;
    }
    if exempt(app, username).await {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        peer.check = Check::Idle;
        peer.verdict = Verdict::Verified;
        return;
    }
    app.client
        .set_user_restriction(username, Restriction::Hold)
        .await;
    app.client.request_user_stats(username).await;
}

pub async fn stats_received(app: &Arc<App>, username: &str, files: u32, dirs: u32) {
    let _transition = app.behavior.transition.lock().await;
    if is_self(app, username) {
        return;
    }
    enum Action {
        None,
        BrowseVerify,
        Passed,
    }
    let action = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        peer.stats = Some((files, dirs));
        let preset = PRESET_STATS.contains(&(files, dirs))
            && peer.verdict == Verdict::Clean
            && peer.check == Check::Idle;
        if preset || (peer.check == Check::AwaitingStats && files == 0) {
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
        ZeroShare(Option<(u32, u32)>),
        Passed,
    }
    let action = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        let checking = peer.check == Check::AwaitingBrowse;
        peer.check = Check::Idle;
        let stats = peer.stats;
        let stats_files = stats.map(|(files, _)| files);
        if file_count == 0
            && let Some(files) = stats_files
            && files >= CONTRADICTION_MIN_FILES
            && peer.verdict < Verdict::Leech
        {
            Action::Contradiction(files)
        } else if file_count == 0
            && (checking || stats_files == Some(0))
            && peer.verdict < Verdict::Leech
        {
            Action::ZeroShare(stats)
        } else if checking && file_count > 0 {
            if peer.verdict == Verdict::Clean {
                peer.verdict = Verdict::Verified;
            }
            Action::Passed
        } else {
            Action::None
        }
    };
    let evidence = match action {
        Action::Contradiction(stats_files) => format!("browse-contradicts-stats:{stats_files}/0"),
        Action::ZeroShare(Some(stats)) if PRESET_STATS.contains(&stats) => {
            format!("preset-stats:{}/{}", stats.0, stats.1)
        }
        Action::ZeroShare(_) => "zero-share".to_owned(),
        Action::Passed => return sync(app, username).await,
        Action::None => return,
    };
    let exempt = exempt(app, username).await;
    convict(app, username, Verdict::Leech, &evidence, exempt).await;
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

fn is_convicted(app: &App, username: &str) -> bool {
    let peers = app.behavior.peers.lock().unwrap();
    peers
        .get(username)
        .is_some_and(|peer| peer.verdict >= Verdict::Leech)
}

pub async fn buddy_added(app: &Arc<App>, username: &str) {
    let _transition = app.behavior.transition.lock().await;
    if !is_convicted(app, username) {
        return;
    }
    mark_verified(app, username);
    sync(app, username).await;
}

pub async fn message_received(app: &Arc<App>, username: &str) {
    if !app.settings.clear_verdict_on_message() {
        return;
    }
    clear_verdict(app, username).await;
}

pub async fn clear_verdict(app: &Arc<App>, username: &str) {
    if !is_convicted(app, username) {
        return;
    }
    let _transition = app.behavior.transition.lock().await;
    if !is_convicted(app, username) {
        return;
    }
    {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        peer.verdict = Verdict::Clean;
        peer.evidence.clear();
        peer.check = Check::Idle;
        peer.stats = None;
    }
    clear_user_verdict(&app.db, username, now())
        .await
        .unwrap_or_else(|error| fatal(error));
    app.client
        .set_user_restriction(username, Restriction::None)
        .await;
    info!(username, "cleared peer verdict");
}

pub async fn load(app: &Arc<App>) {
    let _transition = app.behavior.transition.lock().await;
    let (level, messages) = policy(app);
    for (username, stored, evidence) in load_verdicts(&app.db).await {
        let mut verdict = Verdict::from_str(&stored);
        if verdict >= Verdict::Leech && exempt(app, &username).await {
            verdict = Verdict::Verified;
            set_user_verdict(
                &app.db,
                &username,
                verdict.as_str(),
                &evidence,
                "none",
                now(),
                None,
            )
            .await
            .unwrap_or_else(|error| fatal(error));
        }
        {
            let mut peers = app.behavior.peers.lock().unwrap();
            peers.insert(
                username.clone(),
                Peer {
                    verdict,
                    evidence: evidence
                        .split(',')
                        .filter(|entry| !entry.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    last_activity: now(),
                    ..Default::default()
                },
            );
        }
        let restriction = restriction_for(level, verdict, &messages);
        if restriction != Restriction::None {
            app.client
                .set_user_restriction(&username, restriction)
                .await;
        }
    }
}

pub(in crate::app) fn router() -> Router<Arc<App>> {
    Router::new().route("/api/users/{username}/clear_verdict", post(clear))
}

async fn clear(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    clear_verdict(&app, &username).await;
    StatusCode::ACCEPTED
}
