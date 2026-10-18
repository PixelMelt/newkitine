mod common;

use std::io::{Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

use common::{client_config, free_port, start_fake_server, temp_dir, tempfile};
use newkitine::client::{
    Client, ClientBootstrap, ClientEvent, EnqueueResult, Observation, RetryResult, SearchScope,
    TransferWork,
};
use newkitine::network::{NetworkCommand, NetworkEvent, NetworkHandle, spawn as spawn_network};
use newkitine::protocol::PeerMessage;
use newkitine::types::{
    ConnectionType, DescriptionTemplate, FileAttributes, FileInfo, FolderContents, Restriction,
    SharedFolder, TransferDirection, TransferId, TransferSnapshot, TransferStatus,
};

async fn wait<E, T>(events: &mut Receiver<E>, mut matcher: impl FnMut(E) -> Option<T>) -> T {
    timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("event channel closed");
            if let Some(value) = matcher(event) {
                return value;
            }
        }
    })
    .await
    .expect("timed out waiting for an event")
}

async fn seen<E>(events: &mut Receiver<E>, mut wanted: impl FnMut(&E) -> bool) {
    wait(events, |event| wanted(&event).then_some(())).await;
}

async fn logged_in(events: &mut Receiver<ClientEvent>) {
    seen(events, |event| {
        matches!(event, ClientEvent::LoggedIn { .. })
    })
    .await;
}

async fn shares_installed(events: &mut Receiver<ClientEvent>) {
    seen(events, |event| {
        matches!(event, ClientEvent::SharesInstalled { .. })
    })
    .await;
}

async fn shared_list(events: &mut Receiver<ClientEvent>, from: &str) -> Vec<FolderContents> {
    wait(events, |event| match event {
        ClientEvent::SharedFileList {
            username, shares, ..
        } if username == from => Some(shares),
        _ => None,
    })
    .await
}

async fn finished_download(transfers: &mut Receiver<TransferWork>) -> PathBuf {
    wait(transfers, |work| match work {
        TransferWork::Finished { snapshot, .. }
            if snapshot.direction == TransferDirection::Download =>
        {
            let path = snapshot.file_path;
            Some(PathBuf::from(
                path.expect("finished download carries its placed path"),
            ))
        }
        TransferWork::Update(snapshot)
            if snapshot.direction == TransferDirection::Download
                && snapshot.status == TransferStatus::Failed =>
        {
            panic!("download failed: {:?}", snapshot.failure_reason)
        }
        _ => None,
    })
    .await
}

async fn updated(
    transfers: &mut Receiver<TransferWork>,
    direction: TransferDirection,
    status: TransferStatus,
) -> TransferSnapshot {
    wait(transfers, |work| match work {
        TransferWork::Update(snapshot)
            if snapshot.direction == direction && snapshot.status == status =>
        {
            Some(snapshot)
        }
        _ => None,
    })
    .await
}

async fn net_peer(server: SocketAddr, username: &str) -> (NetworkHandle, Receiver<NetworkEvent>) {
    let (peer, mut events) = spawn_network();
    peer.send(NetworkCommand::ServerConnect {
        address: server,
        username: username.into(),
        password: "secret".into(),
        listen_port: free_port(),
    });
    seen(&mut events, |event| {
        matches!(event, NetworkEvent::LoggedIn { .. })
    })
    .await;
    (peer, events)
}

async fn queue_upload_seen(events: &mut Receiver<NetworkEvent>, virtual_path: &str) {
    seen(events, |event| {
        matches!(
            event,
            NetworkEvent::PeerMessage {
                message: PeerMessage::QueueUpload { file, .. },
                ..
            } if file == virtual_path
        )
    })
    .await;
}

async fn browse_request_seen(events: &mut Receiver<ClientEvent>) {
    seen(events, |event| {
        matches!(
            event,
            ClientEvent::Observed(Observation::BrowseRequest { username }) if username == "frank"
        )
    })
    .await;
}

async fn queue_request_accepted(events: &mut Receiver<ClientEvent>) -> bool {
    wait(events, |event| match event {
        ClientEvent::Observed(Observation::QueueRequest {
            username, accepted, ..
        }) if username == "frank" => Some(accepted),
        _ => None,
    })
    .await
}

async fn search_results(events: &mut Receiver<ClientEvent>, token: u32) -> (String, Vec<FileInfo>) {
    wait(events, |event| match event {
        ClientEvent::SearchResults(result) if result.token == token => {
            Some((result.username, result.results))
        }
        _ => None,
    })
    .await
}

