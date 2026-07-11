use std::collections::HashMap;

use serde::Serialize;

use crate::types::{TransferDirection, TransferId};

use super::chat::{ChatMessage, RoomEntry, RoomsView};
use super::interests::InterestsView;
use super::search::{SearchResponseView, SearchView};
use super::session::Status;
use super::settings::PublicSettings;
use super::transfers::TransferView;
use super::users::{BuddyView, UserInfoView};

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    Status {
        status: Status,
    },
    ConnCount {
        count: usize,
    },
    LoginFailed {
        reason: String,
        detail: Option<String>,
    },
    ServerMessage {
        message: String,
    },
    Settings(SettingsPayload),
    Transfer {
        direction: TransferDirection,
        transfer: TransferView,
    },
    TransfersRemoved {
        direction: TransferDirection,
        ids: Vec<TransferId>,
    },
    SearchAdded {
        search: SearchView,
    },
    SearchResults {
        token: u32,
        response: SearchResponseView,
    },
    SearchRemoved {
        token: u32,
    },
    Wishlist {
        wishlist: Vec<String>,
    },
    Interests {
        interests: InterestsView,
    },
    UserStatus {
        username: String,
        status: &'static str,
        privileged: bool,
    },
    Buddy {
        buddy: BuddyView,
    },
    BuddyRemoved {
        username: String,
    },
    Banned {
        users: Vec<String>,
    },
    Ignored {
        users: Vec<String>,
    },
    UserInfo {
        info: UserInfoView,
    },
    BrowseLoaded {
        username: String,
        received_at: i64,
    },
    PrivateMessage {
        username: String,
        message: ChatMessage,
    },
    ChatOpened {
        username: String,
    },
    ChatClosed {
        username: String,
    },
    RoomMessage {
        room: String,
        message: ChatMessage,
    },
    RoomList {
        rooms: Vec<RoomEntry>,
    },
    RoomJoined {
        room: String,
        users: Vec<String>,
    },
    RoomLeft {
        room: String,
    },
    RoomUserJoined {
        room: String,
        username: String,
    },
    RoomUserLeft {
        room: String,
        username: String,
    },
}

#[derive(Serialize)]
pub struct Envelope<'a> {
    pub rev: u64,
    #[serde(flatten)]
    pub event: &'a AppEvent,
}

#[derive(Serialize)]
pub struct SettingsPayload {
    pub settings: PublicSettings,
    pub locked: Vec<&'static str>,
    pub gluetun: bool,
}

#[derive(Serialize)]
pub struct Snapshot<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub rev: u64,
    pub status: &'a Status,
    pub downloads: Vec<&'a TransferView>,
    pub uploads: Vec<&'a TransferView>,
    pub searches: &'a [SearchView],
    pub buddies: Vec<&'a BuddyView>,
    pub banned: &'a [String],
    pub ignored: &'a [String],
    pub wishlist: &'a [String],
    pub interests: &'a InterestsView,
    pub rooms: &'a RoomsView,
    pub chat_partners: &'a [String],
    pub browses: HashMap<&'a String, i64>,
    pub settings: SettingsPayload,
}

#[cfg(test)]
mod tests {
    use super::*;

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
            settings: PublicSettings::from(&crate::types::Settings::default()),
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
}
