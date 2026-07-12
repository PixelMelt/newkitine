mod cache;
mod scan;

pub use scan::{ScanError, scan};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::types::{FileAttributes, FileInfo, FolderContents};

const MAX_SEARCH_RESULTS: usize = 300;
const MIN_SEARCH_CHARS: usize = 3;

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
    paths_lower: HashMap<String, Vec<u32>>,
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
        let candidates = self.paths_lower.get(&virtual_path.to_lowercase())?;
        let index = candidates
            .iter()
            .copied()
            .find(|&index| self.files[index as usize].virtual_path == virtual_path)
            .unwrap_or(candidates[0]);
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

fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}
