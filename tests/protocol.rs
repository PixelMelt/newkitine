use newkitine::protocol::{
    DistributedMessage, FileOffset, FileTransferInit, LoginOutcome, MessageWriter,
    PeerInitMessage, PeerMessage, ServerRequest, ServerResponse,
};
use newkitine::types::{
    ConnectionType, FileAttributes, FileInfo, FolderContents, TransferDirection,
};

fn payload(request: &ServerRequest) -> Vec<u8> {
    let mut w = MessageWriter::new();
    request.write_payload(&mut w);
    w.into_bytes()
}

#[test]
fn pack_primitives() {
    let mut w = MessageWriter::new();
    w.write_bool(true);
    w.write_u8(123);
    w.write_u32(123);
    w.write_i32(123);
    w.write_u64(123);
    w.write_bytes(b"testbytes");
    w.write_string("teststring");
    let expected: Vec<u8> = [
        &b"\x01\x7b"[..],
        &b"\x7b\x00\x00\x00"[..],
        &b"\x7b\x00\x00\x00"[..],
        &b"\x7b\x00\x00\x00\x00\x00\x00\x00"[..],
        &b"\x09\x00\x00\x00testbytes"[..],
        &b"\x0a\x00\x00\x00teststring"[..],
    ]
    .concat();
    assert_eq!(w.into_bytes(), expected);
}

#[test]
fn login_make() {
    let request = ServerRequest::Login {
        username: "test".into(),
        password: "s33cr3t".into(),
        version: 157,
        minor_version: 19,
    };
    let expected: Vec<u8> = [
        &b"\x04\x00\x00\x00test\x07\x00\x00\x00s33cr3t\x9d\x00\x00\x00"[..],
        &b" \x00\x00\x00dbc93f24d8f3f109deed23c3e2f8b74c\x13\x00\x00\x00"[..],
    ]
    .concat();
    assert_eq!(payload(&request), expected);
    assert_eq!(request.code(), 1);
}

#[test]
fn simple_server_requests() {
    assert_eq!(
        payload(&ServerRequest::ChangePassword { password: "s33cr3t".into() }),
        b"\x07\x00\x00\x00s33cr3t"
    );
    assert_eq!(payload(&ServerRequest::SetWaitPort { port: 1337 }), b"9\x05\x00\x00");
    assert_eq!(
        payload(&ServerRequest::GetPeerAddress { user: "user1".into() }),
        b"\x05\x00\x00\x00user1"
    );
    assert_eq!(
        payload(&ServerRequest::WatchUser { user: "user2".into() }),
        b"\x05\x00\x00\x00user2"
    );
    assert_eq!(
        payload(&ServerRequest::UnwatchUser { user: "user3".into() }),
        b"\x05\x00\x00\x00user3"
    );
    assert_eq!(
        payload(&ServerRequest::GetUserStatus { user: "user4".into() }),
        b"\x05\x00\x00\x00user4"
    );
    assert_eq!(
        payload(&ServerRequest::FileSearch { token: 524700074, search_term: "70 gwen auto".into() }),
        b"\xaaIF\x1f\x0c\x00\x00\x0070 gwen auto"
    );
    assert_eq!(payload(&ServerRequest::SetStatus { status: 1 }), b"\x01\x00\x00\x00");
    assert_eq!(payload(&ServerRequest::JoinGlobalRoom), b"");
    assert_eq!(payload(&ServerRequest::LeaveGlobalRoom), b"");
    assert_eq!(
        payload(&ServerRequest::SayChatroom { room: "room1".into(), message: "Wassup?".into() }),
        b"\x05\x00\x00\x00room1\x07\x00\x00\x00Wassup?"
    );
    assert_eq!(
        payload(&ServerRequest::JoinRoom { room: "room2".into(), private: false }),
        b"\x05\x00\x00\x00room2\x00\x00\x00\x00"
    );
    assert_eq!(
        payload(&ServerRequest::JoinRoom { room: "room2".into(), private: true }),
        b"\x05\x00\x00\x00room2\x01\x00\x00\x00"
    );
    assert_eq!(
        payload(&ServerRequest::AddRoomMember { room: "room3".into(), user: "admin".into() }),
        b"\x05\x00\x00\x00room3\x05\x00\x00\x00admin"
    );
    assert_eq!(
        payload(&ServerRequest::CancelRoomMembership { room: "room4".into() }),
        b"\x05\x00\x00\x00room4"
    );
    assert_eq!(
        payload(&ServerRequest::CancelRoomOwnership { room: "room5".into() }),
        b"\x05\x00\x00\x00room5"
    );
    assert_eq!(
        payload(&ServerRequest::RemoveRoomMember { room: "room7".into(), user: "admin".into() }),
        b"\x05\x00\x00\x00room7\x05\x00\x00\x00admin"
    );
}