async fn login(
    server: SocketAddr,
    username: &str,
) -> (Client, Receiver<ClientEvent>, Receiver<TransferWork>) {
    let (client, mut events, transfers) = Client::spawn(client_config(server, username));
    logged_in(&mut events).await;
    (client, events, transfers)
}

fn sharing_config(server: SocketAddr, username: &str, share_dir: PathBuf) -> ClientBootstrap {
    let mut config = client_config(server, username);
    config.runtime.shared_folders = vec![SharedFolder {
        virtual_name: "Music".into(),
        path: share_dir,
        buddy_only: false,
    }];
    config
}

fn payload() -> Vec<u8> {
    (0u32..50_000)
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[tokio::test]
async fn client_downloads_file_from_peer() {
    let (server_addr, _registry) = start_fake_server().await;

    let (uploader, mut uploader_events) = net_peer(server_addr, "alice").await;

    let bob_config = client_config(server_addr, "bob");
    let download_dir = bob_config.runtime.transfers.download_dir.clone();
    let (bob, mut bob_events, mut bob_transfers) = Client::spawn(bob_config);
    logged_in(&mut bob_events).await;

    let payload = payload();
    let virtual_path = "Music\\Album\\song.mp3";
    assert_eq!(
        bob.download(
            "alice",
            virtual_path,
            payload.len() as u64,
            FileAttributes::default(),
            None,
        )
        .await,
        EnqueueResult::Enqueued
    );

    queue_upload_seen(&mut uploader_events, virtual_path).await;

    uploader.peer(
        "bob",
        PeerMessage::TransferRequest {
            direction: TransferDirection::Upload,
            token: 99,
            file: virtual_path.into(),
            filesize: Some(payload.len() as u64),
        },
    );

    seen(&mut uploader_events, |event| {
        matches!(
            event,
            NetworkEvent::PeerMessage {
                message: PeerMessage::TransferResponse {
                    token: 99,
                    allowed: true,
                    ..
                },
                ..
            }
        )
    })
    .await;

    let mut source = tempfile();
    source.write_all(&payload).unwrap();
    source.seek(SeekFrom::Start(0)).unwrap();

    uploader.send(NetworkCommand::RequestFileConnection {
        username: "bob".into(),
        token: 99,
    });
    let upload_conn = wait(&mut uploader_events, |event| match event {
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

    let snapshot = updated(
        &mut bob_transfers,
        TransferDirection::Download,
        TransferStatus::Queued,
    )
    .await;
    assert_eq!(snapshot.username, "alice");
    let queued_id = snapshot.id;

    let snapshot = updated(
        &mut bob_transfers,
        TransferDirection::Download,
        TransferStatus::Transferring,
    )
    .await;
    assert_eq!(snapshot.id, queued_id);

    let file_path = finished_download(&mut bob_transfers).await;
    assert_eq!(file_path.parent().unwrap(), download_dir);
    assert_eq!(file_path.file_name().unwrap().to_str().unwrap(), "song.mp3");
    assert_eq!(std::fs::read(&file_path).unwrap(), payload);
}

#[tokio::test]
async fn client_shares_answers_search_and_uploads() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir();
    let album_dir = share_dir.join("Best Album");
    std::fs::create_dir_all(&album_dir).unwrap();
    let payload = payload();
    std::fs::write(album_dir.join("unique melody.mp3"), &payload).unwrap();
    std::fs::write(album_dir.join("other tune.mp3"), b"tiny").unwrap();

    let eve_config = sharing_config(server_addr, "eve", share_dir);
    let (_eve, mut eve_events, mut eve_transfers) = Client::spawn(eve_config);
    logged_in(&mut eve_events).await;
    seen(&mut eve_events, |event| {
        matches!(
            event,
            ClientEvent::SharesInstalled {
                folders: 2,
                files: 2,
            }
        )
    })
    .await;

    let frank_config = client_config(server_addr, "frank");
    let download_dir = frank_config.runtime.transfers.download_dir.clone();
    let (frank, mut frank_events, mut frank_transfers) = Client::spawn(frank_config);
    logged_in(&mut frank_events).await;

    let token = frank
        .search("unique melody", SearchScope::Global)
        .await
        .unwrap();
    let (result_username, results) = search_results(&mut frank_events, token).await;
    assert_eq!(result_username, "eve");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "Music\\Best Album\\unique melody.mp3");
    assert_eq!(results[0].size, payload.len() as u64);

    frank.browse_user("eve").await;
    let shares = shared_list(&mut frank_events, "eve").await;
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
                FileAttributes::default(),
                Some("Music"),
            )
            .await,
        EnqueueResult::Enqueued
    );

    let snapshot = updated(
        &mut eve_transfers,
        TransferDirection::Upload,
        TransferStatus::Queued,
    )
    .await;
    assert_eq!(snapshot.username, "frank");
    assert_eq!(
        snapshot.virtual_path,
        "Music\\Best Album\\unique melody.mp3"
    );
    let upload_id = snapshot.id;

    let snapshot = updated(
        &mut eve_transfers,
        TransferDirection::Upload,
        TransferStatus::Transferring,
    )
    .await;
    assert_eq!(snapshot.id, upload_id);

    let file_path = finished_download(&mut frank_transfers).await;
    assert_eq!(
        file_path,
        download_dir.join("Music/Best Album/unique melody.mp3")
    );
    assert_eq!(std::fs::read(&file_path).unwrap(), payload);

    wait(&mut eve_transfers, |work| match work {
        TransferWork::Finished { snapshot, .. }
            if snapshot.direction == TransferDirection::Upload =>
        {
            assert_eq!(snapshot.id, upload_id);
            assert_eq!(snapshot.size, payload.len() as u64);
            Some(())
        }
        TransferWork::Update(snapshot)
            if snapshot.direction == TransferDirection::Upload
                && snapshot.status == TransferStatus::Failed =>
        {
            panic!("upload failed: {:?}", snapshot.failure_reason)
        }
        _ => None,
    })
    .await;
}

