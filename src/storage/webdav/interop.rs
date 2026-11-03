use std::io;
use std::time::Duration;

use reqwest::Method;
use soulseek::storage::{
    DEFAULT_BASENAME_BYTE_LIMIT, Download, DownloadRequest, DownloadStore, ShareFilters, ShareKey,
    ShareSource, incomplete_basename,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::client::WebDav;
use super::downloads::WebDavDownloads;
use super::path::collection;
use super::shares::{WebDavShare, WebDavShares};

fn variable(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must name the scratch WebDAV server"))
}

#[tokio::test]
#[ignore = "needs a scratch WebDAV server in NEWKITINE_WEBDAV_URL, _USERNAME and _PASSWORD"]
async fn the_stores_work_against_a_live_webdav_server() {
    let client = WebDav::new(
        &variable("NEWKITINE_WEBDAV_URL"),
        &variable("NEWKITINE_WEBDAV_USERNAME"),
        &variable("NEWKITINE_WEBDAV_PASSWORD"),
    )
    .unwrap();
    let root = format!("/newkitine-interop-{}", std::process::id());
    let store = WebDavDownloads::new(
        client.clone(),
        format!("{root}/complete"),
        format!("{root}/incomplete/deep"),
        true,
    )
    .unwrap();
    let request = DownloadRequest {
        username: "peer",
        virtual_path: "share\\Album #1\\片 100% & more.mp3",
        root: Some("share\\Album #1"),
    };
    let destination = store.destination(&request);
    assert_eq!(destination, format!("{root}/complete/peer/Album #1"));
    let content: Vec<u8> = (0..300_000u32).map(|index| (index % 251) as u8).collect();
    let download = Download {
        username: "peer".to_owned(),
        virtual_path: request.virtual_path.to_owned(),
        size: content.len() as u64,
        destination,
    };

    for name in ["片 100% & more.mp3", "片 100% & more (1).mp3"] {
        let mut opened = store.open(download.clone(), false).await.unwrap();
        assert_eq!(opened.offset, 0);
        for chunk in content.chunks(65_536) {
            opened.writer.write_all(chunk).await.unwrap();
        }
        opened.writer.shutdown().await.unwrap();
        assert_eq!(
            store.finish(download.clone()).await.unwrap(),
            format!("{}/{name}", download.destination)
        );
    }
    assert_eq!(
        store.existing(vec![download.clone()]).await[0]
            .as_ref()
            .unwrap(),
        &Some(format!("{}/片 100% & more.mp3", download.destination))
    );
    store.discard(download.clone()).await.unwrap();

    let shares = WebDavShares::new(
        client.clone(),
        vec![WebDavShare {
            virtual_name: "Interop".to_owned(),
            path: format!("{root}/complete/"),
            buddy_only: true,
        }],
        ShareFilters::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let catalog = shares.load().await.unwrap().expect("the share is listed");
    let folders: Vec<&str> = catalog
        .folders()
        .iter()
        .map(|folder| folder.virtual_path())
        .collect();
    assert_eq!(
        folders,
        ["Interop", "Interop\\peer", "Interop\\peer\\Album #1"]
    );
    let album = &catalog.folders()[2];
    assert!(album.buddy_only());
    assert_eq!(
        album.key(),
        format!("webdav:{root}/complete/peer/Album #1").as_bytes()
    );
    let files: Vec<(&str, u64)> = catalog
        .files(album)
        .iter()
        .map(|file| (file.name(), file.size()))
        .collect();
    assert_eq!(
        files,
        [
            ("片 100% & more (1).mp3", 300_000),
            ("片 100% & more.mp3", 300_000)
        ]
    );
    let key = ShareKey::new(album.key(), catalog.files(album)[1].key());
    assert_eq!(shares.size(key.clone()).await.unwrap(), 300_000);
    for offset in [0, 123_457, 300_000] {
        let mut opened = shares.open(key.clone(), offset).await.unwrap();
        assert_eq!(opened.size, 300_000);
        let mut read = Vec::new();
        opened.reader.read_to_end(&mut read).await.unwrap();
        assert!(read == content[offset as usize..], "offset {offset}");
    }

    let mut opened = store.open(download.clone(), false).await.unwrap();
    opened.writer.write_all(&content[..100_000]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(opened.writer.shutdown().await.is_err());
    drop(opened);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let incomplete = format!(
        "{root}/incomplete/deep/{}",
        incomplete_basename(&download, DEFAULT_BASENAME_BYTE_LIMIT)
    );
    store.discard(download.clone()).await.unwrap();
    let truncated = client.stat(incomplete.as_bytes()).await;

    let cleanup = client
        .call(Method::DELETE, &collection(root.as_bytes()), |request| {
            request
        })
        .await
        .unwrap();
    assert!(cleanup.status().is_success());
    match truncated {
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}"),
        Ok(entry) => panic!(
            "discarding the truncated PUT left {} with {:?} bytes behind",
            incomplete, entry.size
        ),
    }
}
