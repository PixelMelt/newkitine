use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use futures_util::TryStreamExt;
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, HeaderName, RANGE};
use reqwest::{Method, StatusCode};
use soulseek::storage::{
    CatalogFile, OpenedShare, ScanError, ScanProgress, ShareCatalog, ShareFilters, ShareKey,
    ShareSource, check_share_name, escape_backslashes,
};
use soulseek::types::{BoxFuture, FileAttributes};
use tokio_util::io::StreamReader;
use tracing::{info, warn};

use super::client::{Answer, WebDav};
use super::path::remote_path;

const KEY_PREFIX: &[u8] = b"webdav:";

#[derive(Debug, Clone)]
pub struct WebDavShare {
    pub virtual_name: String,
    pub path: String,
    pub buddy_only: bool,
}

pub struct WebDavShares {
    client: WebDav,
    config: Arc<Config>,
}

struct Config {
    shares: Vec<WebDavShare>,
    filters: ShareFilters,
}

impl WebDavShares {
    pub fn new(
        client: WebDav,
        shares: Vec<WebDavShare>,
        filters: ShareFilters,
    ) -> Result<WebDavShares, String> {
        let shares = shares
            .into_iter()
            .map(|share| {
                check_share_name(&share.virtual_name)?;
                Ok(WebDavShare {
                    path: remote_path(&share.path)?,
                    ..share
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(WebDavShares {
            client,
            config: Arc::new(Config { shares, filters }),
        })
    }
}

fn source_error(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ScanError {
    ScanError::Source(error.into())
}

async fn list(
    client: &WebDav,
    config: &Config,
    progress: &ScanProgress,
) -> Result<ShareCatalog, ScanError> {
    let mut names = HashSet::new();
    for share in &config.shares {
        if !names.insert(share.virtual_name.as_str()) {
            return Err(source_error(format!(
                "duplicate virtual folder name {}",
                share.virtual_name
            )));
        }
    }
    let mut walk = Walk {
        client,
        filters: &config.filters,
        progress,
        virtual_paths: HashSet::new(),
        catalog: ShareCatalog::new(),
    };
    for share in &config.shares {
        walk.share(share).await?;
    }
    if progress.is_cancelled() {
        return Err(ScanError::Superseded);
    }
    info!(
        folders = walk.catalog.folders().len(),
        files = walk.catalog.file_count(),
        "WebDAV share listing complete"
    );
    Ok(walk.catalog)
}

struct Walk<'a> {
    client: &'a WebDav,
    filters: &'a ShareFilters,
    progress: &'a ScanProgress,
    virtual_paths: HashSet<String>,
    catalog: ShareCatalog,
}

impl Walk<'_> {
    async fn share(&mut self, share: &WebDavShare) -> Result<(), ScanError> {
        let mut stack = vec![(share.path.as_bytes().to_vec(), share.virtual_name.clone())];
        while let Some((remote_dir, virtual_dir)) = stack.pop() {
            if self.progress.is_cancelled() {
                return Err(ScanError::Superseded);
            }
            if !self.virtual_paths.insert(virtual_dir.clone()) {
                warn!(path = %String::from_utf8_lossy(&remote_dir), virtual_path = %virtual_dir, "skipping folder with a duplicate virtual path");
                continue;
            }
            let entries = self.client.list(&remote_dir).await.map_err(source_error)?;
            let mut names = HashSet::new();
            let mut files = Vec::new();
            for entry in entries {
                let name = String::from_utf8_lossy(entry.name()).into_owned();
                if self.filters.skips(&name) {
                    continue;
                }
                let name = escape_backslashes(&name);
                if entry.collection {
                    stack.push((entry.path, format!("{virtual_dir}\\{name}")));
                    continue;
                }
                if !names.insert(name.clone()) {
                    warn!(path = %String::from_utf8_lossy(&entry.path), folder = %virtual_dir, name, "skipping file with a duplicate virtual path");
                    continue;
                }
                let size = entry.size.ok_or_else(|| {
                    source_error(format!(
                        "{} has no getcontentlength",
                        String::from_utf8_lossy(&entry.path)
                    ))
                })?;
                self.progress.add_file();
                files.push(CatalogFile::new(
                    name,
                    entry.name().to_vec(),
                    size,
                    0,
                    FileAttributes::default(),
                ));
            }
            self.catalog
                .push_folder(
                    virtual_dir,
                    [KEY_PREFIX, remote_dir.as_slice()].concat(),
                    share.buddy_only,
                    files,
                )
                .map_err(source_error)?;
        }
        Ok(())
    }
}

fn key_path(key: &ShareKey) -> io::Result<Vec<u8>> {
    let mut path = key
        .folder()
        .strip_prefix(KEY_PREFIX)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("share key {key} does not belong to the WebDAV share"),
            )
        })?
        .to_vec();
    if path != b"/" {
        path.push(b'/');
    }
    path.extend_from_slice(key.file());
    Ok(path)
}

