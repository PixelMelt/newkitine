use std::collections::HashMap;
use std::process::Command;

use super::{AppEvent, Envelope, SettingsPayload, Snapshot};
use crate::types::{
    BuddyView, ChatMessage, FileAttributes, InterestsView, PublicSettings, RoomEntry, RoomView,
    RoomsView, SearchFileView, SearchResponseView, SearchView, Settings, SimilarUser, Status,
    TransferDirection, TransferId, TransferStatus, TransferView, UserInfoView, UserStats,
};

fn type_tag(event: &AppEvent) -> &'static str {
    match event {
        AppEvent::Status { .. } => "status",
        AppEvent::ConnCount { .. } => "conn_count",
        AppEvent::LoginFailed { .. } => "login_failed",
        AppEvent::ServerMessage { .. } => "server_message",
        AppEvent::Settings(_) => "settings",
        AppEvent::Transfer { .. } => "transfer",
        AppEvent::TransfersRemoved { .. } => "transfers_removed",
        AppEvent::SearchAdded { .. } => "search_added",
        AppEvent::SearchResults { .. } => "search_results",
        AppEvent::SearchRemoved { .. } => "search_removed",
        AppEvent::Wishlist { .. } => "wishlist",
        AppEvent::Interests { .. } => "interests",
        AppEvent::Buddy { .. } => "buddy",
        AppEvent::BuddyRemoved { .. } => "buddy_removed",
        AppEvent::Banned { .. } => "banned",
        AppEvent::Ignored { .. } => "ignored",
        AppEvent::UserInfo { .. } => "user_info",
        AppEvent::BrowseLoaded { .. } => "browse_loaded",
        AppEvent::PrivateMessage { .. } => "private_message",
        AppEvent::ChatOpened { .. } => "chat_opened",
        AppEvent::ChatClosed { .. } => "chat_closed",
        AppEvent::RoomMessage { .. } => "room_message",
        AppEvent::RoomList { .. } => "room_list",
        AppEvent::RoomJoined { .. } => "room_joined",
        AppEvent::RoomLeft { .. } => "room_left",
        AppEvent::RoomUserJoined { .. } => "room_user_joined",
        AppEvent::RoomUserLeft { .. } => "room_user_left",
    }
}

fn attributes() -> FileAttributes {
    FileAttributes {
        bitrate: Some(320),
        length: Some(240),
        vbr: Some(1),
        sample_rate: None,
        bit_depth: None,
    }
}

fn stats() -> UserStats {
    UserStats {
        avgspeed: 100,
        uploadnum: 2,
        unknown: 0,
        files: 10,
        dirs: 3,
    }
}

fn status_fixture() -> Status {
    Status {
        connected: true,
        logged_in: true,
        username: "me".into(),
        server: "server.slsknet.org:2242".into(),
        banner: "welcome".into(),
        listen_port: 2234,
        shared_folders: 1,
        shared_files: 2,
        scanning: true,
        scan_progress: 42,
        share_scan_error: Some("scan failed".into()),
        privileges_secs: 0,
        peer_connections: 3,
    }
}

fn transfer_view() -> TransferView {
    TransferView {
        id: TransferId(7),
        direction: TransferDirection::Download,
        username: "peer".into(),
        virtual_path: "Music\\song.mp3".into(),
        size: 10,
        bytes_done: 5,
        status: TransferStatus::Transferring,
        failure_reason: Some("reason".into()),
        file_path: Some("/downloads/song.mp3".into()),
        queue_place: 1,
        speed_bps: 99,
        attributes: attributes(),
        updated_at: 1,
    }
}

fn search_response() -> SearchResponseView {
    SearchResponseView {
        username: "peer".into(),
        free_upload_slots: true,
        upload_speed: 500,
        queue_size: 2,
        files: vec![SearchFileView {
            name: "Music\\song.mp3".into(),
            size: 10,
            attributes: attributes(),
        }],
    }
}

fn search_view() -> SearchView {
    SearchView {
        token: 9,
        query: "song".into(),
        results: vec![search_response()],
    }
}

