use std::collections::{HashMap, HashSet, VecDeque};
use std::net::Ipv4Addr;
use std::time::Instant;

use crate::network::NetworkHandle;
use crate::protocol::ServerRequest;
use crate::types::{Restriction, UserStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Presence {
    WentOffline,
    CameOnline,
    Unchanged,
}

const WATCH_BATCH: usize = 100;

fn request_watch(net: &NetworkHandle, username: &str) {
    net.server(ServerRequest::WatchUser {
        user: username.to_owned(),
    });
    net.server(ServerRequest::GetUserStatus {
        user: username.to_owned(),
    });
}

pub(super) struct Users {
    pub buddies: HashSet<String>,
    pub banned: HashSet<String>,
    pub ignored: HashSet<String>,
    ip_bans: Vec<String>,
    restrictions: HashMap<String, Restriction>,
    file_denials: HashMap<String, HashMap<String, Instant>>,
    privileged: HashSet<String>,
    watched: HashMap<String, Option<UserStatus>>,
    unsent_watches: VecDeque<String>,
}

impl Users {
    pub fn new(
        buddies: HashSet<String>,
        banned: HashSet<String>,
        ignored: HashSet<String>,
        ip_bans: Vec<String>,
    ) -> Self {
        Self {
            buddies,
            banned,
            ignored,
            ip_bans,
            restrictions: HashMap::new(),
            file_denials: HashMap::new(),
            privileged: HashSet::new(),
            watched: HashMap::new(),
            unsent_watches: VecDeque::new(),
        }
    }

    pub fn set_ip_bans(&mut self, patterns: Vec<String>) {
        self.ip_bans = patterns;
    }

    pub fn is_ip_banned(&self, ip: Ipv4Addr) -> bool {
        let octets = ip.octets();
        self.ip_bans.iter().any(|pattern| {
            let mut parts = pattern.split('.');
            let matched = octets.iter().all(|octet| {
                parts
                    .next()
                    .is_some_and(|part| part == "*" || part == octet.to_string())
            });
            matched && parts.next().is_none()
        })
    }

    pub fn set_restriction(&mut self, username: String, restriction: Restriction) {
        match restriction {
            Restriction::None => {
                self.restrictions.remove(&username);
            }
            restriction => {
                self.restrictions.insert(username, restriction);
            }
        }
    }

    pub fn restriction(&self, username: &str) -> Option<&Restriction> {
        self.restrictions.get(username)
    }

    pub fn deny_file(&mut self, username: String, virtual_path: String, until: Instant) {
        let now = Instant::now();
        self.file_denials.retain(|_, files| {
            files.retain(|_, expiry| *expiry > now);
            !files.is_empty()
        });
        self.file_denials
            .entry(username)
            .or_default()
            .insert(virtual_path, until);
    }

    pub fn clear_file_denials(&mut self, username: &str) {
        self.file_denials.remove(username);
    }

    pub fn is_file_denied(&self, username: &str, virtual_path: &str) -> bool {
        self.file_denials
            .get(username)
            .and_then(|files| files.get(virtual_path))
            .is_some_and(|until| *until > Instant::now())
    }

    pub fn is_buddy(&self, username: &str) -> bool {
        self.buddies.contains(username)
    }

    pub fn is_banned(&self, username: &str) -> bool {
        self.banned.contains(username)
    }

    pub fn is_ignored(&self, username: &str) -> bool {
        self.ignored.contains(username)
    }

    pub fn is_privileged(&self, username: &str) -> bool {
        self.privileged.contains(username)
    }

    pub fn start_session(&mut self, own_username: &str) {
        self.watched.clear();
        self.unsent_watches.clear();
        self.watch(own_username);
        let buddies: Vec<String> = self.buddies.iter().cloned().collect();
        for buddy in &buddies {
            self.watch(buddy);
        }
    }

    pub fn watch(&mut self, username: &str) {
        if self.watched.contains_key(username) {
            return;
        }
        self.watched.insert(username.to_owned(), None);
        self.unsent_watches.push_back(username.to_owned());
    }

    pub fn send_watches(&mut self, net: &NetworkHandle) {
        let mut sent = 0;
        while sent < WATCH_BATCH {
            let Some(username) = self.unsent_watches.pop_front() else {
                break;
            };
            if self.watched.contains_key(&username) {
                request_watch(net, &username);
                sent += 1;
            }
        }
    }

    pub fn forget_watch(&mut self, username: &str) {
        self.watched.remove(username);
    }

    pub fn add_buddy(&mut self, net: &NetworkHandle, username: String) {
        if !self.buddies.insert(username.clone()) {
            return;
        }
        self.watched.entry(username.clone()).or_insert(None);
        request_watch(net, &username);
    }

    pub fn remove_buddy(&mut self, net: &NetworkHandle, username: &str, keep_watch: bool) {
        if self.buddies.remove(username) && !keep_watch && self.watched.remove(username).is_some() {
            net.server(ServerRequest::UnwatchUser {
                user: username.to_owned(),
            });
        }
    }

    pub fn set_privileged(&mut self, username: &str, privileged: bool) {
        if privileged {
            self.privileged.insert(username.to_owned());
        } else {
            self.privileged.remove(username);
        }
    }

    pub fn handle_user_status(
        &mut self,
        net: &NetworkHandle,
        username: &str,
        status: Option<UserStatus>,
        privileged: bool,
    ) -> Presence {
        self.set_privileged(username, privileged);
        let previous = self
            .watched
            .get_mut(username)
            .map(|known| std::mem::replace(known, status));
        let online = |status: Option<UserStatus>| {
            matches!(status, Some(UserStatus::Online | UserStatus::Away))
        };
        if status == Some(UserStatus::Offline) {
            return Presence::WentOffline;
        }
        match previous {
            Some(previous) if online(status) && !online(previous) => {
                if previous == Some(UserStatus::Offline) {
                    net.server(ServerRequest::GetUserStats {
                        user: username.to_owned(),
                    });
                }
                Presence::CameOnline
            }
            _ => Presence::Unchanged,
        }
    }

    pub fn handle_privileged_users(&mut self, users: Vec<String>) {
        self.privileged = users.into_iter().collect();
    }

    pub fn reset(&mut self) {
        self.privileged.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_denials_expire_and_clear() {
        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        let later = Instant::now() + std::time::Duration::from_secs(60);
        users.deny_file("peer".into(), "a\\b.mp3".into(), later);
        users.deny_file("peer".into(), "a\\c.mp3".into(), Instant::now());
        assert!(users.is_file_denied("peer", "a\\b.mp3"));
        assert!(!users.is_file_denied("peer", "a\\c.mp3"));
        assert!(!users.is_file_denied("other", "a\\b.mp3"));
        users.clear_file_denials("peer");
        assert!(!users.is_file_denied("peer", "a\\b.mp3"));
    }

    #[test]
    fn ip_ban_patterns() {
        let users = Users::new(
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
            vec!["1.2.3.4".into(), "10.0.*.*".into(), "192.168.1".into()],
        );
        assert!(users.is_ip_banned(Ipv4Addr::new(1, 2, 3, 4)));
        assert!(users.is_ip_banned(Ipv4Addr::new(10, 0, 99, 1)));
        assert!(!users.is_ip_banned(Ipv4Addr::new(1, 2, 3, 5)));
        assert!(!users.is_ip_banned(Ipv4Addr::new(10, 1, 0, 0)));
        assert!(!users.is_ip_banned(Ipv4Addr::new(192, 168, 1, 1)));
    }

    #[test]
    fn privilege_revokes_on_false_status() {
        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.set_privileged("peer", true);
        assert!(users.is_privileged("peer"));
        users.set_privileged("peer", false);
        assert!(!users.is_privileged("peer"));
        users.handle_privileged_users(vec!["peer".into()]);
        assert!(users.is_privileged("peer"));
        users.handle_privileged_users(Vec::new());
        assert!(!users.is_privileged("peer"));
    }

    fn sent_requests(
        commands: &mut tokio::sync::mpsc::Receiver<crate::network::NetworkCommand>,
    ) -> Vec<ServerRequest> {
        let mut sent = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let crate::network::NetworkCommand::SendServerMessage(request) = command {
                sent.push(request);
            }
        }
        sent
    }

    #[test]
    fn removing_a_buddy_keeps_a_watch_downloads_still_need() {
        let (net, mut commands) = crate::network::test_channel();
        let mut users = Users::new(
            ["kept".to_owned(), "dropped".to_owned()].into(),
            HashSet::new(),
            HashSet::new(),
            Vec::new(),
        );
        users.start_session("me");
        users.send_watches(&net);
        sent_requests(&mut commands);
        users.remove_buddy(&net, "kept", true);
        users.remove_buddy(&net, "dropped", false);
        assert_eq!(
            sent_requests(&mut commands),
            vec![ServerRequest::UnwatchUser {
                user: "dropped".into()
            }]
        );
        users.watch("kept");
        users.send_watches(&net);
        assert!(sent_requests(&mut commands).is_empty());
    }

    #[test]
    fn watch_requests_are_paced_and_skip_dropped_users() {
        let (net, mut commands) = crate::network::test_channel();
        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        for index in 0..WATCH_BATCH + 10 {
            users.watch(&format!("peer{index}"));
        }
        users.watch("peer0");
        users.forget_watch("peer1");
        users.send_watches(&net);
        assert_eq!(sent_requests(&mut commands).len(), 2 * WATCH_BATCH);
        users.send_watches(&net);
        assert_eq!(sent_requests(&mut commands).len(), 2 * 9);
    }

    #[test]
    fn befriending_a_watched_uploader_refreshes_its_details() {
        let (net, mut commands) = crate::network::test_channel();
        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.watch("uploader");
        users.send_watches(&net);
        sent_requests(&mut commands);
        users.add_buddy(&net, "uploader".into());
        assert_eq!(
            sent_requests(&mut commands),
            vec![
                ServerRequest::WatchUser {
                    user: "uploader".into()
                },
                ServerRequest::GetUserStatus {
                    user: "uploader".into()
                },
            ]
        );
        users.add_buddy(&net, "uploader".into());
        assert!(sent_requests(&mut commands).is_empty());
    }

    #[test]
    fn a_new_session_rewatches_and_asks_for_privilege_status() {
        let (net, mut commands) = crate::network::test_channel();
        let mut users = Users::new(
            ["buddy".to_owned()].into(),
            HashSet::new(),
            HashSet::new(),
            Vec::new(),
        );
        users.start_session("me");
        users.send_watches(&net);
        users.start_session("me");
        users.send_watches(&net);
        let sent = sent_requests(&mut commands);
        let buddy_requests = sent
            .iter()
            .filter(|request| {
                matches!(
                    request,
                    ServerRequest::WatchUser { user } | ServerRequest::GetUserStatus { user }
                        if user == "buddy"
                )
            })
            .count();
        assert_eq!(buddy_requests, 4);
    }

    #[test]
    fn returning_watched_users_come_online_once_and_refresh_their_stats() {
        let (net, mut commands) = crate::network::test_channel();
        let mut users = Users::new(HashSet::new(), HashSet::new(), HashSet::new(), Vec::new());
        users.watch("peer");
        users.send_watches(&net);
        sent_requests(&mut commands);
        let online = Some(UserStatus::Online);
        let away = Some(UserStatus::Away);
        let offline = Some(UserStatus::Offline);
        assert_eq!(
            users.handle_user_status(&net, "peer", online, false),
            Presence::CameOnline
        );
        assert!(sent_requests(&mut commands).is_empty());
        assert_eq!(
            users.handle_user_status(&net, "peer", away, true),
            Presence::Unchanged
        );
        assert!(users.is_privileged("peer"));
        assert_eq!(
            users.handle_user_status(&net, "peer", offline, false),
            Presence::WentOffline
        );
        assert_eq!(
            users.handle_user_status(&net, "peer", away, false),
            Presence::CameOnline
        );
        assert_eq!(
            sent_requests(&mut commands),
            vec![ServerRequest::GetUserStats {
                user: "peer".into()
            }]
        );
        assert_eq!(
            users.handle_user_status(&net, "stranger", online, false),
            Presence::Unchanged
        );
    }
}
