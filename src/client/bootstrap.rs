use std::path::PathBuf;

use crate::types::{RuntimeConfig, TransferSnapshot};

#[derive(Debug, Clone)]
pub struct ClientBootstrap {
    pub runtime: RuntimeConfig,
    pub share_catalog: PathBuf,
    pub scan_on_startup: bool,
    pub connect_on_startup: bool,
    pub buddies: Vec<String>,
    pub banned: Vec<String>,
    pub ignored: Vec<String>,
    pub ip_bans: Vec<String>,
    pub wishlist: Vec<String>,
    pub liked_interests: Vec<String>,
    pub hated_interests: Vec<String>,
    pub transfers: Vec<TransferSnapshot>,
}
