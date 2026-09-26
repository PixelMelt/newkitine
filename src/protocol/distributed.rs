use super::wire::{MessageReader, MessageWriter, ProtocolError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistributedMessage {
    Ping,
    Search(DistributedSearch),
    BranchLevel { level: i32 },
    BranchRoot { root_username: String },
    ChildDepth { value: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributedSearch {
    pub identifier: u32,
    pub search_username: String,
    pub token: u32,
    pub search_term: String,
    payload: Vec<u8>,
}

impl DistributedSearch {
    pub const CODE: u8 = 3;

    pub fn parse(payload: &[u8]) -> Result<Self, ProtocolError> {
        let r = &mut MessageReader::new(payload);
        Ok(Self {
            identifier: r.read_u32()?,
            search_username: r.read_string()?,
            token: r.read_u32()?,
            search_term: r.read_string()?,
            payload: payload.to_vec(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        frame(Self::CODE, &self.payload)
    }
}

impl DistributedMessage {
    pub fn code(&self) -> u8 {
        match self {
            Self::Ping => 0,
            Self::Search(_) => DistributedSearch::CODE,
            Self::BranchLevel { .. } => 4,
            Self::BranchRoot { .. } => 5,
            Self::ChildDepth { .. } => 7,
        }
    }

    pub fn write_payload(&self, w: &mut MessageWriter) {
        match self {
            Self::Ping => {}
            Self::Search(search) => w.write_raw(&search.payload),
            Self::BranchLevel { level } => w.write_i32(*level),
            Self::BranchRoot { root_username } => w.write_string(root_username),
            Self::ChildDepth { value } => w.write_u32(*value),
        }
    }

    pub fn parse(code: u8, payload: &[u8]) -> Result<Self, ProtocolError> {
        let r = &mut MessageReader::new(payload);
        Ok(match code {
            0 => Self::Ping,
            DistributedSearch::CODE => Self::Search(DistributedSearch::parse(payload)?),
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
                if payload.starts_with(b"\x00\x00\x00") {
                    r.skip(3)?;
                }
                let distrib_code = r.read_u8()?;
                if distrib_code != DistributedSearch::CODE {
                    return Err(ProtocolError::UnknownMessageCode {
                        family: "embedded distributed",
                        code: distrib_code as u32,
                    });
                }
                Self::Search(DistributedSearch::parse(r.rest())?)
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
        frame(self.code(), &w.into_bytes())
    }
}

fn frame(code: u8, payload: &[u8]) -> Vec<u8> {
    let mut framed = MessageWriter::new();
    framed.write_u32(payload.len() as u32 + 1);
    framed.write_u8(code);
    framed.write_raw(payload);
    framed.into_bytes()
}
