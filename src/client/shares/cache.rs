use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, ErrorKind, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use tracing::{error, warn};

use crate::types::FileAttributes;

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CacheEntry {
    pub(super) size: u64,
    pub(super) mtime: u64,
    pub(super) attributes: FileAttributes,
}

pub(super) fn load(path: &Path) -> HashMap<String, CacheEntry> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return HashMap::new(),
        Err(error) => {
            warn!(path = %path.display(), %error, "cannot open attribute cache, reading all attributes");
            return HashMap::new();
        }
    };
    match serde_json::from_reader(GzDecoder::new(BufReader::new(file))) {
        Ok(entries) => entries,
        Err(error) => {
            warn!(path = %path.display(), %error, "corrupt attribute cache, reading all attributes");
            HashMap::new()
        }
    }
}

pub(crate) fn save(path: &Path, entries: &HashMap<String, CacheEntry>) {
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp{}",
        TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = write(&tmp, entries).and_then(|()| fs::rename(&tmp, path));
    if let Err(error) = result {
        error!(path = %path.display(), %error, "cannot persist attribute cache");
    }
}

fn write(path: &Path, entries: &HashMap<String, CacheEntry>) -> std::io::Result<()> {
    let mut encoder = GzEncoder::new(BufWriter::new(File::create(path)?), Compression::default());
    serde_json::to_writer(&mut encoder, entries)?;
    encoder.finish()?.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "newkitine-cache-{}-{}.json.gz",
            std::process::id(),
            name
        ))
    }

    #[test]
    fn roundtrip() {
        let path = temp_path("roundtrip");
        let mut entries = HashMap::new();
        entries.insert(
            "/music/song.mp3".to_owned(),
            CacheEntry {
                size: 300,
                mtime: 1234567890,
                attributes: FileAttributes {
                    bitrate: Some(320),
                    length: Some(210),
                    ..Default::default()
                },
            },
        );
        save(&path, &entries);
        assert_eq!(load(&path), entries);
    }

    #[test]
    fn missing_file_is_empty() {
        assert!(load(&temp_path("missing")).is_empty());
    }

    #[test]
    fn corrupt_file_is_empty() {
        let path = temp_path("corrupt");
        fs::write(&path, b"not gzip at all").unwrap();
        assert!(load(&path).is_empty());
    }
}