#[tokio::test]
async fn wishlist_search_runs_on_interval() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir();
    std::fs::create_dir_all(&share_dir).unwrap();
    std::fs::write(share_dir.join("wishlist gem.mp3"), b"g".repeat(500)).unwrap();

    let mut grace_config = client_config(server_addr, "grace");
    grace_config.runtime.shared_folders = vec![SharedFolder {
        virtual_name: "Stash".into(),
        path: share_dir,
        buddy_only: false,
    }];
    let (_grace, mut grace_events, _grace_transfers) = Client::spawn(grace_config);
    shares_installed(&mut grace_events).await;

    let mut heidi_config = client_config(server_addr, "heidi");
    heidi_config.wishlist = vec!["wishlist gem".into()];
    let (_heidi, mut heidi_events, _heidi_transfers) = Client::spawn(heidi_config);

    let started = wait(&mut heidi_events, |event| match event {
        ClientEvent::SearchStarted { token, query } => Some((token, query)),
        ClientEvent::SearchResults(_) => panic!("wish results before the wish search was shown"),
        _ => None,
    })
    .await;
    assert_eq!(started.1, "wishlist gem");
    let (token, username, results) = wait(&mut heidi_events, |event| match event {
        ClientEvent::SearchResults(result) => Some((result.token, result.username, result.results)),
        _ => None,
    })
    .await;
    assert_eq!(token, started.0);
    assert_eq!(username, "grace");
    assert_eq!(results[0].name, "Stash\\wishlist gem.mp3");

    let rerun = wait(&mut heidi_events, |event| match event {
        ClientEvent::SearchStarted { .. } => panic!("a wish rerun must not start a new search"),
        ClientEvent::SearchResults(result) => Some(result.token),
        _ => None,
    })
    .await;
    assert_eq!(rerun, started.0);
}

#[tokio::test]
async fn search_results_are_filtered_against_the_query() {
    let (server_addr, _registry) = start_fake_server().await;

    let (responder, _responder_events) = net_peer(server_addr, "ivan").await;

    let (judy, mut judy_events, _judy_transfers) = login(server_addr, "judy").await;

    let token = judy
        .search("\"night drive\" -live", SearchScope::Global)
        .await
        .unwrap();
    assert_eq!(judy.search(" \t ", SearchScope::Global).await, None);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let file = |name: &str| FileInfo {
        name: name.into(),
        size: 1,
        attributes: FileAttributes::default(),
    };
    let respond = |results: Vec<FileInfo>| {
        responder.peer(
            "judy",
            PeerMessage::FileSearchResponse {
                username: "ivan".into(),
                token,
                results,
                free_upload_slots: true,
                upload_speed: 1,
                queue_size: 0,
                unknown: 0,
                private_results: Vec::new(),
            },
        );
    };
    respond(vec![file("Music\\unrelated.mp3")]);
    respond(vec![
        file("Music\\Night Drive.mp3"),
        file("Music\\Night Drive (Live).mp3"),
        file("Music\\Night - Drive.mp3"),
    ]);

    let (_, received) = search_results(&mut judy_events, token).await;
    assert_eq!(received, vec![file("Music\\Night Drive.mp3")]);
}

