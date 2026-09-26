mod common;

use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

use common::{frame, free_port, start_fake_server, tempfile};
use newkitine::network::spawn;
use newkitine::network::{NetworkCommand, NetworkEvent};
use newkitine::protocol::{
    DistributedMessage, MessageWriter, PeerInitMessage, PeerMessage, ServerResponse,
};
use newkitine::types::{ConnectionType, FileAttributes, FileInfo, TransferDirection};

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
            direction,
        } => {
            assert_eq!(username, "bob");
            assert_eq!(token, 42);
            assert_eq!(direction, TransferDirection::Download);
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
        NetworkEvent::FileConnectionClosed {
            token, direction, ..
        } => {
            assert_eq!(token, 42);
            assert_eq!(direction, TransferDirection::Download);
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

struct ScriptedServer {
    stream: TcpStream,
}

impl ScriptedServer {
    async fn expect(&mut self, wanted: u32) -> Vec<u8> {
        timeout(Duration::from_secs(10), async {
            loop {
                let size = self.stream.read_u32_le().await.unwrap() as usize;
                let code = self.stream.read_u32_le().await.unwrap();
                let mut payload = vec![0u8; size - 4];
                self.stream.read_exact(&mut payload).await.unwrap();
                if code == wanted {
                    return payload;
                }
            }
        })
        .await
        .expect("timed out waiting for server request")
    }

    async fn send(&mut self, code: u32, payload: MessageWriter) {
        self.stream
            .write_all(&frame(code, payload.into_bytes()))
            .await
            .unwrap();
    }

    async fn expect_connect_to_peer(&mut self) -> u32 {
        let payload = self.expect(18).await;
        u32::from_le_bytes(payload[..4].try_into().unwrap())
    }

    async fn send_peer_address(&mut self, user: &str, port: u32) {
        let mut w = MessageWriter::new();
        w.write_string(user);
        w.write_ip(Ipv4Addr::LOCALHOST);
        w.write_u32(port);
        w.write_u32(0);
        w.write_u32(0);
        self.send(3, w).await;
    }

    async fn send_connect_to_peer(&mut self, user: &str, port: u32, token: u32) {
        let mut w = MessageWriter::new();
        w.write_string(user);
        w.write_string("P");
        w.write_ip(Ipv4Addr::LOCALHOST);
        w.write_u32(port);
        w.write_u32(token);
        w.write_bool(false);
        w.write_u32(0);
        w.write_u32(0);
        self.send(18, w).await;
    }

    async fn send_cant_connect(&mut self, token: u32) {
        let mut w = MessageWriter::new();
        w.write_u32(token);
        self.send(1001, w).await;
    }
}

struct ScriptedStack {
    handle: newkitine::network::NetworkHandle,
    events: Receiver<NetworkEvent>,
    server: ScriptedServer,
    listen_port: u16,
}

fn login_success() -> MessageWriter {
    let mut w = MessageWriter::new();
    w.write_bool(true);
    w.write_string("Welcome to the scripted server");
    w.write_ip(Ipv4Addr::LOCALHOST);
    w.write_string("checksum");
    w.write_bool(false);
    w
}

async fn scripted_stack(username: &str, preamble: Option<(u32, MessageWriter)>) -> ScriptedStack {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (handle, mut events) = spawn();
    let listen_port = free_port();
    handle.send(NetworkCommand::ServerConnect {
        address: listener.local_addr().unwrap(),
        username: username.into(),
        password: "secret".into(),
        listen_port,
    });
    let (stream, _) = timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut server = ScriptedServer { stream };
    server.expect(1).await;
    if let Some((code, payload)) = preamble {
        server.send(code, payload).await;
    }
    server.send(1, login_success()).await;
    wait_for(&mut events, |event| match event {
        NetworkEvent::LoggedIn { .. } => Some(()),
        NetworkEvent::ServerDisconnected { .. } => panic!("server connection dropped"),
        _ => None,
    })
    .await;
    ScriptedStack {
        handle,
        events,
        server,
        listen_port,
    }
}

async fn read_peer_init(stream: &mut TcpStream) -> PeerInitMessage {
    let size = stream.read_u32_le().await.unwrap() as usize;
    let code = stream.read_u8().await.unwrap();
    let mut payload = vec![0u8; size - 1];
    stream.read_exact(&mut payload).await.unwrap();
    PeerInitMessage::parse(code, &payload).unwrap()
}

async fn read_peer_message(stream: &mut TcpStream) -> PeerMessage {
    timeout(Duration::from_secs(10), async {
        let size = stream.read_u32_le().await.unwrap() as usize;
        let code = stream.read_u32_le().await.unwrap();
        let mut payload = vec![0u8; size - 4];
        stream.read_exact(&mut payload).await.unwrap();
        PeerMessage::parse(code, &payload).unwrap()
    })
    .await
    .expect("timed out waiting for peer message")
}

async fn write_peer_init(stream: &mut TcpStream, init: PeerInitMessage) {
    stream.write_all(&init.to_bytes()).await.unwrap();
}

async fn accept(listener: &TcpListener) -> TcpStream {
    timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("timed out waiting for peer connection")
        .unwrap()
        .0
}

async fn assert_no_connection(listener: &TcpListener) {
    assert!(
        timeout(Duration::from_secs(1), listener.accept())
            .await
            .is_err(),
        "unexpected connection attempt"
    );
}

async fn assert_closed(stream: &mut TcpStream) {
    let mut buf = [0u8; 64];
    timeout(Duration::from_secs(10), async {
        while let Ok(1..) = stream.read(&mut buf).await {}
    })
    .await
    .expect("connection was not closed");
}

fn queue_request(file: &str) -> PeerMessage {
    PeerMessage::PlaceInQueueRequest {
        file: file.into(),
        legacy_client: false,
    }
}

#[tokio::test]
async fn malformed_server_message_is_skipped() {
    let mut malformed = MessageWriter::new();
    malformed.write_raw(&[1]);
    scripted_stack("alice", Some((1001, malformed))).await;
}

#[tokio::test]
async fn cant_connect_keeps_established_direct_connection() {
    let mut alice = scripted_stack("alice", None).await;
    let bob = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bob_port = bob.local_addr().unwrap().port();

    alice.handle.peer("bob", queue_request("a.mp3"));
    let token = alice.server.expect_connect_to_peer().await;
    alice.server.expect(3).await;
    alice.server.send_peer_address("bob", bob_port as u32).await;

    let mut conn = accept(&bob).await;
    assert_eq!(
        read_peer_init(&mut conn).await,
        PeerInitMessage::PeerInit {
            username: "alice".into(),
            conn_type: ConnectionType::Peer
        }
    );
    assert_eq!(read_peer_message(&mut conn).await, queue_request("a.mp3"));

    alice.server.send_cant_connect(token).await;
    wait_for(&mut alice.events, |event| match event {
        NetworkEvent::ServerMessage(ServerResponse::CantConnectToPeer { .. }) => Some(()),
        NetworkEvent::PeerConnectionError { .. } => panic!("live connection reported as failed"),
        _ => None,
    })
    .await;
    alice.handle.peer("bob", queue_request("b.mp3"));
    assert_eq!(read_peer_message(&mut conn).await, queue_request("b.mp3"));
    assert_no_connection(&bob).await;
}

#[tokio::test]
async fn late_peer_address_after_pierce_does_not_dial() {
    let mut alice = scripted_stack("alice", None).await;
    let bob = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bob_port = bob.local_addr().unwrap().port();

    alice.handle.peer("bob", queue_request("a.mp3"));
    let token = alice.server.expect_connect_to_peer().await;
    alice.server.expect(3).await;

    let mut pierced = TcpStream::connect(("127.0.0.1", alice.listen_port))
        .await
        .unwrap();
    write_peer_init(&mut pierced, PeerInitMessage::PierceFireWall { token }).await;
    assert_eq!(
        read_peer_message(&mut pierced).await,
        queue_request("a.mp3")
    );

    alice.server.send_peer_address("bob", bob_port as u32).await;
    assert_no_connection(&bob).await;

    alice.handle.peer("bob", queue_request("b.mp3"));
    assert_eq!(
        read_peer_message(&mut pierced).await,
        queue_request("b.mp3")
    );
}

#[tokio::test]
async fn outgoing_pierce_is_reused_then_replaced_by_incoming_init() {
    let mut alice = scripted_stack("alice", None).await;
    let bob = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bob_port = bob.local_addr().unwrap().port();

    alice
        .server
        .send_connect_to_peer("bob", bob_port as u32, 777)
        .await;
    let mut pierced = accept(&bob).await;
    assert_eq!(
        read_peer_init(&mut pierced).await,
        PeerInitMessage::PierceFireWall { token: 777 }
    );

    alice.handle.peer("bob", queue_request("a.mp3"));
    assert_eq!(
        read_peer_message(&mut pierced).await,
        queue_request("a.mp3")
    );

    let mut direct = TcpStream::connect(("127.0.0.1", alice.listen_port))
        .await
        .unwrap();
    write_peer_init(
        &mut direct,
        PeerInitMessage::PeerInit {
            username: "bob".into(),
            conn_type: ConnectionType::Peer,
        },
    )
    .await;
    assert_closed(&mut pierced).await;

    alice.handle.peer("bob", queue_request("b.mp3"));
    assert_eq!(read_peer_message(&mut direct).await, queue_request("b.mp3"));
}

#[tokio::test]
async fn out_of_range_ports_are_not_truncated() {
    let mut alice = scripted_stack("alice", None).await;
    let bob = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let wrapped_port = bob.local_addr().unwrap().port() as u32 + 65536;

    alice
        .server
        .send_connect_to_peer("bob", wrapped_port, 5)
        .await;
    assert_no_connection(&bob).await;

    alice.handle.peer("bob", queue_request("a.mp3"));
    alice.server.expect_connect_to_peer().await;
    alice.server.expect(3).await;
    alice.server.send_peer_address("bob", wrapped_port).await;
    assert_no_connection(&bob).await;
}

async fn read_distributed_message(stream: &mut TcpStream) -> DistributedMessage {
    timeout(Duration::from_secs(10), async {
        let size = stream.read_u32_le().await.unwrap() as usize;
        let code = stream.read_u8().await.unwrap();
        let mut payload = vec![0u8; size - 1];
        stream.read_exact(&mut payload).await.unwrap();
        DistributedMessage::parse(code, &payload).unwrap()
    })
    .await
    .expect("timed out waiting for distributed message")
}

async fn connect_distributed_child(listen_port: u16, username: &str) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", listen_port))
        .await
        .unwrap();
    write_peer_init(
        &mut stream,
        PeerInitMessage::PeerInit {
            username: username.into(),
            conn_type: ConnectionType::Distributed,
        },
    )
    .await;
    stream
}

#[tokio::test]
async fn incoming_distributed_child_replaces_previous_connection() {
    let mut alice = scripted_stack("alice", None).await;
    let mut stats = MessageWriter::new();
    stats.write_string("alice");
    for value in [100_000, 0, 0, 0, 0] {
        stats.write_u32(value);
    }
    alice.server.send(36, stats).await;
    let mut embedded = MessageWriter::new();
    embedded.write_u8(3);
    embedded.write_u32(49);
    embedded.write_string("carol");
    embedded.write_u32(1);
    embedded.write_string("term");
    alice.server.send(93, embedded).await;
    wait_for(&mut alice.events, |event| match event {
        NetworkEvent::DistributedSearch { .. } => Some(()),
        _ => None,
    })
    .await;

    let mut first = connect_distributed_child(alice.listen_port, "bob").await;
    assert!(matches!(
        read_distributed_message(&mut first).await,
        DistributedMessage::BranchLevel { .. }
    ));

    let mut second = connect_distributed_child(alice.listen_port, "bob").await;
    assert_closed(&mut first).await;
    assert!(matches!(
        read_distributed_message(&mut second).await,
        DistributedMessage::BranchLevel { .. }
    ));
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

    assert_eq!(
        next_peer_message(&mut alice.events).await,
        search_response(7)
    );
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
    assert_eq!(
        next_peer_message(&mut alice.events).await,
        search_response(7)
    );

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
        alice
            .handle
            .peer_frame("mallory", vec![0u8; 4 * 1024 * 1024]);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    alice.handle.send(NetworkCommand::CloseConnection(conn_id));

    wait_for(&mut alice.events, |event| match event {
        NetworkEvent::ConnectionCount(0) => Some(()),
        _ => None,
    })
    .await;
}
