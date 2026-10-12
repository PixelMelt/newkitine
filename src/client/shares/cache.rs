use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Deserializer, Serializer};
use tracing::{error, warn};

use super::ShareCatalog;

const FORMAT_VERSION: u32 = 2;
const DECODE_BUFFER: usize = 64 * 1024;

pub(super) mod path_bytes {
    use super::*;

    pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(path.as_os_str().as_bytes())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
        Vec::deserialize(deserializer).map(|bytes| OsString::from_vec(bytes).into())
    }
}

pub fn load(path: &Path) -> ShareCatalog {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return ShareCatalog::empty(),
        Err(error) => {
            warn!(path = %path.display(), %error, "cannot open share catalog cache, starting empty");
            return ShareCatalog::empty();
        }
    };
    match decode(&mut BufReader::new(GzDecoder::new(BufReader::new(file)))) {
        Ok(catalog) => catalog,
        Err(error) => {
            warn!(path = %path.display(), %error, "corrupt share catalog cache, starting empty");
            ShareCatalog::empty()
        }
    }
}

pub fn save(path: &Path, catalog: &ShareCatalog) {
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp{}",
        TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = write(&tmp, catalog).and_then(|()| fs::rename(&tmp, path));
    if let Err(error) = result {
        error!(path = %path.display(), %error, "cannot persist share catalog cache");
        if let Err(error) = fs::remove_file(&tmp)
            && error.kind() != ErrorKind::NotFound
        {
            warn!(path = %tmp.display(), %error, "cannot remove partial share catalog cache");
        }
    }
}

fn write(path: &Path, catalog: &ShareCatalog) -> io::Result<()> {
    let mut writer = BufWriter::new(GzEncoder::new(File::create(path)?, Compression::fast()));
    encode(&mut writer, catalog)?;
    writer
        .into_inner()
        .map_err(|error| error.into_error())?
        .finish()?
        .flush()?;
    Ok(())
}

fn encode(writer: &mut impl Write, catalog: &ShareCatalog) -> io::Result<()> {
    postcard::to_io(&(FORMAT_VERSION, catalog), writer)
        .map(drop)
        .map_err(io::Error::other)
}

fn decode(reader: &mut impl Read) -> io::Result<ShareCatalog> {
    let mut buffer = vec![0; DECODE_BUFFER];
    let (version, rest) =
        postcard::from_io::<u32, _>((reader, &mut buffer)).map_err(io::Error::other)?;
    if version != FORMAT_VERSION {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("share catalog cache version {version}, expected {FORMAT_VERSION}"),
        ));
    }
    postcard::from_io(rest)
        .map(|(catalog, _)| catalog)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::shares::ShareCatalogFile;
    use crate::types::FileAttributes;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "newkitine-catalog-{}-{}.gz",
            std::process::id(),
            name
        ))
    }

    fn sample() -> ShareCatalog {
        let mut catalog = ShareCatalog::empty();
        catalog.push_folder(
            "Music\\Album".into(),
            PathBuf::from("/music/album"),
            true,
            [
                ShareCatalogFile::new(
                    "song.mp3".into(),
                    OsString::from("song.mp3"),
                    300,
                    1234567890,
                    FileAttributes {
                        bitrate: Some(320),
                        length: Some(210),
                        ..Default::default()
                    },
                ),
                ShareCatalogFile::new(
                    "odd\u{fffd}name.flac".into(),
                    OsString::from_vec(b"odd\xffname.flac".to_vec()),
                    1,
                    2,
                    FileAttributes::default(),
                ),
            ],
        );
        catalog.push_folder("Music".into(), PathBuf::from("/music"), false, []);
        catalog
    }

    #[test]
    fn roundtrip() {
        let path = temp_path("roundtrip");
        let catalog = sample();
        save(&path, &catalog);
        let loaded = load(&path);
        assert_eq!(loaded.folders.len(), 2);
        assert_eq!(loaded.files.len(), 2);
        for (left, right) in catalog.folders.iter().zip(&loaded.folders) {
            assert_eq!(left.virtual_path, right.virtual_path);
            assert_eq!(left.virtual_path_lower, right.virtual_path_lower);
            assert_eq!(left.real_path, right.real_path);
            assert_eq!(left.files, right.files);
            assert_eq!(left.buddy_only, right.buddy_only);
        }
        for (left, right) in catalog.files.iter().zip(&loaded.files) {
            assert_eq!(left.name, right.name);
            assert_eq!(left.name_lower, right.name_lower);
            assert_eq!(left.real_name, right.real_name);
            assert_eq!(left.size, right.size);
            assert_eq!(left.mtime, right.mtime);
            assert_eq!(left.attributes, right.attributes);
        }
    }

    #[test]
    fn write_failure_is_an_error_not_a_panic() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(ErrorKind::StorageFull, "disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(encode(&mut Full, &sample()).is_err());
    }

    #[test]
    fn missing_file_is_empty() {
        assert!(load(&temp_path("missing")).folders.is_empty());
    }

    #[test]
    fn corrupt_file_is_empty() {
        let path = temp_path("corrupt");
        fs::write(&path, b"not gzip at all").unwrap();
        assert!(load(&path).folders.is_empty());
    }
}
