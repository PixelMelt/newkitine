use std::net::Ipv4Addr;

use crate::types::ConnectionType;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Observation {
    SearchSeen {
        username: String,
        query: String,
        matched: bool,
    },
    QueueRequest {
        username: String,
        virtual_path: String,
        accepted: bool,
    },
    BrowseRequest {
        username: String,
    },
    UserInfoRequest {
        username: String,
    },
    FolderContentsRequest {
        username: String,
    },
    PeerConnected {
        username: String,
        ip: Option<Ipv4Addr>,
        conn_type: ConnectionType,
    },
}
