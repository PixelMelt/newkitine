mod cache;
mod scan;
mod wire;

pub(crate) use cache::save as save_cache;
pub(crate) use scan::AttributeCache;
pub use scan::{ScanError, ScanOutcome, scan};
pub(crate) use wire::encode_shared_file_list;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use crate::types::{FileAttributes, FileInfo, FolderContents};

#[derive(Debug)]
pub struct ShareCatalog {
    pub folders: Vec<ShareCatalogFolder>,
    pub files: Vec<ShareCatalogFile>,
    pub folders_by_path: Vec<u32>,
}

#[derive(Debug)]
pub struct ShareCatalogFolder {
    pub virtual_path: Box<str>,
    pub virtual_path_lower: Box<str>,
    pub real_path: std::path::PathBuf,
    pub files: Range<u32>,
    pub buddy_only: bool,
}

#[derive(Debug)]
pub struct ShareCatalogFile {
    pub name: Box<str>,
    pub name_lower: Box<str>,
    pub real_name: std::ffi::OsString,
    pub size: u64,
    pub attributes: FileAttributes,
}

#[derive(Debug, Default)]
pub(super) struct WordPostings {
    pub(super) files: Vec<u32>,
    pub(super) folders: Vec<u32>,
}

#[derive(Debug)]
pub struct SharesIndex {
    catalog: Arc<ShareCatalog>,
    folders_by_lower_path: Vec<u32>,
    resolve_files: Vec<u32>,
    word_index: HashMap<Box<str>, WordPostings>,
}

impl SharesIndex {
    pub(super) fn new(
        mut catalog: ShareCatalog,
        word_index: HashMap<Box<str>, WordPostings>,
    ) -> Self {
        catalog.folders_by_path = (0..catalog.folders.len() as u32).collect();
        catalog.folders_by_path.sort_by(|&left, &right| {
            catalog.folders[left as usize]
                .virtual_path
                .cmp(&catalog.folders[right as usize].virtual_path)
        });
        let mut folders_by_lower_path = catalog.folders_by_path.clone();
        folders_by_lower_path.sort_by(|&left, &right| {
            let left_folder = &catalog.folders[left as usize];
            let right_folder = &catalog.folders[right as usize];
            left_folder
                .virtual_path_lower
                .cmp(&right_folder.virtual_path_lower)
                .then_with(|| left.cmp(&right))
        });
        let mut resolve_files = Vec::with_capacity(catalog.files.len());
        for folder in &catalog.folders {
            let start = resolve_files.len();
            resolve_files.extend(folder.files.clone());
            resolve_files[start..].sort_by(|&left, &right| {
                catalog.files[left as usize]
                    .name_lower
                    .cmp(&catalog.files[right as usize].name_lower)
                    .then_with(|| left.cmp(&right))
            });
        }
        Self {
            catalog: Arc::new(catalog),
            folders_by_lower_path,
            resolve_files,
            word_index,
        }
    }

    pub fn catalog(&self) -> Arc<ShareCatalog> {
        self.catalog.clone()
    }

    pub fn counts(&self) -> (u32, u32) {
        let folders = self
            .catalog
            .folders
            .iter()
            .filter(|folder| !folder.buddy_only)
            .count();
        let files = self
            .catalog
            .folders
            .iter()
            .filter(|folder| !folder.buddy_only)
            .map(|folder| folder.files.len())
            .sum::<usize>();
        (folders as u32, files as u32)
    }

    #[cfg(test)]
    pub fn browse(&self, is_buddy: bool) -> Vec<FolderContents> {
        self.catalog
            .folders_by_path
            .iter()
            .map(|&folder| &self.catalog.folders[folder as usize])
            .filter(|folder| is_buddy || !folder.buddy_only)
            .map(|folder| self.folder_view(folder))
            .collect()
    }

    pub fn folder_contents(&self, directory: &str, is_buddy: bool) -> Vec<FolderContents> {
        let Ok(position) = self.catalog.folders_by_path.binary_search_by(|&folder| {
            self.catalog.folders[folder as usize]
                .virtual_path
                .as_ref()
                .cmp(directory)
        }) else {
            return Vec::new();
        };
        let folder = &self.catalog.folders[self.catalog.folders_by_path[position] as usize];
        if folder.buddy_only && !is_buddy {
            return Vec::new();
        }
        vec![self.folder_view(folder)]
    }

