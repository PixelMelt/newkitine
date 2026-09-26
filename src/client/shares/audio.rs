use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::FileType;
use lofty::prelude::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use tracing::warn;

use crate::types::{FileAttributes, UINT32_LIMIT};

const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "ac3", "afc", "aif", "aifc", "aiff", "ape", "au", "bwav", "bwf", "dff", "dsd", "dsf",
    "dts", "flac", "m4a", "m4b", "mka", "mp1", "mp2", "mp3", "mp+", "mpc", "oga", "ogg", "opus",
    "spx", "tak", "tta", "wav", "wma", "wv",
];

const MPEG_FRAME_SEARCH_BYTES: u64 = 16 * 1024;

pub(super) fn has_audio_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        AUDIO_EXTENSIONS
            .iter()
            .any(|audio| audio.eq_ignore_ascii_case(ext))
    })
}

pub(super) fn read_attributes(path: &Path) -> FileAttributes {
    match catch_unwind(AssertUnwindSafe(|| probe_attributes(path))) {
        Ok(attributes) => attributes,
        Err(panic) => {
            let reason = panic
                .downcast_ref::<&str>()
                .map(|reason| reason.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            warn!(path = %path.display(), reason, "audio metadata parser panicked, sharing without attributes");
            FileAttributes::default()
        }
    }
}

pub(super) fn is_missing_vbr(attributes: &FileAttributes) -> bool {
    attributes.bitrate.is_some() && attributes.bit_depth.is_none() && attributes.vbr.is_none()
}

fn probe_attributes(path: &Path) -> FileAttributes {
    let mut attributes = FileAttributes::default();
    let parse_options = ParseOptions::new().read_tags(false).read_cover_art(false);
    let Ok(tagged) = Probe::open(path).and_then(|probe| probe.options(parse_options).read()) else {
        return attributes;
    };
    let properties = tagged.properties();
    attributes.bitrate = properties.audio_bitrate().filter(|&value| value > 0);
    attributes.sample_rate = properties.sample_rate().filter(|&value| value > 0);
    attributes.bit_depth = properties
        .bit_depth()
        .map(u32::from)
        .filter(|&value| value > 0);
    let duration = properties.duration().as_secs();
    if duration < UINT32_LIMIT {
        attributes.length = Some(duration as u32);
    }
    if attributes.bitrate.is_some() && attributes.bit_depth.is_none() {
        attributes.vbr = match tagged.file_type() {
            FileType::Mpeg => match mpeg_has_xing_header(path) {
                Ok(vbr) => Some(u32::from(vbr)),
                Err(error) => {
                    warn!(path = %path.display(), %error, "cannot read MPEG VBR header");
                    None
                }
            },
            _ => Some(0),
        };
    }
    attributes
}

fn mpeg_has_xing_header(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut id3 = [0u8; 10];
    file.read_exact(&mut id3)?;
    let audio_start = if &id3[..3] == b"ID3" {
        let size = id3[6..10]
            .iter()
            .fold(0u64, |size, &byte| (size << 7) | u64::from(byte & 0x7f));
        let footer = if id3[5] & 0x10 != 0 { 10 } else { 0 };
        10 + size + footer
    } else {
        0
    };
    file.seek(SeekFrom::Start(audio_start))?;
    let mut window = Vec::new();
    file.take(MPEG_FRAME_SEARCH_BYTES)
        .read_to_end(&mut window)?;
    Ok(first_frame_is_xing(&window))
}

fn first_frame_is_xing(window: &[u8]) -> bool {
    let Some(start) = window.windows(4).position(is_mpeg_frame_header) else {
        return false;
    };
    let header = &window[start..start + 4];
    let mpeg1 = (header[1] >> 3) & 0x03 == 0x03;
    let mono = (header[3] >> 6) == 0x03;
    let side_info = match (mpeg1, mono) {
        (true, false) => 32,
        (true, true) | (false, false) => 17,
        (false, true) => 9,
    };
    let tag = start + 4 + side_info;
    let Some(xing) = window.get(tag..tag + 16) else {
        return false;
    };
    if &xing[..4] != b"Xing" {
        return false;
    }
    let flags = u32::from_be_bytes(xing[4..8].try_into().unwrap());
    let frames = u32::from_be_bytes(xing[8..12].try_into().unwrap());
    let bytes = u32::from_be_bytes(xing[12..16].try_into().unwrap());
    flags & 0x03 == 0x03 && frames > 0 && bytes > 0
}

fn is_mpeg_frame_header(header: &[u8]) -> bool {
    header[0] == 0xff
        && header[1] & 0xe0 == 0xe0
        && (header[1] >> 3) & 0x03 != 0x01
        && (header[1] >> 1) & 0x03 != 0x00
        && header[2] >> 4 != 0x0f
        && header[2] >> 4 != 0x00
        && (header[2] >> 2) & 0x03 != 0x03
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(tag: &[u8; 4], flags: u32, frames: u32, bytes: u32) -> Vec<u8> {
        let mut data = vec![0u8; 3];
        data.extend([0xff, 0xfb, 0x90, 0x44]);
        data.extend([0u8; 32]);
        data.extend(tag);
        data.extend(flags.to_be_bytes());
        data.extend(frames.to_be_bytes());
        data.extend(bytes.to_be_bytes());
        data.extend([0u8; 400]);
        data
    }

    #[test]
    fn xing_header_marks_vbr() {
        assert!(first_frame_is_xing(&frame(b"Xing", 0x0f, 1000, 4_000_000)));
    }

    #[test]
    fn info_header_is_cbr() {
        assert!(!first_frame_is_xing(&frame(b"Info", 0x0f, 1000, 4_000_000)));
    }

    #[test]
    fn xing_header_without_counts_is_not_vbr() {
        assert!(!first_frame_is_xing(&frame(b"Xing", 0x0c, 0, 0)));
        assert!(!first_frame_is_xing(&frame(b"Xing", 0x0f, 0, 4_000_000)));
    }

    #[test]
    fn plain_frame_is_not_vbr() {
        assert!(!first_frame_is_xing(&[0xff, 0xfb, 0x90, 0x44, 0, 0, 0, 0]));
        assert!(!first_frame_is_xing(b"not audio at all"));
    }

    #[test]
    fn id3_prefixed_file_is_scanned_after_the_tag() {
        let path = std::env::temp_dir().join(format!("newkitine-xing-{}.mp3", std::process::id()));
        let mut data = b"ID3\x04\x00\x00\x00\x00\x00\x0a".to_vec();
        data.extend([0xffu8; 10]);
        data.extend(frame(b"Xing", 0x03, 10, 1000));
        std::fs::write(&path, data).unwrap();
        assert!(mpeg_has_xing_header(&path).unwrap());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn missing_vbr_only_for_lossy_bitrate() {
        let lossy = FileAttributes {
            bitrate: Some(320),
            ..Default::default()
        };
        assert!(is_missing_vbr(&lossy));
        assert!(!is_missing_vbr(&FileAttributes {
            vbr: Some(0),
            ..lossy.clone()
        }));
        assert!(!is_missing_vbr(&FileAttributes {
            bit_depth: Some(16),
            ..lossy
        }));
        assert!(!is_missing_vbr(&FileAttributes::default()));
    }
}
