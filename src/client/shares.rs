use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use lofty::prelude::AudioFile;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::types::{FileAttributes, FileInfo, FolderContents, UINT32_LIMIT};

const BACKSLASH_SENTINEL: &str = "@@BACKSLASH@@";
pub const MAX_SEARCH_RESULTS: usize = 300;
pub const MIN_SEARCH_CHARS: usize = 3;

const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "ac3", "afc", "aif", "aifc", "aiff", "ape", "au", "bwav", "bwf", "dff", "dsd", "dsf",
    "dts", "flac", "m4a", "m4b", "mka", "mp1", "mp2", "mp3", "mp+", "mpc", "oga", "ogg", "opus",
    "spx", "tak", "tta", "wav", "wma", "wv",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedFolder {
    pub virtual_name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub buddy_only: bool,
}

#[derive(Debug)]
struct ScannedFolder {
    virtual_path: String,
    files: Vec<FileInfo>,
    buddy_only: bool,
}

#[derive(Debug)]
struct FileEntry {
    virtual_path: String,
    real_path: PathBuf,
    size: u64,
    attributes: FileAttributes,
    buddy_only: bool,
}

#[derive(Debug, Default)]
pub struct SharesIndex {
    folders: Vec<ScannedFolder>,
    folder_lookup: HashMap<String, usize>,
    files: Vec<FileEntry>,
    paths_lower: HashMap<String, u32>,
    word_index: HashMap<String, Vec<u32>>,
}

impl SharesIndex {
    pub fn counts(&self) -> (u32, u32) {
        let folders = self
            .folders
            .iter()
            .filter(|folder| !folder.buddy_only)
            .count();
        let files = self.files.iter().filter(|file| !file.buddy_only).count();
        (folders as u32, files as u32)
    }

    pub fn browse(&self, is_buddy: bool) -> Vec<FolderContents> {
        let mut list: Vec<FolderContents> = self
            .folders
            .iter()
            .filter(|folder| is_buddy || !folder.buddy_only)
            .map(|folder| FolderContents {
                directory: folder.virtual_path.clone(),
                files: folder.files.clone(),
            })
            .collect();
        list.sort_by(|a, b| a.directory.cmp(&b.directory));
        list
    }

    pub fn folder_contents(&self, directory: &str, is_buddy: bool) -> Vec<FolderContents> {
        let Some(&index) = self.folder_lookup.get(directory) else {
            return Vec::new();
        };
        let folder = &self.folders[index];
        if folder.buddy_only && !is_buddy {
            return Vec::new();
        }
        vec![FolderContents {
            directory: folder.virtual_path.clone(),
            files: folder.files.clone(),
        }]
    }

    pub fn resolve(
        &self,
        virtual_path: &str,
        is_buddy: bool,
    ) -> Option<(&Path, u64, &FileAttributes)> {
        let &index = self.paths_lower.get(&virtual_path.to_lowercase())?;
        let entry = &self.files[index as usize];
        if entry.buddy_only && !is_buddy {
            return None;
        }
        Some((&entry.real_path, entry.size, &entry.attributes))
    }

    pub fn search(
        &self,
        search_term: &str,
        is_buddy: bool,
        excluded_phrases: &[String],
    ) -> Vec<FileInfo> {
        if search_term.len() < MIN_SEARCH_CHARS {
            return Vec::new();
        }
        let term_lower = search_term.to_lowercase();

        let mut excluded_words = HashSet::new();
        let mut partial_words = HashSet::new();
        if term_lower.contains('-') || term_lower.contains('*') {
            for word in term_lower.split_whitespace() {
                if let Some(rest) = word.strip_prefix('-') {
                    excluded_words.extend(split_words(rest));
                } else if let Some(rest) = word.strip_prefix('*') {
                    partial_words.extend(split_words(rest));
                }
            }
        }
        let included_words: Vec<&str> = split_words(&term_lower)
            .collect::<HashSet<&str>>()
            .into_iter()
            .filter(|word| !excluded_words.contains(word) && !partial_words.contains(word))
            .collect();

        let Some(indices) = self.match_indices(&included_words, &excluded_words, &partial_words)
        else {
            return Vec::new();
        };

        let mut results: Vec<FileInfo> = indices
            .into_iter()
            .map(|index| &self.files[index as usize])
            .filter(|entry| is_buddy || !entry.buddy_only)
            .filter(|entry| {
                let path_lower = entry.virtual_path.to_lowercase();
                !excluded_phrases
                    .iter()
                    .any(|phrase| path_lower.contains(phrase))
            })
            .take(MAX_SEARCH_RESULTS)
            .map(|entry| FileInfo {
                code: 1,
                name: entry.virtual_path.clone(),
                size: entry.size,
                attributes: entry.attributes.clone(),
            })
            .collect();
        results.sort_by(|a, b| a.name.cmp(&b.name));
        results
    }

