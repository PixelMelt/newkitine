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

    pub fn from_str_value(value: &str) -> Option<Self> {
        match value {
            "P" => Some(Self::Peer),
            "F" => Some(Self::File),
            "D" => Some(Self::Distributed),
            _ => None,
        }
    }
}
