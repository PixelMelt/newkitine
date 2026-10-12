use tracing::warn;

use super::Actor;
use crate::network::ConnId;
use crate::network::NetworkEvent;
use crate::network::conn::FileEvent;
use crate::types::TransferDirection;

impl Actor {
    fn file_transfer_identity(
        &mut self,
        conn_id: ConnId,
    ) -> Option<(String, u32, TransferDirection)> {
        let conn = self.peers.get(conn_id)?;
        let Some((token, direction)) = conn.file_transfer else {
            warn!(conn_id, "file event without transfer identity, closing");
            self.close_conn(conn_id);
            return None;
        };
        Some((conn.username().to_owned(), token, direction))
    }

    pub(super) fn handle_file_event(&mut self, conn_id: ConnId, event: FileEvent) {
        match event {
            FileEvent::Init { token } => {
                let Some(conn) = self.peers.get_mut(conn_id) else {
                    return;
                };
                let username = conn.username().to_owned();
                conn.file_transfer = Some((token, TransferDirection::Download));
                self.emit(NetworkEvent::FileTransferInit {
                    username,
                    token,
                    conn_id,
                    direction: TransferDirection::Download,
                });
            }
            FileEvent::DownloadProgress { bytes_left } => {
                self.emit(NetworkEvent::FileDownloadProgress {
                    conn_id,
                    bytes_left,
                });
            }
            FileEvent::UploadProgress { offset, bytes_sent } => {
                if let Some((username, token, _)) = self.file_transfer_identity(conn_id) {
                    self.emit(NetworkEvent::FileUploadProgress {
                        username,
                        token,
                        offset,
                        bytes_sent,
                    });
                }
            }
            FileEvent::Error(error) => {
                if let Some((username, token, direction)) = self.file_transfer_identity(conn_id) {
                    self.emit(NetworkEvent::FileTransferError {
                        username,
                        token,
                        conn_id,
                        direction,
                        error,
                    });
                }
            }
        }
    }
}
