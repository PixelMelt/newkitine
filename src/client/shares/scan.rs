use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use tracing::{info, warn};

use crate::types::{FileAttributes, SharedFolder};

use super::audio::{has_audio_extension, is_missing_vbr, read_attributes};
use super::{ShareCatalog, ShareCatalogFile};

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("cannot resolve shared folder {path}: {error}")]
    Root {
        path: PathBuf,
        error: std::io::Error,
    },
    #[error("duplicate virtual folder name {name}")]
    DuplicateVirtualName { name: String },
    #[error("cannot scan folder {path}: {error}")]
    Folder {
        path: PathBuf,
        error: std::io::Error,
    },
    #[error("cannot stat {path}: {error}")]
    Metadata {
        path: PathBuf,
        error: std::io::Error,
    },
    #[error("scan superseded by a newer scan")]
    Superseded,
    #[error("scan task panicked: {reason}")]
    Panicked { reason: String },
}

const BACKSLASH_SENTINEL: &str = "@@BACKSLASH@@";
const ATTRIBUTE_WORKER_CAP: usize = 12;
const PROGRESS_INTERVAL: u64 = 1000;

type AttributeCache<'a> = HashMap<&'a Path, HashMap<&'a OsStr, &'a ShareCatalogFile>>;

struct RawFile {
    name: String,
    real_name: OsString,
    size: u64,
    mtime: u64,
    attributes: Option<FileAttributes>,
}

struct RawFolder {
    virtual_path: String,
    real_path: PathBuf,
    files: Vec<RawFile>,
}

struct Miss {
    file_index: u32,
    path: PathBuf,
}

struct Merger {
    catalog: ShareCatalog,
    misses: Vec<Miss>,
}

struct Progress<'a> {
    count: AtomicU64,
    notify: &'a (dyn Fn(u64) + Sync),
}

impl Progress<'_> {
    fn add(&self) {
        let count = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(PROGRESS_INTERVAL) {
            (self.notify)(count);
        }
    }
}

pub fn restrict(catalog: &ShareCatalog, shared_folders: &[SharedFolder]) -> ShareCatalog {
    let roots: Vec<(&str, PathBuf, bool)> = shared_folders
        .iter()
        .filter_map(|shared| {
            fs::canonicalize(&shared.path)
                .ok()
                .map(|root| (shared.virtual_name.as_str(), root, shared.buddy_only))
        })
        .collect();
    let mut restricted = ShareCatalog::empty();
    for folder in &catalog.folders {
        let root_name = folder
            .virtual_path
            .split_once('\\')
            .map_or(folder.virtual_path.as_ref(), |(root, _)| root);
        let Some((_, _, buddy_only)) = roots
            .iter()
            .find(|(name, root, _)| *name == root_name && folder.real_path.starts_with(root))
        else {
            continue;
        };
        restricted.push_folder(
            folder.virtual_path.to_string(),
            folder.real_path.clone(),
            *buddy_only,
            catalog.folder_files(folder).iter().cloned(),
        );
    }
    restricted
}

pub fn walk(
    shared_folders: &[SharedFolder],
    share_filters: &[String],
    cached: &ShareCatalog,
    cancelled: &AtomicBool,
    progress: &(dyn Fn(u64) + Sync),
) -> Result<ShareCatalog, ScanError> {
    let mut virtual_names = HashSet::new();
    for shared in shared_folders {
        if !virtual_names.insert(shared.virtual_name.as_str()) {
            return Err(ScanError::DuplicateVirtualName {
                name: shared.virtual_name.clone(),
            });
        }
    }
    let share_filters: HashSet<&str> = share_filters.iter().map(String::as_str).collect();
    let cache: AttributeCache = cached
        .folders
        .iter()
        .map(|folder| {
            let files = cached
                .folder_files(folder)
                .iter()
                .map(|file| (file.real_name.as_os_str(), file))
                .collect();
            (folder.real_path.as_path(), files)
        })
        .collect();

    let progress = Progress {
        count: AtomicU64::new(0),
        notify: progress,
    };
    let mut merger = Merger {
        catalog: ShareCatalog::empty(),
        misses: Vec::new(),
    };
    let mut virtual_paths = HashSet::new();
    for shared in shared_folders {
        walk_root(
            shared,
            &share_filters,
            &cache,
            &mut virtual_paths,
            cancelled,
            &progress,
            |folder| merger.add_folder(shared.buddy_only, folder),
        )?;
    }
    let Merger {
        mut catalog,
        misses,
    } = merger;

    let attribute_reads = misses.len();
    read_missing_attributes(&mut catalog, misses, cancelled, &progress);
    if cancelled.load(Ordering::Relaxed) {
        return Err(ScanError::Superseded);
    }
    info!(
        folders = catalog.folders.len(),
        files = catalog.files.len(),
        cache_hits = catalog.files.len() - attribute_reads,
        attribute_reads,
        "share scan complete"
    );
    Ok(catalog)
}