fn buddy_view() -> BuddyView {
    BuddyView {
        username: "friend".into(),
        status: "online".into(),
        privileged: true,
        stats: stats(),
        note: "note".into(),
    }
}

fn user_info_view() -> UserInfoView {
    UserInfoView {
        username: "peer".into(),
        received: true,
        description: "hello".into(),
        picture_base64: Some("aGk=".into()),
        upload_slots: 2,
        queue_size: 1,
        slots_available: true,
        stats: Some(stats()),
        interests_liked: vec!["jazz".into()],
        interests_hated: vec![],
    }
}

fn message() -> ChatMessage {
    ChatMessage {
        sender: "peer".into(),
        message: "hi".into(),
        timestamp: 1,
    }
}

fn rooms_view() -> RoomsView {
    RoomsView {
        available: vec![RoomEntry {
            name: "indie".into(),
            users: 5,
        }],
        joined: HashMap::from([(
            "indie".to_owned(),
            RoomView {
                users: vec!["peer".into()],
                messages: vec![message()],
            },
        )]),
    }
}

fn interests_view() -> InterestsView {
    InterestsView {
        liked: vec!["jazz".into()],
        hated: vec!["noise".into()],
        recommendations: vec![("ambient".into(), 5)],
        unrecommendations: vec![("polka".into(), -2)],
        recommendations_for: Some("jazz".into()),
        recommendations_global: false,
        similar_users: vec![SimilarUser {
            username: "peer".into(),
            rating: 3,
        }],
        similar_users_for: None,
    }
}

fn settings_payload() -> SettingsPayload {
    SettingsPayload {
        settings: PublicSettings::from(&Settings::default()),
        locked: vec!["server"],
        gluetun: true,
    }
}

fn fixture_events() -> Vec<AppEvent> {
    vec![
        AppEvent::Status {
            status: status_fixture(),
        },
        AppEvent::ConnCount { count: 3 },
        AppEvent::LoginFailed {
            reason: "INVALIDPASS".into(),
            detail: Some("detail".into()),
        },
        AppEvent::ServerMessage {
            message: "hello".into(),
        },
        AppEvent::Settings(settings_payload()),
        AppEvent::Transfer {
            direction: TransferDirection::Download,
            transfer: transfer_view(),
        },
        AppEvent::TransfersRemoved {
            direction: TransferDirection::Upload,
            ids: vec![TransferId(4)],
        },
        AppEvent::SearchAdded {
            search: search_view(),
        },
        AppEvent::SearchResults {
            token: 9,
            response: search_response(),
        },
        AppEvent::SearchRemoved { token: 9 },
        AppEvent::Wishlist {
            wishlist: vec!["term".into()],
        },
        AppEvent::Interests {
            interests: interests_view(),
        },
        AppEvent::Buddy {
            buddy: buddy_view(),
        },
        AppEvent::BuddyRemoved {
            username: "peer".into(),
        },
        AppEvent::Banned {
            users: vec!["peer".into()],
        },
        AppEvent::Ignored { users: vec![] },
        AppEvent::UserInfo {
            info: user_info_view(),
        },
        AppEvent::BrowseLoaded {
            username: "peer".into(),
            received_at: 1,
        },
        AppEvent::PrivateMessage {
            username: "peer".into(),
            message: message(),
        },
        AppEvent::ChatOpened {
            username: "peer".into(),
        },
        AppEvent::ChatClosed {
            username: "peer".into(),
        },
        AppEvent::RoomMessage {
            room: "indie".into(),
            message: message(),
        },
        AppEvent::RoomList {
            rooms: vec![RoomEntry {
                name: "indie".into(),
                users: 5,
            }],
        },
        AppEvent::RoomJoined {
            room: "indie".into(),
            users: vec!["peer".into()],
        },
        AppEvent::RoomLeft {
            room: "indie".into(),
        },
        AppEvent::RoomUserJoined {
            room: "indie".into(),
            username: "peer".into(),
        },
        AppEvent::RoomUserLeft {
            room: "indie".into(),
            username: "peer".into(),
        },
    ]
}

