use std::time::Duration;

use tokio::time::Instant;
use tracing::{info, warn};

use super::ClientActor;
use crate::protocol::ServerRequest;
use crate::types::{ClientEvent, NetworkCommand, RuntimeConfig};

const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(15);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(300);

pub(super) struct Session {
    pub(super) connected: bool,
    pub(super) logged_in: bool,
    pub(super) reconnect_now: bool,
    pub(super) reconnect_at: Option<Instant>,
    reconnect_delay: Duration,
}

impl Session {
    pub(super) fn new() -> Self {
        Self {
            connected: false,
            logged_in: false,
            reconnect_now: false,
            reconnect_at: None,
            reconnect_delay: RECONNECT_INITIAL_DELAY,
        }
    }
}

impl ClientActor {
    pub(super) fn connect(&mut self) {
        self.session.connected = true;
        self.net.send(NetworkCommand::ServerConnect {
            address: self.config.login.server,
            username: self.config.login.username.clone(),
            password: self.config.login.password.clone(),
            listen_port: self.config.login.listen_port,
        });
    }

    pub(super) fn reconnect(&mut self) {
        self.session.reconnect_at = None;
        if self.session.connected {
            self.session.reconnect_now = true;
            self.net.send(NetworkCommand::ServerDisconnect);
        } else {
            self.connect();
        }
    }

    pub(super) fn set_transfer_limits(&self, upload_kbps: u32, download_kbps: u32) {
        self.net.send(NetworkCommand::SetTransferLimits {
            upload_bps: upload_kbps as u64 * 1024,
            download_bps: download_kbps as u64 * 1024,
        });
    }

    pub(super) fn apply_config(&mut self, config: RuntimeConfig) {
        let old = std::mem::replace(&mut self.config, config);
        if self.config.transfers != old.transfers {
            let transfers = self.config.transfers.clone();
            self.uploads.set_limits(
                transfers.upload_slots,
                transfers.queue_file_limit,
                transfers.uploads_per_user,
            );
            self.downloads
                .set_dirs(transfers.download_dir, transfers.incomplete_dir);
            self.set_transfer_limits(transfers.upload_limit_kbps, transfers.download_limit_kbps);
        }
        if self.config.shared_folders != old.shared_folders {
            self.apply_shared_folders();
        }
        if self.config.login != old.login {
            info!("login configuration changed, reconnecting");
            if self.session.connected {
                self.reconnect();
            } else if !self.config.login.username.is_empty() {
                self.connect();
            }
        }
        info!("runtime configuration applied");
    }

    pub(super) fn handle_logged_in(&mut self, username: String, banner: String) {
        self.session.logged_in = true;
        self.session.reconnect_delay = RECONNECT_INITIAL_DELAY;
        self.session.reconnect_at = None;
        self.users.watch_buddies(&self.net);
        if let Some(shares) = &self.sharing.index {
            let (folders, files) = shares.counts();
            self.net
                .server(ServerRequest::SharedFoldersFiles { folders, files });
        }
        for thing in &self.liked_interests {
            self.net.server(ServerRequest::AddThingILike {
                thing: thing.clone(),
            });
        }
        for thing in &self.hated_interests {
            self.net.server(ServerRequest::AddThingIHate {
                thing: thing.clone(),
            });
        }
        self.net.server(ServerRequest::CheckPrivileges);
        self.downloads.request_queued();
        self.schedule_wishlist();
        self.emit(ClientEvent::LoggedIn { username, banner });
    }

    pub(super) fn handle_server_disconnected(&mut self, manual: bool) {
        self.session.logged_in = false;
        self.session.connected = false;
        self.searches.clear();
        let downloads = self.downloads.reset();
        self.emit_transfers(downloads);
        let uploads = self.uploads.reset();
        self.emit_transfers(uploads);
        self.users.reset();
        self.wishlist.at = None;
        if self.session.reconnect_now {
            self.session.reconnect_now = false;
            self.connect();
        } else if !manual && self.config.auto_reconnect {
            self.session.reconnect_at = Some(Instant::now() + self.session.reconnect_delay);
            warn!(delay = ?self.session.reconnect_delay, "server connection lost, reconnecting");
            self.session.reconnect_delay =
                (self.session.reconnect_delay * 2).min(RECONNECT_MAX_DELAY);
        }
        self.emit(ClientEvent::Disconnected);
    }
}
