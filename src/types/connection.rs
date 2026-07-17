#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum ConnectionType {
    Peer,
    File,
    Distributed,
}
