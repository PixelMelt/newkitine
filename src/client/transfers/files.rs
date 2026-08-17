use std::ffi::CString;
use std::fs;
use std::io::Seek;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use tracing::debug;

const DEFAULT_BASENAME_BYTE_LIMIT: usize = 255;

pub(super) fn clean_file_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\u{0}'..='\u{1f}'
            ) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stripped = replaced.trim_start_matches(' ').trim_end_matches(['.', ' ']);
    if stripped.is_empty() {
        return "_".repeat(replaced.chars().count());
    }
    stripped.to_owned()
}

fn join_clean(base: &Path, parts: impl IntoIterator<Item = impl AsRef<str>>) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in parts {
        let part = part.as_ref();
        if !part.is_empty() {
            path.push(clean_file_name(part));
        }
    }
    path
}

fn virtual_folder(virtual_path: &str) -> &str {
    virtual_path
        .rsplit_once('\\')
        .map_or("", |(folder, _)| folder)
}

pub(super) fn virtual_basename(virtual_path: &str) -> &str {
    virtual_path
        .rsplit_once('\\')
        .map_or(virtual_path, |(_, basename)| basename)
}

pub(super) fn default_download_dir(
    download_dir: &Path,
    username_subfolders: bool,
    username: &str,
) -> PathBuf {
    if username_subfolders {
        join_clean(download_dir, [username])
    } else {
        download_dir.to_path_buf()
    }
}

pub(super) fn folder_destination(
    download_dir: &Path,
    username_subfolders: bool,
    username: &str,
    virtual_path: &str,
    root: Option<&str>,
) -> PathBuf {
    let base = default_download_dir(download_dir, username_subfolders, username);
    let Some(root) = root else {
        return base;
    };
    let removed_parents = root.rsplit_once('\\').map_or("", |(parent, _)| parent);
    let folder = virtual_folder(virtual_path);
    let target = folder.strip_prefix(removed_parents).unwrap_or(folder);
    join_clean(&base, target.split('\\'))
}

pub(super) fn basename_byte_limit(dir: &Path) -> usize {
    let dir = CString::new(dir.as_os_str().as_bytes()).expect("download path without interior nul");
    let limit = unsafe { libc::pathconf(dir.as_ptr(), libc::_PC_NAME_MAX) };
    if limit <= 0 {
        return DEFAULT_BASENAME_BYTE_LIMIT;
    }
    limit as usize
}

pub(super) fn download_basename(virtual_path: &str, max_bytes: usize) -> String {
    let basename = clean_file_name(virtual_basename(virtual_path));
    let (stem, extension) = split_extension(&basename);
    if extension.len() > max_bytes {
        return truncate_bytes(extension, max_bytes).to_owned();
    }
    format!(
        "{}{extension}",
        truncate_bytes(stem, max_bytes - extension.len())
    )
}

pub(super) fn incomplete_file_path(
    incomplete_dir: &Path,
    username: &str,
    virtual_path: &str,
    max_bytes: usize,
) -> PathBuf {
    let digest = md5::compute(format!("{virtual_path}{username}").as_bytes());
    let prefix = format!("INCOMPLETE{digest:x}");
    let basename = download_basename(virtual_path, max_bytes.saturating_sub(prefix.len()));
    incomplete_dir.join(format!("{prefix}{basename}"))
}

pub(super) fn complete_file_path(dir: &Path, basename: &str, size: u64) -> Option<PathBuf> {
    let (stem, extension) = split_extension(basename);
    let mut candidate = dir.join(basename);
    let mut counter = 1;
    loop {
        if fs::metadata(&candidate).ok()?.len() == size {
            return Some(candidate);
        }
        candidate = dir.join(format!("{stem} ({counter}){extension}"));
        counter += 1;
    }
}

pub(super) fn open_incomplete(
    incomplete_dir: &Path,
    incomplete_path: &Path,
    truncate: bool,
) -> std::io::Result<(fs::File, u64)> {
    fs::create_dir_all(incomplete_dir)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(incomplete_path)?;
    if truncate {
        file.set_len(0)?;
    }
    let offset = file.seek(std::io::SeekFrom::End(0))?;
    Ok((file, offset))
}

