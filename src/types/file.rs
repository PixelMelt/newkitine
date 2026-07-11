pub const UINT32_LIMIT: u64 = 4294967295;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAttributeType {
    Bitrate,
    Length,
    Vbr,
    Encoder,
    SampleRate,
    BitDepth,
}

impl FileAttributeType {
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Bitrate => 0,
            Self::Length => 1,
            Self::Vbr => 2,
            Self::Encoder => 3,
            Self::SampleRate => 4,
            Self::BitDepth => 5,
        }
    }
}

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
    pub code: u8,
    pub name: String,
    pub size: u64,
    pub attributes: FileAttributes,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FolderContents {
    pub directory: String,
    pub files: Vec<FileInfo>,
}
