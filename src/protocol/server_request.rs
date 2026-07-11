use super::wire::MessageWriter;
use crate::types::ConnectionType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequest {
    Login { username: String, password: String, version: u32, minor_version: u32 },
    SetWaitPort { port: u32 },
    GetPeerAddress { user: String },
    WatchUser { user: String },
    UnwatchUser { user: String },
    GetUserStatus { user: String },
    SayChatroom { room: String, message: String },
    JoinRoom { room: String, private: bool },
    LeaveRoom { room: String },
    ConnectToPeer { token: u32, user: String, conn_type: ConnectionType },
    MessageUser { user: String, message: String },
    MessageAcked { message_id: u32 },
    FileSearch { token: u32, search_term: String },
    SetStatus { status: i32 },
    ServerPing,
    SharedFoldersFiles { folders: u32, files: u32 },
    GetUserStats { user: String },
    UserSearch { search_username: String, token: u32, search_term: String },
    AddThingILike { thing: String },
    RemoveThingILike { thing: String },
    Recommendations,
    GlobalRecommendations,
    UserInterests { user: String },
    RoomList,
    HaveNoParent { no_parent: bool },
    CheckPrivileges,
    AcceptChildren { enabled: bool },
    WishlistSearch { token: u32, search_term: String },
    SimilarUsers,
    ItemRecommendations { thing: String },
    ItemSimilarUsers { thing: String },
    SetRoomTicker { room: String, msg: String },
    AddThingIHate { thing: String },
    RemoveThingIHate { thing: String },
    RoomSearch { room: String, token: u32, search_term: String },
    SendUploadSpeed { speed: u32 },
    GivePrivileges { user: String, days: u32 },
    BranchLevel { value: u32 },
    BranchRoot { user: String },
    AddRoomMember { room: String, user: String },
    RemoveRoomMember { room: String, user: String },
    CancelRoomMembership { room: String },
    CancelRoomOwnership { room: String },
    EnableRoomInvitations { enabled: bool },
    ChangePassword { password: String },
    AddRoomOperator { room: String, user: String },
    RemoveRoomOperator { room: String, user: String },
    MessageUsers { users: Vec<String>, msg: String },
    JoinGlobalRoom,
    LeaveGlobalRoom,
    CantConnectToPeer { token: u32, user: String },
}

impl ServerRequest {
    pub fn code(&self) -> u32 {
        match self {
            Self::Login { .. } => 1,
            Self::SetWaitPort { .. } => 2,
            Self::GetPeerAddress { .. } => 3,
            Self::WatchUser { .. } => 5,
            Self::UnwatchUser { .. } => 6,
            Self::GetUserStatus { .. } => 7,
            Self::SayChatroom { .. } => 13,
            Self::JoinRoom { .. } => 14,
            Self::LeaveRoom { .. } => 15,
            Self::ConnectToPeer { .. } => 18,
            Self::MessageUser { .. } => 22,
            Self::MessageAcked { .. } => 23,
            Self::FileSearch { .. } => 26,
            Self::SetStatus { .. } => 28,
            Self::ServerPing => 32,
            Self::SharedFoldersFiles { .. } => 35,
            Self::GetUserStats { .. } => 36,
            Self::UserSearch { .. } => 42,
            Self::AddThingILike { .. } => 51,
            Self::RemoveThingILike { .. } => 52,
            Self::Recommendations => 54,
            Self::GlobalRecommendations => 56,
            Self::UserInterests { .. } => 57,
            Self::RoomList => 64,
            Self::HaveNoParent { .. } => 71,
            Self::CheckPrivileges => 92,
            Self::AcceptChildren { .. } => 100,
            Self::WishlistSearch { .. } => 103,
            Self::SimilarUsers => 110,
            Self::ItemRecommendations { .. } => 111,
            Self::ItemSimilarUsers { .. } => 112,
            Self::SetRoomTicker { .. } => 116,
            Self::AddThingIHate { .. } => 117,
            Self::RemoveThingIHate { .. } => 118,
            Self::RoomSearch { .. } => 120,
            Self::SendUploadSpeed { .. } => 121,
            Self::GivePrivileges { .. } => 123,
            Self::BranchLevel { .. } => 126,
            Self::BranchRoot { .. } => 127,
            Self::AddRoomMember { .. } => 134,
            Self::RemoveRoomMember { .. } => 135,
            Self::CancelRoomMembership { .. } => 136,
            Self::CancelRoomOwnership { .. } => 137,
            Self::EnableRoomInvitations { .. } => 141,
            Self::ChangePassword { .. } => 142,
            Self::AddRoomOperator { .. } => 143,
            Self::RemoveRoomOperator { .. } => 144,
            Self::MessageUsers { .. } => 149,
            Self::JoinGlobalRoom => 150,
            Self::LeaveGlobalRoom => 151,
            Self::CantConnectToPeer { .. } => 1001,
        }
    }