    pub fn resolve(
        &self,
        virtual_path: &str,
        is_buddy: bool,
    ) -> Option<(PathBuf, u64, &crate::types::FileAttributes)> {
        let (directory, name) = virtual_path.rsplit_once('\\')?;
        let directory_lower = directory.to_lowercase();
        let name_lower = name.to_lowercase();
        let folder_range = equal_range(&self.folders_by_lower_path, |&folder| {
            self.catalog.folders[folder as usize]
                .virtual_path_lower
                .as_ref()
                .cmp(directory_lower.as_str())
        });
        let mut first = None;
        let mut exact = None;
        for &folder_id in &self.folders_by_lower_path[folder_range] {
            let folder = &self.catalog.folders[folder_id as usize];
            let resolve_range = self.resolve_range(folder_id);
            let file_range = equal_range(&self.resolve_files[resolve_range], |&file| {
                self.catalog.files[file as usize]
                    .name_lower
                    .as_ref()
                    .cmp(name_lower.as_str())
            });
            for &file_id in &self.resolve_files[self.resolve_range(folder_id)][file_range] {
                first.get_or_insert((folder_id, file_id));
                let file = &self.catalog.files[file_id as usize];
                if folder.virtual_path.as_ref() == directory && file.name.as_ref() == name {
                    exact = Some((folder_id, file_id));
                    break;
                }
            }
            if exact.is_some() {
                break;
            }
        }
        let (folder_id, file_id) = exact.or(first)?;
        let folder = &self.catalog.folders[folder_id as usize];
        if folder.buddy_only && !is_buddy {
            return None;
        }
        let file = &self.catalog.files[file_id as usize];
        Some((
            folder.real_path.join(&file.real_name),
            file.size,
            &file.attributes,
        ))
    }

    pub fn search(
        &self,
        search_term: &str,
        is_buddy: bool,
        excluded_phrases: &[String],
        max_results: usize,
        min_chars: usize,
    ) -> Vec<FileInfo> {
        if search_term.len() < min_chars {
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
            .filter_map(|file_id| {
                let folder_id = self.folder_for_file(file_id);
                let folder = &self.catalog.folders[folder_id as usize];
                let file = &self.catalog.files[file_id as usize];
                if !is_buddy && folder.buddy_only {
                    return None;
                }
                let name = format!("{}\\{}", folder.virtual_path, file.name);
                let name_lower = name.to_lowercase();
                if excluded_phrases
                    .iter()
                    .any(|phrase| name_lower.contains(phrase))
                {
                    return None;
                }
                Some(FileInfo {
                    name,
                    size: file.size,
                    attributes: file.attributes.clone(),
                })
            })
            .take(max_results)
            .collect();
        results.sort_by(|left, right| left.name.cmp(&right.name));
        results
    }

    fn match_indices(
        &self,
        included_words: &[&str],
        excluded_words: &HashSet<&str>,
        partial_words: &HashSet<&str>,
    ) -> Option<Vec<u32>> {
        let (&start_word, rest) = included_words.split_first()?;
        let mut results = self.posting_files(self.word_index.get(start_word)?);
        for word in rest {
            let matches = self.posting_files(self.word_index.get(*word)?);
            results.retain(|file| matches.contains(file));
            if results.is_empty() {
                return None;
            }
        }
        for partial_word in partial_words {
            let mut matches = HashSet::new();
            for (word, postings) in &self.word_index {
                if word.ends_with(partial_word) {
                    matches.extend(self.posting_files(postings));
                }
            }
            results.retain(|file| matches.contains(file));
            if results.is_empty() {
                return None;
            }
        }
        for excluded_word in excluded_words {
            if let Some(postings) = self.word_index.get(*excluded_word) {
                for file in self.posting_files(postings) {
                    results.remove(&file);
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

    fn posting_files(&self, postings: &WordPostings) -> HashSet<u32> {
        let capacity = postings.files.len()
            + postings
                .folders
                .iter()
                .map(|&folder| self.catalog.folders[folder as usize].files.len())
                .sum::<usize>();
        let mut files = HashSet::with_capacity(capacity);
        files.extend(postings.files.iter().copied());
        for &folder in &postings.folders {
            files.extend(self.catalog.folders[folder as usize].files.clone());
        }
        files
    }

    fn folder_view(&self, folder: &ShareCatalogFolder) -> FolderContents {
        FolderContents {
            directory: folder.virtual_path.to_string(),
            files: folder
                .files
                .clone()
                .map(|file| self.file_view(&self.catalog.files[file as usize]))
                .collect(),
        }
    }

    fn file_view(&self, file: &ShareCatalogFile) -> FileInfo {
        FileInfo {
            name: file.name.to_string(),
            size: file.size,
            attributes: file.attributes.clone(),
        }
    }

    fn folder_for_file(&self, file_id: u32) -> u32 {
        self.catalog
            .folders
            .partition_point(|folder| folder.files.end <= file_id) as u32
    }

    fn resolve_range(&self, folder_id: u32) -> Range<usize> {
        let folder = &self.catalog.folders[folder_id as usize];
        folder.files.start as usize..folder.files.end as usize
    }
}

fn equal_range<T>(slice: &[T], mut compare: impl FnMut(&T) -> std::cmp::Ordering) -> Range<usize> {
    let start = slice.partition_point(|item| compare(item).is_lt());
    let end = start + slice[start..].partition_point(|item| compare(item).is_eq());
    start..end
}

fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}
