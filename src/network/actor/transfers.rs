use tracing::warn;

use super::Actor;
use crate::network::ConnId;
use crate::network::NetworkEvent;
use crate::network::conn::ConnEvent;

impl Actor {
    fn file_transfer_identity(&mut self, conn_id: ConnId) -> Option<(String, u32)> {
        let conn = self.peers.get(conn_id)?;
        match (&conn.identity, conn.file_token) {
            (Some(identity), Some(token)) => Some((identity.username.clone(), token)),
            _ => {
                warn!(conn_id, "file event without transfer identity, closing");
                self.close_conn(conn_id);
                None
            }
        }
    }

    pub(super) fn handle_file_event(&mut self, event: ConnEvent) {
        match event {
            ConnEvent::FileInit { conn_id, token } => {
                let Some(conn) = self.peers.get_mut(conn_id) else {
                    return;
                };
                let Some(identity) = conn.identity.clone() else {
                    warn!(conn_id, "file init on unidentified connection, closing");
                    self.close_conn(conn_id);
                    return;
                };
                conn.file_token = Some(token);
                self.emit(NetworkEvent::FileTransferInit {
                    username: identity.username,
                    token,
                    conn_id,
                });
            }
            ConnEvent::FileOffsetReceived { conn_id, offset } => {
                if let Some((username, token)) = self.file_transfer_identity(conn_id) {
                    self.emit(NetworkEvent::FileUploadProgress {
                        username,
                        token,
                        offset,
                        bytes_sent: 0,
                    });
                }
            }
            ConnEvent::DownloadProgress {
                conn_id,
                bytes_left,
            } => {
                if let Some((username, token)) = self.file_transfer_identity(conn_id) {
                    self.emit(NetworkEvent::FileDownloadProgress {
                        username,
                        token,
                        bytes_left,
                    });
                }
            }
            ConnEvent::UploadProgress {
                conn_id,
                offset,
                bytes_sent,
            } => {
                if let Some((username, token)) = self.file_transfer_identity(conn_id) {
                    self.emit(NetworkEvent::FileUploadProgress {
                        username,
                        token,
                        offset,
                        bytes_sent,
                    });
                }
            }
            ConnEvent::FileDone { conn_id } => {
                self.close_conn(conn_id);
            }
            ConnEvent::FileError { conn_id, error } => {
                if let Some((username, token)) = self.file_transfer_identity(conn_id) {
                    self.emit(NetworkEvent::FileTransferError {
                        username,
                        token,
                        error,
                    });
                }
            }
            _ => unreachable!("non-file connection event routed to file handler"),
        }
    }
}
