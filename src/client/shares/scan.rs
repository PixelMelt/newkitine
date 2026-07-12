use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use lofty::prelude::AudioFile;
use tracing::{info, warn};

use crate::types::{FileAttributes, FileInfo, SharedFolder, UINT32_LIMIT};

use super::cache::{self, CacheEntry};
use super::{FileEntry, ScannedFolder, SharesIndex, split_words};

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
    #[error("duplicate virtual path {path}")]
    DuplicateVirtualPath { path: String },
}

const BACKSLASH_SENTINEL: &str = "@@BACKSLASH@@";
const ATTRIBUTE_WORKER_CAP: usize = 12;
const PROGRESS_INTERVAL: u64 = 1000;

const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "ac3", "afc", "aif", "aifc", "aiff", "ape", "au", "bwav", "bwf", "dff", "dsd", "dsf",
    "dts", "flac", "m4a", "m4b", "mka", "mp1", "mp2", "mp3", "mp+", "mpc", "oga", "ogg", "opus",
    "spx", "tak", "tta", "wav", "wma", "wv",
];

struct RawFile {
    name: String,
    real_path: PathBuf,
    size: u64,
    mtime: u64,
    class: FileClass,
}

enum FileClass {
    Ready(FileAttributes),
    CacheHit(FileAttributes),
    NeedsRead,
}

struct RawFolder {
    virtual_path: String,
    files: Vec<RawFile>,
}

struct Miss {
    file_index: u32,
    folder_index: usize,
    position: usize,
    key: String,
    size: u64,
    mtime: u64,
}

struct Merger {
    index: SharesIndex,
    new_cache: HashMap<String, CacheEntry>,
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

pub fn scan(
    shared_folders: &[SharedFolder],
    cache_path: &Path,
    progress: &(dyn Fn(u64) + Sync),
) -> Result<SharesIndex, ScanError> {
    let mut virtual_names = HashSet::new();
    for shared in shared_folders {
        if !virtual_names.insert(shared.virtual_name.as_str()) {
            return Err(ScanError::DuplicateVirtualName {
                name: shared.virtual_name.clone(),
            });
        }
    }

    let cache = cache::load(cache_path);
    let progress = Progress {
        count: AtomicU64::new(0),
        notify: progress,
    };
    let roots: Vec<Vec<RawFolder>> = {
        let cache = &cache;
        let progress = &progress;
        std::thread::scope(|scope| {
            let handles: Vec<_> = shared_folders
                .iter()
                .map(|shared| scope.spawn(move || walk_root(shared, cache, progress)))
                .collect();
            handles
                .into_iter()
                .map(join_scan_thread)
                .collect::<Result<_, _>>()
        })?
    };

    let mut merger = Merger {
        index: SharesIndex::default(),
        new_cache: HashMap::new(),
        misses: Vec::new(),
    };
    for (shared, folders) in shared_folders.iter().zip(roots) {
        for folder in folders {
            merger.add_folder(shared.buddy_only, folder)?;
        }
    }
    let Merger {
        mut index,
        mut new_cache,
        misses,
    } = merger;

    let cache_hits = new_cache.len();
    let attribute_reads = misses.len();
    read_missing_attributes(&mut index, &mut new_cache, misses, &progress);
    cache::save(cache_path, &new_cache);

    let (folders, files) = index.counts();
    info!(
        folders,
        files,
        total_files = index.files.len(),
        cache_hits,
        attribute_reads,
        "share scan complete"
    );
    Ok(index)
}

fn walk_root(
    shared: &SharedFolder,
    cache: &HashMap<String, CacheEntry>,
    progress: &Progress,
) -> Result<Vec<RawFolder>, ScanError> {
    let root = fs::canonicalize(&shared.path).map_err(|error| ScanError::Root {
        path: shared.path.clone(),
        error,
    })?;
    let mut folders = Vec::new();
    let mut stack = vec![(root, shared.virtual_name.clone())];
    while let Some((real_dir, virtual_dir)) = stack.pop() {
        let entries = fs::read_dir(&real_dir).map_err(|error| ScanError::Folder {
            path: real_dir.clone(),
            error,
        })?;
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| ScanError::Folder {
                path: real_dir.clone(),
                error,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| ScanError::Metadata {
                path: entry.path(),
                error,
            })?;
            if file_type.is_symlink() {
                warn!(path = %entry.path().display(), "skipping symlink in shared folder");
                continue;
            }
            let name = name.replace('\\', BACKSLASH_SENTINEL);
            if file_type.is_dir() {
                stack.push((entry.path(), format!("{virtual_dir}\\{name}")));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let metadata = entry.metadata().map_err(|error| ScanError::Metadata {
                path: entry.path(),
                error,
            })?;
            let real_path = entry.path();
            let size = metadata.len();
            let mtime = unix_mtime(&metadata);
            let class = if size <= 128 || !has_audio_extension(&real_path) {
                FileClass::Ready(FileAttributes::default())
            } else {
                match cache.get(real_path.to_string_lossy().as_ref()) {
                    Some(entry) if entry.size == size && entry.mtime == mtime => {
                        FileClass::CacheHit(entry.attributes.clone())
                    }
                    _ => FileClass::NeedsRead,
                }
            };
            if !matches!(class, FileClass::NeedsRead) {
                progress.add();
            }
            files.push(RawFile {
                name,
                real_path,
                size,
                mtime,
                class,
            });
        }
        folders.push(RawFolder {
            virtual_path: virtual_dir,
            files,
        });
    }
    Ok(folders)
}

impl Merger {
    fn add_folder(&mut self, buddy_only: bool, folder: RawFolder) -> Result<(), ScanError> {
        let RawFolder {
            virtual_path: virtual_dir,
            mut files,
        } = folder;
        if self.index.folder_lookup.contains_key(&virtual_dir) {
            return Err(ScanError::DuplicateVirtualPath { path: virtual_dir });
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));

