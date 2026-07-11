mod common;

use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::timeout;

use common::{free_port, start_fake_server, tempfile};
use newkitine::network::{NetworkCommand, NetworkEvent, spawn};
use newkitine::protocol::PeerMessage;
use newkitine::types::ConnectionType;

async fn wait_for<T>(
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
    .expect("timed out waiting for event")
}

struct Stack {
    handle: newkitine::network::NetworkHandle,
    events: UnboundedReceiver<NetworkEvent>,
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
        PeerMessage::QueueUpload { file: "Music\\song.mp3".into(), legacy_client: false },
    );
    let (from_user, received) = wait_for(&mut bob.events, |event| match event {
        NetworkEvent::PeerMessage { username, message, .. } => Some((username, message)),
        _ => None,
    })
    .await;
    assert_eq!(from_user, "alice");
    assert_eq!(
        received,
        PeerMessage::QueueUpload { file: "Music\\song.mp3".into(), legacy_client: false }
    );

    let payload: Vec<u8> = (0u32..100_000).flat_map(|value| value.to_le_bytes()).collect();
    let mut source = tempfile();
    source.write_all(&payload).unwrap();
    source.seek(SeekFrom::Start(0)).unwrap();
    let dest = tempfile();
    let dest_read = dest.try_clone().unwrap();

    bob.handle.send(NetworkCommand::RequestFileConnection { username: "alice".into(), token: 42 });

    let download_conn = wait_for(&mut alice.events, |event| match event {
        NetworkEvent::FileTransferInit { username, token, conn_id } => {
            assert_eq!(username, "bob");
            assert_eq!(token, 42);
            Some(conn_id)
        }
        _ => None,
    })
    .await;

    let upload_conn = wait_for(&mut bob.events, |event| match event {
        NetworkEvent::PeerConnected { conn_type: ConnectionType::File, conn_id, .. } => {
            Some(conn_id)
        }
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
        PeerMessage::PlaceInQueueRequest { file: "Music\\song.mp3".into(), legacy_client: false },
    );

    let (from_user, received) = wait_for(&mut bob.events, |event| match event {
        NetworkEvent::PeerMessage { username, message, .. } => Some((username, message)),
        _ => None,
    })
    .await;
    assert_eq!(from_user, "alice");
    assert_eq!(
        received,
        PeerMessage::PlaceInQueueRequest { file: "Music\\song.mp3".into(), legacy_client: false }
    );

    wait_for(&mut alice.events, |event| match event {
        NetworkEvent::PeerConnected { username, conn_type: ConnectionType::Peer, .. }
            if username == "bob" =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
}
