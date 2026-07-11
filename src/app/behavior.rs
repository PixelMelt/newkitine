use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::types::Restriction;

use super::db;
use super::state::{App, now};

const WINDOW_SECS: i64 = 600;
const SEARCH_FLOOD: usize = 30;
const QUEUE_FLOOD: usize = 100;
const PRESET_STATS: &[(u32, u32)] = &[
    (1, 1),
    (1, 499),
    (500, 25),
    (1000, 50),
    (1500, 75),
    (2000, 100),
];
const CONTRADICTION_MIN_FILES: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Verdict {
    #[default]
    Clean,
    Verified,
    Suspect,
    Leech,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Clean => "clean",
            Verdict::Verified => "verified",
            Verdict::Suspect => "suspect",
            Verdict::Leech => "leech",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "clean" => Verdict::Clean,
            "verified" => Verdict::Verified,
            "suspect" => Verdict::Suspect,
            "leech" => Verdict::Leech,
            other => panic!("unknown verdict {other}"),
        }
    }
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Check {
    #[default]
    Idle,
    AwaitingStats,
    AwaitingBrowse,
}

#[derive(Default)]
struct Peer {
    searches: VecDeque<i64>,
    queue_requests: VecDeque<i64>,
    stats: Option<(u32, u32)>,
    verdict: Verdict,
    evidence: Vec<String>,
    check: Check,
}

#[derive(Default)]
pub struct Behavior {
    peers: Mutex<HashMap<String, Peer>>,
}

fn policy(app: &App) -> (String, String) {
    app.settings.behavior_policy()
}

fn restriction_for(level: &str, verdict: Verdict, denied_message: &str) -> Restriction {
    match level {
        "open" => Restriction::None,
        "guarded" => match verdict {
            Verdict::Leech => Restriction::Denied {
                reason: denied_message.to_owned(),
            },
            Verdict::Suspect => Restriction::Deprioritized,
            Verdict::Clean | Verdict::Verified => Restriction::None,
        },
        "strict" => match verdict {
            Verdict::Leech | Verdict::Suspect => Restriction::Denied {
                reason: denied_message.to_owned(),
            },
            Verdict::Clean | Verdict::Verified => Restriction::None,
        },
        other => panic!("unknown filter level {other}"),
    }
}

fn restriction_str(restriction: &Restriction) -> &'static str {
    match restriction {
        Restriction::None => "none",
        Restriction::Deprioritized => "deprioritized",
        Restriction::Hold => "hold",
        Restriction::Denied { .. } => "denied",
    }
}

fn prune(window: &mut VecDeque<i64>, timestamp: i64) {
    while window
        .front()
        .is_some_and(|entry| timestamp - entry > WINDOW_SECS)
    {
        window.pop_front();
    }
}

fn is_self(app: &App, username: &str) -> bool {
    app.data.read().unwrap().session.status().username == username
}

async fn exempt(app: &Arc<App>, username: &str) -> bool {
    if app.data.read().unwrap().users.is_buddy(username) {
        return true;
    }
    has_downloaded_from(&app.db, username).await
}