#[tokio::test]
async fn client_receives_search_results() {
    let (server_addr, _registry) = start_fake_server().await;

    let (responder, _responder_events) = net_peer(server_addr, "carol").await;

    let (dave, mut dave_events, _dave_transfers) = login(server_addr, "dave").await;

    let token = dave
        .search("test query", SearchScope::Global)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let results = vec![FileInfo {
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

    let (username, received) = search_results(&mut dave_events, token).await;
    assert_eq!(username, "carol");
    assert_eq!(received, results);
}

#[tokio::test]
async fn restrictions_gate_uploads_and_actions_are_observed() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir();
    let album_dir = share_dir.join("Best Album");
    std::fs::create_dir_all(&album_dir).unwrap();
    let payload = b"restricted payload".repeat(500);
    std::fs::write(album_dir.join("guarded song.mp3"), &payload).unwrap();

    let eve_config = sharing_config(server_addr, "eve", share_dir);
    let (eve, mut eve_events, mut eve_transfers) = Client::spawn(eve_config);
    shares_installed(&mut eve_events).await;

    let (frank, mut frank_events, mut frank_transfers) = login(server_addr, "frank").await;

    frank.browse_user("eve").await;
    let ip = wait(&mut eve_events, |event| match event {
        ClientEvent::Observed(Observation::PeerConnected {
            username,
            ip,
            conn_type: ConnectionType::Peer,
        }) if username == "frank" => Some(ip),
        _ => None,
    })
    .await;
    assert_eq!(ip, Some(std::net::Ipv4Addr::LOCALHOST));
    browse_request_seen(&mut eve_events).await;
    shared_list(&mut frank_events, "eve").await;

    let token = frank
        .search("guarded song", SearchScope::Global)
        .await
        .unwrap();
    let (matched, query) = wait(&mut eve_events, |event| match event {
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
    let (_, results) = search_results(&mut frank_events, token).await;
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
            .download("eve", &virtual_path, size, FileAttributes::default(), None)
            .await,
        EnqueueResult::Enqueued
    );

    assert!(!queue_request_accepted(&mut eve_events).await);
    let snapshot = updated(
        &mut frank_transfers,
        TransferDirection::Download,
        TransferStatus::Failed,
    )
    .await;
    assert_eq!(snapshot.failure_reason.as_deref(), Some("not welcome"));

    frank
        .search("guarded song", SearchScope::Global)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(event) = eve_events.try_recv() {
        if let ClientEvent::Observed(Observation::SearchSeen { username, .. }) = &event {
            assert_ne!(
                username, "frank",
                "a denied peer's search must not be recorded as an observation"
            );
        }
    }

    eve.set_user_restriction("frank", Restriction::Hold).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        frank
            .download("eve", &virtual_path, size, FileAttributes::default(), None)
            .await,
        EnqueueResult::Enqueued
    );

    assert!(queue_request_accepted(&mut eve_events).await);
    let snapshot = updated(
        &mut eve_transfers,
        TransferDirection::Upload,
        TransferStatus::Queued,
    )
    .await;
    assert_eq!(snapshot.username, "frank");

    tokio::time::sleep(Duration::from_millis(800)).await;
    while let Ok(work) = eve_transfers.try_recv() {
        if let TransferWork::Update(snapshot) = &work
            && snapshot.direction == TransferDirection::Upload
            && snapshot.status == TransferStatus::Transferring
        {
            panic!("held upload was granted a slot");
        }
    }

    eve.set_user_restriction("frank", Restriction::None).await;
    let file_path = finished_download(&mut frank_transfers).await;
    assert_eq!(std::fs::read(&file_path).unwrap(), payload);
}

#[tokio::test]
async fn description_template_renders_for_the_asking_user() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir();
    std::fs::create_dir_all(share_dir.join("Album")).unwrap();
    std::fs::write(share_dir.join("Album").join("song.mp3"), b"payload").unwrap();

    let mut gina_config = sharing_config(server_addr, "gina", share_dir);
    gina_config.runtime.description = DescriptionTemplate::parse(
        "${user.is_buddy?Hey buddy:Hi} ${user.name}! ${me.name} shares ${me.shared_files} \
         file in ${me.shared_folders} folders, ${me.free_slots}/${me.slots} slots free. 100$$",
    )
    .unwrap();
    let (gina, mut gina_events, _gina_transfers) = Client::spawn(gina_config);
    shares_installed(&mut gina_events).await;

    let (hank, mut hank_events, _hank_transfers) = login(server_addr, "hank").await;

    async fn description(events: &mut Receiver<ClientEvent>) -> String {
        wait(events, |event| match event {
            ClientEvent::UserInfo(info) if info.username == "gina" => Some(info.description),
            _ => None,
        })
        .await
    }

    hank.request_user_info("gina").await;
    assert_eq!(
        description(&mut hank_events).await,
        "Hi hank! gina shares 1 file in 2 folders, 2/2 slots free. 100$"
    );

    gina.add_buddy("hank").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    hank.request_user_info("gina").await;
    assert_eq!(
        description(&mut hank_events).await,
        "Hey buddy hank! gina shares 1 file in 2 folders, 2/2 slots free. 100$"
    );
}

