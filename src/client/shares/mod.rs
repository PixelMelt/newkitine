mod cache;
mod scan;
mod wire;

pub use cache::{load as load_catalog, save as save_catalog};
pub use scan::{ScanError, restrict, walk};

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;

use super::punctuation::split_words;
use crate::types::{FileAttributes, FileInfo, FolderContents};

#[derive(Debug, Clone, Default)]
pub struct ShareCatalog {
    pub folders: Vec<ShareCatalogFolder>,
    pub files: Vec<ShareCatalogFile>,
    folders_by_path: Vec<u32>,
}

pub fn empty_browse_frame() -> Vec<u8> {
    wire::encode_shared_file_list(&ShareCatalog::empty(), false)
}

impl ShareCatalog {
    pub(super) fn empty() -> Self {
        Self::default()
    }

    pub(super) fn push_folder(
        &mut self,
        virtual_path: String,
        real_path: PathBuf,
        buddy_only: bool,
        files: impl IntoIterator<Item = ShareCatalogFile>,
    ) {
        let start = self.files.len() as u32;
        self.files.extend(files);
        self.folders.push(ShareCatalogFolder {
            virtual_path_lower: virtual_path.to_lowercase().into_boxed_str(),
            virtual_path: virtual_path.into_boxed_str(),
            real_path,
            files: start..self.files.len() as u32,
            buddy_only,
        });
    }

    pub(super) fn folder_files(&self, folder: &ShareCatalogFolder) -> &[ShareCatalogFile] {
        &self.files[folder.files.start as usize..folder.files.end as usize]
    }
}

#[derive(Debug, Clone)]
pub struct ShareCatalogFolder {
    pub virtual_path: Box<str>,
    pub virtual_path_lower: Box<str>,
    pub real_path: PathBuf,
    pub files: Range<u32>,
    pub buddy_only: bool,
}

#[derive(Debug, Clone)]
pub struct ShareCatalogFile {
    pub name: Box<str>,
    pub name_lower: Box<str>,
    pub real_name: std::ffi::OsString,
    pub size: u64,
    pub mtime: u64,
    pub attributes: FileAttributes,
}

