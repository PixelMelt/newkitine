use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, mpsc};

use super::ClientActor;
use crate::client::shares::{
    self, AttributeCache, ScanError, ScanOutcome, ShareCatalog, SharesIndex,
    encode_shared_file_list,
};
use crate::client::{ClientEvent, Observation};
use crate::protocol::{PeerMessage, ServerRequest};
use crate::types::Restriction;

const BROWSE_QUEUE_CAPACITY: usize = 32;
pub(super) const PENDING_REQUEST_LIMIT: usize = 128;

struct BrowseJob {
    username: String,
    catalog: Arc<ShareCatalog>,
    is_buddy: bool,
    catalog_version: u64,
}

pub(super) struct BrowseEncoded {
    username: String,
    is_buddy: bool,
    catalog_version: u64,
    bytes: Vec<u8>,
}

pub(super) enum ScanUpdate {
    Progress(u64),
    Complete(Result<ScanOutcome, ScanError>),
}

pub(super) struct Sharing {
    pub(super) index: Option<Arc<SharesIndex>>,
    running: bool,
    rescan_pending: bool,
    pub(super) pending_requests: Vec<(String, PeerMessage)>,
    scan_results: mpsc::UnboundedSender<(u64, ScanUpdate)>,
    browse_jobs: mpsc::Sender<BrowseJob>,
    pending_save: Arc<Mutex<Option<AttributeCache>>>,
    save_ready: Arc<Notify>,
    scan_cache: PathBuf,
    generation: u64,
    catalog_version: u64,
    last_progress: u64,
    pub(super) excluded_phrases: Vec<String>,
}

impl Sharing {
    pub(super) fn new(
        scan_results: mpsc::UnboundedSender<(u64, ScanUpdate)>,
        browse_encoded: mpsc::UnboundedSender<BrowseEncoded>,
        scan_cache: PathBuf,
    ) -> Self {
        let (browse_jobs, mut jobs) = mpsc::channel::<BrowseJob>(BROWSE_QUEUE_CAPACITY);
        tokio::spawn(async move {
            while let Some(job) = jobs.recv().await {
                let BrowseJob {
                    username,
                    catalog,
                    is_buddy,
                    catalog_version,
                } = job;
                let bytes = tokio::task::spawn_blocking(move || {
                    encode_shared_file_list(&catalog, is_buddy)
                })
                .await
                .expect("browse encoder panicked");
                let _ = browse_encoded.send(BrowseEncoded {
                    username,
                    is_buddy,
                    catalog_version,
                    bytes,
                });
            }
        });
        let pending_save: Arc<Mutex<Option<AttributeCache>>> = Arc::default();
        let save_ready = Arc::new(Notify::new());
        let save_path = scan_cache.clone();
        let (save_slot, save_notify) = (pending_save.clone(), save_ready.clone());
        tokio::spawn(async move {
            loop {
                save_notify.notified().await;
                loop {
                    let taken = save_slot.lock().unwrap().take();
                    let Some(cache) = taken else { break };
                    let path = save_path.clone();
                    tokio::task::spawn_blocking(move || shares::save_cache(&path, &cache))
                        .await
                        .expect("cache save panicked");
                }
            }
        });
        Self {
            index: None,
            running: false,
            rescan_pending: false,
            pending_requests: Vec::new(),
            scan_results,
            browse_jobs,
            pending_save,
            save_ready,
            scan_cache,
            generation: 0,
            catalog_version: 0,
            last_progress: 0,
            excluded_phrases: Vec::new(),
        }
    }
}

impl ClientActor {
    pub(super) fn awaiting_share_index(&self) -> bool {
        self.sharing.index.is_none() && (self.sharing.running || self.sharing.rescan_pending)
    }

    pub(super) fn start_scan(&mut self) {
        if self.config.shared_folders.is_empty() {
            return;
        }
        self.sharing.generation += 1;
        if self.sharing.running {
            self.sharing.rescan_pending = true;
            self.emit(ClientEvent::ShareScanStarted);
            return;
        }
        self.spawn_scan();
    }

    fn spawn_scan(&mut self) {
        self.sharing.last_progress = 0;
        self.sharing.running = true;
        let generation = self.sharing.generation;
        self.emit(ClientEvent::ShareScanStarted);
        let shared_folders = self.config.shared_folders.clone();
        let share_filters = self.config.share_filters.clone();
        let cache_path = self.sharing.scan_cache.clone();
        let results = self.sharing.scan_results.clone();
        let task = tokio::task::spawn_blocking({
            let results = results.clone();
            move || {
                let progress = |files| {
                    let _ = results.send((generation, ScanUpdate::Progress(files)));
                };
                let result = shares::scan(&shared_folders, &share_filters, &cache_path, &progress);
                let _ = results.send((generation, ScanUpdate::Complete(result)));
            }
        });
        tokio::spawn(async move {
            if let Err(error) = task.await {
                let _ = results.send((
                    generation,
                    ScanUpdate::Complete(Err(ScanError::Panicked {
                        reason: error.to_string(),
                    })),
                ));
            }
        });
    }

