use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use newkitine::protocol::MessageWriter;

pub struct FakeClient {
    pub port: u16,
    pub outgoing: UnboundedSender<Vec<u8>>,
    pub hidden: bool,
}

pub type Registry = Arc<Mutex<HashMap<String, FakeClient>>>;

pub async fn run_fake_server(listener: TcpListener, registry: Registry) {
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::spawn(handle_fake_client(stream, registry.clone()));
    }
}

pub fn frame(code: u32, payload: Vec<u8>) -> Vec<u8> {
    let mut framed = MessageWriter::new();
    framed.write_u32(payload.len() as u32 + 4);
    framed.write_u32(code);
    framed.write_raw(&payload);
    framed.into_bytes()
}

fn read_string(payload: &[u8], offset: &mut usize) -> String {
    let len = u32::from_le_bytes(payload[*offset..*offset + 4].try_into().unwrap()) as usize;
    *offset += 4;
    let value = String::from_utf8(payload[*offset..*offset + len].to_vec()).unwrap();
    *offset += len;
    value
}

fn read_u32(payload: &[u8], offset: &mut usize) -> u32 {
    let value = u32::from_le_bytes(payload[*offset..*offset + 4].try_into().unwrap());
    *offset += 4;
    value
}

async fn handle_fake_client(stream: TcpStream, registry: Registry) {
    let (mut reader, mut writer) = stream.into_split();
    let (outgoing_tx, mut outgoing_rx) = unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        while let Some(bytes) = outgoing_rx.recv().await {
            if writer.write_all(&bytes).await.is_err() {
                return;
            }
        }
    });

    let mut username = String::new();
    loop {
        let size = match reader.read_u32_le().await {
            Ok(size) => size as usize,
            Err(_) => return,
        };
        let mut msg = vec![0u8; size];
        if reader.read_exact(&mut msg).await.is_err() {
            return;
        }
        let code = u32::from_le_bytes(msg[..4].try_into().unwrap());
        let payload = &msg[4..];
        let mut offset = 0;
        match code {
            1 => {
                username = read_string(payload, &mut offset);
                registry.lock().await.insert(
                    username.clone(),
                    FakeClient {
                        port: 0,
                        outgoing: outgoing_tx.clone(),
                        hidden: false,
                    },
                );
                let mut w = MessageWriter::new();
                w.write_bool(true);
                w.write_string("Welcome to the fake server");
                w.write_ip("127.0.0.1".parse().unwrap());
                w.write_string("checksum");
                w.write_bool(false);
                let _ = outgoing_tx.send(frame(1, w.into_bytes()));
                let mut interval = MessageWriter::new();
                interval.write_u32(1);
                let _ = outgoing_tx.send(frame(104, interval.into_bytes()));
            }
            2 => {
                let port = read_u32(payload, &mut offset) as u16;
                if let Some(client) = registry.lock().await.get_mut(&username) {
                    client.port = port;
                }
            }
            3 => {
                let target = read_string(payload, &mut offset);
                let registry = registry.lock().await;
                let port = registry
                    .get(&target)
                    .filter(|client| !client.hidden)
                    .map(|client| client.port)
                    .unwrap_or(0);
                let mut w = MessageWriter::new();
                w.write_string(&target);
                w.write_ip("127.0.0.1".parse().unwrap());
                w.write_u32(port as u32);
                w.write_u32(0);
                w.write_u32(0);
                let _ = outgoing_tx.send(frame(3, w.into_bytes()));
            }
            26 | 103 => {
                let token = read_u32(payload, &mut offset);
                let term = read_string(payload, &mut offset);
                let mut w = MessageWriter::new();
                w.write_string(&username);
                w.write_u32(token);
                w.write_string(&term);
                let relayed = frame(26, w.into_bytes());
                for (name, client) in registry.lock().await.iter() {
                    if *name != username {
                        let _ = client.outgoing.send(relayed.clone());
                    }
                }
            }
            18 => {
                let token = read_u32(payload, &mut offset);
                let target = read_string(payload, &mut offset);
                let conn_type = read_string(payload, &mut offset);
                let registry = registry.lock().await;
                let requester_port = registry
                    .get(&username)
                    .map(|client| client.port)
                    .unwrap_or(0);
                if let Some(target_client) = registry.get(&target) {
                    let mut w = MessageWriter::new();
                    w.write_string(&username);
                    w.write_string(&conn_type);
                    w.write_ip("127.0.0.1".parse().unwrap());
                    w.write_u32(requester_port as u32);
                    w.write_u32(token);
                    w.write_bool(false);
                    w.write_u32(0);
                    w.write_u32(0);
                    let _ = target_client.outgoing.send(frame(18, w.into_bytes()));
                }
            }
            _ => {}
        }
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub async fn start_fake_server() -> (SocketAddr, Registry) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    let registry: Registry = Arc::default();
    tokio::spawn(run_fake_server(listener, registry.clone()));
    (server_addr, registry)
}

pub fn tempfile() -> std::fs::File {
    use std::io::{Seek, SeekFrom};
    let mut file = tempfile_named().1;
    file.seek(SeekFrom::Start(0)).unwrap();
    file
}

pub fn tempfile_named() -> (std::path::PathBuf, std::fs::File) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "newkitine-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    (path, file)
}

#[allow(dead_code)]
pub fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("newkitine-test-{}-{}", std::process::id(), label));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[allow(dead_code)]
pub fn client_config(
    server: SocketAddr,
    username: &str,
    listen_port: u16,
    download_dir: std::path::PathBuf,
) -> newkitine::types::ClientBootstrap {
    newkitine::types::ClientBootstrap {
        runtime: newkitine::types::RuntimeConfig {
            server,
            username: username.into(),
            password: "secret".into(),
            listen_port,
            description: String::new(),
            incomplete_dir: download_dir.join("incomplete"),
            download_dir,
            shared_folders: Vec::new(),
            upload_slots: newkitine::types::DEFAULT_UPLOAD_SLOTS,
            queue_file_limit: newkitine::types::DEFAULT_QUEUE_FILE_LIMIT,
            upload_limit_kbps: 0,
            download_limit_kbps: 0,
            auto_reconnect: true,
        },
        buddies: Vec::new(),
        banned: Vec::new(),
        ignored: Vec::new(),
        ip_bans: Vec::new(),
        wishlist: Vec::new(),
        liked_interests: Vec::new(),
        hated_interests: Vec::new(),
        transfers: Vec::new(),
    }
}