fn header(answer: &Answer, name: HeaderName) -> Option<&str> {
    answer.response.headers().get(name)?.to_str().ok()
}

fn full_size(answer: &Answer) -> io::Result<u64> {
    header(answer, CONTENT_LENGTH)
        .and_then(|length| length.parse().ok())
        .ok_or_else(|| answer.unusable("without a usable Content-Length"))
}

fn partial_size(answer: &Answer, offset: u64) -> io::Result<u64> {
    let range = header(answer, CONTENT_RANGE);
    let parsed = range.and_then(|range| {
        let (span, total) = range.strip_prefix("bytes ")?.split_once('/')?;
        let (start, end) = span.split_once('-')?;
        Some((
            start.parse::<u64>().ok()?,
            end.parse::<u64>().ok()?,
            total.parse::<u64>().ok()?,
        ))
    });
    match parsed {
        Some((start, end, total)) if start == offset && end.checked_add(1) == Some(total) => {
            Ok(total)
        }
        _ => Err(answer.unusable(format_args!(
            "with Content-Range {range:?} for offset {offset}"
        ))),
    }
}

fn unsatisfiable_size(answer: &Answer) -> Option<u64> {
    header(answer, CONTENT_RANGE)?
        .strip_prefix("bytes */")?
        .parse()
        .ok()
}

impl ShareSource for WebDavShares {
    fn load(&self) -> BoxFuture<Result<Option<ShareCatalog>, ScanError>> {
        let (client, config) = (self.client.clone(), self.config.clone());
        Box::pin(async move {
            if config.shares.is_empty() {
                return Ok(None);
            }
            let progress = ScanProgress::new(Arc::new(AtomicBool::new(false)), |_| {});
            list(&client, &config, &progress).await.map(Some)
        })
    }

    fn scan(&self, progress: ScanProgress) -> BoxFuture<Result<ShareCatalog, ScanError>> {
        let (client, config) = (self.client.clone(), self.config.clone());
        Box::pin(async move { list(&client, &config, &progress).await })
    }

    fn size(&self, key: ShareKey) -> BoxFuture<io::Result<u64>> {
        let client = self.client.clone();
        Box::pin(async move {
            let path = key_path(&key)?;
            client.stat(&path).await?.size.ok_or_else(|| {
                io::Error::other(format!(
                    "{} is not a file with a getcontentlength",
                    String::from_utf8_lossy(&path)
                ))
            })
        })
    }