impl ShareCatalogFile {
    pub(super) fn new(
        name: String,
        real_name: std::ffi::OsString,
        size: u64,
        mtime: u64,
        attributes: FileAttributes,
    ) -> Self {
        Self {
            name_lower: name.to_lowercase().into_boxed_str(),
            name: name.into_boxed_str(),
            real_name,
            size,
            mtime,
            attributes,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct WordPostings {
    pub(super) files: Vec<u32>,
    pub(super) folders: Vec<u32>,
}

#[derive(Debug)]
pub struct SharesIndex {
    catalog: ShareCatalog,
    folders_by_lower_path: Vec<u32>,
    resolve_files: Vec<u32>,
    word_ids: HashMap<Box<str>, u32>,
    postings: Vec<WordPostings>,
    suffixes: Vec<(Box<str>, u32)>,
    public_counts: (u32, u32),
    public_browse_frame: Vec<u8>,
    buddy_browse_frame: Option<Vec<u8>>,
}

fn index_words(
    word_index: &mut HashMap<Box<str>, WordPostings>,
    text: &str,
    mut post: impl FnMut(&mut WordPostings),
) {
    let mut words: Vec<&str> = split_words(text).collect();
    words.sort_unstable();
    words.dedup();
    for word in words {
        match word_index.get_mut(word) {
            Some(postings) => post(postings),
            None => {
                let mut postings = WordPostings::default();
                post(&mut postings);
                word_index.insert(word.into(), postings);
            }
        }
    }
}

impl SharesIndex {
    pub fn from_catalog(mut catalog: ShareCatalog) -> Self {
        let mut word_index: HashMap<Box<str>, WordPostings> = HashMap::new();
        for (folder_id, folder) in catalog.folders.iter().enumerate() {
            if !folder.files.is_empty() {
                index_words(&mut word_index, &folder.virtual_path_lower, |postings| {
                    postings.folders.push(folder_id as u32)
                });
            }
            for file_id in folder.files.clone() {
                index_words(
                    &mut word_index,
                    &catalog.files[file_id as usize].name_lower,
                    |postings| postings.files.push(file_id),
                );
            }
        }
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

        let mut word_ids = HashMap::with_capacity(word_index.len());
        let mut postings = Vec::with_capacity(word_index.len());
        let mut suffixes = Vec::with_capacity(word_index.len());
        for (word, word_postings) in word_index {
            let id = postings.len() as u32;
            suffixes.push((word.chars().rev().collect::<Box<str>>(), id));
            word_ids.insert(word, id);
            postings.push(word_postings);
        }
        suffixes.sort_unstable();

        let public_folders = catalog
            .folders
            .iter()
            .filter(|folder| !folder.buddy_only)
            .count();
        let public_files = catalog
            .folders
            .iter()
            .filter(|folder| !folder.buddy_only)
            .map(|folder| folder.files.len())
            .sum::<usize>();
        let has_buddy_folders = public_folders < catalog.folders.len();
        let public_browse_frame = wire::encode_shared_file_list(&catalog, false);
        let buddy_browse_frame =
            has_buddy_folders.then(|| wire::encode_shared_file_list(&catalog, true));

        Self {
            catalog,
            folders_by_lower_path,
            resolve_files,
            word_ids,
            postings,
            suffixes,
            public_counts: (public_folders as u32, public_files as u32),
            public_browse_frame,
            buddy_browse_frame,
        }
    }

    pub fn counts(&self) -> (u32, u32) {
        self.public_counts
    }

    pub fn browse_frame(&self, is_buddy: bool) -> &[u8] {
        match (&self.buddy_browse_frame, is_buddy) {
            (Some(frame), true) => frame,
            _ => &self.public_browse_frame,
        }
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
        if search_term.chars().count() < min_chars {
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
        let mut included = Vec::new();
        for word in split_words(&term_lower).collect::<HashSet<&str>>() {
            if excluded_words.contains(word) || partial_words.contains(word) {
                continue;
            }
            let Some(postings) = self.postings(word) else {
                return Vec::new();
            };
            included.push(postings);
        }
        included.sort_by_key(|postings| self.posting_len(postings));
        let Some((seed, rest)) = included.split_first() else {
            return Vec::new();
        };
        let partial: Vec<HashSet<u32>> = partial_words
            .iter()
            .map(|suffix| self.suffix_files(suffix))
            .collect();
        if partial.iter().any(HashSet::is_empty) {
            return Vec::new();
        }
        let excluded: Vec<&WordPostings> = excluded_words
            .iter()
            .filter_map(|word| self.postings(word))
            .collect();

        let mut results = Vec::new();
        let mut path_lower = String::new();
        for file_id in self.posting_files(seed) {
            if !rest
                .iter()
                .all(|postings| self.posting_contains(postings, file_id))
                || !partial.iter().all(|files| files.contains(&file_id))
                || excluded
                    .iter()
                    .any(|postings| self.posting_contains(postings, file_id))
            {
                continue;
            }
            let folder = &self.catalog.folders[self.folder_for_file(file_id) as usize];
            if !is_buddy && folder.buddy_only {
                continue;
            }
            let file = &self.catalog.files[file_id as usize];
            if !excluded_phrases.is_empty() {
                path_lower.clear();
                path_lower.push_str(&folder.virtual_path_lower);
                path_lower.push('\\');
                path_lower.push_str(&file.name_lower);
                if excluded_phrases
                    .iter()
                    .any(|phrase| path_lower.contains(phrase.as_str()))
                {
                    continue;
                }
            }
            results.push(FileInfo {
                name: format!("{}\\{}", folder.virtual_path, file.name),
                size: file.size,
                attributes: file.attributes.clone(),
            });
            if results.len() == max_results {
                break;
            }
        }
        results.sort_by(|left, right| left.name.cmp(&right.name));
        results
    }

    fn postings(&self, word: &str) -> Option<&WordPostings> {
        self.word_ids
            .get(word)
            .map(|&id| &self.postings[id as usize])
    }

    fn posting_len(&self, postings: &WordPostings) -> usize {
        postings.files.len()
            + postings
                .folders
                .iter()
                .map(|&folder| self.catalog.folders[folder as usize].files.len())
                .sum::<usize>()
    }

    fn posting_contains(&self, postings: &WordPostings, file_id: u32) -> bool {
        postings.files.binary_search(&file_id).is_ok()
            || postings
                .folders
                .binary_search(&self.folder_for_file(file_id))
                .is_ok()
    }

    fn posting_files<'a>(&'a self, postings: &'a WordPostings) -> impl Iterator<Item = u32> + 'a {
        let mut next_file = 0usize;
        let mut next_folder = 0usize;
        let mut folder_files: Option<Range<u32>> = None;
        std::iter::from_fn(move || {
            if folder_files.as_ref().is_none_or(Range::is_empty) {
                folder_files = postings.folders.get(next_folder).map(|&folder| {
                    next_folder += 1;
                    self.catalog.folders[folder as usize].files.clone()
                });
            }
            let from_file = postings.files.get(next_file).copied();
            let from_folder = folder_files.as_ref().map(|range| range.start);
            match (from_file, from_folder) {
                (None, None) => None,
                (Some(file), None) => {
                    next_file += 1;
                    Some(file)
                }
                (None, Some(file)) => {
                    folder_files.as_mut().unwrap().start += 1;
                    Some(file)
                }
                (Some(file), Some(folder_file)) => {
                    if file <= folder_file {
                        next_file += 1;
                    }
                    if folder_file <= file {
                        folder_files.as_mut().unwrap().start += 1;
                    }
                    Some(file.min(folder_file))
                }
            }
        })
    }

    fn suffix_files(&self, suffix: &str) -> HashSet<u32> {
        let key: String = suffix.chars().rev().collect();
        let start = self
            .suffixes
            .partition_point(|(word, _)| word.as_ref() < key.as_str());
        let end = start
            + self.suffixes[start..].partition_point(|(word, _)| word.starts_with(key.as_str()));
        let mut files = HashSet::new();
        for &(_, id) in &self.suffixes[start..end] {
            files.extend(self.posting_files(&self.postings[id as usize]));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn index(names: &[&str]) -> SharesIndex {
        let mut catalog = ShareCatalog::empty();
        catalog.push_folder(
            "Music\\Caf\u{e9}".into(),
            PathBuf::from("/music"),
            false,
            names.iter().map(|name| {
                ShareCatalogFile::new(
                    name.to_string(),
                    name.into(),
                    1,
                    0,
                    FileAttributes::default(),
                )
            }),
        );
        SharesIndex::from_catalog(catalog)
    }

    fn names(results: Vec<FileInfo>) -> Vec<String> {
        results.into_iter().map(|file| file.name).collect()
    }

    #[test]
    fn minimum_length_counts_characters_not_bytes() {
        let index = index(&["\u{e4}\u{f6}.mp3"]);
        assert_eq!(index.search("\u{e4}\u{f6}", false, &[], 10, 3), Vec::new());
        assert_eq!(
            names(index.search("\u{e4}\u{f6}", false, &[], 10, 2)),
            ["Music\\Caf\u{e9}\\\u{e4}\u{f6}.mp3"]
        );
    }

    #[test]
    fn index_and_queries_split_on_the_nicotine_punctuation_set() {
        let index = index(&[
            "\u{2665}love.mp3",
            "live\u{2013}set.mp3",
            "caf\u{e9}_tunes.mp3",
        ]);
        assert!(index.search("love", false, &[], 10, 1).is_empty());
        assert_eq!(
            names(index.search("\u{2665}love", false, &[], 10, 1)),
            ["Music\\Caf\u{e9}\\\u{2665}love.mp3"]
        );
        assert_eq!(
            names(index.search("set live", false, &[], 10, 1)),
            ["Music\\Caf\u{e9}\\live\u{2013}set.mp3"]
        );
        assert_eq!(
            names(index.search("caf\u{e9}\u{2014}tunes", false, &[], 10, 1)),
            ["Music\\Caf\u{e9}\\caf\u{e9}_tunes.mp3"]
        );
    }
}
