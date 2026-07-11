use std::io::Read;

use flate2::Compression;
use flate2::read::{ZlibDecoder, ZlibEncoder};

use super::wire::ProtocolError;

pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(data, Compression::new(4));
    let mut out = Vec::new();
    encoder.read_to_end(&mut out).unwrap();
    out
}

pub fn decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, ProtocolError> {
    let mut decoder = ZlibDecoder::new(data).take(limit as u64 + 1);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    if out.len() > limit {
        return Err(ProtocolError::DecompressedTooLarge { limit });
    }
    Ok(out)
}