#[tokio::test]
async fn retry_download_is_resolved_by_the_client_actor() {
    let (server_addr, _registry) = start_fake_server().await;

    let (uploader, mut uploader_events) = net_peer(server_addr, "alice").await;

    let (bob, _bob_events, mut bob_transfers) = login(server_addr, "bob").await;

    assert_eq!(
        bob.retry_download(TransferId(4242)).await,
        RetryResult::NotFound
    );

    let virtual_path = "Music\\Album\\song.mp3";
    assert_eq!(
        bob.download("alice", virtual_path, 64, FileAttributes::default(), None)
            .await,
        EnqueueResult::Enqueued
    );
    let id = updated(
        &mut bob_transfers,
        TransferDirection::Download,
        TransferStatus::Queued,
    )
    .await
    .id;
    assert_eq!(bob.retry_download(id).await, RetryResult::AlreadyActive);

    queue_upload_seen(&mut uploader_events, virtual_path).await;
    uploader.peer(
        "bob",
        PeerMessage::UploadDenied {
            file: virtual_path.into(),
            reason: "Cancelled".into(),
        },
    );
    let snapshot = updated(
        &mut bob_transfers,
        TransferDirection::Download,
        TransferStatus::Failed,
    )
    .await;
    assert_eq!(snapshot.id, id);

    assert_eq!(bob.retry_download(id).await, RetryResult::Requeued);
    let snapshot = updated(
        &mut bob_transfers,
        TransferDirection::Download,
        TransferStatus::Queued,
    )
    .await;
    assert_eq!(snapshot.id, id);
    queue_upload_seen(&mut uploader_events, virtual_path).await;
}

#[tokio::test]
async fn shares_are_served_from_the_catalog_cache_before_any_scan() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir();
    let payload = payload();
    let album_dir = share_dir.join("Album");
    std::fs::create_dir_all(&album_dir).unwrap();
    std::fs::write(album_dir.join("song.mp3"), &payload).unwrap();

    let mut eve_config = sharing_config(server_addr, "eve", share_dir);
    let (eve, mut eve_events, _eve_transfers) = Client::spawn(eve_config.clone());
    seen(&mut eve_events, |event| {
        matches!(event, ClientEvent::ShareScanFinished)
    })
    .await;
    drop(eve);
    drop(eve_events);

    eve_config.scan_on_startup = false;
    eve_config.runtime.login.listen_port = free_port();
    let (_eve, mut eve_events, _eve_transfers) = Client::spawn(eve_config);
    wait(&mut eve_events, |event| match event {
        ClientEvent::SharesInstalled {
            folders: 2,
            files: 1,
        } => Some(()),
        ClientEvent::ShareScanProgress { .. } => panic!("cached shares must not walk the disk"),
        _ => None,
    })
    .await;
    logged_in(&mut eve_events).await;

    let (frank, _frank_events, mut frank_transfers) = login(server_addr, "frank").await;

    assert_eq!(
        frank
            .download(
                "eve",
                "Music\\Album\\song.mp3",
                payload.len() as u64,
                FileAttributes::default(),
                None,
            )
            .await,
        EnqueueResult::Enqueued
    );

    let file_path = finished_download(&mut frank_transfers).await;
    assert_eq!(std::fs::read(&file_path).unwrap(), payload);
}

