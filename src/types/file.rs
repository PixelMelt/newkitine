pub const UINT32_LIMIT: u64 = 4294967295;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileAttributes {
    pub bitrate: Option<u32>,
    pub length: Option<u32>,
    pub vbr: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
}

impl FileAttributes {
    pub fn wire_pairs(&self) -> [Option<(u32, u32)>; 3] {
        match self.bit_depth {
            Some(bit_depth) => [
                self.length.map(|value| (1, value)),
                self.sample_rate.map(|value| (4, value)),
                Some((5, bit_depth)),
            ],
            None => [
                self.bitrate.map(|value| (0, value)),
                self.length.map(|value| (1, value)),
                self.bitrate.and(self.vbr).map(|value| (2, value)),
            ],
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(attributes: &FileAttributes) -> Vec<(u32, u32)> {
        attributes.wire_pairs().into_iter().flatten().collect()
    }

    #[test]
    fn lossless_advertises_duration_sample_rate_and_bit_depth() {
        let attributes = FileAttributes {
            bitrate: Some(1411),
            length: Some(200),
            vbr: Some(0),
            sample_rate: Some(44100),
            bit_depth: Some(16),
        };
        assert_eq!(pairs(&attributes), vec![(1, 200), (4, 44100), (5, 16)]);
    }

    #[test]
    fn lossy_advertises_bitrate_duration_and_vbr() {
        let attributes = FileAttributes {
            bitrate: Some(245),
            length: Some(200),
            vbr: Some(1),
            sample_rate: Some(44100),
            bit_depth: None,
        };
        assert_eq!(pairs(&attributes), vec![(0, 245), (1, 200), (2, 1)]);
        let no_bitrate = FileAttributes {
            bitrate: None,
            ..attributes
        };
        assert_eq!(pairs(&no_bitrate), vec![(1, 200)]);
        assert!(pairs(&FileAttributes::default()).is_empty());
    }
}