fn walk_root(
    shared: &SharedFolder,
    share_filters: &HashSet<&str>,
    cache: &AttributeCache,
    virtual_paths: &mut HashSet<String>,
    cancelled: &AtomicBool,
    progress: &Progress,
    mut add_folder: impl FnMut(RawFolder),
) -> Result<(), ScanError> {
    let root = fs::canonicalize(&shared.path).map_err(|error| ScanError::Root {
        path: shared.path.clone(),
        error,
    })?;
    let mut visited = HashSet::new();
    let mut stack = vec![(root.clone(), root, shared.virtual_name.clone())];
    while let Some((real_dir, canonical_dir, virtual_dir)) = stack.pop() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(ScanError::Superseded);
        }
        if !visited.insert(canonical_dir.clone()) {
            warn!(path = %real_dir.display(), target = %canonical_dir.display(), "skipping folder already shared under this share");
            continue;
        }
        if !virtual_paths.insert(virtual_dir.clone()) {
            warn!(path = %real_dir.display(), virtual_path = %virtual_dir, "skipping folder with a duplicate virtual path");
            continue;
        }
        let entries = fs::read_dir(&real_dir).map_err(|error| ScanError::Folder {
            path: real_dir.clone(),
            error,
        })?;
        let cached_files = cache.get(real_dir.as_path());
        let mut names = HashSet::new();
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| ScanError::Folder {
                path: real_dir.clone(),
                error,
            })?;
            let real_name = entry.file_name();
            let name = real_name.to_string_lossy().into_owned();
            if name.starts_with('.') || share_filters.contains(name.as_str()) {
                continue;
            }
            let entry_type = entry.file_type().map_err(|error| ScanError::Metadata {
                path: entry.path(),
                error,
            })?;
            let target = if entry_type.is_symlink() {
                match fs::metadata(entry.path()) {
                    Ok(metadata) => Some(metadata),
                    Err(error) => {
                        warn!(path = %entry.path().display(), %error, "skipping unresolvable symlink in shared folder");
                        continue;
                    }
                }
            } else {
                None
            };
            let file_type = target.as_ref().map_or(entry_type, fs::Metadata::file_type);
            let name = name.replace('\\', BACKSLASH_SENTINEL);
            if file_type.is_dir() {
                let canonical = if target.is_some() {
                    fs::canonicalize(entry.path()).map_err(|error| ScanError::Metadata {
                        path: entry.path(),
                        error,
                    })?
                } else {
                    canonical_dir.join(&real_name)
                };
                stack.push((entry.path(), canonical, format!("{virtual_dir}\\{name}")));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if !names.insert(name.clone()) {
                warn!(path = %entry.path().display(), folder = %virtual_dir, name, "skipping file with a duplicate virtual path");
                continue;
            }
            let metadata = match target {
                Some(metadata) => metadata,
                None => entry.metadata().map_err(|error| ScanError::Metadata {
                    path: entry.path(),
                    error,
                })?,
            };
            let size = metadata.len();
            let mtime = unix_mtime(&metadata);
            let attributes = if size <= 128 || !has_audio_extension(&name) {
                Some(FileAttributes::default())
            } else {
                cached_files
                    .and_then(|files| files.get(real_name.as_os_str()))
                    .filter(|cached| {
                        cached.size == size
                            && cached.mtime == mtime
                            && !is_missing_vbr(&cached.attributes)
                    })
                    .map(|cached| cached.attributes.clone())
            };
            if attributes.is_some() {
                progress.add();
            }
            files.push(RawFile {
                name,
                real_name,
                size,
                mtime,
                attributes,
            });
        }
        add_folder(RawFolder {
            virtual_path: virtual_dir,
            real_path: real_dir,
            files,
        });
    }
    Ok(())
}

