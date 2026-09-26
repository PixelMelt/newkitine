use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use super::ClientActor;
use crate::client::shares::{self, ScanError, SharesIndex};
use crate::client::{ClientEvent, Observation};
use crate::protocol::{PeerMessage, ServerRequest};
use crate::types::Restriction;

const BROWSE_COALESCE_WINDOW: Duration = Duration::from_millis(400);

pub(super) enum ScanUpdate {
    Progress(u64),
    Index(Box<SharesIndex>),
    Done(Result<(), ScanError>),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ScanJob {
    pub(super) install_cached: bool,
    pub(super) walk: bool,
}

impl ScanJob {
    pub(super) const RESCAN: Self = Self {
        install_cached: false,
        walk: true,
    };
    pub(super) const RELOAD: Self = Self {
        install_cached: true,
        walk: true,
    };

    fn merge(self, other: Self) -> Self {
        Self {
            install_cached: self.install_cached || other.install_cached,
            walk: self.walk || other.walk,
        }
    }
}

pub(super) struct Sharing {
    pub(super) index: Option<SharesIndex>,
    running: bool,
    pending: Option<ScanJob>,
    cancel: Arc<AtomicBool>,
    scan_results: mpsc::UnboundedSender<(u64, ScanUpdate)>,
    scan_cache: PathBuf,
    generation: u64,
    last_progress: u64,
    pub(super) excluded_phrases: Vec<String>,
    empty_browse_frame: Vec<u8>,
    browse_times: HashMap<String, Instant>,
}

impl Sharing {
    pub(super) fn new(
        scan_results: mpsc::UnboundedSender<(u64, ScanUpdate)>,
        scan_cache: PathBuf,
    ) -> Self {
        Self {
            index: None,
            running: false,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            scan_results,
            scan_cache,
            generation: 0,
            last_progress: 0,
            excluded_phrases: Vec::new(),
            empty_browse_frame: shares::empty_browse_frame(),
            browse_times: HashMap::new(),
        }
    }

    pub(super) fn counts(&self) -> (u32, u32) {
        self.index.as_ref().map_or((0, 0), |index| index.counts())
    }

    fn replace_index(&mut self, index: SharesIndex) {
        if let Some(old) = self.index.replace(index) {
            tokio::task::spawn_blocking(move || drop(old));
        }
    }
}

impl ClientActor {
    pub(super) fn start_scan(&mut self, job: ScanJob) {
        self.sharing.generation += 1;
        self.emit(ClientEvent::ShareScanStarted);
        if self.sharing.running {
            self.sharing.cancel.store(true, Ordering::Relaxed);
            self.sharing.pending = Some(
                self.sharing
                    .pending
                    .map_or(job, |pending| pending.merge(job)),
            );
            return;
        }
        self.spawn_scan(job);
    }

    fn spawn_scan(&mut self, job: ScanJob) {
        self.sharing.last_progress = 0;
        self.sharing.running = true;
        self.sharing.cancel = Arc::new(AtomicBool::new(false));
        let generation = self.sharing.generation;
        let shared_folders = self.config.shared_folders.clone();
        let share_filters = self.config.share_filters.clone();
        let cache_path = self.sharing.scan_cache.clone();
        let cancel = self.sharing.cancel.clone();
        let results = self.sharing.scan_results.clone();
        let walk = job.walk && !shared_folders.is_empty();
        let task = tokio::task::spawn_blocking({
            let results = results.clone();
            move || {
                let send = |update: ScanUpdate| {
                    let _ = results.send((generation, update));
                };
                let cached = shares::load_catalog(&cache_path);
                if job.install_cached && !cached.folders.is_empty() {
                    let restricted = shares::restrict(&cached, &shared_folders);
                    send(ScanUpdate::Index(Box::new(SharesIndex::from_catalog(
                        restricted,
                    ))));
                }
                if !walk {
                    send(ScanUpdate::Done(Ok(())));
                    return;
                }
                let progress = |files| send(ScanUpdate::Progress(files));
                let result =
                    shares::walk(&shared_folders, &share_filters, &cached, &cancel, &progress);
                drop(cached);
                match result {
                    Ok(catalog) => {
                        shares::save_catalog(&cache_path, &catalog);
                        send(ScanUpdate::Index(Box::new(SharesIndex::from_catalog(
                            catalog,
                        ))));
                        send(ScanUpdate::Done(Ok(())));
                    }
                    Err(error) => send(ScanUpdate::Done(Err(error))),
                }
            }
        });
        tokio::spawn(async move {
            if let Err(error) = task.await {
                let _ = results.send((
                    generation,
                    ScanUpdate::Done(Err(ScanError::Panicked {
                        reason: error.to_string(),
                    })),
                ));
            }
        });
    }

