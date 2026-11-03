use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use reqwest::{Method, StatusCode};
use soulseek::storage::{
    DEFAULT_BASENAME_BYTE_LIMIT, Download, DownloadRequest, DownloadStore, OpenedDownload,
    candidate_basenames, download_basename, download_folder, incomplete_basename,
};
use soulseek::types::BoxFuture;

use super::client::{Answer, WebDav, method};
use super::path::{child, collection, parent, remote_path};
use super::put::PutWriter;

type Listing = Result<HashMap<Vec<u8>, Option<u64>>, String>;

pub struct WebDavDownloads {
    client: WebDav,
    dirs: Arc<Dirs>,
}

struct Dirs {
    download_dir: String,
    incomplete_dir: String,
    username_subfolders: bool,
}

impl WebDavDownloads {
    pub fn new(
        client: WebDav,
        download_dir: String,
        incomplete_dir: String,
        username_subfolders: bool,
    ) -> Result<WebDavDownloads, String> {
        Ok(WebDavDownloads {
            client,
            dirs: Arc::new(Dirs {
                download_dir: remote_path(&download_dir)?,
                incomplete_dir: remote_path(&incomplete_dir)?,
                username_subfolders,
            }),
        })
    }
}

impl Dirs {
    fn incomplete_path(&self, download: &Download) -> String {
        child(
            &self.incomplete_dir,
            &incomplete_basename(download, DEFAULT_BASENAME_BYTE_LIMIT),
        )
    }
}

fn basename(download: &Download) -> String {
    download_basename(&download.virtual_path, DEFAULT_BASENAME_BYTE_LIMIT)
}

fn complete_file(names: &HashMap<Vec<u8>, Option<u64>>, download: &Download) -> Option<String> {
    let basename = basename(download);
    candidate_basenames(&basename)
        .map_while(|candidate| Some((*names.get(candidate.as_bytes())?, candidate)))
        .find_map(|(size, candidate)| {
            (size == Some(download.size)).then(|| child(&download.destination, &candidate))
        })
}

async fn mkcol(client: &WebDav, path: &str) -> io::Result<Answer> {
    client
        .call(method("MKCOL"), &collection(path.as_bytes()), |request| {
            request
        })
        .await
}

async fn make_collections(client: &WebDav, path: &str) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut current = path;
    while current != "/" {
        let answer = mkcol(client, current).await?;
        match answer.status() {
            StatusCode::CREATED | StatusCode::METHOD_NOT_ALLOWED => break,
            StatusCode::CONFLICT => {
                missing.push(current);
                current = parent(current);
            }
            _ => return Err(answer.refused().await),
        }
    }
    for path in missing.into_iter().rev() {
        let answer = mkcol(client, path).await?;
        if !matches!(
            answer.status(),
            StatusCode::CREATED | StatusCode::METHOD_NOT_ALLOWED
        ) {
            return Err(answer.refused().await);
        }
    }
    Ok(())
}

impl DownloadStore for WebDavDownloads {
    fn destination(&self, request: &DownloadRequest<'_>) -> String {
        download_folder(request, self.dirs.username_subfolders)
            .iter()
            .fold(self.dirs.download_dir.clone(), |folder, part| {
                child(&folder, part)
            })
    }

