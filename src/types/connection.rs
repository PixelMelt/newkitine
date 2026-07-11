use crate::protocol::ProtocolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum ConnectionType {
    Peer,
    File,
    Distributed,
}

impl ConnectionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Peer => "P",
            Self::File => "F",
            Self::Distributed => "D",
        }
    }

    pub fn from_str_value(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "P" => Ok(Self::Peer),
            "F" => Ok(Self::File),
            "D" => Ok(Self::Distributed),
            _ => Err(ProtocolError::InvalidValue {
                field: "conn_type",
                value: value.bytes().next().unwrap_or(0) as u64,
            }),
        }
    }
}
