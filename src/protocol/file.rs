use super::wire::{MessageReader, MessageWriter, ProtocolError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileTransferInit {
    pub token: u32,
}

impl FileTransferInit {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = MessageWriter::new();
        w.write_u32(self.token);
        w.into_bytes()
    }

    pub fn parse(payload: &[u8]) -> Result<Self, ProtocolError> {
        let r = &mut MessageReader::new(payload);
        Ok(Self { token: r.read_u32()? })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileOffset {
    pub offset: u64,
}

impl FileOffset {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = MessageWriter::new();
        w.write_u64(self.offset);
        w.into_bytes()
    }

    pub fn parse(payload: &[u8]) -> Result<Self, ProtocolError> {
        let r = &mut MessageReader::new(payload);
        Ok(Self { offset: r.read_u64()? })
    }
}