    fn match_indices(
        &self,
        included_words: &[&str],
        excluded_words: &HashSet<&str>,
        partial_words: &HashSet<&str>,
    ) -> Option<Vec<u32>> {
        let (&start_word, rest) = included_words.split_first()?;
        for word in included_words {
            if !self.word_index.contains_key(*word) {
                return None;
            }
        }

        let mut results: HashSet<u32> = self.word_index[start_word].iter().copied().collect();
        for word in rest {
            let indices = &self.word_index[*word];
            results.retain(|index| indices.contains(index));
            if results.is_empty() {
                return None;
            }
        }

        for partial_word in partial_words {
            let mut partial_results = HashSet::new();
            for (complete_word, indices) in &self.word_index {
                if !complete_word.ends_with(partial_word) {
                    continue;
                }
                partial_results.extend(indices.iter().filter(|index| results.contains(index)));
            }
            if partial_results.is_empty() {
                return None;
            }
            results = partial_results;
        }

        for excluded_word in excluded_words {
            if let Some(indices) = self.word_index.get(*excluded_word) {
                for index in indices {
                    results.remove(index);
                }
            }
            if results.is_empty() {
                return None;
            }
        }

        let mut indices: Vec<u32> = results.into_iter().collect();
        indices.sort_unstable();
        Some(indices)
    }
}

pub fn scan(shared_folders: &[SharedFolder]) -> SharesIndex {
    let mut index = SharesIndex::default();
    for shared in shared_folders {
        scan_shared_folder(&mut index, shared);
    }
    let (folders, files) = index.counts();
    info!(
        folders,
        files,
        total_files = index.files.len(),
        "share scan complete"
    );
    index
}

fn scan_shared_folder(index: &mut SharesIndex, shared: &SharedFolder) {
    let mut stack = vec![(shared.path.clone(), shared.virtual_name.clone())];
    while let Some((real_dir, virtual_dir)) = stack.pop() {
        if index.folder_lookup.contains_key(&virtual_dir) {
            continue;
        }
        let entries = match fs::read_dir(&real_dir) {
            Ok(entries) => entries,
            Err(error) => {
                warn!(path = %real_dir.display(), %error, "cannot scan folder");
                continue;
            }
        };

        let mut files = Vec::new();
        let virtual_dir_lower = virtual_dir.to_lowercase();
        let folder_words: HashSet<String> =
            split_words(&virtual_dir_lower).map(str::to_owned).collect();

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    warn!(path = %path.display(), %error, "cannot stat entry");
                    continue;
                }
            };
            if metadata.is_dir() {
                stack.push((path, format!("{virtual_dir}\\{name}")));
                continue;
            }
            if !metadata.is_file() {
                continue;
            }

            let basename = name.replace('\\', BACKSLASH_SENTINEL);
            let virtual_path = format!("{virtual_dir}\\{basename}");
            let virtual_path_lower = virtual_path.to_lowercase();
            if index.paths_lower.contains_key(&virtual_path_lower) {
                continue;
            }

            let size = metadata.len();
            let attributes = audio_attributes(&path, size);
            let file_index = index.files.len() as u32;

            let basename_lower = basename.to_lowercase();
            let mut words = folder_words.clone();
            words.extend(split_words(&basename_lower).map(str::to_owned));
            for word in words {
                index.word_index.entry(word).or_default().push(file_index);
            }

            files.push(FileInfo {
                code: 1,
                name: basename,
                size,
                attributes: attributes.clone(),
            });
            index.paths_lower.insert(virtual_path_lower, file_index);
            index.files.push(FileEntry {
                virtual_path,
                real_path: path,
                size,
                attributes,
                buddy_only: shared.buddy_only,
            });
        }

        files.sort_by(|a, b| a.name.cmp(&b.name));
        index
            .folder_lookup
            .insert(virtual_dir.clone(), index.folders.len());
        index.folders.push(ScannedFolder {
            virtual_path: virtual_dir,
            files,
            buddy_only: shared.buddy_only,
        });
    }
}

fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

fn audio_attributes(path: &Path, size: u64) -> FileAttributes {
    let mut attributes = FileAttributes::default();
    if size <= 128 || !has_audio_extension(path) {
        return attributes;
    }
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

    fn test_index() -> SharesIndex {
        let base = std::env::temp_dir().join(format!("newkitine-shares-{}", std::process::id()));
        let album = base.join("public/Sample Album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("First Song.flac"), b"x".repeat(300)).unwrap();
        std::fs::write(album.join("Second Tune.ogg"), b"y".repeat(300)).unwrap();
        let secret = base.join("secret");
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("hidden song.wav"), b"z".repeat(300)).unwrap();

        scan(&[
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
        ])
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
}
