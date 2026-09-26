use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use tracing::{error, warn};

use crate::types::FileAttributes;

use super::wire::{write_string_to, write_u32_to};
use super::{ShareCatalog, ShareCatalogFile};

const FORMAT_VERSION: u32 = 1;

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
    write_u32_to(writer, FORMAT_VERSION)?;
    write_u32_to(writer, catalog.folders.len() as u32)?;
    for folder in &catalog.folders {
        write_string_to(writer, &folder.virtual_path)?;
        write_bytes_to(writer, folder.real_path.as_os_str().as_bytes())?;
        writer.write_all(&[folder.buddy_only as u8])?;
        write_u32_to(writer, folder.files.len() as u32)?;
        for file in catalog.folder_files(folder) {
            write_string_to(writer, &file.name)?;
            write_bytes_to(writer, file.real_name.as_bytes())?;
            writer.write_all(&file.size.to_le_bytes())?;
            writer.write_all(&file.mtime.to_le_bytes())?;
            write_attributes_to(writer, &file.attributes)?;
        }
    }
    Ok(())
}

fn write_bytes_to(writer: &mut impl Write, value: &[u8]) -> io::Result<()> {
    write_u32_to(writer, value.len() as u32)?;
    writer.write_all(value)
}

fn write_attributes_to(writer: &mut impl Write, attributes: &FileAttributes) -> io::Result<()> {
    let values = [
        (0u32, attributes.bitrate),
        (1, attributes.length),
        (2, attributes.vbr),
        (4, attributes.sample_rate),
        (5, attributes.bit_depth),
    ];
    write_u32_to(
        writer,
        values.iter().filter(|(_, value)| value.is_some()).count() as u32,
    )?;
    for (kind, value) in values {
        if let Some(value) = value {
            write_u32_to(writer, kind)?;
            write_u32_to(writer, value)?;
        }
    }
    Ok(())
}

fn decode(reader: &mut impl Read) -> io::Result<ShareCatalog> {
    let version = read_u32(reader)?;
    if version != FORMAT_VERSION {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("share catalog cache version {version}, expected {FORMAT_VERSION}"),
        ));
    }
    let mut catalog = ShareCatalog::empty();
    let folder_count = read_u32(reader)?;
    for _ in 0..folder_count {
        let virtual_path = read_string(reader)?;
        let real_path = PathBuf::from(OsString::from_vec(read_bytes(reader)?));
        let buddy_only = read_u8(reader)? != 0;
        let file_count = read_u32(reader)?;
        let mut files = Vec::with_capacity(file_count.min(65536) as usize);
        for _ in 0..file_count {
            let name = read_string(reader)?;
            let real_name = OsString::from_vec(read_bytes(reader)?);
            let size = read_u64(reader)?;
            let mtime = read_u64(reader)?;
            let attributes = read_attributes(reader)?;
            files.push(ShareCatalogFile::new(
                name, real_name, size, mtime, attributes,
            ));
        }
        catalog.push_folder(virtual_path, real_path, buddy_only, files);
    }
    Ok(catalog)
}

fn read_u8(reader: &mut impl Read) -> io::Result<u8> {
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_bytes(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let len = read_u32(reader)? as usize;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

fn read_string(reader: &mut impl Read) -> io::Result<String> {
    String::from_utf8(read_bytes(reader)?)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
}

fn read_attributes(reader: &mut impl Read) -> io::Result<FileAttributes> {
    let mut attributes = FileAttributes::default();
    for _ in 0..read_u32(reader)? {
        let kind = read_u32(reader)?;
        let value = Some(read_u32(reader)?);
        match kind {
            0 => attributes.bitrate = value,
            1 => attributes.length = value,
            2 => attributes.vbr = value,
            4 => attributes.sample_rate = value,
            5 => attributes.bit_depth = value,
            other => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("unknown file attribute {other} in share catalog cache"),
                ));
            }
        }
    }
    Ok(attributes)
}

#[cfg(test)]
mod tests {
    use super::*;

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