    fn open(&self, key: ShareKey, offset: u64) -> BoxFuture<io::Result<OpenedShare>> {
        let client = self.client.clone();
        Box::pin(async move {
            let path = key_path(&key)?;
            let answer = client
                .stream(Method::GET, &path, |request| match offset {
                    0 => request,
                    offset => request.header(RANGE, format!("bytes={offset}-")),
                })
                .await?;
            let size = match answer.status() {
                StatusCode::OK if offset == 0 => full_size(&answer)?,
                StatusCode::PARTIAL_CONTENT => partial_size(&answer, offset)?,
                StatusCode::RANGE_NOT_SATISFIABLE
                    if unsatisfiable_size(&answer) == Some(offset) =>
                {
                    return Ok(OpenedShare {
                        reader: Box::new(tokio::io::empty()),
                        size: offset,
                    });
                }
                _ => return Err(answer.refused().await),
            };
            let body = answer.response.bytes_stream().map_err(io::Error::other);
            Ok(OpenedShare {
                reader: Box::new(StreamReader::new(body)),
                size,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::io::AsyncReadExt;

    use super::super::fake::Fake;
    use super::*;

    fn share(virtual_name: &str, path: &str, buddy_only: bool) -> WebDavShare {
        WebDavShare {
            virtual_name: virtual_name.to_owned(),
            path: path.to_owned(),
            buddy_only,
        }
    }

    fn filters(names: &[&str]) -> ShareFilters {
        ShareFilters::new(names.iter().map(|name| (*name).to_owned()).collect()).unwrap()
    }

    fn progress(cancelled: bool) -> ScanProgress {
        ScanProgress::new(Arc::new(AtomicBool::new(cancelled)), |_| {})
    }

    type Listed = (String, Vec<u8>, bool, Vec<(String, Vec<u8>, u64)>);

    fn listed(catalog: &ShareCatalog) -> Vec<Listed> {
        catalog
            .folders()
            .iter()
            .map(|folder| {
                let files = catalog
                    .files(folder)
                    .iter()
                    .map(|file| {
                        assert_eq!(file.modified(), 0);
                        assert_eq!(*file.attributes(), FileAttributes::default());
                        (file.name().to_owned(), file.key().to_vec(), file.size())
                    })
                    .collect();
                (
                    folder.virtual_path().to_owned(),
                    folder.key().to_vec(),
                    folder.buddy_only(),
                    files,
                )
            })
            .collect()
    }

    fn entry(name: &str, size: u64) -> (String, Vec<u8>, u64) {
        (name.to_owned(), name.as_bytes().to_vec(), size)
    }

    async fn music_library() -> (Fake, WebDavShares) {
        let fake = Fake::start().await;
        fake.file("/music/song one.mp3", b"0123456789");
        fake.file("/music/.dotfile", b"d");
        fake.file("/music/.hidden/inside.mp3", b"h");
        fake.file("/music/Covers/front.jpg", b"c");
        fake.file("/music/Album #1/片 100%.flac", b"abcde");
        fake.file("/music/Album #1/Thumbs.db", b"t");
        fake.file("/music/back\\slash/rock & roll.ogg", b"rr");
        fake.collection("/music/Empty");
        fake.file("/private/secret.ogg", b"sss");
        let shares = WebDavShares::new(
            fake.client(),
            vec![
                share("Music", "/music/", false),
                share("Private", "/private", true),
            ],
            filters(&["Thumbs.db", "Covers"]),
        )
        .unwrap();
        (fake, shares)
    }

    #[tokio::test]
    async fn the_listing_builds_the_catalog_the_local_walk_would() {
        let (fake, shares) = music_library().await;

        let catalog = shares.scan(progress(false)).await.unwrap();

        assert_eq!(
            listed(&catalog),
            [
                (
                    "Music".to_owned(),
                    b"webdav:/music".to_vec(),
                    false,
                    vec![entry("song one.mp3", 10)]
                ),
                (
                    "Music\\back@@BACKSLASH@@slash".to_owned(),
                    b"webdav:/music/back\\slash".to_vec(),
                    false,
                    vec![entry("rock & roll.ogg", 2)]
                ),
                (
                    "Music\\Empty".to_owned(),
                    b"webdav:/music/Empty".to_vec(),
                    false,
                    Vec::new()
                ),
                (
                    "Music\\Album #1".to_owned(),
                    b"webdav:/music/Album #1".to_vec(),
                    false,
                    vec![entry("片 100%.flac", 5)]
                ),
                (
                    "Private".to_owned(),
                    b"webdav:/private".to_vec(),
                    true,
                    vec![entry("secret.ogg", 3)]
                ),
            ]
        );
        let listings: Vec<String> = fake
            .requests()
            .into_iter()
            .filter(|request| request.starts_with("PROPFIND"))
            .collect();
        assert_eq!(
            listings,
            [
                "PROPFIND /music",
                "PROPFIND /music/back\\slash",
                "PROPFIND /music/Empty",
                "PROPFIND /music/Album #1",
                "PROPFIND /private",
            ]
        );

        let loaded = shares.load().await.unwrap().expect("shares are listed");
        assert_eq!(listed(&loaded), listed(&catalog));
    }

    #[tokio::test]
    async fn without_shares_nothing_is_requested() {
        let fake = Fake::start().await;
        let shares = WebDavShares::new(fake.client(), Vec::new(), filters(&[])).unwrap();
        assert!(shares.load().await.unwrap().is_none());
        assert!(
            shares
                .scan(progress(false))
                .await
                .unwrap()
                .folders()
                .is_empty()
        );
        assert!(fake.requests().is_empty());
    }

    #[tokio::test]
    async fn scans_report_progress_and_stop_when_cancelled() {
        let fake = Fake::start().await;
        for index in 0..1001 {
            fake.file(&format!("/many/{index:04}.mp3"), b"x");
        }
        let shares = WebDavShares::new(
            fake.client(),
            vec![share("Many", "/many", false)],
            filters(&[]),
        )
        .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let counting = ScanProgress::new(Arc::new(AtomicBool::new(false)), {
            let seen = seen.clone();
            move |count| seen.lock().unwrap().push(count)
        });

        let catalog = shares.scan(counting).await.unwrap();

        assert_eq!(catalog.file_count(), 1001);
        assert_eq!(*seen.lock().unwrap(), [1000]);
        let requests = fake.requests().len();
        assert!(matches!(
            shares.scan(progress(true)).await,
            Err(ScanError::Superseded)
        ));
        assert_eq!(fake.requests().len(), requests);
    }

    #[tokio::test]
    async fn missing_shares_and_duplicate_names_fail_the_scan() {
        let fake = Fake::start().await;
        fake.collection("/music");
        let missing = WebDavShares::new(
            fake.client(),
            vec![share("Gone", "/gone", false)],
            filters(&[]),
        )
        .unwrap();
        let error = missing.scan(progress(false)).await.unwrap_err().to_string();
        assert!(error.contains("404"), "{error}");

        fake.file("/song.mp3", b"x");
        let file_root = WebDavShares::new(
            fake.client(),
            vec![share("Song", "/song.mp3", false)],
            filters(&[]),
        )
        .unwrap();
        let error = file_root
            .scan(progress(false))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a collection"), "{error}");

        let duplicate = WebDavShares::new(
            fake.client(),
            vec![
                share("Music", "/music", false),
                share("Music", "/music", true),
            ],
            filters(&[]),
        )
        .unwrap();
        assert!(matches!(
            duplicate.scan(progress(false)).await,
            Err(ScanError::Source(_))
        ));
    }

    #[tokio::test]
    async fn files_report_their_size_and_open_at_any_offset() {
        let (_fake, shares) = music_library().await;
        let catalog = shares.scan(progress(false)).await.unwrap();
        let folder = catalog
            .folders()
            .iter()
            .find(|folder| folder.virtual_path() == "Music")
            .unwrap();
        let key = ShareKey::new(folder.key(), catalog.files(folder)[0].key());

        assert_eq!(shares.size(key.clone()).await.unwrap(), 10);
        for (offset, rest) in [(0, &b"0123456789"[..]), (4, b"456789"), (10, b"")] {
            let mut opened = shares.open(key.clone(), offset).await.unwrap();
            assert_eq!(opened.size, 10);
            let mut read = Vec::new();
            opened.reader.read_to_end(&mut read).await.unwrap();
            assert_eq!(read, rest, "offset {offset}");
        }

        let album = catalog
            .folders()
            .iter()
            .find(|folder| folder.virtual_path() == "Music\\Album #1")
            .unwrap();
        let encoded = ShareKey::new(album.key(), catalog.files(album)[0].key());
        let mut opened = shares.open(encoded.clone(), 2).await.unwrap();
        let mut read = Vec::new();
        opened.reader.read_to_end(&mut read).await.unwrap();
        assert_eq!((opened.size, read), (5, b"cde".to_vec()));
        assert_eq!(shares.size(encoded).await.unwrap(), 5);

        let gone = ShareKey::new(folder.key(), b"gone.mp3".to_vec());
        assert_eq!(
            shares.size(gone.clone()).await.unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(shares.open(gone, 0).await.is_err());
        let collection = ShareKey::new(b"webdav:/music".to_vec(), b"Empty".to_vec());
        assert!(shares.size(collection).await.is_err());

        let local = ShareKey::new(b"/music".to_vec(), b"a.mp3".to_vec());
        assert_eq!(
            shares.size(local.clone()).await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            shares.open(local, 0).await.err().unwrap().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[tokio::test]
    async fn wrong_credentials_surface_the_401() {
        let fake = Fake::start().await;
        let client = WebDav::new(&fake.url, "user", "wrong").unwrap();
        let shares =
            WebDavShares::new(client, vec![share("Music", "/", false)], filters(&[])).unwrap();
        let error = shares.scan(progress(false)).await.unwrap_err().to_string();
        assert!(
            error.contains("PROPFIND / answered 401 Unauthorized"),
            "{error}"
        );
    }

    #[test]
    fn share_names_and_paths_are_validated() {
        let client = WebDav::new("http://127.0.0.1:1/", "u", "p").unwrap();
        let build = |share| WebDavShares::new(client.clone(), vec![share], filters(&[]));
        assert!(build(share("a\\b", "/music", false)).is_err());
        assert!(build(share("", "/music", false)).is_err());
        assert!(build(share("Music", "music", false)).is_err());
        assert!(build(share("Music", "/music/../etc", false)).is_err());
        let built = build(share("Music", "/music/", false)).unwrap();
        assert_eq!(built.config.shares[0].path, "/music");
    }
}
