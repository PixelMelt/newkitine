use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedFolder {
    pub virtual_name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub buddy_only: bool,
}