#[tokio::test]
async fn banned_peer_browse_gets_an_empty_share_list() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir();
    let album_dir = share_dir.join("Album");
    std::fs::create_dir_all(&album_dir).unwrap();
    std::fs::write(album_dir.join("song.mp3"), b"payload".repeat(100)).unwrap();

    let eve_config = sharing_config(server_addr, "eve", share_dir);
    let (eve, mut eve_events, _eve_transfers) = Client::spawn(eve_config);
    shares_installed(&mut eve_events).await;

    let (frank, mut frank_events, _frank_transfers) = login(server_addr, "frank").await;

    eve.ban_user("frank").await;
    frank.browse_user("eve").await;
    let shares = shared_list(&mut frank_events, "eve").await;
    assert!(shares.is_empty());

    eve.unban_user("frank").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    frank.browse_user("eve").await;
    let shares = shared_list(&mut frank_events, "eve").await;
    assert_eq!(shares.len(), 2);
}

#[tokio::test]
async fn browsing_an_offline_peer_reports_the_failure() {
    let (server_addr, _registry) = start_fake_server().await;
    let (grace, mut grace_events, _grace_transfers) = login(server_addr, "grace").await;

    grace.browse_user("nobody").await;
    seen(
        &mut grace_events,
        |event| matches!(event, ClientEvent::BrowseFailed { username } if username == "nobody"),
    )
    .await;
}

#[tokio::test]
async fn connect_while_connected_is_ignored() {
    let (server_addr, _registry) = start_fake_server().await;
    let (carol, mut carol_events, _carol_transfers) = login(server_addr, "carol").await;

    carol.connect().await;
    carol.reconnect().await;

    let first = wait(&mut carol_events, |event| match event {
        ClientEvent::Connecting | ClientEvent::Disconnected => Some(event),
        _ => None,
    })
    .await;
    assert!(matches!(first, ClientEvent::Disconnected));
    logged_in(&mut carol_events).await;
}

#[tokio::test]
async fn a_requested_connect_waits_for_the_share_cache_and_then_logs_in() {
    let (server_addr, _registry) = start_fake_server().await;
    let mut config = client_config(server_addr, "ivan");
    config.connect_on_startup = false;
    let (ivan, mut ivan_events, _ivan_transfers) = Client::spawn(config);
    ivan.connect().await;
    logged_in(&mut ivan_events).await;
}

#[tokio::test]
async fn removing_every_share_installs_an_empty_index_without_a_cache() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir();
    std::fs::write(share_dir.join("song.mp3"), b"payload".repeat(100)).unwrap();
    let eve_config = sharing_config(server_addr, "eve", share_dir);
    let (eve, mut eve_events, _eve_transfers) = Client::spawn(eve_config.clone());
    seen(&mut eve_events, |event| {
        matches!(event, ClientEvent::SharesInstalled { files: 1, .. })
    })
    .await;
    seen(&mut eve_events, |event| {
        matches!(event, ClientEvent::ShareScanFinished)
    })
    .await;

    std::fs::remove_file(&eve_config.share_catalog).unwrap();
    let mut runtime = eve_config.runtime.clone();
    runtime.shared_folders = Vec::new();
    eve.apply_config(runtime).await;
    wait(&mut eve_events, |event| match event {
        ClientEvent::SharesInstalled {
            folders: 0,
            files: 0,
        } => Some(()),
        ClientEvent::SharesInstalled { folders, files } => {
            panic!("expected an empty index, got {folders}/{files}")
        }
        _ => None,
    })
    .await;
}

#[tokio::test]
async fn dropped_browse_retries_do_not_extend_the_coalesce_window() {
    let (server_addr, _registry) = start_fake_server().await;

    let share_dir = temp_dir();
    std::fs::write(share_dir.join("song.mp3"), b"payload".repeat(100)).unwrap();
    let eve_config = sharing_config(server_addr, "eve", share_dir);
    let (_eve, mut eve_events, _eve_transfers) = Client::spawn(eve_config);
    shares_installed(&mut eve_events).await;

    let (frank, mut frank_events, _frank_transfers) = login(server_addr, "frank").await;

    frank.browse_user("eve").await;
    shared_list(&mut frank_events, "eve").await;
    let started = std::time::Instant::now();
    loop {
        frank.browse_user("eve").await;
        browse_request_seen(&mut eve_events).await;
        if started.elapsed() >= Duration::from_millis(900) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    shared_list(&mut frank_events, "eve").await;
}
