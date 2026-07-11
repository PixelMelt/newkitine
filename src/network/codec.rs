use tokio::io::{AsyncRead, AsyncReadExt};

use crate::protocol::ProtocolError;

pub const MAX_MESSAGE_SIZE_LARGE: usize = 469762048;
pub const MAX_MESSAGE_SIZE_MEDIUM: usize = 16777216;
pub const MAX_MESSAGE_SIZE_SMALL: usize = 16384;

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
    let mut payload = vec![0u8; size - 1];
    reader.read_exact(&mut payload).await?;
    Ok((code, payload))
}
