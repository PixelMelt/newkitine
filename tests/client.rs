mod common;

use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;

use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

use common::{client_config, free_port, start_fake_server, temp_dir, tempfile};
use newkitine::client::Client;
use newkitine::network::spawn as spawn_network;
use newkitine::protocol::PeerMessage;
use newkitine::types::{
    ClientEvent, ConnectionType, EnqueueResult, FileAttributes, NetworkCommand, NetworkEvent,
    Observation, Restriction, SearchScope, SharedFolder, TransferDirection, TransferEvent,
    TransferUpdate,
};

async fn wait_client<T>(
    events: &mut Receiver<ClientEvent>,
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
    events: &mut Receiver<NetworkEvent>,
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
    let mut bob_config = client_config(server_addr, "bob", free_port(), download_dir.clone());
    bob_config.runtime.incomplete_dir = incomplete_dir;
    bob_config.runtime.description = "test client".into();
    let (bob, mut bob_events) = Client::spawn(bob_config);
    wait_client(&mut bob_events, |event| match event {
        ClientEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    let payload: Vec<u8> = (0u32..50_000)
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let virtual_path = "Music\\Album\\song.mp3";
    assert_eq!(
        bob.download(
            "alice",
            virtual_path,
            payload.len() as u64,
            FileAttributes::default()
        )
        .await,
        EnqueueResult::Enqueued
    );

    wait_net(&mut uploader_events, |event| match event {
        NetworkEvent::PeerMessage {
            message: PeerMessage::QueueUpload { file, .. },
            ..
        } if file == virtual_path => Some(()),
        _ => None,
    })
    .await;

    uploader.peer(
        "bob",
        PeerMessage::TransferRequest {
            direction: TransferDirection::Upload,
            token: 99,
            file: virtual_path.into(),
            filesize: Some(payload.len() as u64),
        },
    );

    wait_net(&mut uploader_events, |event| match event {
        NetworkEvent::PeerMessage {
            message:
                PeerMessage::TransferResponse {
                    token: 99,
                    allowed: true,
                    ..
                },
            ..
        } => Some(()),
        _ => None,
    })
    .await;

    let mut source = tempfile();
    source.write_all(&payload).unwrap();
    source.seek(SeekFrom::Start(0)).unwrap();

    uploader.send(NetworkCommand::RequestFileConnection {
        username: "bob".into(),
        token: 99,
    });
    let upload_conn = wait_net(&mut uploader_events, |event| match event {
        NetworkEvent::PeerConnected {
            conn_type: ConnectionType::File,
            conn_id,
            ..
        } => Some(conn_id),
        _ => None,
    })
    .await;
    uploader.send(NetworkCommand::UploadFile {
        conn_id: upload_conn,
        file: source,
        size: payload.len() as u64,
    });

    let queued_id = wait_client(&mut bob_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            id,
            direction: TransferDirection::Download,
            update: TransferUpdate::Queued { username, .. },
        }) => {
            assert_eq!(username, "alice");
            Some(id)
        }
        _ => None,
    })
    .await;

    wait_client(&mut bob_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            id,
            direction: TransferDirection::Download,
            update: TransferUpdate::Started { .. },
        }) => {
            assert_eq!(id, queued_id);
            Some(())
        }
        _ => None,
    })
    .await;

    let file_path = wait_client(&mut bob_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Download,
            update: TransferUpdate::Finished { file_path, .. },
            ..
        }) => Some(file_path.expect("finished download carries its placed path")),
        ClientEvent::Transfer(TransferEvent {
            update: TransferUpdate::Failed { reason },
            ..
        }) => {
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
    let payload: Vec<u8> = (0u32..50_000)
        .flat_map(|value| value.to_le_bytes())
        .collect();
    std::fs::write(album_dir.join("unique melody.mp3"), &payload).unwrap();
    std::fs::write(album_dir.join("other tune.mp3"), b"tiny").unwrap();

    let mut eve_config = client_config(server_addr, "eve", free_port(), temp_dir("eve-downloads"));
    eve_config.runtime.shared_folders = vec![SharedFolder {
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
    let frank_config = client_config(server_addr, "frank", free_port(), download_dir.clone());
    let (frank, mut frank_events) = Client::spawn(frank_config);
    wait_client(&mut frank_events, |event| match event {
        ClientEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    let token = frank.search("unique melody", SearchScope::Global).await;
    let (result_username, results) = wait_client(&mut frank_events, |event| match event {
        ClientEvent::SearchResults(result) if result.token == token => {
            Some((result.username, result.results))
        }
        _ => None,
    })
    .await;
    assert_eq!(result_username, "eve");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "Music\\Best Album\\unique melody.mp3");
    assert_eq!(results[0].size, payload.len() as u64);

    frank.browse_user("eve").await;
    let shares = wait_client(&mut frank_events, |event| match event {
        ClientEvent::SharedFileList {
            username, shares, ..
        } if username == "eve" => Some(shares),
        _ => None,
    })
    .await;
    let album = shares
        .iter()
        .find(|folder| folder.directory == "Music\\Best Album")
        .expect("album folder in browse response");
    assert_eq!(album.files.len(), 2);

    assert_eq!(
        frank
            .download(
                "eve",
                &results[0].name,
                results[0].size,
                FileAttributes::default()
            )
            .await,
        EnqueueResult::Enqueued
    );

    let upload_id = wait_client(&mut eve_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            id,
            direction: TransferDirection::Upload,
            update:
                TransferUpdate::Queued {
                    username,
                    virtual_path,
                    ..
                },
        }) => {
            assert_eq!(username, "frank");
            assert_eq!(virtual_path, "Music\\Best Album\\unique melody.mp3");
            Some(id)
        }
        _ => None,
    })
    .await;

    wait_client(&mut eve_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            id,
            direction: TransferDirection::Upload,
            update: TransferUpdate::Started { .. },
        }) => {
            assert_eq!(id, upload_id);
            Some(())
        }
        _ => None,
    })
    .await;

    let file_path = wait_client(&mut frank_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Download,
            update: TransferUpdate::Finished { file_path, .. },
            ..
        }) => Some(file_path.expect("finished download carries its placed path")),
        ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Download,
            update: TransferUpdate::Failed { reason },
            ..
        }) => {
            panic!("download failed: {reason}")
        }
        _ => None,
    })
    .await;
    assert_eq!(std::fs::read(&file_path).unwrap(), payload);

    wait_client(&mut eve_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            id,
            direction: TransferDirection::Upload,
            update: TransferUpdate::Finished { size, .. },
        }) => {
            assert_eq!(id, upload_id);
            assert_eq!(size, payload.len() as u64);
            Some(())
        }
        ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Upload,
            update: TransferUpdate::Failed { reason },
            ..
        }) => {
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

    let mut grace_config = client_config(
        server_addr,
        "grace",
        free_port(),
        temp_dir("grace-downloads"),
    );
    grace_config.runtime.shared_folders = vec![SharedFolder {
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

    let mut heidi_config = client_config(
        server_addr,
        "heidi",
        free_port(),
        temp_dir("heidi-downloads"),
    );
    heidi_config.wishlist = vec!["wishlist gem".into()];
    let (_heidi, mut heidi_events) = Client::spawn(heidi_config);

    let (username, results) = wait_client(&mut heidi_events, |event| match event {
        ClientEvent::SearchResults(result) => Some((result.username, result.results)),
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

    let mut dave_config = client_config(server_addr, "dave", free_port(), temp_dir("downloads2"));
    dave_config.runtime.incomplete_dir = temp_dir("incomplete2");
    let (dave, mut dave_events) = Client::spawn(dave_config);
    wait_client(&mut dave_events, |event| match event {
        ClientEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    let token = dave.search("test query", SearchScope::Global).await;
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
    responder.peer(
        "dave",
        PeerMessage::FileSearchResponse {
            username: "carol".into(),
            token,
            results: results.clone(),
            free_upload_slots: true,
            upload_speed: 500000,
            queue_size: 0,
            unknown: 0,
            private_results: Vec::new(),
        },
    );

    let (username, received) = wait_client(&mut dave_events, |event| match event {
        ClientEvent::SearchResults(result) if result.token == token => {
            Some((result.username, result.results))
        }
        _ => None,
    })
    .await;
    assert_eq!(username, "carol");
    assert_eq!(received, results);
}

#[tokio::test]
async fn restrictions_gate_uploads_and_actions_are_observed() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir("restricted-music");
    let album_dir = share_dir.join("Best Album");
    std::fs::create_dir_all(&album_dir).unwrap();
    let payload = b"restricted payload".repeat(500);
    std::fs::write(album_dir.join("guarded song.mp3"), &payload).unwrap();

    let mut eve_config = client_config(server_addr, "eve", free_port(), temp_dir("eve-dl"));
    eve_config.runtime.shared_folders = vec![SharedFolder {
        virtual_name: "Music".into(),
        path: share_dir,
        buddy_only: false,
    }];
    let (eve, mut eve_events) = Client::spawn(eve_config);
    wait_client(&mut eve_events, |event| match event {
        ClientEvent::SharesScanned { .. } => Some(()),
        _ => None,
    })
    .await;

    let frank_config = client_config(server_addr, "frank", free_port(), temp_dir("frank-dl"));
    let (frank, mut frank_events) = Client::spawn(frank_config);
    wait_client(&mut frank_events, |event| match event {
        ClientEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;

    frank.browse_user("eve").await;
    let ip = wait_client(&mut eve_events, |event| match event {
        ClientEvent::Observed(Observation::PeerConnected {
            username,
            ip,
            conn_type: ConnectionType::Peer,
        }) if username == "frank" => Some(ip),
        _ => None,
    })
    .await;
    assert_eq!(ip, Some(std::net::Ipv4Addr::LOCALHOST));
    wait_client(&mut eve_events, |event| match event {
        ClientEvent::Observed(Observation::BrowseRequest { username }) if username == "frank" => {
            Some(())
        }
        _ => None,
    })
    .await;
    wait_client(&mut frank_events, |event| match event {
        ClientEvent::SharedFileList { username, .. } if username == "eve" => Some(()),
        _ => None,
    })
    .await;

    let token = frank.search("guarded song", SearchScope::Global).await;
    let (matched, query) = wait_client(&mut eve_events, |event| match event {
        ClientEvent::Observed(Observation::SearchSeen {
            username,
            query,
            matched,
        }) if username == "frank" => Some((matched, query)),
        _ => None,
    })
    .await;
    assert!(matched);
    assert_eq!(query, "guarded song");
    let results = wait_client(&mut frank_events, |event| match event {
        ClientEvent::SearchResults(result) if result.token == token => Some(result.results),
        _ => None,
    })
    .await;
    let virtual_path = results[0].name.clone();
    let size = results[0].size;

    eve.set_user_restriction(
        "frank",
        Restriction::Denied {
            reason: "not welcome".into(),
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        frank
            .download("eve", &virtual_path, size, FileAttributes::default())
            .await,
        EnqueueResult::Enqueued
    );

    wait_client(&mut eve_events, |event| match event {
        ClientEvent::Observed(Observation::QueueRequest {
            username, accepted, ..
        }) if username == "frank" => {
            assert!(!accepted);
            Some(())
        }
        _ => None,
    })
    .await;
    wait_client(&mut frank_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Download,
            update: TransferUpdate::Failed { reason },
            ..
        }) => {
            assert_eq!(reason, "not welcome");
            Some(())
        }
        _ => None,
    })
    .await;

    eve.set_user_restriction("frank", Restriction::Hold).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        frank
            .download("eve", &virtual_path, size, FileAttributes::default())
            .await,
        EnqueueResult::Enqueued
    );

    wait_client(&mut eve_events, |event| match event {
        ClientEvent::Observed(Observation::QueueRequest {
            username, accepted, ..
        }) if username == "frank" => {
            assert!(accepted);
            Some(())
        }
        _ => None,
    })
    .await;
    wait_client(&mut eve_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Upload,
            update: TransferUpdate::Queued { username, .. },
            ..
        }) if username == "frank" => Some(()),
        _ => None,
    })
    .await;

    tokio::time::sleep(Duration::from_millis(800)).await;
    while let Ok(event) = eve_events.try_recv() {
        if let ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Upload,
            update: TransferUpdate::Started { .. },
            ..
        }) = event
        {
            panic!("held upload was granted a slot");
        }
    }

    eve.set_user_restriction("frank", Restriction::None).await;
    let file_path = wait_client(&mut frank_events, |event| match event {
        ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Download,
            update: TransferUpdate::Finished { file_path, .. },
            ..
        }) => Some(file_path.expect("finished download carries its placed path")),
        ClientEvent::Transfer(TransferEvent {
            direction: TransferDirection::Download,
            update: TransferUpdate::Failed { reason },
            ..
        }) => {
            panic!("download failed: {reason}")
        }
        _ => None,
    })
    .await;
    assert_eq!(std::fs::read(&file_path).unwrap(), payload);
}