impl Merger {
    fn add_folder(&mut self, buddy_only: bool, folder: RawFolder) {
        let RawFolder {
            virtual_path,
            real_path,
            mut files,
        } = folder;
        files.sort_by(|a, b| a.name.cmp(&b.name));
        let first_index = self.catalog.files.len() as u32;
        for (offset, file) in files.iter().enumerate() {
            if file.attributes.is_none() {
                self.misses.push(Miss {
                    file_index: first_index + offset as u32,
                    path: real_path.join(&file.real_name),
                });
            }
        }
        self.catalog.push_folder(
            virtual_path,
            real_path,
            buddy_only,
            files.into_iter().map(|file| {
                ShareCatalogFile::new(
                    file.name,
                    file.real_name,
                    file.size,
                    file.mtime,
                    file.attributes.unwrap_or_default(),
                )
            }),
        );
    }
}

fn read_missing_attributes(
    catalog: &mut ShareCatalog,
    misses: Vec<Miss>,
    cancelled: &AtomicBool,
    progress: &Progress,
) {
    if misses.is_empty() {
        return;
    }
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .min(ATTRIBUTE_WORKER_CAP)
        .min(misses.len());
    let cursor = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let misses = &misses;
        let cursor = &cursor;
        let (results, received) = std::sync::mpsc::sync_channel(workers * 2);
        for _ in 0..workers {
            let results = results.clone();
            scope.spawn(move || {
                while !cancelled.load(Ordering::Relaxed) {
                    let position = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(miss) = misses.get(position) else {
                        break;
                    };
                    let attributes = read_attributes(&miss.path);
                    progress.add();
                    results.send((position, attributes)).unwrap();
                }
            });
        }
        drop(results);
        for (position, attributes) in received {
            catalog.files[misses[position].file_index as usize].attributes = attributes;
        }
    });
}