    pub(super) fn handle_scan_update(&mut self, generation: u64, update: ScanUpdate) {
        match update {
            ScanUpdate::Progress(files) => {
                if generation == self.sharing.generation && files > self.sharing.last_progress {
                    self.sharing.last_progress = files;
                    self.emit(ClientEvent::ShareScanProgress { files });
                }
            }
            ScanUpdate::Complete(result) => self.handle_scan_complete(generation, result),
        }
    }

    fn handle_scan_complete(&mut self, generation: u64, result: Result<ScanOutcome, ScanError>) {
        self.sharing.running = false;
        if generation != self.sharing.generation {
            tracing::warn!(
                generation,
                current = self.sharing.generation,
                "discarding stale share scan result"
            );
        } else {
            match result {
                Ok(outcome) => {
                    let (folders, files) = outcome.index.counts();
                    self.sharing.index = Some(Arc::new(outcome.index));
                    self.sharing.catalog_version += 1;
                    *self.sharing.pending_save.lock().unwrap() = Some(outcome.cache);
                    self.sharing.save_ready.notify_one();
                    if self.session.logged_in {
                        self.net
                            .server(ServerRequest::SharedFoldersFiles { folders, files });
                    }
                    self.emit(ClientEvent::SharesScanned { folders, files });
                }
                Err(error) => {
                    tracing::error!(%error, "share scan failed");
                    self.emit(ClientEvent::ShareScanFailed {
                        error: error.to_string(),
                    });
                }
            }
        }
        if std::mem::take(&mut self.sharing.rescan_pending) {
            self.spawn_scan();
        }
        self.replay_pending_requests();
        if !self.awaiting_share_index() {
            self.downloads.flush_queued_requests();
        }
    }

    fn replay_pending_requests(&mut self) {
        let pending = std::mem::take(&mut self.sharing.pending_requests);
        if pending.is_empty() {
            return;
        }
        tracing::info!(
            count = pending.len(),
            "replaying upload requests deferred during share scan"
        );
        for (username, message) in pending {
            self.handle_peer_message(username, message);
        }
    }

    pub(super) fn apply_shared_folders(&mut self) {
        self.sharing.index = None;
        self.sharing.catalog_version += 1;
        let revoked = self.uploads.revoke_all();
        self.emit_transfers(revoked);
        if self.session.logged_in {
            self.net.server(ServerRequest::SharedFoldersFiles {
                folders: 0,
                files: 0,
            });
        }
        self.emit(ClientEvent::SharesScanned {
            folders: 0,
            files: 0,
        });
        if self.config.shared_folders.is_empty() {
            self.sharing.generation += 1;
            self.sharing.rescan_pending = false;
            if !self.sharing.running {
                self.replay_pending_requests();
                self.downloads.flush_queued_requests();
            }
        } else {
            self.start_scan();
        }
    }

    pub(super) fn respond_to_search(&mut self, username: &str, token: u32, search_term: &str) {
        if !self.config.search.respond_to_searches {
            return;
        }
        if username == self.config.login.username {
            return;
        }
        let blocked = self.users.is_banned(username)
            || matches!(
                self.users.restriction(username),
                Some(Restriction::Denied { .. })
            );
        let results = match (&self.sharing.index, blocked) {
            (Some(shares), false) => shares.search(
                search_term,
                self.users.is_buddy(username),
                &self.sharing.excluded_phrases,
                self.config.search.max_search_results,
                self.config.search.min_search_chars,
            ),
            _ => Vec::new(),
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
        self.queue_browse_job(username);
    }

    fn queue_browse_job(&mut self, username: String) {
        let is_buddy = self.users.is_buddy(&username);
        let catalog = match (&self.sharing.index, self.users.is_banned(&username)) {
            (Some(index), false) => index.catalog(),
            _ => Arc::new(ShareCatalog {
                folders: Vec::new(),
                files: Vec::new(),
                folders_by_path: Vec::new(),
            }),
        };
        self.sharing
            .browse_jobs
            .try_send(BrowseJob {
                username,
                catalog,
                is_buddy,
                catalog_version: self.sharing.catalog_version,
            })
            .expect("browse response queue overflowed");
    }

    pub(super) fn handle_browse_encoded(&mut self, encoded: BrowseEncoded) {
        if encoded.catalog_version != self.sharing.catalog_version {
            tracing::debug!(
                username = encoded.username,
                "re-encoding browse response, catalog changed while encoding"
            );
            self.queue_browse_job(encoded.username);
            return;
        }
        if self.users.is_banned(&encoded.username)
            || self.users.is_buddy(&encoded.username) != encoded.is_buddy
        {
            tracing::debug!(
                username = encoded.username,
                "re-encoding browse response, peer access changed while encoding"
            );
            self.queue_browse_job(encoded.username);
            return;
        }
        self.net.peer_frame(encoded.username, encoded.bytes);
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