async fn has_downloaded_from(pool: &sqlx::MySqlPool, username: &str) -> bool {
    use sqlx::Row;
    let count: i64 = sqlx::query(
        "SELECT COUNT(*) FROM transfer_history WHERE direction = 'download' AND username = ?",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("downloaded-from check")
    .get(0);
    count > 0
}

async fn set_user_verdict(
    pool: &sqlx::MySqlPool,
    username: &str,
    verdict: &str,
    evidence: &str,
    restriction: &str,
    timestamp: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users_seen (username, first_seen, last_seen, verdict, evidence, restriction)
         VALUES (?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
            last_seen = VALUES(last_seen),
            verdict = VALUES(verdict),
            evidence = VALUES(evidence),
            restriction = VALUES(restriction)",
    )
    .bind(username)
    .bind(timestamp)
    .bind(timestamp)
    .bind(verdict)
    .bind(evidence)
    .bind(restriction)
    .execute(pool)
    .await?;
    Ok(())
}

async fn load_verdicts(pool: &sqlx::MySqlPool) -> Vec<(String, String, String)> {
    use sqlx::Row;
    sqlx::query(
        "SELECT username, verdict, COALESCE(evidence, '') FROM users_seen WHERE verdict != 'clean'",
    )
    .fetch_all(pool)
    .await
    .expect("load verdicts")
    .into_iter()
    .map(|row| (row.get(0), row.get(1), row.get(2)))
    .collect()
}

async fn sync(app: &Arc<App>, username: &str) {
    let (level, denied_message) = policy(app);
    let (verdict, evidence) = {
        let peers = app.behavior.peers.lock().unwrap();
        let peer = &peers[username];
        (peer.verdict, peer.evidence.clone())
    };
    let restriction = restriction_for(&level, verdict, &denied_message);
    let restriction_label = restriction_str(&restriction);
    app.client.set_user_restriction(username, restriction).await;
    set_user_verdict(
        &app.db,
        username,
        verdict.as_str(),
        &evidence.join(","),
        restriction_label,
        now(),
    )
    .await
    .unwrap_or_else(|error| db::fatal(error));
}

async fn convict(app: &Arc<App>, username: &str, verdict: Verdict, evidence: &str) {
    if exempt(app, username).await {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = peers.entry(username.to_owned()).or_default();
        if peer.verdict < Verdict::Verified {
            peer.verdict = Verdict::Verified;
        }
        peer.searches.clear();
        peer.queue_requests.clear();
        peer.check = Check::Idle;
        return;
    }
    {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = peers.entry(username.to_owned()).or_default();
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
    if is_self(app, username) {
        return;
    }
    let flooded = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = peers.entry(username.to_owned()).or_default();
        let timestamp = now();
        peer.searches.push_back(timestamp);
        prune(&mut peer.searches, timestamp);
        peer.searches.len() > SEARCH_FLOOD && peer.verdict < Verdict::Suspect
    };
    if flooded {
        convict(app, username, Verdict::Suspect, "search-flood").await;
    }
}

pub async fn queue_request(app: &Arc<App>, username: &str) {
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
        let peer = peers.entry(username.to_owned()).or_default();
        let timestamp = now();
        peer.queue_requests.push_back(timestamp);
        prune(&mut peer.queue_requests, timestamp);
        if peer.queue_requests.len() > QUEUE_FLOOD && peer.verdict < Verdict::Suspect {
            Action::Flood
        } else if peer.verdict == Verdict::Clean
            && peer.check == Check::Idle
            && peer.stats.is_none()
            && level != "open"
        {
            peer.check = Check::AwaitingStats;
            Action::Probe {
                hold: level == "strict",
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
                let peer = peers.entry(username.to_owned()).or_default();
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
        let peer = peers.entry(username.to_owned()).or_default();
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
        let peer = peers.entry(username.to_owned()).or_default();
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
    {
        let mut peers = app.behavior.peers.lock().unwrap();
        let Some(peer) = peers.get_mut(username) else {
            return;
        };
        if peer.verdict < Verdict::Suspect {
            return;
        }
        peer.verdict = Verdict::Verified;
        peer.evidence.clear();
        peer.check = Check::Idle;
    }
    sync(app, username).await;
}

pub async fn load(app: &Arc<App>) {
    let (level, denied_message) = policy(app);
    for (username, verdict, evidence) in load_verdicts(&app.db).await {
        let verdict = Verdict::from_str(&verdict);
        let evidence: Vec<String> = evidence
            .split(',')
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect();
        {
            let mut peers = app.behavior.peers.lock().unwrap();
            peers.insert(
                username.clone(),
                Peer {
                    verdict,
                    evidence,
                    ..Default::default()
                },
            );
        }
        let restriction = restriction_for(&level, verdict, &denied_message);
        if restriction != Restriction::None {
            app.client
                .set_user_restriction(&username, restriction)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_map_verdicts_to_restrictions() {
        assert!(Verdict::Clean < Verdict::Verified);
        assert!(Verdict::Verified < Verdict::Suspect);
        assert!(Verdict::Suspect < Verdict::Leech);
        assert_eq!(
            restriction_for("open", Verdict::Leech, "m"),
            Restriction::None
        );
        assert_eq!(
            restriction_for("guarded", Verdict::Suspect, "m"),
            Restriction::Deprioritized
        );
        assert_eq!(
            restriction_for("guarded", Verdict::Leech, "m"),
            Restriction::Denied { reason: "m".into() }
        );
        assert_eq!(
            restriction_for("strict", Verdict::Suspect, "m"),
            Restriction::Denied { reason: "m".into() }
        );
        assert_eq!(
            restriction_for("strict", Verdict::Verified, "m"),
            Restriction::None
        );
    }

    #[test]
    fn verdict_round_trips_through_storage() {
        for verdict in [
            Verdict::Clean,
            Verdict::Verified,
            Verdict::Suspect,
            Verdict::Leech,
        ] {
            assert_eq!(Verdict::from_str(verdict.as_str()), verdict);
        }
    }
}
