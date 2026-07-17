use std::net::Ipv4Addr;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("unexpected end of message at offset {offset}, needed {needed} bytes of {available}")]
    UnexpectedEof {
        offset: usize,
        needed: usize,
        available: usize,
    },
    #[error("unknown {family} message code {code}")]
    UnknownMessageCode { family: &'static str, code: u32 },
    #[error("decompression failed: {0}")]
    Decompress(#[from] std::io::Error),
    #[error("decompressed payload exceeds {limit} bytes")]
    DecompressedTooLarge { limit: usize },
    #[error("{what} count {count} exceeds limit {limit}")]
    TooManyEntries {
        what: &'static str,
        count: usize,
        limit: usize,
    },
    #[error("invalid value for {field}: {value}")]
    InvalidValue { field: &'static str, value: u64 },
    #[error("invalid connection type {value:?}")]
    InvalidConnectionType { value: String },
}

fn decode_legacy_latin1(content: &[u8]) -> String {
    tracing::debug!(
        len = content.len(),
        "non-utf8 protocol string, decoding as latin-1 for legacy clients"
    );
    content.iter().map(|&b| b as char).collect()
}

pub struct MessageReader<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> MessageReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(len)
            .filter(|&end| end <= self.buf.len())
            .ok_or(ProtocolError::UnexpectedEof {
                offset: self.offset,
                needed: len,
                available: self.buf.len(),
            })?;
        let slice = &self.buf[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    pub fn has_remaining(&self) -> bool {
        self.offset < self.buf.len()
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.offset
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let slice = &self.buf[self.offset..];
        self.offset = self.buf.len();
        slice
    }

    pub fn skip(&mut self, len: usize) -> Result<(), ProtocolError> {
        self.take(len).map(|_| ())
    }

    pub fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }

    pub fn read_bool(&mut self) -> Result<bool, ProtocolError> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn read_i32(&mut self) -> Result<i32, ProtocolError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn read_u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn read_u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn read_ip(&mut self) -> Result<Ipv4Addr, ProtocolError> {
        Ok(Ipv4Addr::from(self.read_u32()?))
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>, ProtocolError> {
        let len = self.read_u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    pub fn read_string(&mut self) -> Result<String, ProtocolError> {
        let len = self.read_u32()? as usize;
        let content = self.take(len)?;
        Ok(match std::str::from_utf8(content) {
            Ok(s) => s.to_owned(),
            Err(_) => decode_legacy_latin1(content),
        })
    }

    pub fn read_file_size(&mut self) -> Result<u64, ProtocolError> {
        let size_bytes = self.take(8)?;
        if size_bytes[7] == 255 {
            Ok(u32::from_le_bytes(size_bytes[..4].try_into().unwrap()) as u64)
        } else {
            Ok(u64::from_le_bytes(size_bytes.try_into().unwrap()))
        }
    }
}

#[derive(Default)]
pub struct MessageWriter {
    buf: Vec<u8>,
}

impl MessageWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn write_u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    pub fn write_bool(&mut self, value: bool) {
        self.buf.push(value as u8);
    }

    pub fn write_u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_i32(&mut self, value: i32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_ip(&mut self, value: Ipv4Addr) {
        self.write_u32(u32::from(value));
    }

    pub fn write_bytes(&mut self, content: &[u8]) {
        self.write_u32(content.len() as u32);
        self.buf.extend_from_slice(content);
    }

    pub fn write_string(&mut self, content: &str) {
        self.write_bytes(content.as_bytes());
    }

    pub fn write_string_legacy(&mut self, content: &str) {
        if content.chars().all(|c| (c as u32) <= 0xFF) {
            let encoded: Vec<u8> = content.chars().map(|c| c as u8).collect();
            self.write_bytes(&encoded);
        } else {
            self.write_bytes(content.as_bytes());
        }
    }

    pub fn write_raw(&mut self, content: &[u8]) {
        self.buf.extend_from_slice(content);
    }
}
