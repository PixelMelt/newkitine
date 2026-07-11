use std::net::Ipv4Addr;

use super::wire::{MessageReader, ProtocolError};
use crate::types::{Recommendations, SimilarUser, UserData, UserStats};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    Success {
        banner: String,
        ip_address: Ipv4Addr,
        is_supporter: bool,
    },
    Failure {
        reason: String,
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentCandidate {
    pub username: String,
    pub ip_address: Ipv4Addr,
    pub port: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerResponse {
    Login(LoginOutcome),
    GetPeerAddress {
        user: String,
        ip_address: Ipv4Addr,
        port: u32,
        obfuscation_type: u32,
        obfuscated_port: u16,
    },
    WatchUser {
        user: String,
        user_exists: bool,
        status: Option<u32>,
        stats: Option<UserStats>,
        country: Option<String>,
    },
    GetUserStatus {
        user: String,
        status: u32,
        privileged: bool,
    },
    IgnoreUser {
        user: String,
    },
    UnignoreUser {
        user: String,
    },
    SayChatroom {
        room: String,
        user: String,
        message: String,
    },
    JoinRoom {
        room: String,
        users: Vec<UserData>,
        owner: Option<String>,
        operators: Vec<String>,
    },
    LeaveRoom {
        room: String,
    },
    UserJoinedRoom {
        room: String,
        userdata: UserData,
    },
    UserLeftRoom {
        room: String,
        username: String,
    },
    ConnectToPeer {
        user: String,
        conn_type: String,
        ip_address: Ipv4Addr,
        port: u32,
        token: u32,
        privileged: bool,
        obfuscation_type: u32,
        obfuscated_port: u32,
    },
    MessageUser {
        message_id: u32,
        timestamp: u32,
        user: String,
        message: String,
        is_new_message: bool,
    },
    FileSearch {
        search_username: String,
        token: u32,
        search_term: String,
    },
    ServerPing,
    SendConnectToken {
        user: String,
        token: u32,
    },
    GetUserStats {
        user: String,
        stats: UserStats,
    },
    UploadSlotsFull {
        user: String,
        slotsfull: u32,
    },
    Relogged,
    SimilarRecommendations {
        recommendation: String,
        similar_recommendations: Vec<String>,
    },
    Recommendations {
        recommendations: Recommendations,
        unrecommendations: Recommendations,
    },
    MyRecommendations {
        my_recommendations: Vec<String>,
    },
    GlobalRecommendations {
        recommendations: Recommendations,
        unrecommendations: Recommendations,
    },
    UserInterests {
        user: String,
        likes: Vec<String>,
        hates: Vec<String>,
    },
    PlaceInLineRequest {
        user: String,
        token: u32,
    },
    PlaceInLineResponse {
        user: String,
        token: u32,
        place: u32,
    },
    RoomAdded {
        room: String,
    },
    RoomRemoved {
        room: String,
    },
    RoomList {
        rooms: Vec<(String, u32)>,
        rooms_owner: Vec<(String, u32)>,
        rooms_member: Vec<(String, u32)>,
        rooms_operator: Vec<String>,
    },
    ExactFileSearch {
        user: String,
        token: u32,
        file: String,
        folder: String,
        size: u64,
        checksum: u32,
    },
    AdminMessage {
        msg: String,
    },
    GlobalUserList {
        users: Vec<UserData>,
    },
    TunneledMessage {
        user: String,
        code: u32,
        token: u32,
        ip_address: Ipv4Addr,
        port: u32,
        msg: String,
    },
    PrivilegedUsers {
        users: Vec<String>,
    },
    ParentMinSpeed {
        speed: u32,
    },
    ParentSpeedRatio {
        ratio: u32,
    },
    ParentInactivityTimeout {
        seconds: u32,
    },
    SearchInactivityTimeout {
        seconds: u32,
    },
    MinParentsInCache {
        num: u32,
    },
    DistribPingInterval {
        seconds: u32,
    },
    AddToPrivileged {
        user: String,
    },
    CheckPrivileges {
        seconds: u32,
    },
    EmbeddedMessage {
        distrib_code: u8,
        distrib_message: Vec<u8>,
    },
    PossibleParents {
        parents: Vec<ParentCandidate>,
    },
    WishlistInterval {
        seconds: u32,
    },
    SimilarUsers {
        users: Vec<SimilarUser>,
    },
    ItemRecommendations {
        thing: String,
        recommendations: Recommendations,
        unrecommendations: Recommendations,
    },
    ItemSimilarUsers {
        thing: String,
        usernames: Vec<String>,
    },
    RoomTickers {
        room: String,
        tickers: Vec<(String, String)>,
    },
    RoomTickerAdded {
        room: String,
        user: String,
        msg: String,
    },
    RoomTickerRemoved {
        room: String,
        user: String,
    },
    UserPrivileged {
        user: String,
        privileged: bool,
    },
    NotifyPrivileges {
        token: u32,
        user: String,
    },
    AckNotifyPrivileges {
        token: u32,
    },
    ResetDistributed,
    RoomMembers {
        room: String,
        members: Vec<String>,
    },
    AddRoomMember {
        room: String,
        user: String,
    },
    RemoveRoomMember {
        room: String,
        user: String,
    },
    RoomSomething {
        room: String,
    },
    RoomMembershipGranted {
        room: String,
    },
    RoomMembershipRevoked {
        room: String,
    },
    EnableRoomInvitations {
        enabled: bool,
    },
    ChangePassword {
        password: String,
    },
    AddRoomOperator {
        room: String,
        user: String,
    },
    RemoveRoomOperator {
        room: String,
        user: String,
    },
    RoomOperatorshipGranted {
        room: String,
    },
    RoomOperatorshipRevoked {
        room: String,
    },
    RoomOperators {
        room: String,
        operators: Vec<String>,
    },
    GlobalRoomMessage {
        room: String,
        user: String,
        message: String,
    },
    RelatedSearch {
        query: String,
        terms: Vec<(String, u32)>,
    },
    ExcludedSearchPhrases {
        phrases: Vec<String>,
    },
    CantConnectToPeer {
        token: u32,
    },
    CantCreateRoom {
        room: String,
    },
}

fn read_user_stats(r: &mut MessageReader) -> Result<UserStats, ProtocolError> {
    Ok(UserStats {
        avgspeed: r.read_u32()?,
        uploadnum: r.read_u32()?,
        unknown: r.read_u32()?,
        files: r.read_u32()?,
        dirs: r.read_u32()?,
    })
}

fn read_users(r: &mut MessageReader) -> Result<Vec<UserData>, ProtocolError> {
    let num_users = r.read_u32()? as usize;
    let mut users = Vec::with_capacity(num_users.min(4096));
    for _ in 0..num_users {
        users.push(UserData {
            username: r.read_string()?,
            ..UserData::default()
        });
    }

    fn entry(users: &mut [UserData], i: usize) -> Result<&mut UserData, ProtocolError> {
        users.get_mut(i).ok_or(ProtocolError::InvalidValue {
            field: "user_index",
            value: i as u64,
        })
    }

    let status_len = r.read_u32()? as usize;
    for i in 0..status_len {
        entry(&mut users, i)?.status = r.read_u32()?;
    }

    let stats_len = r.read_u32()? as usize;
    for i in 0..stats_len {
        entry(&mut users, i)?.stats = read_user_stats(r)?;
    }

    let slots_len = r.read_u32()? as usize;
    for i in 0..slots_len {
        entry(&mut users, i)?.slotsfull = r.read_u32()?;
    }

    let country_len = r.read_u32()? as usize;
    for i in 0..country_len {
        entry(&mut users, i)?.country = Some(r.read_string()?);
    }

    Ok(users)
}

fn read_recommendation_group(
    r: &mut MessageReader,
    recommendations: &mut Recommendations,
    unrecommendations: &mut Recommendations,
) -> Result<(), ProtocolError> {
    let num = r.read_u32()?;
    for _ in 0..num {
        let key = r.read_string()?;
        let rating = r.read_i32()?;
        let list = if rating >= 0 {
            &mut *recommendations
        } else {
            &mut *unrecommendations
        };
        let item = (key, rating);
        if !list.contains(&item) {
            list.push(item);
        }
    }
    Ok(())
}

fn read_recommendations(
    r: &mut MessageReader,
) -> Result<(Recommendations, Recommendations), ProtocolError> {
    let mut recommendations = Vec::new();
    let mut unrecommendations = Vec::new();
    read_recommendation_group(r, &mut recommendations, &mut unrecommendations)?;
    if r.has_remaining() {
        read_recommendation_group(r, &mut recommendations, &mut unrecommendations)?;
    }
    Ok((recommendations, unrecommendations))
}

fn read_string_list(r: &mut MessageReader) -> Result<Vec<String>, ProtocolError> {
    let num = r.read_u32()? as usize;
    let mut items = Vec::with_capacity(num.min(4096));
    for _ in 0..num {
        items.push(r.read_string()?);
    }
    Ok(items)
}

fn read_rooms_with_counts(r: &mut MessageReader) -> Result<Vec<(String, u32)>, ProtocolError> {
    let names = read_string_list(r)?;
    let num_counts = r.read_u32()? as usize;
    let mut rooms: Vec<(String, u32)> = names.into_iter().map(|name| (name, 0)).collect();
    for i in 0..num_counts {
        let count = r.read_u32()?;
        rooms
            .get_mut(i)
            .ok_or(ProtocolError::InvalidValue {
                field: "room_index",
                value: i as u64,
            })?
            .1 = count;
    }
    Ok(rooms)
}

impl ServerResponse {
    pub fn parse(code: u32, payload: &[u8]) -> Result<Self, ProtocolError> {
        let r = &mut MessageReader::new(payload);
        Ok(match code {
            1 => {
                let success = r.read_bool()?;
                if !success {
                    let reason = r.read_string()?;
                    let detail = if r.has_remaining() {
                        Some(r.read_string()?)
                    } else {
                        None
                    };
                    Self::Login(LoginOutcome::Failure { reason, detail })
                } else {
                    let banner = r.read_string()?;
                    let ip_address = r.read_ip()?;
                    let _checksum = r.read_string()?;
                    let is_supporter = r.read_bool()?;
                    Self::Login(LoginOutcome::Success {
                        banner,
                        ip_address,
                        is_supporter,
                    })
                }
            }
            3 => Self::GetPeerAddress {
                user: r.read_string()?,
                ip_address: r.read_ip()?,
                port: r.read_u32()?,
                obfuscation_type: r.read_u32()?,
                obfuscated_port: r.read_u16_padded()?,
            },
            5 => {
                let user = r.read_string()?;
                let user_exists = r.read_bool()?;
                if !user_exists {
                    Self::WatchUser {
                        user,
                        user_exists,
                        status: None,
                        stats: None,
                        country: None,
                    }
                } else {
                    let status = r.read_u32()?;
                    let stats = read_user_stats(r)?;
                    let country = if r.has_remaining() {
                        Some(r.read_string()?)
                    } else {
                        None
                    };
                    Self::WatchUser {
                        user,
                        user_exists,
                        status: Some(status),
                        stats: Some(stats),
                        country,
                    }
                }
            }
            7 => Self::GetUserStatus {
                user: r.read_string()?,
                status: r.read_u32()?,
                privileged: r.read_bool()?,
            },
            11 => Self::IgnoreUser {
                user: r.read_string()?,
            },
            12 => Self::UnignoreUser {
                user: r.read_string()?,
            },
            13 => Self::SayChatroom {
                room: r.read_string()?,
                user: r.read_string()?,
                message: r.read_string()?,
            },
            14 => {
                let room = r.read_string()?;
                let users = read_users(r)?;
                let mut owner = None;
                let mut operators = Vec::new();
                if r.has_remaining() {
                    owner = Some(r.read_string()?);
                    operators = read_string_list(r)?;
                }
                Self::JoinRoom {
                    room,
                    users,
                    owner,
                    operators,
                }
            }
            15 => Self::LeaveRoom {
                room: r.read_string()?,
            },
            16 => Self::UserJoinedRoom {
                room: r.read_string()?,
                userdata: UserData {
                    username: r.read_string()?,
                    status: r.read_u32()?,
                    stats: read_user_stats(r)?,
                    slotsfull: r.read_u32()?,
                    country: Some(r.read_string()?),
                },
            },
            17 => Self::UserLeftRoom {
                room: r.read_string()?,
                username: r.read_string()?,
            },
            18 => Self::ConnectToPeer {
                user: r.read_string()?,
                conn_type: r.read_string()?,
                ip_address: r.read_ip()?,
                port: r.read_u32()?,
                token: r.read_u32()?,
                privileged: r.read_bool()?,
                obfuscation_type: r.read_u32()?,
                obfuscated_port: r.read_u32()?,
            },
            22 => Self::MessageUser {
                message_id: r.read_u32()?,
                timestamp: r.read_u32()?,
                user: r.read_string()?,
                message: r.read_string()?,
                is_new_message: r.read_bool()?,
            },
            26 => Self::FileSearch {
                search_username: r.read_string()?,
                token: r.read_u32()?,
                search_term: r.read_string()?,
            },
            32 => Self::ServerPing,
            33 => Self::SendConnectToken {
                user: r.read_string()?,
                token: r.read_u32()?,
            },
            36 => Self::GetUserStats {
                user: r.read_string()?,
                stats: read_user_stats(r)?,
            },
            40 => Self::UploadSlotsFull {
                user: r.read_string()?,
                slotsfull: r.read_u32()?,
            },
            41 => Self::Relogged,
            50 => Self::SimilarRecommendations {
                recommendation: r.read_string()?,
                similar_recommendations: read_string_list(r)?,
            },
            54 => {
                let (recommendations, unrecommendations) = read_recommendations(r)?;
                Self::Recommendations {
                    recommendations,
                    unrecommendations,
                }
            }
            55 => Self::MyRecommendations {
                my_recommendations: read_string_list(r)?,
            },
            56 => {
                let (recommendations, unrecommendations) = read_recommendations(r)?;
                Self::GlobalRecommendations {
                    recommendations,
                    unrecommendations,
                }
            }
            57 => Self::UserInterests {
                user: r.read_string()?,
                likes: read_string_list(r)?,
                hates: read_string_list(r)?,
            },
            59 => Self::PlaceInLineRequest {
                user: r.read_string()?,
                token: r.read_u32()?,
            },
            60 => Self::PlaceInLineResponse {
                user: r.read_string()?,
                token: r.read_u32()?,
                place: r.read_u32()?,
            },
            62 => Self::RoomAdded {
                room: r.read_string()?,
            },
            63 => Self::RoomRemoved {
                room: r.read_string()?,
            },
            64 => Self::RoomList {
                rooms: read_rooms_with_counts(r)?,
                rooms_owner: read_rooms_with_counts(r)?,
                rooms_member: read_rooms_with_counts(r)?,
                rooms_operator: read_string_list(r)?,
            },
            65 => Self::ExactFileSearch {
                user: r.read_string()?,
                token: r.read_u32()?,
                file: r.read_string()?,
                folder: r.read_string()?,
                size: r.read_u64()?,
                checksum: r.read_u32()?,
            },
            66 => Self::AdminMessage {
                msg: r.read_string()?,
            },
            67 => Self::GlobalUserList {
                users: read_users(r)?,
            },
            68 => Self::TunneledMessage {
                user: r.read_string()?,
                code: r.read_u32()?,
                token: r.read_u32()?,
                ip_address: r.read_ip()?,
                port: r.read_u32()?,
                msg: r.read_string()?,
            },
            69 => Self::PrivilegedUsers {
                users: read_string_list(r)?,
            },
            83 => Self::ParentMinSpeed {
                speed: r.read_u32()?,
            },
            84 => Self::ParentSpeedRatio {
                ratio: r.read_u32()?,
            },
            86 => Self::ParentInactivityTimeout {
                seconds: r.read_u32()?,
            },
            87 => Self::SearchInactivityTimeout {
                seconds: r.read_u32()?,
            },
            88 => Self::MinParentsInCache { num: r.read_u32()? },
            90 => Self::DistribPingInterval {
                seconds: r.read_u32()?,
            },
            91 => Self::AddToPrivileged {
                user: r.read_string()?,
            },
            92 => Self::CheckPrivileges {
                seconds: r.read_u32()?,
            },
            93 => Self::EmbeddedMessage {
                distrib_code: r.read_u8()?,
                distrib_message: r.rest().to_vec(),
            },
            102 => {
                let num = r.read_u32()?;
                let mut parents = Vec::new();
                for _ in 0..num {
                    parents.push(ParentCandidate {
                        username: r.read_string()?,
                        ip_address: r.read_ip()?,
                        port: r.read_u32()?,
                    });
                }
                Self::PossibleParents { parents }
            }
            104 => Self::WishlistInterval {
                seconds: r.read_u32()?,
            },
            110 => {
                let num = r.read_u32()?;
                let mut users = Vec::new();
                for _ in 0..num {
                    users.push(SimilarUser {
                        username: r.read_string()?,
                        rating: r.read_u32()?,
                    });
                }
                users.sort_by_key(|user| std::cmp::Reverse(user.rating));
                Self::SimilarUsers { users }
            }
            111 => {
                let thing = r.read_string()?;
                let (recommendations, unrecommendations) = read_recommendations(r)?;
                Self::ItemRecommendations {
                    thing,
                    recommendations,
                    unrecommendations,
                }
            }
            112 => Self::ItemSimilarUsers {
                thing: r.read_string()?,
                usernames: read_string_list(r)?,
            },
            113 => {
                let room = r.read_string()?;
                let num = r.read_u32()?;
                let mut tickers = Vec::new();
                for _ in 0..num {
                    tickers.push((r.read_string()?, r.read_string()?));
                }
                Self::RoomTickers { room, tickers }
            }
            114 => Self::RoomTickerAdded {
                room: r.read_string()?,
                user: r.read_string()?,
                msg: r.read_string()?,
            },
            115 => Self::RoomTickerRemoved {
                room: r.read_string()?,
                user: r.read_string()?,
            },
            122 => Self::UserPrivileged {
                user: r.read_string()?,
                privileged: r.read_bool()?,
            },
            124 => Self::NotifyPrivileges {
                token: r.read_u32()?,
                user: r.read_string()?,
            },
            125 => Self::AckNotifyPrivileges {
                token: r.read_u32()?,
            },
            130 => Self::ResetDistributed,
            133 => Self::RoomMembers {
                room: r.read_string()?,
                members: read_string_list(r)?,
            },
            134 => Self::AddRoomMember {
                room: r.read_string()?,
                user: r.read_string()?,
            },
            135 => Self::RemoveRoomMember {
                room: r.read_string()?,
                user: r.read_string()?,
            },
            138 => Self::RoomSomething {
                room: r.read_string()?,
            },
            139 => Self::RoomMembershipGranted {
                room: r.read_string()?,
            },
            140 => Self::RoomMembershipRevoked {
                room: r.read_string()?,
            },
            141 => Self::EnableRoomInvitations {
                enabled: r.read_bool()?,
            },
            142 => Self::ChangePassword {
                password: r.read_string()?,
            },
            143 => Self::AddRoomOperator {
                room: r.read_string()?,
                user: r.read_string()?,
            },
            144 => Self::RemoveRoomOperator {
                room: r.read_string()?,
                user: r.read_string()?,
            },
            145 => Self::RoomOperatorshipGranted {
                room: r.read_string()?,
            },
            146 => Self::RoomOperatorshipRevoked {
                room: r.read_string()?,
            },
            148 => Self::RoomOperators {
                room: r.read_string()?,
                operators: read_string_list(r)?,
            },
            152 => Self::GlobalRoomMessage {
                room: r.read_string()?,
                user: r.read_string()?,
                message: r.read_string()?,
            },
            153 => {
                let query = r.read_string()?;
                let num = r.read_u32()?;
                let mut terms = Vec::new();
                for _ in 0..num {
                    terms.push((r.read_string()?, r.read_u32()?));
                }
                Self::RelatedSearch { query, terms }
            }
            160 => Self::ExcludedSearchPhrases {
                phrases: read_string_list(r)?,
            },
            1001 => Self::CantConnectToPeer {
                token: r.read_u32()?,
            },
            1003 => Self::CantCreateRoom {
                room: r.read_string()?,
            },
            _ => {
                return Err(ProtocolError::UnknownMessageCode {
                    family: "server",
                    code,
                });
            }
        })
    }
}
