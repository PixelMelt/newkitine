use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::ClientActor;
use crate::client::shares::{self, ScanError, SharesIndex};
use crate::network::NetworkHandle;
use crate::protocol::{PeerMessage, ServerRequest, encode_shared_file_list};
use crate::types::{ClientEvent, Observation, Restriction, ShareCatalog};

const BROWSE_QUEUE_CAPACITY: usize = 32;

struct BrowseJob {
    username: String,
    catalog: Arc<ShareCatalog>,
    is_buddy: bool,
}

pub(super) enum ScanUpdate {
    Progress(u64),
    Complete(Result<SharesIndex, ScanError>),
}

pub(super) struct Sharing {
    pub(super) index: Option<Arc<SharesIndex>>,
    scan_results: mpsc::UnboundedSender<(u64, ScanUpdate)>,
    browse_jobs: mpsc::Sender<BrowseJob>,
    scan_cache: PathBuf,
    generation: u64,
    last_progress: u64,
    pub(super) excluded_phrases: Vec<String>,
}

impl Sharing {
    pub(super) fn new(
        scan_results: mpsc::UnboundedSender<(u64, ScanUpdate)>,
        scan_cache: PathBuf,
        net: NetworkHandle,
    ) -> Self {
        let (browse_jobs, mut jobs) = mpsc::channel::<BrowseJob>(BROWSE_QUEUE_CAPACITY);
        tokio::spawn(async move {
            while let Some(job) = jobs.recv().await {
                let username = job.username;
                let bytes = tokio::task::spawn_blocking(move || {
                    encode_shared_file_list(&job.catalog, job.is_buddy)
                })
                .await
                .expect("browse encoder panicked");
                net.peer_frame(username, bytes);
            }
        });
        Self {
            index: None,
            scan_results,
            browse_jobs,
            scan_cache,
            generation: 0,
            last_progress: 0,
            excluded_phrases: Vec::new(),
        }
    }
}

impl ClientActor {
    pub(super) fn start_scan(&mut self) {
        if self.config.shared_folders.is_empty() {
            return;
        }
        self.sharing.generation += 1;
        self.sharing.last_progress = 0;
        let generation = self.sharing.generation;
        self.emit(ClientEvent::ShareScanStarted);
        let shared_folders = self.config.shared_folders.clone();
        let cache_path = self.sharing.scan_cache.clone();
        let results = self.sharing.scan_results.clone();
        tokio::task::spawn_blocking(move || {
            let progress = |files| {
                let _ = results.send((generation, ScanUpdate::Progress(files)));
            };
            let result = shares::scan(&shared_folders, &cache_path, &progress);
            let _ = results.send((generation, ScanUpdate::Complete(result)));
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

    fn handle_scan_complete(&mut self, generation: u64, result: Result<SharesIndex, ScanError>) {
        if generation != self.sharing.generation {
            tracing::warn!(
                generation,
                current = self.sharing.generation,
                "discarding stale share scan result"
            );
            return;
        }
        let index = match result {
            Ok(index) => index,
            Err(error) => {
                tracing::error!(%error, "share scan failed, keeping previous index");
                self.emit(ClientEvent::ShareScanFailed {
                    error: error.to_string(),
                });
                return;
            }
        };
        let (folders, files) = index.counts();
        self.sharing.index = Some(Arc::new(index));
        if self.session.logged_in {
            self.net
                .server(ServerRequest::SharedFoldersFiles { folders, files });
        }
        self.emit(ClientEvent::SharesScanned { folders, files });
    }

    pub(super) fn apply_shared_folders(&mut self) {
        if self.config.shared_folders.is_empty() {
            self.sharing.generation += 1;
            self.sharing.index = None;
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
        } else {
            self.start_scan();
        }
    }

    pub(super) fn respond_to_search(&mut self, username: &str, token: u32, search_term: &str) {
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
            })
            .expect("browse response queue overflowed");
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
