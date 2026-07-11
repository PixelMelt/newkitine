use super::wire::{MessageReader, MessageWriter, ProtocolError};
use crate::types::ConnectionType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerInitMessage {
    PierceFireWall { token: u32 },
    PeerInit { username: String, conn_type: ConnectionType },
}

impl PeerInitMessage {
    pub fn code(&self) -> u8 {
        match self {
            Self::PierceFireWall { .. } => 0,
            Self::PeerInit { .. } => 1,
        }
    }

    pub fn write_payload(&self, w: &mut MessageWriter) {
        match self {
            Self::PierceFireWall { token } => w.write_u32(*token),
            Self::PeerInit { username, conn_type } => {
                w.write_string(username);
                w.write_string(conn_type.as_str());
                w.write_u32(0);
            }
        }
    }

    pub fn parse(code: u8, payload: &[u8]) -> Result<Self, ProtocolError> {
        let r = &mut MessageReader::new(payload);
        Ok(match code {
            0 => Self::PierceFireWall { token: r.read_u32()? },
            1 => Self::PeerInit {
                username: r.read_string()?,
                conn_type: ConnectionType::from_str_value(&r.read_string()?)?,
            },
            _ => {
                return Err(ProtocolError::UnknownMessageCode {
                    family: "peer_init",
                    code: code as u32,
                });
            }
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = MessageWriter::new();
        self.write_payload(&mut w);
        let payload = w.into_bytes();
        let mut framed = MessageWriter::new();
        framed.write_u32(payload.len() as u32 + 1);
        framed.write_u8(self.code());
        framed.write_raw(&payload);
        framed.into_bytes()
    }
}
