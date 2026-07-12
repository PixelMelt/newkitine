use tokio::io::{AsyncRead, AsyncReadExt};

use crate::protocol::ProtocolError;

pub const MAX_SERVER_MESSAGE_SIZE: usize = 16777216;
pub const MAX_PEER_MESSAGE_SIZE: usize = 16777216;
pub const MAX_LARGE_RESPONSE_SIZE: usize = 469762048;
pub const MAX_CONTROL_MESSAGE_SIZE: usize = 16384;

const INITIAL_PAYLOAD_CAPACITY: usize = 65536;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("message size {size} exceeds limit {limit}")]
    TooLarge { size: usize, limit: usize },
    #[error("frame too short for message code")]
    Truncated,
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

pub async fn read_payload<R: AsyncRead + Unpin>(
    reader: &mut R,
    size: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let mut payload = Vec::with_capacity(size.min(INITIAL_PAYLOAD_CAPACITY));
    let received = reader.take(size as u64).read_to_end(&mut payload).await?;
    if received < size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "connection closed mid-frame",
        ));
    }
    Ok(payload)
}

pub async fn read_frame_u8<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_size: usize,
) -> Result<(u8, Vec<u8>), FrameError> {
    let size = reader.read_u32_le().await? as usize;
    if size > max_size {
        return Err(FrameError::TooLarge {
            size,
            limit: max_size,
        });
    }
    if size < 1 {
        return Err(FrameError::Truncated);
    }
    let code = reader.read_u8().await?;
    let payload = read_payload(reader, size - 1).await?;
    Ok((code, payload))
}
