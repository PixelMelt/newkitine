mod common;

use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::timeout;

use common::{free_port, start_fake_server, temp_dir, tempfile};
use newkitine::client::{Client, ClientConfig, ClientEvent, DownloadUpdate, SharedFolder, UploadUpdate};
use newkitine::network::{NetworkCommand, NetworkEvent, spawn as spawn_network};
use newkitine::protocol::PeerMessage;
use newkitine::types::{ConnectionType, FileAttributes, SearchScope, TransferDirection};

async fn wait_client<T>(
    events: &mut UnboundedReceiver<ClientEvent>,
    mut matcher: impl FnMut(ClientEvent) -> Option<T>,
) -> T {
    timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("event channel closed");
            if let Some(value) = matcher(event) {
                return value;
            }
        }
    })
    .await
    .expect("timed out waiting for client event")
}

async fn wait_net<T>(
    events: &mut UnboundedReceiver<NetworkEvent>,
    mut matcher: impl FnMut(NetworkEvent) -> Option<T>,
) -> T {
    timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("event channel closed");
            if let Some(value) = matcher(event) {
                return value;
            }
        }
    })
    .await
    .expect("timed out waiting for network event")
}

#[tokio::test]
async fn client_downloads_file_from_peer() {
    let (server_addr, _registry) = start_fake_server().await;

    let (uploader, mut uploader_events) = spawn_network();
    uploader.send(NetworkCommand::ServerConnect {
        address: server_addr,
        username: "alice".into(),
        password: "secret".into(),
        listen_port: free_port(),
    });
    wait_net(&mut uploader_events, |event| match event {
        NetworkEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    let download_dir = temp_dir("downloads");
    let incomplete_dir = temp_dir("incomplete");
    let mut bob_config =
        ClientConfig::new(server_addr, "bob", "secret", free_port(), download_dir.clone());
    bob_config.incomplete_dir = incomplete_dir;
    bob_config.description = "test client".into();
    let (bob, mut bob_events) = Client::spawn(bob_config);
    wait_client(&mut bob_events, |event| match event {
        ClientEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    let payload: Vec<u8> = (0u32..50_000).flat_map(|value| value.to_le_bytes()).collect();
    let virtual_path = "Music\\Album\\song.mp3";
    bob.download("alice", virtual_path, payload.len() as u64, FileAttributes::default());

    wait_net(&mut uploader_events, |event| match event {
        NetworkEvent::PeerMessage { message: PeerMessage::QueueUpload { file, .. }, .. }
            if file == virtual_path =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    uploader.peer("bob", PeerMessage::TransferRequest {
        direction: TransferDirection::Upload,
        token: 99,
        file: virtual_path.into(),
        filesize: Some(payload.len() as u64),
    });

    wait_net(&mut uploader_events, |event| match event {
        NetworkEvent::PeerMessage {
            message: PeerMessage::TransferResponse { token: 99, allowed: true, .. },
            ..
        } => Some(()),
        _ => None,
    })
    .await;

    let mut source = tempfile();
    source.write_all(&payload).unwrap();
    source.seek(SeekFrom::Start(0)).unwrap();

    uploader.send(NetworkCommand::RequestFileConnection { username: "bob".into(), token: 99 });
    let upload_conn = wait_net(&mut uploader_events, |event| match event {
        NetworkEvent::PeerConnected { conn_type: ConnectionType::File, conn_id, .. } => {
            Some(conn_id)
        }
        _ => None,
    })
    .await;
    uploader.send(NetworkCommand::UploadFile {
        conn_id: upload_conn,
        file: source,
        size: payload.len() as u64,
    });

    wait_client(&mut bob_events, |event| match event {
        ClientEvent::Download(DownloadUpdate::Started { username, .. }) => {
            assert_eq!(username, "alice");
            Some(())
        }
        _ => None,
    })
    .await;

    let file_path = wait_client(&mut bob_events, |event| match event {
        ClientEvent::Download(DownloadUpdate::Finished { file_path, .. }) => Some(file_path),
        ClientEvent::Download(DownloadUpdate::Failed { reason, .. }) => {
            panic!("download failed: {reason}")
        }
        _ => None,
    })
    .await;

    assert!(file_path.starts_with(&download_dir));
    assert_eq!(file_path.file_name().unwrap().to_str().unwrap(), "song.mp3");
    assert_eq!(std::fs::read(&file_path).unwrap(), payload);
}

#[tokio::test]
async fn client_shares_answers_search_and_uploads() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir("shared-music");
    let album_dir = share_dir.join("Best Album");
    std::fs::create_dir_all(&album_dir).unwrap();
    let payload: Vec<u8> = (0u32..50_000).flat_map(|value| value.to_le_bytes()).collect();
    std::fs::write(album_dir.join("unique melody.mp3"), &payload).unwrap();
    std::fs::write(album_dir.join("other tune.mp3"), b"tiny").unwrap();

    let mut eve_config =
        ClientConfig::new(server_addr, "eve", "secret", free_port(), temp_dir("eve-downloads"));
    eve_config.shared_folders = vec![SharedFolder {
        virtual_name: "Music".into(),
        path: share_dir,
        buddy_only: false,
    }];
    let (_eve, mut eve_events) = Client::spawn(eve_config);
    wait_client(&mut eve_events, |event| match event {
        ClientEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;
    let (folders, files) = wait_client(&mut eve_events, |event| match event {
        ClientEvent::SharesScanned { folders, files } => Some((folders, files)),
        _ => None,
    })
    .await;
    assert_eq!(folders, 2);
    assert_eq!(files, 2);

    let download_dir = temp_dir("frank-downloads");
    let frank_config =
        ClientConfig::new(server_addr, "frank", "secret", free_port(), download_dir.clone());
    let (frank, mut frank_events) = Client::spawn(frank_config);
    wait_client(&mut frank_events, |event| match event {
        ClientEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    let token = frank.search("unique melody", SearchScope::Global);
    let (result_username, results) = wait_client(&mut frank_events, |event| match event {
        ClientEvent::SearchResults { token: got, username, results, .. } if got == token => {
            Some((username, results))
        }
        _ => None,
    })
    .await;
    assert_eq!(result_username, "eve");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "Music\\Best Album\\unique melody.mp3");
    assert_eq!(results[0].size, payload.len() as u64);

    frank.browse_user("eve");
    let shares = wait_client(&mut frank_events, |event| match event {
        ClientEvent::SharedFileList { username, shares, .. } if username == "eve" => {
            Some(shares)
        }
        _ => None,
    })
    .await;
    let album = shares
        .iter()
        .find(|folder| folder.directory == "Music\\Best Album")
        .expect("album folder in browse response");
    assert_eq!(album.files.len(), 2);

    frank.download("eve", &results[0].name, results[0].size, results[0].attributes.clone());

    wait_client(&mut eve_events, |event| match event {
        ClientEvent::Upload(UploadUpdate::Started { username, .. }) => {
            assert_eq!(username, "frank");
            Some(())
        }
        _ => None,
    })
    .await;

    let file_path = wait_client(&mut frank_events, |event| match event {
        ClientEvent::Download(DownloadUpdate::Finished { file_path, .. }) => Some(file_path),
        ClientEvent::Download(DownloadUpdate::Failed { reason, .. }) => {
            panic!("download failed: {reason}")
        }
        _ => None,
    })
    .await;
    assert_eq!(std::fs::read(&file_path).unwrap(), payload);

    wait_client(&mut eve_events, |event| match event {
        ClientEvent::Upload(UploadUpdate::Finished { username, virtual_path }) => {
            assert_eq!(username, "frank");
            assert_eq!(virtual_path, "Music\\Best Album\\unique melody.mp3");
            Some(())
        }
        ClientEvent::Upload(UploadUpdate::Failed { reason, .. }) => {
            panic!("upload failed: {reason}")
        }
        _ => None,
    })
    .await;
}

#[tokio::test]
async fn wishlist_search_runs_on_interval() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir("grace-shared");
    std::fs::create_dir_all(&share_dir).unwrap();
    std::fs::write(share_dir.join("wishlist gem.mp3"), b"g".repeat(500)).unwrap();

    let mut grace_config = ClientConfig::new(
        server_addr,
        "grace",
        "secret",
        free_port(),
        temp_dir("grace-downloads"),
    );
    grace_config.shared_folders = vec![SharedFolder {
        virtual_name: "Stash".into(),
        path: share_dir,
        buddy_only: false,
    }];
    let (_grace, mut grace_events) = Client::spawn(grace_config);
    wait_client(&mut grace_events, |event| match event {
        ClientEvent::SharesScanned { .. } => Some(()),
        _ => None,
    })
    .await;

    let mut heidi_config = ClientConfig::new(
        server_addr,
        "heidi",
        "secret",
        free_port(),
        temp_dir("heidi-downloads"),
    );
    heidi_config.wishlist = vec!["wishlist gem".into()];
    let (_heidi, mut heidi_events) = Client::spawn(heidi_config);

    let (username, results) = wait_client(&mut heidi_events, |event| match event {
        ClientEvent::SearchResults { username, results, .. } => Some((username, results)),
        _ => None,
    })
    .await;
    assert_eq!(username, "grace");
    assert_eq!(results[0].name, "Stash\\wishlist gem.mp3");
}

#[tokio::test]
async fn client_receives_search_results() {
    let (server_addr, _registry) = start_fake_server().await;

    let (responder, mut responder_events) = spawn_network();
    responder.send(NetworkCommand::ServerConnect {
        address: server_addr,
        username: "carol".into(),
        password: "secret".into(),
        listen_port: free_port(),
    });
    wait_net(&mut responder_events, |event| match event {
        NetworkEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    let mut dave_config =
        ClientConfig::new(server_addr, "dave", "secret", free_port(), temp_dir("downloads2"));
    dave_config.incomplete_dir = temp_dir("incomplete2");
    let (dave, mut dave_events) = Client::spawn(dave_config);
    wait_client(&mut dave_events, |event| match event {
        ClientEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    let token = dave.search("test query", SearchScope::Global);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let results = vec![newkitine::types::FileInfo {
        code: 1,
        name: "Music\\test query.flac".into(),
        size: 123456,
        attributes: FileAttributes {
            length: Some(200),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            ..FileAttributes::default()
        },
    }];
    responder.peer("dave", PeerMessage::FileSearchResponse {
        username: "carol".into(),
        token,
        results: results.clone(),
        free_upload_slots: true,
        upload_speed: 500000,
        queue_size: 0,
        unknown: 0,
        private_results: Vec::new(),
    });

    let (username, received) = wait_client(&mut dave_events, |event| match event {
        ClientEvent::SearchResults { token: got, username, results, .. } if got == token => {
            Some((username, results))
        }
        _ => None,
    })
    .await;
    assert_eq!(username, "carol");
    assert_eq!(received, results);
}