    fn existing(&self, downloads: Vec<Download>) -> BoxFuture<Vec<io::Result<Option<String>>>> {
        let client = self.client.clone();
        Box::pin(async move {
            let mut folders: HashMap<&str, Listing> = HashMap::new();
            for download in &downloads {
                if folders.contains_key(download.destination.as_str()) {
                    continue;
                }
                let listing = match client.list(download.destination.as_bytes()).await {
                    Ok(entries) => Ok(entries
                        .into_iter()
                        .map(|entry| (entry.name().to_vec(), entry.size))
                        .collect()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
                    Err(error) => Err(error.to_string()),
                };
                folders.insert(&download.destination, listing);
            }
            downloads
                .iter()
                .map(|download| match &folders[download.destination.as_str()] {
                    Ok(names) => Ok(complete_file(names, download)),
                    Err(error) => Err(io::Error::other(error.clone())),
                })
                .collect()
        })
    }

    fn open(&self, download: Download, _restart: bool) -> BoxFuture<io::Result<OpenedDownload>> {
        let (client, dirs) = (self.client.clone(), self.dirs.clone());
        Box::pin(async move {
            make_collections(&client, &dirs.incomplete_dir).await?;
            let writer = PutWriter::start(&client, dirs.incomplete_path(&download), download.size);
            Ok(OpenedDownload {
                writer: Box::new(writer),
                offset: 0,
            })
        })
    }

    fn finish(&self, download: Download) -> BoxFuture<io::Result<String>> {
        let (client, dirs) = (self.client.clone(), self.dirs.clone());
        Box::pin(async move {
            make_collections(&client, &download.destination).await?;
            let incomplete = dirs.incomplete_path(&download);
            let basename = basename(&download);
            for candidate in candidate_basenames(&basename) {
                let target = child(&download.destination, &candidate);
                let destination = client.absolute(target.as_bytes());
                let answer = client
                    .call(method("MOVE"), incomplete.as_bytes(), |request| {
                        request
                            .header("Destination", destination)
                            .header("Overwrite", "F")
                    })
                    .await?;
                match answer.status() {
                    StatusCode::CREATED | StatusCode::NO_CONTENT => return Ok(target),
                    StatusCode::PRECONDITION_FAILED => {}
                    _ => return Err(answer.refused().await),
                }
            }
            unreachable!("download candidates never run out")
        })
    }

    fn discard(&self, download: Download) -> BoxFuture<io::Result<()>> {
        let (client, dirs) = (self.client.clone(), self.dirs.clone());
        Box::pin(async move {
            let path = dirs.incomplete_path(&download);
            let answer = client
                .call(Method::DELETE, path.as_bytes(), |request| request)
                .await?;
            match answer.status() {
                status if status.is_success() || status == StatusCode::NOT_FOUND => Ok(()),
                _ => Err(answer.refused().await),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode as FakeStatus;
    use tokio::io::AsyncWriteExt;

    use super::super::fake::Fake;
    use super::*;

    const INCOMPLETE: &str = "/partials/nested/INCOMPLETE3b547388b3820c2c370aa9c94452674dsong.mp3";

    fn store(fake: &Fake) -> WebDavDownloads {
        WebDavDownloads::new(
            fake.client(),
            "/downloads/".to_owned(),
            "/partials/nested".to_owned(),
            true,
        )
        .unwrap()
    }

    fn download(name: &str, size: u64, destination: &str) -> Download {
        Download {
            username: "peer".to_owned(),
            virtual_path: format!("share\\Album\\{name}"),
            size,
            destination: destination.to_owned(),
        }
    }

    fn song(size: u64) -> Download {
        download("song.mp3", size, "/downloads/peer/Album")
    }

    async fn existing(store: &WebDavDownloads, downloads: Vec<Download>) -> Vec<Option<String>> {
        store
            .existing(downloads)
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect()
    }

    #[tokio::test]
    async fn a_download_streams_to_the_incomplete_path_and_moves_past_collisions() {
        let fake = Fake::start().await;
        fake.file("/downloads/peer/Album/song.mp3", b"old");
        let store = store(&fake);
        let destination = store.destination(&DownloadRequest {
            username: "peer",
            virtual_path: "share\\Album\\song.mp3",
            root: Some("share\\Album"),
        });
        assert_eq!(destination, "/downloads/peer/Album");
        assert_eq!(existing(&store, vec![song(6)]).await, [None]);

        let mut opened = store.open(song(6), true).await.unwrap();
        assert_eq!(opened.offset, 0);
        opened.writer.write_all(b"abc").await.unwrap();
        opened.writer.write_all(b"def").await.unwrap();
        opened.writer.shutdown().await.unwrap();
        assert_eq!(fake.read(INCOMPLETE), Some(b"abcdef".to_vec()));
        let requests = fake.requests();
        assert_eq!(
            requests[requests.len() - 4..],
            [
                "MKCOL /partials/nested",
                "MKCOL /partials",
                "MKCOL /partials/nested",
                &format!("PUT {INCOMPLETE}"),
            ]
        );

        let location = store.finish(song(6)).await.unwrap();
        assert_eq!(location, "/downloads/peer/Album/song (1).mp3");
        assert_eq!(fake.read(&location), Some(b"abcdef".to_vec()));
        assert_eq!(
            fake.read("/downloads/peer/Album/song.mp3"),
            Some(b"old".to_vec())
        );
        assert!(!fake.exists(INCOMPLETE));
        assert_eq!(existing(&store, vec![song(6)]).await, [Some(location)]);
    }

    #[tokio::test]
    async fn finishing_creates_the_destination_folders() {
        let fake = Fake::start().await;
        fake.file(INCOMPLETE, b"abcdef");
        let location = store(&fake)
            .finish(download("song.mp3", 6, "/downloads/peer/New/Deep"))
            .await
            .unwrap();
        assert_eq!(location, "/downloads/peer/New/Deep/song.mp3");
        assert_eq!(fake.read(&location), Some(b"abcdef".to_vec()));
    }

    #[tokio::test]
    async fn a_short_download_aborts_its_put_and_leaves_nothing() {
        let fake = Fake::start().await;
        let store = store(&fake);

        let mut opened = store.open(song(6), false).await.unwrap();
        fake.received(&format!("PUT {INCOMPLETE}"), 1).await;
        opened.writer.write_all(b"abc").await.unwrap();
        let error = opened.writer.shutdown().await.unwrap_err().to_string();
        assert!(error.contains("aborted after 3 of 6 bytes"), "{error}");
        fake.aborted(INCOMPLETE, 1).await;
        assert!(!fake.exists(INCOMPLETE));

        let mut opened = store.open(song(6), false).await.unwrap();
        opened.writer.write_all(b"ab").await.unwrap();
        fake.received(&format!("PUT {INCOMPLETE}"), 2).await;
        drop(opened);
        fake.aborted(INCOMPLETE, 2).await;
        assert!(!fake.exists(INCOMPLETE));
    }

    #[tokio::test]
    async fn an_early_rejection_fails_the_next_write() {
        let fake = Fake::start().await;
        fake.reject_puts(FakeStatus::INSUFFICIENT_STORAGE);
        let size = 256 * 1024 * 1024;
        let mut opened = store(&fake).open(song(size), false).await.unwrap();
        let chunk = vec![7u8; 64 * 1024];
        let mut written = 0;
        let error = loop {
            match opened.writer.write_all(&chunk).await {
                Ok(()) => written += chunk.len() as u64,
                Err(error) => break error.to_string(),
            }
            assert!(written < size, "every write was accepted");
        };
        assert!(
            error.contains(&format!(
                "PUT {INCOMPLETE} answered 507 Insufficient Storage: Insufficient Storage"
            )),
            "{error}"
        );
    }

    #[tokio::test]
    async fn an_empty_download_surfaces_the_refusal_of_empty_uploads() {
        let fake = Fake::start().await;
        fake.refuse_empty_puts();
        let mut opened = store(&fake).open(song(0), false).await.unwrap();
        let error = opened.writer.shutdown().await.unwrap_err().to_string();
        assert_eq!(
            error,
            format!("PUT {INCOMPLETE} answered 405 Method Not Allowed: Method Not Allowed")
        );
        assert!(!fake.exists(INCOMPLETE));
    }

    #[tokio::test]
    async fn existing_files_match_by_size_across_collision_names() {
        let fake = Fake::start().await;
        fake.file("/downloads/peer/Album/song.mp3", b"abc");
        fake.file("/downloads/peer/Album/song (1).mp3", b"abcdef");
        fake.collection("/downloads/peer/Album/other.mp3");
        fake.file("/downloads/peer/Album/other (1).mp3", b"ot");
        let store = store(&fake);

        let answers = existing(
            &store,
            vec![
                song(6),
                song(3),
                song(4),
                download("missing.mp3", 1, "/downloads/peer/Album"),
                download("song.mp3", 3, "/downloads/peer/Gone"),
                download("other.mp3", 2, "/downloads/peer/Album"),
            ],
        )
        .await;

        assert_eq!(
            answers,
            [
                Some("/downloads/peer/Album/song (1).mp3".to_owned()),
                Some("/downloads/peer/Album/song.mp3".to_owned()),
                None,
                None,
                None,
                Some("/downloads/peer/Album/other (1).mp3".to_owned()),
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
                "PROPFIND /downloads/peer/Album",
                "PROPFIND /downloads/peer/Gone"
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_listing_fails_only_its_folder() {
        let fake = Fake::start().await;
        fake.file("/downloads/peer/Album/song.mp3", b"abc");
        let store = WebDavDownloads::new(
            WebDav::new(&fake.url, "user", "wrong").unwrap(),
            "/downloads".to_owned(),
            "/partials".to_owned(),
            true,
        )
        .unwrap();
        let answers = store.existing(vec![song(3), song(3)]).await;
        assert_eq!(answers.len(), 2);
        for answer in answers {
            assert!(answer.unwrap_err().to_string().contains("401"));
        }
    }

    #[tokio::test]
    async fn discarding_deletes_the_partial_and_accepts_a_missing_one() {
        let fake = Fake::start().await;
        fake.file(INCOMPLETE, b"abc");
        let store = store(&fake);
        store.discard(song(6)).await.unwrap();
        assert!(!fake.exists(INCOMPLETE));
        store.discard(song(6)).await.unwrap();
        assert_eq!(
            fake.requests(),
            [
                format!("DELETE {INCOMPLETE}"),
                format!("DELETE {INCOMPLETE}")
            ]
        );
    }

    #[test]
    fn download_directories_are_validated() {
        let client = WebDav::new("http://127.0.0.1:1/", "u", "p").unwrap();
        let build = |download: &str, incomplete: &str| {
            WebDavDownloads::new(client.clone(), download.into(), incomplete.into(), false)
        };
        assert!(build("downloads", "/partials").is_err());
        assert!(build("/downloads", "/a//b").is_err());
        let root = build("/", "/partials/").unwrap();
        assert_eq!(root.dirs.incomplete_dir, "/partials");
        assert_eq!(
            root.destination(&DownloadRequest {
                username: "peer",
                virtual_path: "share\\Album\\song.mp3",
                root: None,
            }),
            "/"
        );
    }
}
