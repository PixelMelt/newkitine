mod common;

use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

use common::{free_port, start_fake_server, tempfile};
use newkitine::network::spawn;
use newkitine::network::{NetworkCommand, NetworkEvent};
use newkitine::protocol::{PeerInitMessage, PeerMessage};
use newkitine::types::{ConnectionType, FileAttributes, FileInfo};

async fn wait_for<T>(
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
    .expect("timed out waiting for event")
}

struct Stack {
    handle: newkitine::network::NetworkHandle,
    events: Receiver<NetworkEvent>,
}

async fn connect_stack(server_addr: SocketAddr, username: &str) -> Stack {
    let (handle, mut events) = spawn();
    handle.send(NetworkCommand::ServerConnect {
        address: server_addr,
        username: username.into(),
        password: "secret".into(),
        listen_port: free_port(),
    });
    wait_for(&mut events, |event| match event {
        NetworkEvent::LoggedIn { .. } => Some(()),
        _ => None,
    })
    .await;
    Stack { handle, events }
}

#[tokio::test]
async fn login_peer_message_and_file_transfer() {
    let (server_addr, _registry) = start_fake_server().await;
    let mut alice = connect_stack(server_addr, "alice").await;
    let mut bob = connect_stack(server_addr, "bob").await;

    alice.handle.peer(
        "bob",
        PeerMessage::QueueUpload {
            file: "Music\\song.mp3".into(),
            legacy_client: false,
        },
    );
    let (from_user, received) = wait_for(&mut bob.events, |event| match event {
        NetworkEvent::PeerMessage {
            username, message, ..
        } => Some((username, message)),
        _ => None,
    })
    .await;
    assert_eq!(from_user, "alice");
    assert_eq!(
        received,
        PeerMessage::QueueUpload {
            file: "Music\\song.mp3".into(),
            legacy_client: false
        }
    );

    let payload: Vec<u8> = (0u32..100_000)
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let mut source = tempfile();
    source.write_all(&payload).unwrap();
    source.seek(SeekFrom::Start(0)).unwrap();
    let dest = tempfile();
    let dest_read = dest.try_clone().unwrap();

    bob.handle.send(NetworkCommand::RequestFileConnection {
        username: "alice".into(),
        token: 42,
    });

    let download_conn = wait_for(&mut alice.events, |event| match event {
        NetworkEvent::FileTransferInit {
            username,
            token,
            conn_id,
        } => {
            assert_eq!(username, "bob");
            assert_eq!(token, 42);
            Some(conn_id)
        }
        _ => None,
    })
    .await;

    let upload_conn = wait_for(&mut bob.events, |event| match event {
        NetworkEvent::PeerConnected {
            conn_type: ConnectionType::File,
            conn_id,
            ..
        } => Some(conn_id),
        _ => None,
    })
    .await;

    alice.handle.send(NetworkCommand::DownloadFile {
        conn_id: download_conn,
        file: dest,
        offset: 0,
        bytes_left: payload.len() as u64,
    });
    bob.handle.send(NetworkCommand::UploadFile {
        conn_id: upload_conn,
        file: source,
        size: payload.len() as u64,
    });

    wait_for(&mut alice.events, |event| match event {
        NetworkEvent::FileDownloadProgress { bytes_left: 0, .. } => Some(()),
        _ => None,
    })
    .await;
    wait_for(&mut alice.events, |event| match event {
        NetworkEvent::FileConnectionClosed { token, .. } => {
            assert_eq!(token, Some(42));
            Some(())
        }
        _ => None,
    })
    .await;

    let mut written = Vec::new();
    let mut dest_read = dest_read;
    dest_read.seek(SeekFrom::Start(0)).unwrap();
    dest_read.read_to_end(&mut written).unwrap();
    assert_eq!(written, payload);
}

#[tokio::test]
async fn indirect_connection_via_pierce_firewall() {
    let (server_addr, registry) = start_fake_server().await;
    let mut alice = connect_stack(server_addr, "alice").await;
    let mut bob = connect_stack(server_addr, "bob").await;

    registry.lock().await.get_mut("bob").unwrap().hidden = true;

    alice.handle.peer(
        "bob",
        PeerMessage::PlaceInQueueRequest {
            file: "Music\\song.mp3".into(),
            legacy_client: false,
        },
    );

    let (from_user, received) = wait_for(&mut bob.events, |event| match event {
        NetworkEvent::PeerMessage {
            username, message, ..
        } => Some((username, message)),
        _ => None,
    })
    .await;
    assert_eq!(from_user, "alice");
    assert_eq!(
        received,
        PeerMessage::PlaceInQueueRequest {
            file: "Music\\song.mp3".into(),
            legacy_client: false
        }
    );

    wait_for(&mut alice.events, |event| match event {
        NetworkEvent::PeerConnected {
            username,
            conn_type: ConnectionType::Peer,
            ..
        } if username == "bob" => Some(()),
        _ => None,
    })
    .await;
}

