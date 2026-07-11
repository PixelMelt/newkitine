use crate::types::{RuntimeConfig, TransferSeed};

#[derive(Debug, Clone)]
pub struct ClientBootstrap {
    pub runtime: RuntimeConfig,
    pub buddies: Vec<String>,
    pub banned: Vec<String>,
    pub ignored: Vec<String>,
    pub ip_bans: Vec<String>,
    pub wishlist: Vec<String>,
    pub liked_interests: Vec<String>,
    pub hated_interests: Vec<String>,
    pub transfers: Vec<TransferSeed>,
}