        let folder_index = self.index.folders.len();
        let virtual_dir_lower = virtual_dir.to_lowercase();
        let folder_words: Vec<&str> = split_words(&virtual_dir_lower).collect();
        let mut infos = Vec::with_capacity(files.len());

        for (position, file) in files.into_iter().enumerate() {
            let virtual_path = format!("{virtual_dir}\\{}", file.name);
            let virtual_path_lower = virtual_path.to_lowercase();
            let file_index = self.index.files.len() as u32;

            let basename_lower = file.name.to_lowercase();
            let words: HashSet<&str> = folder_words
                .iter()
                .copied()
                .chain(split_words(&basename_lower))
                .collect();
            for word in words {
                self.index
                    .word_index
                    .entry(word.to_owned())
                    .or_default()
                    .push(file_index);
            }

            let attributes = match file.class {
                FileClass::Ready(attributes) => attributes,
                FileClass::CacheHit(attributes) => {
                    self.new_cache.insert(
                        file.real_path.to_string_lossy().into_owned(),
                        CacheEntry {
                            size: file.size,
                            mtime: file.mtime,
                            attributes: attributes.clone(),
                        },
                    );
                    attributes
                }
                FileClass::NeedsRead => {
                    self.misses.push(Miss {
                        file_index,
                        folder_index,
                        position,
                        key: file.real_path.to_string_lossy().into_owned(),
                        size: file.size,
                        mtime: file.mtime,
                    });
                    FileAttributes::default()
                }
            };

            let candidates = self.index.paths_lower.entry(virtual_path_lower).or_default();
            if !candidates.is_empty() {
                warn!(path = %virtual_path, "shared file path differs only by case from another");
            }
            candidates.push(file_index);
            infos.push(FileInfo {
                code: 1,
                name: file.name,
                size: file.size,
                attributes: attributes.clone(),
            });
            self.index.files.push(FileEntry {
                virtual_path,
                real_path: file.real_path,
                size: file.size,
                attributes,
                buddy_only,
            });
        }

        self.index
            .folder_lookup
            .insert(virtual_dir.clone(), folder_index);
        self.index.folders.push(ScannedFolder {
            virtual_path: virtual_dir,
            files: infos,
            buddy_only,
        });
        Ok(())
    }
}

fn read_missing_attributes(
    index: &mut SharesIndex,
    new_cache: &mut HashMap<String, CacheEntry>,
    mut misses: Vec<Miss>,
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
    let results: Vec<(usize, FileAttributes)> = {
        let files = &index.files;
        let misses = &misses;
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let mut done = Vec::new();
                        loop {
                            let position = cursor.fetch_add(1, Ordering::Relaxed);
                            let Some(miss) = misses.get(position) else {
                                break done;
                            };
                            done.push((
                                position,
                                audio_attributes(&files[miss.file_index as usize].real_path),
                            ));
                            progress.add();
                        }
                    })
                })
                .collect();
            handles.into_iter().flat_map(join_scan_thread).collect()
        })
    };
    for (position, attributes) in results {
        let miss = &mut misses[position];
        index.files[miss.file_index as usize].attributes = attributes.clone();
        index.folders[miss.folder_index].files[miss.position].attributes = attributes.clone();
        new_cache.insert(
            std::mem::take(&mut miss.key),
            CacheEntry {
                size: miss.size,
                mtime: miss.mtime,
                attributes,
            },
        );
    }
}