#[test]
fn remove_room_member_parse() {
    let parsed =
        ServerResponse::parse(135, b"\x05\x00\x00\x00room7\x05\x00\x00\x00admin").unwrap();
    assert_eq!(
        parsed,
        ServerResponse::RemoveRoomMember { room: "room7".into(), user: "admin".into() }
    );
}

#[test]
fn login_response_parse() {
    let mut w = MessageWriter::new();
    w.write_bool(true);
    w.write_string("Welcome!");
    w.write_ip("192.168.1.2".parse().unwrap());
    w.write_string("checksum");
    w.write_bool(false);
    let parsed = ServerResponse::parse(1, &w.into_bytes()).unwrap();
    assert_eq!(
        parsed,
        ServerResponse::Login(LoginOutcome::Success {
            banner: "Welcome!".into(),
            ip_address: "192.168.1.2".parse().unwrap(),
            is_supporter: false,
        })
    );

    let mut w = MessageWriter::new();
    w.write_bool(false);
    w.write_string("INVALIDPASS");
    let parsed = ServerResponse::parse(1, &w.into_bytes()).unwrap();
    assert_eq!(
        parsed,
        ServerResponse::Login(LoginOutcome::Failure {
            reason: "INVALIDPASS".into(),
            detail: None,
        })
    );
}

#[test]
fn server_message_framing() {
    let framed = ServerRequest::SetWaitPort { port: 1337 }.to_bytes();
    assert_eq!(framed, b"\x08\x00\x00\x00\x02\x00\x00\x009\x05\x00\x00");
}

#[test]
fn peer_init_round_trip() {
    let msg = PeerInitMessage::PeerInit {
        username: "alice".into(),
        conn_type: ConnectionType::Peer,
    };
    let bytes = msg.to_bytes();
    assert_eq!(&bytes[..4], &19u32.to_le_bytes());
    assert_eq!(bytes[4], 1);
    let parsed = PeerInitMessage::parse(bytes[4], &bytes[5..]).unwrap();
    assert_eq!(parsed, msg);

    let pierce = PeerInitMessage::PierceFireWall { token: 42 };
    let bytes = pierce.to_bytes();
    let parsed = PeerInitMessage::parse(bytes[4], &bytes[5..]).unwrap();
    assert_eq!(parsed, pierce);
}

fn sample_files() -> Vec<FileInfo> {
    vec![
        FileInfo {
            code: 1,
            name: "Album\\01 - Track.mp3".into(),
            size: 9876543,
            attributes: FileAttributes {
                bitrate: Some(320),
                length: Some(255),
                vbr: Some(0),
                ..FileAttributes::default()
            },
        },
        FileInfo {
            code: 1,
            name: "Album\\02 - Track.flac".into(),
            size: 33445566,
            attributes: FileAttributes {
                length: Some(200),
                sample_rate: Some(44100),
                bit_depth: Some(16),
                ..FileAttributes::default()
            },
        },
    ]
}

#[test]
fn file_search_response_round_trip() {
    let msg = PeerMessage::FileSearchResponse {
        username: "bob".into(),
        token: 1234,
        results: sample_files(),
        free_upload_slots: true,
        upload_speed: 100000,
        queue_size: 3,
        unknown: 0,
        private_results: Vec::new(),
    };
    let bytes = msg.make_payload();
    let parsed = PeerMessage::parse(9, &bytes).unwrap();
    assert_eq!(parsed, msg);
}

