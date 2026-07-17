use std::fs;
use std::io::Seek;
use std::path::{Path, PathBuf};

use tracing::debug;

pub(super) fn clean_file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

pub(super) fn incomplete_file_path(
    incomplete_dir: &Path,
    username: &str,
    virtual_path: &str,
) -> PathBuf {
    let digest = md5::compute(format!("{virtual_path}{username}").as_bytes());
    let basename = clean_file_name(virtual_path.rsplit('\\').next().unwrap_or(virtual_path));
    incomplete_dir.join(format!("INCOMPLETE{digest:x}{basename}"))
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
    download_dir: &Path,
    incomplete_path: &Path,
    basename: &str,
) -> std::io::Result<PathBuf> {
    fs::create_dir_all(download_dir)?;
    let (destination, mut claimed) = claim_destination(download_dir, basename)?;
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

fn claim_destination(download_dir: &Path, basename: &str) -> std::io::Result<(PathBuf, fs::File)> {
    let basename = clean_file_name(basename);
    let (stem, extension) = match basename.rfind('.') {
        Some(index) if index > 0 => basename.split_at(index),
        _ => (basename.as_str(), ""),
    };
    let mut candidate = download_dir.join(&basename);
    let mut counter = 1;
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                candidate = download_dir.join(format!("{stem} ({counter}){extension}"));
                counter += 1;
            }
            Err(error) => return Err(error),
        }
    }
}
