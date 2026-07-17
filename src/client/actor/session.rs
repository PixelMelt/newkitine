use std::time::Duration;

use tokio::time::Instant;
use tracing::{info, warn};

use super::ClientActor;
use crate::client::ClientEvent;
use crate::network::NetworkCommand;
use crate::protocol::ServerRequest;
use crate::types::RuntimeConfig;

const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(5);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(300);

fn reconnect_jitter() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    Duration::from_millis((nanos % 10_000) as u64)
}

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
        self.emit(ClientEvent::Connecting);
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
                transfers.queue_size_limit_mb,
                transfers.banned_message,
            );
            self.uploads.check_queue(&self.users);
            self.downloads.set_dirs(
                transfers.download_dir,
                transfers.incomplete_dir,
                transfers.download_username_subfolders,
            );
            self.set_transfer_limits(transfers.upload_limit_kbps, transfers.download_limit_kbps);
        }
        if self.config.shared_folders != old.shared_folders
            || self.config.share_filters != old.share_filters
        {
            self.apply_shared_folders();
        }
        if !self.config.auto_reconnect {
            self.session.reconnect_at = None;
        }
        if self.config.login != old.login {
            if self.config.login.username.is_empty() {
                info!("login credentials cleared, disconnecting");
                self.session.reconnect_at = None;
                self.session.reconnect_now = false;
                if self.session.connected {
                    self.net.send(NetworkCommand::ServerDisconnect);
                }
            } else if self.session.connected {
                info!("login configuration changed, reconnecting");
                self.reconnect();
            } else {
                info!("login configuration changed, connecting");
                self.connect();
            }
        }
        info!("runtime configuration applied");
    }

    pub(super) fn add_interest(&mut self, thing: String, hated: bool) {
        let (list, request) = if hated {
            (
                &mut self.hated_interests,
                ServerRequest::AddThingIHate {
                    thing: thing.clone(),
                },
            )
        } else {
            (
                &mut self.liked_interests,
                ServerRequest::AddThingILike {
                    thing: thing.clone(),
                },
            )
        };
        if !list.contains(&thing) {
            list.push(thing);
        }
        self.net.server(request);
    }

    pub(super) fn remove_interest(&mut self, thing: &str, hated: bool) {
        let (list, request) = if hated {
            (
                &mut self.hated_interests,
                ServerRequest::RemoveThingIHate {
                    thing: thing.to_owned(),
                },
            )
        } else {
            (
                &mut self.liked_interests,
                ServerRequest::RemoveThingILike {
                    thing: thing.to_owned(),
                },
            )
        };
        list.retain(|entry| entry != thing);
        self.net.server(request);
    }

    pub(super) fn handle_logged_in(&mut self, username: String, banner: String) {
        self.session.logged_in = true;
        self.session.reconnect_delay = RECONNECT_INITIAL_DELAY;
        self.session.reconnect_at = None;
        self.users.watch_buddies(&self.net);
        self.net.server(ServerRequest::WatchUser {
            user: username.clone(),
        });
        let (folders, files) = match &self.sharing.index {
            Some(shares) => shares.counts(),
            None => (0, 0),
        };
        self.net
            .server(ServerRequest::SharedFoldersFiles { folders, files });
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
        let defer_requests = self.awaiting_share_index();
        self.downloads.request_queued(defer_requests);
        self.schedule_wishlist();
        self.emit(ClientEvent::LoggedIn { username, banner });
    }

    fn schedule_reconnect(&mut self, cause: &str) {
        self.session.reconnect_at =
            Some(Instant::now() + self.session.reconnect_delay + reconnect_jitter());
        warn!(delay = ?self.session.reconnect_delay, cause);
        self.session.reconnect_delay = (self.session.reconnect_delay * 2).min(RECONNECT_MAX_DELAY);
    }

    pub(super) fn handle_listen_failed(&mut self, port: u16, error: String) {
        self.session.connected = false;
        self.emit(ClientEvent::LoginFailed {
            reason: format!("cannot bind listen port {port}"),
            detail: Some(error),
        });
        if self.config.auto_reconnect {
            self.schedule_reconnect("listen port unavailable, retrying");
        }
    }

    pub(super) fn handle_server_disconnected(&mut self, manual: bool) {
        self.session.logged_in = false;
        self.session.connected = false;
        self.search_tokens.clear();
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
            self.schedule_reconnect("server connection lost, reconnecting");
        }
        self.emit(ClientEvent::Disconnected);
    }
}