#[test]
fn shared_file_list_response_round_trip() {
    let msg = PeerMessage::SharedFileListResponse {
        shares: vec![
            FolderContents { directory: "Music\\Album".into(), files: sample_files() },
            FolderContents { directory: "Music\\Extra".into(), files: Vec::new() },
        ],
        unknown: 0,
        private_shares: vec![FolderContents {
            directory: "Private".into(),
            files: sample_files(),
        }],
    };
    let bytes = msg.make_payload();
    let parsed = PeerMessage::parse(5, &bytes).unwrap();
    assert_eq!(parsed, msg);
}

#[test]
fn folder_contents_response_round_trip() {
    let msg = PeerMessage::FolderContentsResponse {
        token: 55,
        directory: "Music\\Album".into(),
        folders: vec![FolderContents { directory: "Music\\Album".into(), files: sample_files() }],
    };
    let bytes = msg.make_payload();
    let parsed = PeerMessage::parse(37, &bytes).unwrap();
    assert_eq!(parsed, msg);
}

#[test]
fn transfer_negotiation_round_trip() {
    let request = PeerMessage::TransferRequest {
        direction: TransferDirection::Upload,
        token: 77,
        file: "Music\\Album\\01 - Track.mp3".into(),
        filesize: Some(9876543),
    };
    let parsed = PeerMessage::parse(40, &request.make_payload()).unwrap();
    assert_eq!(parsed, request);

    let response = PeerMessage::TransferResponse {
        token: 77,
        allowed: false,
        reason: Some("Queued".into()),
        filesize: None,
    };
    let parsed = PeerMessage::parse(41, &response.make_payload()).unwrap();
    assert_eq!(parsed, response);

    let accepted = PeerMessage::TransferResponse {
        token: 78,
        allowed: true,
        reason: None,
        filesize: Some(1024),
    };
    let parsed = PeerMessage::parse(41, &accepted.make_payload()).unwrap();
    assert_eq!(parsed, accepted);
}

#[test]
fn user_info_response_round_trip() {
    let msg = PeerMessage::UserInfoResponse {
        description: "hello".into(),
        picture: Some(vec![1, 2, 3]),
        total_uploads: 5,
        queue_size: 2,
        slots_available: true,
        upload_allowed: Some(1),
    };
    let parsed = PeerMessage::parse(16, &msg.make_payload()).unwrap();
    assert_eq!(parsed, msg);
}

#[test]
fn ns_large_file_size_workaround() {
    let mut w = MessageWriter::new();
    w.write_string("bob");
    w.write_u32(1);
    w.write_u32(1);
    w.write_u8(1);
    w.write_string("big.bin");
    w.write_u32(3000000000);
    w.write_u32(4294967295);
    w.write_u32(0);
    w.write_u32(0);
    w.write_bool(true);
    w.write_u32(0);
    w.write_u32(0);
    let compressed = newkitine::protocol::compress(&w.into_bytes());
    let parsed = PeerMessage::parse(9, &compressed).unwrap();
    let PeerMessage::FileSearchResponse { results, .. } = parsed else {
        panic!("wrong variant");
    };
    assert_eq!(results[0].size, 3000000000);
}

#[test]
fn distributed_round_trip() {
    let search = DistributedMessage::Search {
        identifier: 49,
        search_username: "carol".into(),
        token: 999,
        search_term: "test query".into(),
    };
    let bytes = search.to_bytes();
    let parsed = DistributedMessage::parse(bytes[4], &bytes[5..]).unwrap();
    assert_eq!(parsed, search);

    let level = DistributedMessage::BranchLevel { level: 2 };
    let bytes = level.to_bytes();
    let parsed = DistributedMessage::parse(bytes[4], &bytes[5..]).unwrap();
    assert_eq!(parsed, level);
}

#[test]
fn file_connection_messages() {
    let init = FileTransferInit { token: 31337 };
    assert_eq!(FileTransferInit::parse(&init.to_bytes()).unwrap(), init);

    let offset = FileOffset { offset: 1 << 33 };
    assert_eq!(FileOffset::parse(&offset.to_bytes()).unwrap(), offset);
}
