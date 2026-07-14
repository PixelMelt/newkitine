use std::ffi::OsString;
use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::FileAttributes;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedFolder {
    pub virtual_name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub buddy_only: bool,
}

#[derive(Debug)]
pub struct ShareCatalog {
    pub folders: Vec<ShareCatalogFolder>,
    pub files: Vec<ShareCatalogFile>,
    pub folders_by_path: Vec<u32>,
}

#[derive(Debug)]
pub struct ShareCatalogFolder {
    pub virtual_path: Box<str>,
    pub virtual_path_lower: Box<str>,
    pub real_path: PathBuf,
    pub files: Range<u32>,
    pub buddy_only: bool,
}

#[derive(Debug)]
pub struct ShareCatalogFile {
    pub name: Box<str>,
    pub name_lower: Box<str>,
    pub real_name: OsString,
    pub size: u64,
    pub attributes: FileAttributes,
}