fn unix_mtime(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .expect("mtime unavailable on this platform")
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::shares::{SharesIndex, cache};
    use crate::types::{DEFAULT_MAX_SEARCH_RESULTS, DEFAULT_MIN_SEARCH_CHARS, FileInfo};

    fn scan(
        shared_folders: &[SharedFolder],
        share_filters: &[String],
        cache_path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<SharesIndex, ScanError> {
        let catalog = walk(
            shared_folders,
            share_filters,
            &cache::load(cache_path),
            cancelled,
            &|_| {},
        )?;
        cache::save(cache_path, &catalog);
        Ok(SharesIndex::from_catalog(catalog))
    }

    fn search(
        index: &SharesIndex,
        term: &str,
        is_buddy: bool,
        phrases: &[String],
    ) -> Vec<FileInfo> {
        index.search(
            term,
            is_buddy,
            phrases,
            DEFAULT_MAX_SEARCH_RESULTS,
            DEFAULT_MIN_SEARCH_CHARS,
        )
    }

    fn temp_base() -> PathBuf {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "newkitine-shares-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn cache_path(base: &Path) -> PathBuf {
        base.join("share-catalog.gz")
    }

    fn write_wav(path: &Path) {
        let data = vec![0u8; 44100 * 2];
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend(((36 + data.len()) as u32).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(44100u32.to_le_bytes());
        bytes.extend(88200u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        fs::write(path, bytes).unwrap();
    }

    fn cached_attributes(cache_path: &Path, real_dir: &Path, name: &str) -> FileAttributes {
        let catalog = cache::load(cache_path);
        let real_dir = fs::canonicalize(real_dir).unwrap();
        let folder = catalog
            .folders
            .iter()
            .find(|folder| folder.real_path == real_dir)
            .expect("folder in cache");
        catalog
            .folder_files(folder)
            .iter()
            .find(|file| file.real_name == name)
            .expect("file in cache")
            .attributes
            .clone()
    }

    fn cache_with(cache_path: &Path, real_dir: &Path, file: ShareCatalogFile) {
        let mut catalog = ShareCatalog::empty();
        catalog.push_folder(
            "Music".into(),
            fs::canonicalize(real_dir).unwrap(),
            false,
            [file],
        );
        cache::save(cache_path, &catalog);
    }

    fn test_shares(base: &Path) -> Vec<SharedFolder> {
        let album = base.join("public/Sample Album");
        fs::create_dir_all(&album).unwrap();
        fs::write(album.join("First Song.flac"), b"x".repeat(300)).unwrap();
        fs::write(album.join("Second Tune.ogg"), b"y".repeat(300)).unwrap();
        let secret = base.join("secret");
        fs::create_dir_all(&secret).unwrap();
        fs::write(secret.join("hidden song.wav"), b"z".repeat(300)).unwrap();
        vec![
            SharedFolder {
                virtual_name: "Public".into(),
                path: base.join("public"),
                buddy_only: false,
            },
            SharedFolder {
                virtual_name: "Private".into(),
                path: secret,
                buddy_only: true,
            },
        ]
    }

    fn test_index() -> SharesIndex {
        let base = temp_base();
        scan(
            &test_shares(&base),
            &[],
            &cache_path(&base),
            &AtomicBool::new(false),
        )
        .expect("scan test shares")
    }

    #[test]
    fn search_word_matching() {
        let index = test_index();

        let results = search(&index, "sample first", false, &[]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Public\\Sample Album\\First Song.flac");

        assert!(search(&index, "sample missing", false, &[]).is_empty());

        let excluded = search(&index, "sample -tune", false, &[]);
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0].name, "Public\\Sample Album\\First Song.flac");

        let partial = search(&index, "sample *une", false, &[]);
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].name, "Public\\Sample Album\\Second Tune.ogg");

        assert_eq!(search(&index, "SAMPLE ALBUM", false, &[]).len(), 2);
        assert!(search(&index, "ab", false, &[]).is_empty());
    }

    #[test]
    fn search_respects_permissions_and_phrases() {
        let index = test_index();

        assert!(search(&index, "hidden song", false, &[]).is_empty());
        let buddy = search(&index, "hidden song", true, &[]);
        assert_eq!(buddy.len(), 1);
        assert_eq!(buddy[0].name, "Private\\hidden song.wav");

        assert!(search(&index, "first song", false, &["first".into()]).is_empty());
    }

    #[test]
    fn case_colliding_paths_scan_and_resolve() {
        let base = temp_base();
        let lower = base.join("music/moe shop - pure");
        let upper = base.join("music/Moe Shop - Pure");
        fs::create_dir_all(&lower).unwrap();
        fs::create_dir_all(&upper).unwrap();
        fs::write(lower.join("Crush.mp3"), b"a".repeat(200)).unwrap();
        fs::write(upper.join("Crush.mp3"), b"b".repeat(300)).unwrap();

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: base.join("music"),
                buddy_only: false,
            }],
            &[],
            &cache_path(&base),
            &AtomicBool::new(false),
        )
        .expect("case collision must not fail the scan");

        let (folders, files) = index.counts();
        assert_eq!(folders, 3);
        assert_eq!(files, 2);

        let (path, size, _) = index
            .resolve("Music\\Moe Shop - Pure\\Crush.mp3", false)
            .expect("resolve exact case");
        assert_eq!(size, 300);
        assert!(path.starts_with(&upper));

        let (path, size, _) = index
            .resolve("Music\\moe shop - pure\\Crush.mp3", false)
            .expect("resolve other exact case");
        assert_eq!(size, 200);
        assert!(path.starts_with(&lower));

        assert!(
            index
                .resolve("MUSIC\\MOE SHOP - PURE\\CRUSH.MP3", false)
                .is_some()
        );
    }

    #[test]
    fn browse_and_resolve() {
        let index = test_index();

        let public = index.browse(false);
        assert!(
            public
                .iter()
                .all(|folder| !folder.directory.starts_with("Private"))
        );
        let buddy = index.browse(true);
        assert!(buddy.iter().any(|folder| folder.directory == "Private"));

        let (path, size, _) = index
            .resolve("public\\sample album\\FIRST SONG.FLAC", false)
            .expect("case-insensitive resolve");
        assert_eq!(size, 300);
        assert!(path.ends_with("Sample Album/First Song.flac"));
        assert!(index.resolve("Private\\hidden song.wav", false).is_none());
        assert!(index.resolve("Private\\hidden song.wav", true).is_some());

        assert_eq!(
            index.folder_contents("Public\\Sample Album", false).len(),
            1
        );
        assert!(index.folder_contents("Private", false).is_empty());
        assert!(index.folder_contents("Nope", true).is_empty());
    }

    #[test]
    fn streaming_browse_matches_peer_message_encoding() {
        let index = test_index();
        for is_buddy in [false, true] {
            let expected = crate::protocol::PeerMessage::SharedFileListResponse {
                shares: index.browse(is_buddy),
                unknown: 0,
                private_shares: Vec::new(),
            };
            let bytes = index.browse_frame(is_buddy);
            assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 5);
            let parsed = crate::protocol::PeerMessage::parse(5, &bytes[8..]).unwrap();
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn attributes_are_read_and_cached() {
        let base = temp_base();
        let dir = base.join("music");
        fs::create_dir_all(&dir).unwrap();
        write_wav(&dir.join("tone.wav"));
        let cache_path = cache_path(&base);

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir.clone(),
                buddy_only: false,
            }],
            &[],
            &cache_path,
            &AtomicBool::new(false),
        )
        .unwrap();

        let (_, _, attributes) = index.resolve("Music\\tone.wav", false).unwrap();
        assert_eq!(attributes.sample_rate, Some(44100));
        assert_eq!(attributes.bit_depth, Some(16));
        assert_eq!(attributes.length, Some(1));

        let contents = index.folder_contents("Music", false);
        assert_eq!(contents[0].files[0].attributes.sample_rate, Some(44100));

        assert_eq!(
            cached_attributes(&cache_path, &dir, "tone.wav").sample_rate,
            Some(44100)
        );
    }

    #[test]
    fn cache_hit_skips_reading_attributes() {
        let base = temp_base();
        let dir = base.join("music");
        fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.flac");
        fs::write(&song, b"g".repeat(300)).unwrap();
        let metadata = fs::metadata(&song).unwrap();
        let cache_path = cache_path(&base);
        cache_with(
            &cache_path,
            &dir,
            ShareCatalogFile::new(
                "song.flac".into(),
                "song.flac".into(),
                300,
                unix_mtime(&metadata),
                FileAttributes {
                    bitrate: Some(320),
                    vbr: Some(0),
                    ..Default::default()
                },
            ),
        );

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &[],
            &cache_path,
            &AtomicBool::new(false),
        )
        .unwrap();

        let (_, _, attributes) = index.resolve("Music\\song.flac", false).unwrap();
        assert_eq!(attributes.bitrate, Some(320));
    }

    #[test]
    fn changed_file_invalidates_cache_entry() {
        let base = temp_base();
        let dir = base.join("music");
        fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.flac");
        fs::write(&song, b"g".repeat(300)).unwrap();
        let metadata = fs::metadata(&song).unwrap();
        let cache_path = cache_path(&base);
        cache_with(
            &cache_path,
            &dir,
            ShareCatalogFile::new(
                "song.flac".into(),
                "song.flac".into(),
                300,
                unix_mtime(&metadata) + 1,
                FileAttributes {
                    bitrate: Some(320),
                    ..Default::default()
                },
            ),
        );

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir.clone(),
                buddy_only: false,
            }],
            &[],
            &cache_path,
            &AtomicBool::new(false),
        )
        .unwrap();

        let (_, _, attributes) = index.resolve("Music\\song.flac", false).unwrap();
        assert_eq!(*attributes, FileAttributes::default());
        assert_eq!(
            cached_attributes(&cache_path, &dir, "song.flac"),
            FileAttributes::default()
        );
    }

    #[test]
    fn restrict_applies_the_share_configuration_to_a_cached_catalog() {
        let base = temp_base();
        let shares = test_shares(&base);
        let catalog = walk(
            &shares,
            &[],
            &ShareCatalog::empty(),
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        assert_eq!(catalog.folders.len(), 3);

        let public_only = restrict(&catalog, &shares[..1]);
        assert_eq!(public_only.folders.len(), 2);
        assert!(
            public_only
                .folders
                .iter()
                .all(|folder| folder.virtual_path.starts_with("Public"))
        );
        assert_eq!(public_only.files.len(), 2);

        let flipped = restrict(
            &catalog,
            &[SharedFolder {
                virtual_name: "Public".into(),
                path: base.join("public"),
                buddy_only: true,
            }],
        );
        assert!(flipped.folders.iter().all(|folder| folder.buddy_only));

        let renamed = restrict(
            &catalog,
            &[SharedFolder {
                virtual_name: "Elsewhere".into(),
                path: base.join("public"),
                buddy_only: false,
            }],
        );
        assert!(renamed.folders.is_empty());

        let index = SharesIndex::from_catalog(public_only);
        assert_eq!(index.counts(), (2, 2));
        assert!(
            index
                .resolve("Public\\Sample Album\\First Song.flac", false)
                .is_some()
        );
        assert!(index.resolve("Private\\hidden song.wav", true).is_none());
    }

    #[test]
    fn progress_notifies_every_interval() {
        let seen = std::sync::Mutex::new(Vec::new());
        let notify = |count| seen.lock().unwrap().push(count);
        let progress = Progress {
            count: AtomicU64::new(0),
            notify: &notify,
        };
        for _ in 0..(PROGRESS_INTERVAL * 2 + 1) {
            progress.add();
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec![PROGRESS_INTERVAL, PROGRESS_INTERVAL * 2]
        );
    }

    #[test]
    fn share_filters_skip_exact_names() {
        let base = temp_base();
        let dir = base.join("music");
        let covers = dir.join("Covers");
        fs::create_dir_all(&covers).unwrap();
        fs::write(dir.join("song.mp3"), b"g".repeat(300)).unwrap();
        fs::write(dir.join("Thumbs.db"), b"x").unwrap();
        fs::write(covers.join("front.mp3"), b"y".repeat(300)).unwrap();

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &["Thumbs.db".into(), "Covers".into()],
            &cache_path(&base),
            &AtomicBool::new(false),
        )
        .unwrap();

        let (folders, files) = index.counts();
        assert_eq!(folders, 1);
        assert_eq!(files, 1);
        assert!(index.resolve("Music\\song.mp3", false).is_some());
    }

    #[test]
    fn backslash_in_directory_names_is_sanitized() {
        let base = temp_base();
        let dir = base.join("music");
        let weird = dir.join("a\\b");
        fs::create_dir_all(&weird).unwrap();
        fs::write(weird.join("song.mp3"), b"g".repeat(300)).unwrap();

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &[],
            &cache_path(&base),
            &AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(
            index.folder_contents("Music\\a@@BACKSLASH@@b", false).len(),
            1
        );
        assert!(
            index
                .resolve("Music\\a@@BACKSLASH@@b\\song.mp3", false)
                .is_some()
        );
    }

    #[test]
    fn cancelled_scan_is_superseded() {
        let base = temp_base();
        let dir = base.join("music");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("song.mp3"), b"g".repeat(300)).unwrap();

        let result = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &[],
            &cache_path(&base),
            &AtomicBool::new(true),
        );
        assert!(matches!(result, Err(ScanError::Superseded)));
    }

    #[test]
    fn search_skips_trailing_empty_folder() {
        let mut catalog = ShareCatalog::empty();
        catalog.push_folder(
            "Music".into(),
            PathBuf::from("/music"),
            false,
            [ShareCatalogFile::new(
                "song.flac".into(),
                "song.flac".into(),
                300,
                0,
                FileAttributes::default(),
            )],
        );
        catalog.push_folder(
            "Music\\Albums".into(),
            PathBuf::from("/music/Albums"),
            false,
            [],
        );
        let index = SharesIndex::from_catalog(catalog);

        let results = search(&index, "music", false, &[]);
        assert_eq!(
            results
                .into_iter()
                .map(|file| file.name)
                .collect::<Vec<_>>(),
            vec!["Music\\song.flac"]
        );
    }

    #[test]
    fn search_merges_folder_and_file_postings() {
        let base = temp_base();
        let dir = base.join("music/Sample");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Sample.flac"), b"a".repeat(300)).unwrap();
        fs::write(dir.join("Other.flac"), b"b".repeat(300)).unwrap();
        fs::write(base.join("music/Sample Loose.mp3"), b"c".repeat(300)).unwrap();

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: base.join("music"),
                buddy_only: false,
            }],
            &[],
            &cache_path(&base),
            &AtomicBool::new(false),
        )
        .unwrap();

        let names = |results: Vec<FileInfo>| {
            results
                .into_iter()
                .map(|file| file.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(search(&index, "sample", false, &[])),
            vec![
                "Music\\Sample Loose.mp3",
                "Music\\Sample\\Other.flac",
                "Music\\Sample\\Sample.flac",
            ]
        );
        assert_eq!(
            names(search(&index, "sample other", false, &[])),
            vec!["Music\\Sample\\Other.flac"]
        );
        assert_eq!(
            names(search(&index, "sample -other", false, &[])),
            vec!["Music\\Sample Loose.mp3", "Music\\Sample\\Sample.flac"]
        );
        assert_eq!(
            names(search(&index, "sample *ose", false, &[])),
            vec!["Music\\Sample Loose.mp3"]
        );
        assert_eq!(
            names(search(&index, "sample", false, &["sample\\other".into()])),
            vec!["Music\\Sample Loose.mp3", "Music\\Sample\\Sample.flac"]
        );
        assert_eq!(search(&index, "sample", false, &[]).len(), 3);
        let capped = index.search("sample", false, &[], 2, 1);
        assert_eq!(capped.len(), 2);
    }

    fn music_share(path: PathBuf) -> Vec<SharedFolder> {
        vec![SharedFolder {
            virtual_name: "Music".into(),
            path,
            buddy_only: false,
        }]
    }

    #[test]
    fn cached_lossy_attributes_without_vbr_are_reread() {
        let base = temp_base();
        let dir = base.join("music");
        fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.mp3");
        fs::write(&song, b"g".repeat(300)).unwrap();
        let metadata = fs::metadata(&song).unwrap();
        let cache_path = cache_path(&base);
        cache_with(
            &cache_path,
            &dir,
            ShareCatalogFile::new(
                "song.mp3".into(),
                "song.mp3".into(),
                300,
                unix_mtime(&metadata),
                FileAttributes {
                    bitrate: Some(320),
                    ..Default::default()
                },
            ),
        );

        let index = scan(&music_share(dir), &[], &cache_path, &AtomicBool::new(false)).unwrap();

        let (_, _, attributes) = index.resolve("Music\\song.mp3", false).unwrap();
        assert_eq!(*attributes, FileAttributes::default());
    }

    #[test]
    fn lossy_equal_folder_names_are_skipped_not_fatal() {
        use std::os::unix::ffi::OsStrExt;
        let base = temp_base();
        let music = base.join("music");
        for raw in [&b"album\xff"[..], &b"album\xfe"[..]] {
            let dir = music.join(OsStr::from_bytes(raw));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("track.flac"), b"t".repeat(10)).unwrap();
        }

        let index = scan(
            &music_share(music),
            &[],
            &cache_path(&base),
            &AtomicBool::new(false),
        )
        .expect("duplicate virtual folder must not fail the scan");

        assert_eq!(index.counts(), (2, 1));
        assert!(
            index
                .resolve("Music\\album\u{fffd}\\track.flac", false)
                .is_some()
        );
    }

    #[test]
    fn lossy_equal_file_names_are_shared_once() {
        use std::os::unix::ffi::OsStrExt;
        let base = temp_base();
        let music = base.join("music");
        fs::create_dir_all(&music).unwrap();
        fs::write(
            music.join(OsStr::from_bytes(b"song\xff.flac")),
            b"a".repeat(10),
        )
        .unwrap();
        fs::write(
            music.join(OsStr::from_bytes(b"song\xfe.flac")),
            b"b".repeat(20),
        )
        .unwrap();

        let index = scan(
            &music_share(music),
            &[],
            &cache_path(&base),
            &AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(index.counts(), (1, 1));
        let contents = index.folder_contents("Music", false);
        assert_eq!(contents[0].files.len(), 1);
    }

    #[test]
    fn symlinks_are_followed_with_loop_protection() {
        use std::os::unix::fs::symlink;
        let base = temp_base();
        let music = base.join("music");
        let outside = base.join("outside");
        fs::create_dir_all(music.join("real")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("linked.flac"), b"l".repeat(30)).unwrap();
        fs::write(outside.join("single.flac"), b"s".repeat(40)).unwrap();
        symlink(&outside, music.join("linked dir")).unwrap();
        symlink(outside.join("single.flac"), music.join("real/single.flac")).unwrap();
        symlink(&music, music.join("real/loop")).unwrap();
        symlink(base.join("missing"), music.join("dangling")).unwrap();

        let index = scan(
            &music_share(music.clone()),
            &[],
            &cache_path(&base),
            &AtomicBool::new(false),
        )
        .expect("symlinks must not fail the scan");

        let (path, size, _) = index
            .resolve("Music\\linked dir\\linked.flac", false)
            .unwrap();
        assert_eq!(size, 30);
        assert_eq!(
            path,
            fs::canonicalize(&music)
                .unwrap()
                .join("linked dir/linked.flac")
        );
        let (_, size, _) = index.resolve("Music\\real\\single.flac", false).unwrap();
        assert_eq!(size, 40);
        assert!(index.folder_contents("Music\\real\\loop", false).is_empty());
        assert_eq!(index.counts(), (3, 3));
    }
}