fn snapshot_json() -> String {
    let status = status_fixture();
    let downloads = [transfer_view()];
    let uploads = [transfer_view()];
    let searches = vec![search_view()];
    let buddies = [buddy_view()];
    let banned = vec!["peer".to_owned()];
    let ignored: Vec<String> = Vec::new();
    let wishlist = vec!["term".to_owned()];
    let interests = interests_view();
    let rooms = rooms_view();
    let partners = vec!["peer".to_owned()];
    let browsed = "peer".to_owned();
    serde_json::to_string(&Snapshot {
        kind: "snapshot",
        rev: 1,
        status: &status,
        downloads: downloads.iter().collect(),
        uploads: uploads.iter().collect(),
        searches: &searches,
        buddies: buddies.iter().collect(),
        banned: &banned,
        ignored: &ignored,
        wishlist: &wishlist,
        interests: &interests,
        rooms: &rooms,
        chat_partners: &partners,
        browses: HashMap::from([(&browsed, 1i64)]),
        settings: settings_payload(),
    })
    .unwrap()
}

#[test]
fn browser_contract_validates_serialized_events() {
    let events = fixture_events();
    let mut fixtures: Vec<serde_json::Value> = events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let value = serde_json::to_value(Envelope {
                rev: index as u64 + 1,
                event,
            })
            .unwrap();
            assert_eq!(value["type"], type_tag(event));
            value
        })
        .collect();
    fixtures.push(serde_json::from_str(&snapshot_json()).unwrap());

    let dir = std::env::temp_dir().join(format!("newkitine-contract-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fixtures_path = dir.join("fixtures.json");
    std::fs::write(&fixtures_path, serde_json::to_string(&fixtures).unwrap()).unwrap();
    let contract_path = concat!(env!("CARGO_MANIFEST_DIR"), "/web/src/lib/state/contract.js");
    let script_path = dir.join("check.mjs");
    std::fs::write(
        &script_path,
        format!(
            r#"import {{ readFileSync }} from 'node:fs';
import {{ validate, eventTypes }} from '{contract_path}';
const fixtures = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const seen = new Set();
for (const msg of fixtures) {{
	validate(msg);
	seen.add(msg.type);
}}
const missing = eventTypes.filter((type) => !seen.has(type));
if (missing.length) {{
	console.error(`no fixture exercises: ${{missing.join(', ')}}`);
	process.exit(1);
}}
"#
        ),
    )
    .unwrap();
    let output = Command::new("node")
        .arg(&script_path)
        .arg(&fixtures_path)
        .output()
        .expect("node is required to execute the browser contract");
    assert!(
        output.status.success(),
        "contract check failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn events_carry_type_tag_and_revision() {
    let event = AppEvent::ConnCount { count: 3 };
    let value = serde_json::to_value(&Envelope {
        rev: 7,
        event: &event,
    })
    .unwrap();
    assert_eq!(value["type"], "conn_count");
    assert_eq!(value["rev"], 7);
    assert_eq!(value["count"], 3);

    let event = AppEvent::TransfersRemoved {
        direction: TransferDirection::Download,
        ids: vec![TransferId(4)],
    };
    let value = serde_json::to_value(&Envelope {
        rev: 8,
        event: &event,
    })
    .unwrap();
    assert_eq!(value["type"], "transfers_removed");
    assert_eq!(value["direction"], "download");
    assert_eq!(value["ids"][0], 4);

    let event = AppEvent::RoomUserLeft {
        room: "indie".into(),
        username: "someone".into(),
    };
    let value = serde_json::to_value(&Envelope {
        rev: 9,
        event: &event,
    })
    .unwrap();
    assert_eq!(value["type"], "room_user_left");
    assert_eq!(value["room"], "indie");

    let event = AppEvent::Settings(SettingsPayload {
        settings: PublicSettings::from(&Settings::default()),
        locked: vec!["server"],
        gluetun: true,
    });
    let value = serde_json::to_value(&Envelope {
        rev: 10,
        event: &event,
    })
    .unwrap();
    assert_eq!(value["type"], "settings");
    assert_eq!(value["locked"][0], "server");
    assert_eq!(value["gluetun"], true);
    assert!(value["settings"].is_object());
}
