pub const UINT32_LIMIT: u64 = 4294967295;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileAttributes {
    pub bitrate: Option<u32>,
    pub length: Option<u32>,
    pub vbr: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub attributes: FileAttributes,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FolderContents {
    pub directory: String,
    pub files: Vec<FileInfo>,
}