    pub(super) fn handle_scan_update(&mut self, generation: u64, update: ScanUpdate) {
        let current = generation == self.sharing.generation;
        match update {
            ScanUpdate::Progress(files) => {
                if current && files > self.sharing.last_progress {
                    self.sharing.last_progress = files;
                    self.emit(ClientEvent::ShareScanProgress { files });
                }
            }
            ScanUpdate::Index(index) => {
                if current {
                    self.install_index(*index);
                } else {
                    tokio::task::spawn_blocking(move || drop(index));
                }
            }
            ScanUpdate::Done(result) => {
                self.sharing.running = false;
                if current {
                    match result {
                        Ok(()) => self.emit(ClientEvent::ShareScanFinished),
                        Err(error) => {
                            tracing::error!(%error, "share scan failed");
                            self.emit(ClientEvent::ShareScanFailed {
                                error: error.to_string(),
                            });
                        }
                    }
                } else {
                    tracing::info!(
                        generation,
                        current = self.sharing.generation,
                        superseded = matches!(result, Err(ScanError::Superseded)),
                        "discarding stale share scan result"
                    );
                }
                if let Some(job) = self.sharing.pending.take() {
                    self.spawn_scan(job);
                }
            }
        }
    }

    fn install_index(&mut self, index: SharesIndex) {
        let (folders, files) = index.counts();
        let denied = self.uploads.revalidate(&index, &self.users);
        self.sharing.replace_index(index);
        self.emit_transfers(denied);
        if self.session.logged_in {
            self.net
                .server(ServerRequest::SharedFoldersFiles { folders, files });
        }
        self.emit(ClientEvent::SharesInstalled { folders, files });
    }

    pub(super) fn respond_to_search(&mut self, username: &str, token: u32, search_term: &str) {
        if !self.config.search.respond_to_searches {
            return;
        }
        if username == self.config.login.username {
            return;
        }
        if self.users.is_banned(username)
            || matches!(
                self.users.restriction(username),
                Some(Restriction::Denied { .. })
            )
        {
            return;
        }
        let results = match &self.sharing.index {
            Some(shares) => shares.search(
                search_term,
                self.users.is_buddy(username),
                &self.sharing.excluded_phrases,
                self.config.search.max_search_results,
                self.config.search.min_search_chars,
            ),
            None => Vec::new(),
        };
        self.emit(ClientEvent::Observed(Observation::SearchSeen {
            username: username.to_owned(),
            query: search_term.to_owned(),
            matched: !results.is_empty(),
        }));
        if results.is_empty() {
            return;
        }
        self.net.peer(
            username.to_owned(),
            PeerMessage::FileSearchResponse {
                username: self.config.login.username.clone(),
                token,
                results,
                free_upload_slots: self.uploads.is_new_upload_accepted(),
                upload_speed: self.uploads.upload_speed,
                queue_size: self.uploads.queue_size(),
                unknown: 0,
                private_results: Vec::new(),
            },
        );
    }

    pub(super) fn handle_browse_request(&mut self, username: String) {
        self.emit(ClientEvent::Observed(Observation::BrowseRequest {
            username: username.clone(),
        }));
        let now = Instant::now();
        self.sharing
            .browse_times
            .retain(|_, at| now.duration_since(*at) < BROWSE_COALESCE_WINDOW);
        if self
            .sharing
            .browse_times
            .insert(username.clone(), now)
            .is_some()
        {
            return;
        }
        let frame = match (&self.sharing.index, self.users.is_banned(&username)) {
            (Some(index), false) => index.browse_frame(self.users.is_buddy(&username)).to_vec(),
            _ => self.sharing.empty_browse_frame.clone(),
        };
        self.net.peer_frame(username, frame);
    }

    pub(super) fn handle_folder_contents_request(
        &mut self,
        username: String,
        token: u32,
        directory: String,
    ) {
        self.emit(ClientEvent::Observed(Observation::FolderContentsRequest {
            username: username.clone(),
        }));
        let folders = match (&self.sharing.index, self.users.is_banned(&username)) {
            (Some(index), false) => {
                index.folder_contents(&directory, self.users.is_buddy(&username))
            }
            _ => Vec::new(),
        };
        self.net.peer(
            username,
            PeerMessage::FolderContentsResponse {
                token,
                directory,
                folders,
            },
        );
    }
}