pub(super) fn place_download(
    destination_dir: &Path,
    incomplete_path: &Path,
    basename: &str,
) -> std::io::Result<PathBuf> {
    fs::create_dir_all(destination_dir)?;
    let (destination, mut claimed) = claim_destination(destination_dir, basename)?;
    let installed = fs::rename(incomplete_path, &destination).or_else(|error| {
        if error.kind() != std::io::ErrorKind::CrossesDevices {
            return Err(error);
        }
        let mut source = fs::File::open(incomplete_path)?;
        std::io::copy(&mut source, &mut claimed)?;
        claimed.sync_all()
    });
    match installed {
        Ok(()) => {
            if incomplete_path.exists()
                && let Err(error) = fs::remove_file(incomplete_path)
            {
                debug!(
                    incomplete_path = %incomplete_path.display(),
                    %error,
                    "cannot remove incomplete file after copy"
                );
            }
            Ok(destination)
        }
        Err(error) => {
            let _ = fs::remove_file(&destination);
            Err(error)
        }
    }
}

fn claim_destination(
    destination_dir: &Path,
    basename: &str,
) -> std::io::Result<(PathBuf, fs::File)> {
    let (stem, extension) = split_extension(basename);
    let mut candidate = destination_dir.join(basename);
    let mut counter = 1;
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                candidate = destination_dir.join(format!("{stem} ({counter}){extension}"));
                counter += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn split_extension(basename: &str) -> (&str, &str) {
    match basename.rfind('.') {
        Some(index) if index > 0 => basename.split_at(index),
        _ => (basename, ""),
    }
}

fn truncate_bytes(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(username_subfolders: bool, virtual_path: &str, root: Option<&str>) -> String {
        folder_destination(
            Path::new("/data"),
            username_subfolders,
            "newuser",
            virtual_path,
            root,
        )
        .display()
        .to_string()
    }

    #[test]
    fn folder_destination_matches_nicotine() {
        assert_eq!(
            destination(false, "Hello\\Path\\file.mp3", Some("Hello\\Path")),
            "/data/Path"
        );
        assert_eq!(
            destination(true, "Hello\\Path\\file.mp3", Some("Hello\\Path")),
            "/data/newuser/Path"
        );
        assert_eq!(destination(true, "Hello\\file.mp3", Some("Hello")), "/data/newuser/Hello");
        assert_eq!(
            destination(
                true,
                "Hello\\Path\\Depth\\Hello Depth Test Path\\file.mp3",
                Some("Hello\\Path\\Depth\\Hello Depth Test Path")
            ),
            "/data/newuser/Hello Depth Test Path"
        );
    }

    #[test]
    fn folder_destination_keeps_subfolders_below_the_root() {
        let root = Some("share\\Soulseek");
        assert_eq!(
            destination(false, "share\\Soulseek\\file1.mp3", root),
            "/data/Soulseek"
        );
        assert_eq!(
            destination(false, "share\\Soulseek\\folder1\\file3.mp3", root),
            "/data/Soulseek/folder1"
        );
        assert_eq!(
            destination(false, "share\\Soulseek\\folder2\\sub2\\file6.mp3", root),
            "/data/Soulseek/folder2/sub2"
        );
        assert_eq!(
            destination(false, "share\\file.mp3", Some("share")),
            "/data/share"
        );
        assert_eq!(
            destination(false, "share\\Soulseek\\folder1\\sub1\\file4.mp3", Some("share")),
            "/data/share/Soulseek/folder1/sub1"
        );
    }

    #[test]
    fn single_file_downloads_land_in_the_default_folder() {
        assert_eq!(destination(false, "share\\Music\\song.mp3", None), "/data");
        assert_eq!(
            destination(true, "share\\Music\\song.mp3", None),
            "/data/newuser"
        );
    }

    #[test]
    fn clean_file_name_matches_nicotine() {
        assert_eq!(clean_file_name(".."), "__");
        assert_eq!(clean_file_name("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(clean_file_name("song\u{7}name\u{1f}.mp3"), "song_name_.mp3");
        assert_eq!(clean_file_name("  spaced out. . "), "spaced out");
        assert_eq!(clean_file_name("///"), "___");
        assert_eq!(clean_file_name("片片片"), "片片片");
    }

    #[test]
    fn download_basename_stays_within_the_byte_limit() {
        let virtual_path = format!("Music\\{}.mp3", "片".repeat(200));
        let basename = download_basename(&virtual_path, 255);
        assert!(basename.len() <= 255);
        assert!(basename.starts_with('片'));
        assert!(basename.ends_with(".mp3"));

        let long_extension = format!("Music\\name.{}", "x".repeat(300));
        assert_eq!(download_basename(&long_extension, 8).len(), 8);
    }
}
