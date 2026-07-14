mod compress;
mod distributed;
mod file;
mod peer;
mod peer_init;
mod server_request;
mod server_response;
mod wire;

pub use compress::{compress, decompress};
pub use distributed::DistributedMessage;
pub use file::{FileOffset, FileTransferInit};
pub use peer::{PeerMessage, encode_shared_file_list};
pub use peer_init::PeerInitMessage;
pub use server_request::ServerRequest;
pub use server_response::{LoginOutcome, ParentCandidate, ServerResponse};
pub use wire::{MessageReader, MessageWriter, ProtocolError};

pub const LOGIN_VERSION: u32 = 160;
pub const LOGIN_MINOR_VERSION: u32 = 3;

pub fn initial_token() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    nanos % (u32::MAX / 1000)
}

pub fn increment_token(token: u32) -> u32 {
    token.wrapping_add(1).max(1)
}