async fn raw_peer(registry: &common::Registry, target: &str, from: &str) -> TcpStream {
    let port = registry.lock().await.get(target).unwrap().port;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let init = PeerInitMessage::PeerInit {
        username: from.into(),
        conn_type: ConnectionType::Peer,
    };
    stream.write_all(&init.to_bytes()).await.unwrap();
    stream
}

fn search_response(token: u32) -> PeerMessage {
    PeerMessage::FileSearchResponse {
        username: "mallory".into(),
        token,
        results: vec![FileInfo {
            name: "Music\\song.mp3".into(),
            size: 1,
            attributes: FileAttributes::default(),
        }],
        free_upload_slots: true,
        upload_speed: 1,
        queue_size: 0,
        unknown: 0,
        private_results: Vec::new(),
    }
}

fn queue_upload() -> PeerMessage {
    PeerMessage::QueueUpload {
        file: "Music\\song.mp3".into(),
        legacy_client: false,
    }
}

async fn next_peer_message(events: &mut Receiver<NetworkEvent>) -> PeerMessage {
    wait_for(events, |event| match event {
        NetworkEvent::PeerMessage { message, .. } => Some(message),
        _ => None,
    })
    .await
}

#[tokio::test]
async fn unsolicited_compressed_responses_are_dropped() {
    let (server_addr, registry) = start_fake_server().await;
    let mut alice = connect_stack(server_addr, "alice").await;
    let mut stream = raw_peer(&registry, "alice", "mallory").await;

    let folder = PeerMessage::FolderContentsResponse {
        token: 1,
        directory: "Music".into(),
        folders: Vec::new(),
    };
    let mut bytes = search_response(99).to_bytes();
    bytes.extend(folder.to_bytes());
    bytes.extend(queue_upload().to_bytes());
    stream.write_all(&bytes).await.unwrap();

    assert_eq!(next_peer_message(&mut alice.events).await, queue_upload());
}

#[tokio::test]
async fn search_result_connection_stays_open_while_data_is_pending() {
    let (server_addr, registry) = start_fake_server().await;
    let mut alice = connect_stack(server_addr, "alice").await;
    alice.handle.send(NetworkCommand::AllowSearchToken(7));
    let mut stream = raw_peer(&registry, "alice", "mallory").await;

    let follow_up = queue_upload().to_bytes();
    let (head, tail) = follow_up.split_at(6);
    let mut bytes = search_response(7).to_bytes();
    bytes.extend_from_slice(head);
    stream.write_all(&bytes).await.unwrap();

    assert_eq!(next_peer_message(&mut alice.events).await, search_response(7));
    tokio::time::sleep(Duration::from_millis(300)).await;
    stream.write_all(tail).await.unwrap();
    assert_eq!(next_peer_message(&mut alice.events).await, queue_upload());
}

#[tokio::test]
async fn idle_search_result_connection_is_closed() {
    let (server_addr, registry) = start_fake_server().await;
    let mut alice = connect_stack(server_addr, "alice").await;
    alice.handle.send(NetworkCommand::AllowSearchToken(7));
    let mut stream = raw_peer(&registry, "alice", "mallory").await;

    stream
        .write_all(&search_response(7).to_bytes())
        .await
        .unwrap();
    assert_eq!(next_peer_message(&mut alice.events).await, search_response(7));

    let mut buffer = [0u8; 16];
    let read = timeout(Duration::from_secs(5), stream.read(&mut buffer))
        .await
        .expect("search result connection was not closed");
    assert_eq!(read.unwrap_or(0), 0);
}

#[tokio::test]
async fn close_interrupts_a_write_blocked_on_a_peer_that_stopped_reading() {
    let (server_addr, registry) = start_fake_server().await;
    let mut alice = connect_stack(server_addr, "alice").await;
    let _stream = raw_peer(&registry, "alice", "mallory").await;

    let conn_id = wait_for(&mut alice.events, |event| match event {
        NetworkEvent::PeerConnected {
            username, conn_id, ..
        } if username == "mallory" => Some(conn_id),
        _ => None,
    })
    .await;

    for _ in 0..8 {
        alice.handle.peer_frame("mallory", vec![0u8; 4 * 1024 * 1024]);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    alice.handle.send(NetworkCommand::CloseConnection(conn_id));

    wait_for(&mut alice.events, |event| match event {
        NetworkEvent::ConnectionCount(0) => Some(()),
        _ => None,
    })
    .await;
}