    pub fn write_payload(&self, w: &mut MessageWriter) {
        match self {
            Self::Login { username, password, version, minor_version } => {
                w.write_string(username);
                w.write_string(password);
                w.write_u32(*version);
                let digest = md5::compute(format!("{username}{password}").as_bytes());
                w.write_string(&format!("{digest:x}"));
                w.write_u32(*minor_version);
            }
            Self::SetWaitPort { port } => w.write_u32(*port),
            Self::GetPeerAddress { user } => w.write_string(user),
            Self::WatchUser { user } => w.write_string(user),
            Self::UnwatchUser { user } => w.write_string(user),
            Self::GetUserStatus { user } => w.write_string(user),
            Self::SayChatroom { room, message } => {
                w.write_string(room);
                w.write_string(message);
            }
            Self::JoinRoom { room, private } => {
                w.write_string(room);
                w.write_u32(*private as u32);
            }
            Self::LeaveRoom { room } => w.write_string(room),
            Self::ConnectToPeer { token, user, conn_type } => {
                w.write_u32(*token);
                w.write_string(user);
                w.write_string(conn_type.as_str());
            }
            Self::MessageUser { user, message } => {
                w.write_string(user);
                w.write_string(message);
            }
            Self::MessageAcked { message_id } => w.write_u32(*message_id),
            Self::FileSearch { token, search_term }
            | Self::WishlistSearch { token, search_term } => {
                w.write_u32(*token);
                w.write_string(search_term);
            }
            Self::SetStatus { status } => w.write_i32(*status),
            Self::ServerPing => {}
            Self::SharedFoldersFiles { folders, files } => {
                w.write_u32(*folders);
                w.write_u32(*files);
            }
            Self::GetUserStats { user } => w.write_string(user),
            Self::UserSearch { search_username, token, search_term } => {
                w.write_string(search_username);
                w.write_u32(*token);
                w.write_string(search_term);
            }
            Self::AddThingILike { thing }
            | Self::RemoveThingILike { thing }
            | Self::ItemRecommendations { thing }
            | Self::ItemSimilarUsers { thing }
            | Self::AddThingIHate { thing }
            | Self::RemoveThingIHate { thing } => w.write_string(thing),
            Self::Recommendations
            | Self::GlobalRecommendations
            | Self::RoomList
            | Self::CheckPrivileges
            | Self::SimilarUsers
            | Self::JoinGlobalRoom
            | Self::LeaveGlobalRoom => {}
            Self::UserInterests { user } => w.write_string(user),
            Self::HaveNoParent { no_parent } => w.write_bool(*no_parent),
            Self::AcceptChildren { enabled } => w.write_bool(*enabled),
            Self::SetRoomTicker { room, msg } => {
                w.write_string(room);
                w.write_string(msg);
            }
            Self::RoomSearch { room, token, search_term } => {
                w.write_string(room);
                w.write_u32(*token);
                w.write_string(search_term);
            }
            Self::SendUploadSpeed { speed } => w.write_u32(*speed),
            Self::GivePrivileges { user, days } => {
                w.write_string(user);
                w.write_u32(*days);
            }
            Self::BranchLevel { value } => w.write_u32(*value),
            Self::BranchRoot { user } => w.write_string(user),
            Self::AddRoomMember { room, user }
            | Self::RemoveRoomMember { room, user }
            | Self::AddRoomOperator { room, user }
            | Self::RemoveRoomOperator { room, user } => {
                w.write_string(room);
                w.write_string(user);
            }
            Self::CancelRoomMembership { room } | Self::CancelRoomOwnership { room } => {
                w.write_string(room);
            }
            Self::EnableRoomInvitations { enabled } => w.write_bool(*enabled),
            Self::ChangePassword { password } => w.write_string(password),
            Self::MessageUsers { users, msg } => {
                w.write_u32(users.len() as u32);
                for user in users {
                    w.write_string(user);
                }
                w.write_string(msg);
            }
            Self::CantConnectToPeer { token, user } => {
                w.write_u32(*token);
                w.write_string(user);
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = MessageWriter::new();
        self.write_payload(&mut w);
        let payload = w.into_bytes();
        let mut framed = MessageWriter::new();
        framed.write_u32(payload.len() as u32 + 4);
        framed.write_u32(self.code());
        framed.write_raw(&payload);
        framed.into_bytes()
    }
}