fn join_scan_thread<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> T {
    handle
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

fn unix_mtime(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .expect("mtime unavailable on this platform")
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn audio_attributes(path: &Path) -> FileAttributes {
    let mut attributes = FileAttributes::default();
    let Ok(tagged) = lofty::read_from_path(path) else {
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
    attributes
}

fn has_audio_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        base.join("scan-cache.json.gz")
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

    fn test_index() -> SharesIndex {
        let base = temp_base();
        let album = base.join("public/Sample Album");
        fs::create_dir_all(&album).unwrap();
        fs::write(album.join("First Song.flac"), b"x".repeat(300)).unwrap();
        fs::write(album.join("Second Tune.ogg"), b"y".repeat(300)).unwrap();
        let secret = base.join("secret");
        fs::create_dir_all(&secret).unwrap();
        fs::write(secret.join("hidden song.wav"), b"z".repeat(300)).unwrap();

        scan(
            &[
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
            ],
            &cache_path(&base),
            &|_| {},
        )
        .expect("scan test shares")
    }

    #[test]
    fn search_word_matching() {
        let index = test_index();

        let results = index.search("sample first", false, &[]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Public\\Sample Album\\First Song.flac");

        assert!(index.search("sample missing", false, &[]).is_empty());

        let excluded = index.search("sample -tune", false, &[]);
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0].name, "Public\\Sample Album\\First Song.flac");

        let partial = index.search("sample *une", false, &[]);
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].name, "Public\\Sample Album\\Second Tune.ogg");

        assert_eq!(index.search("SAMPLE ALBUM", false, &[]).len(), 2);
        assert!(index.search("ab", false, &[]).is_empty());
    }

    #[test]
    fn search_respects_permissions_and_phrases() {
        let index = test_index();

        assert!(index.search("hidden song", false, &[]).is_empty());
        let buddy = index.search("hidden song", true, &[]);
        assert_eq!(buddy.len(), 1);
        assert_eq!(buddy[0].name, "Private\\hidden song.wav");

        assert!(
            index
                .search("first song", false, &["first".into()])
                .is_empty()
        );
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
            &cache_path(&base),
            &|_| {},
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
    fn attributes_are_read_cached_and_patched_into_folders() {
        let base = temp_base();
        let dir = base.join("music");
        fs::create_dir_all(&dir).unwrap();
        write_wav(&dir.join("tone.wav"));
        let key = fs::canonicalize(&dir)
            .unwrap()
            .join("tone.wav")
            .to_string_lossy()
            .into_owned();
        let cache_path = cache_path(&base);

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &cache_path,
            &|_| {},
        )
        .unwrap();

        let (_, _, attributes) = index.resolve("Music\\tone.wav", false).unwrap();
        assert_eq!(attributes.sample_rate, Some(44100));
        assert_eq!(attributes.bit_depth, Some(16));
        assert_eq!(attributes.length, Some(1));

        let contents = index.folder_contents("Music", false);
        assert_eq!(contents[0].files[0].attributes.sample_rate, Some(44100));

        let saved = cache::load(&cache_path);
        assert_eq!(saved[&key].attributes.sample_rate, Some(44100));
    }

    #[test]
    fn cache_hit_skips_reading_attributes() {
        let base = temp_base();
        let dir = base.join("music");
        fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.flac");
        fs::write(&song, b"g".repeat(300)).unwrap();
        let metadata = fs::metadata(&song).unwrap();
        let key = fs::canonicalize(&dir)
            .unwrap()
            .join("song.flac")
            .to_string_lossy()
            .into_owned();
        let cache_path = cache_path(&base);
        let mut entries = HashMap::new();
        entries.insert(
            key,
            CacheEntry {
                size: 300,
                mtime: unix_mtime(&metadata),
                attributes: FileAttributes {
                    bitrate: Some(320),
                    ..Default::default()
                },
            },
        );
        cache::save(&cache_path, &entries);

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &cache_path,
            &|_| {},
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
        let key = fs::canonicalize(&dir)
            .unwrap()
            .join("song.flac")
            .to_string_lossy()
            .into_owned();
        let cache_path = cache_path(&base);
        let mut entries = HashMap::new();
        entries.insert(
            key.clone(),
            CacheEntry {
                size: 300,
                mtime: unix_mtime(&metadata) + 1,
                attributes: FileAttributes {
                    bitrate: Some(320),
                    ..Default::default()
                },
            },
        );
        cache::save(&cache_path, &entries);

        let index = scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &cache_path,
            &|_| {},
        )
        .unwrap();

        let (_, _, attributes) = index.resolve("Music\\song.flac", false).unwrap();
        assert_eq!(*attributes, FileAttributes::default());

        let saved = cache::load(&cache_path);
        assert_eq!(saved[&key].mtime, unix_mtime(&metadata));
        assert_eq!(saved[&key].attributes, FileAttributes::default());
    }

    #[test]
    fn cache_prunes_files_no_longer_shared() {
        let base = temp_base();
        let dir = base.join("music");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("song.mp3"), b"g".repeat(300)).unwrap();
        let cache_path = cache_path(&base);
        let mut entries = HashMap::new();
        entries.insert(
            "/nowhere/gone.mp3".to_owned(),
            CacheEntry {
                size: 1,
                mtime: 1,
                attributes: FileAttributes::default(),
            },
        );
        cache::save(&cache_path, &entries);

        scan(
            &[SharedFolder {
                virtual_name: "Music".into(),
                path: dir,
                buddy_only: false,
            }],
            &cache_path,
            &|_| {},
        )
        .unwrap();

        assert!(!cache::load(&cache_path).contains_key("/nowhere/gone.mp3"));
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
            &cache_path(&base),
            &|_| {},
        )
        .unwrap();

        assert_eq!(
            index
                .folder_contents("Music\\a@@BACKSLASH@@b", false)
                .len(),
            1
        );
        assert!(
            index
                .resolve("Music\\a@@BACKSLASH@@b\\song.mp3", false)
                .is_some()
        );
    }
}
