mod db;
mod policy;
mod state;

pub use state::Behavior;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use tokio::time::Instant;
use tracing::info;

use crate::types::{DenialMessages, FilterLevel, Restriction};

use db::{
    clear_user_verdict, downloaded_from_any, has_downloaded_from, load_verdicts, repeat_deliveries,
    repeat_delivery, reset_counters, search_scrape_users, set_user_verdict,
};
use policy::{
    CHECK_TIMEOUT_SECS, CONTRADICTION_MIN_FILES, PRESET_STATS, REPEAT_WINDOW_DAYS, SECS_PER_DAY,
    SWEEP_SECS, Verdict, restriction_for,
};
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

fn check_deadline() -> Instant {
    Instant::now() + Duration::from_secs(CHECK_TIMEOUT_SECS)
}

fn expire_check_at(app: &Arc<App>, username: &str, deadline: Instant) {
    let app = app.clone();
    let username = username.to_owned();
    tokio::spawn(async move {
        tokio::time::sleep_until(deadline).await;
        let _transition = app.behavior.transition.lock().await;
        let expired = app
            .behavior
            .peers
            .lock()
            .unwrap()
            .get_mut(&username)
            .is_some_and(|peer| peer.expire_check(deadline));
        if expired {
            info!(username, "behaviour check timed out");
            sync(&app, &username).await;
        }
    });
}

pub async fn sweep_loop(app: Arc<App>) {
    let mut ticks = tokio::time::interval(std::time::Duration::from_secs(SWEEP_SECS));
    loop {
        ticks.tick().await;
        sweep(&app).await;
    }
}

async fn sweep(app: &Arc<App>) {
    let candidates = search_scrape_users(&app.db).await;
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
    let deadline = check_deadline();
    let probe = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        let probe = peer.verdict == Verdict::Clean
            && peer.check == Check::Idle
            && peer.stats.is_none_or(|(files, _)| files == 0)
            && level == FilterLevel::Strict;
        if probe {
            peer.check = Check::AwaitingStats(deadline);
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
    expire_check_at(app, username, deadline);
}

pub async fn stats_received(app: &Arc<App>, username: &str, files: u32, dirs: u32) {
    let _transition = app.behavior.transition.lock().await;
    if is_self(app, username) {
        return;
    }
    enum Action {
        None,
        BrowseVerify(Instant),
        Passed,
    }
    let action = {
        let mut peers = app.behavior.peers.lock().unwrap();
        let peer = touch(&mut peers, username, now());
        peer.stats = Some((files, dirs));
        let awaiting_stats = matches!(peer.check, Check::AwaitingStats(_));
        let preset = PRESET_STATS.contains(&(files, dirs))
            && peer.verdict == Verdict::Clean
            && peer.check == Check::Idle;
        if preset || (awaiting_stats && files == 0) {
            let deadline = check_deadline();
            peer.check = Check::AwaitingBrowse(deadline);
            Action::BrowseVerify(deadline)
        } else if awaiting_stats {
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
        Action::BrowseVerify(deadline) => {
            app.client.browse_user(username).await;
            expire_check_at(app, username, deadline);
        }
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
        let checking = matches!(peer.check, Check::AwaitingBrowse(_));
        if checking {
            peer.check = Check::Idle;
        }
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

pub async fn browse_failed(app: &Arc<App>, username: &str) {
    let _transition = app.behavior.transition.lock().await;
    let failed = app
        .behavior
        .peers
        .lock()
        .unwrap()
        .get_mut(username)
        .is_some_and(|peer| peer.fail_browse());
    if failed {
        info!(username, "behaviour browse failed");
        sync(app, username).await;
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

fn awaits_release(app: &App, username: &str) -> bool {
    let peers = app.behavior.peers.lock().unwrap();
    peers.get(username).is_some_and(Peer::awaits_release)
}

pub async fn buddy_added(app: &Arc<App>, username: &str) {
    let _transition = app.behavior.transition.lock().await;
    if !awaits_release(app, username) {
        return;
    }
    mark_verified(app, username);
    sync(app, username).await;
}

pub async fn message_received(app: &Arc<App>, username: &str) {
    if !app.settings.clear_verdict_on_message() {
        return;
    }
    forgive(app, username).await;
}

async fn forgive(app: &Arc<App>, username: &str) {
    let _transition = app.behavior.transition.lock().await;
    let released = release(app, username);
    if released {
        clear_user_verdict(&app.db, username)
            .await
            .unwrap_or_else(|error| fatal(error));
    }
    reset_counters(&app.db, username, now())
        .await
        .unwrap_or_else(|error| fatal(error));
    if released {
        app.client
            .set_user_restriction(username, Restriction::None)
            .await;
        info!(username, "cleared peer verdict");
    }
    app.client.clear_file_denials(username).await;
}

fn release(app: &App, username: &str) -> bool {
    let mut peers = app.behavior.peers.lock().unwrap();
    let Some(peer) = peers.get_mut(username).filter(|peer| peer.awaits_release()) else {
        return false;
    };
    peer.verdict = Verdict::Clean;
    peer.evidence.clear();
    peer.abandon_check();
    true
}

pub async fn upload_delivered(app: &Arc<App>, username: &str, virtual_path: &str) {
    let timestamp = now();
    if let Some(last_at) = repeat_delivery(&app.db, username, virtual_path, timestamp).await {
        deny_file(app, username, virtual_path, last_at, timestamp).await;
    }
}

async fn deny_file(app: &Arc<App>, username: &str, virtual_path: &str, last_at: i64, now: i64) {
    let expires_at = last_at + REPEAT_WINDOW_DAYS * SECS_PER_DAY;
    let ttl = Duration::from_secs(expires_at.saturating_sub(now) as u64);
    app.client.deny_file(username, virtual_path, ttl).await;
    info!(username, virtual_path, "repeat downloads capped");
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
    let timestamp = now();
    let repeats = repeat_deliveries(&app.db, timestamp).await;
    info!(files = repeats.len(), "repeat download caps loaded");
    for repeat in repeats {
        deny_file(
            app,
            &repeat.username,
            &repeat.virtual_path,
            repeat.last_at,
            timestamp,
        )
        .await;
    }
}

pub(in crate::app) fn router() -> Router<Arc<App>> {
    Router::new().route("/api/users/{username}/clear_verdict", post(clear))
}

async fn clear(State(app): State<Arc<App>>, Path(username): Path<String>) -> StatusCode {
    forgive(&app, &username).await;
    StatusCode::ACCEPTED
}
