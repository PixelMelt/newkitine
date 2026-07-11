use super::wire::{MessageReader, MessageWriter, ProtocolError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistributedMessage {
    Ping,
    Search {
        identifier: u32,
        search_username: String,
        token: u32,
        search_term: String,
    },
    BranchLevel {
        level: i32,
    },
    BranchRoot {
        root_username: String,
    },
    ChildDepth {
        value: u32,
    },
    EmbeddedMessage {
        distrib_code: u8,
        distrib_message: Vec<u8>,
    },
}

impl DistributedMessage {
    pub fn code(&self) -> u8 {
        match self {
            Self::Ping => 0,
            Self::Search { .. } => 3,
            Self::BranchLevel { .. } => 4,
            Self::BranchRoot { .. } => 5,
            Self::ChildDepth { .. } => 7,
            Self::EmbeddedMessage { .. } => 93,
        }
    }

    pub fn write_payload(&self, w: &mut MessageWriter) {
        match self {
            Self::Ping => {}
            Self::Search {
                identifier,
                search_username,
                token,
                search_term,
            } => {
                w.write_u32(*identifier);
                w.write_string(search_username);
                w.write_u32(*token);
                w.write_string(search_term);
            }
            Self::BranchLevel { level } => w.write_i32(*level),
            Self::BranchRoot { root_username } => w.write_string(root_username),
            Self::ChildDepth { value } => w.write_u32(*value),
            Self::EmbeddedMessage {
                distrib_code,
                distrib_message,
            } => {
                w.write_u8(*distrib_code);
                w.write_raw(distrib_message);
            }
        }
    }

    pub fn parse(code: u8, payload: &[u8]) -> Result<Self, ProtocolError> {
        let r = &mut MessageReader::new(payload);
        Ok(match code {
            0 => Self::Ping,
            3 => Self::Search {
                identifier: r.read_u32()?,
                search_username: r.read_string()?,
                token: r.read_u32()?,
                search_term: r.read_string()?,
            },
            4 => Self::BranchLevel {
                level: r.read_i32()?,
            },
            5 => Self::BranchRoot {
                root_username: r.read_string()?,
            },
            7 => Self::ChildDepth {
                value: r.read_u32()?,
            },
            93 => {
                if payload.len() >= 3 && &payload[..3] == b"\x00\x00\x00" {
                    r.skip(3)?;
                }
                Self::EmbeddedMessage {
                    distrib_code: r.read_u8()?,
                    distrib_message: r.rest().to_vec(),
                }
            }
            _ => {
                return Err(ProtocolError::UnknownMessageCode {
                    family: "distributed",
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
