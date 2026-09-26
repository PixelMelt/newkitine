use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::time::Instant;

use crate::network::NetworkHandle;
use crate::protocol::ServerRequest;
use crate::types::Restriction;

pub(super) struct Users {
    pub buddies: HashSet<String>,
    pub banned: HashSet<String>,
    pub ignored: HashSet<String>,
    ip_bans: Vec<String>,
    restrictions: HashMap<String, Restriction>,
    file_denials: HashMap<String, HashMap<String, Instant>>,
    privileged: HashSet<String>,
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

    pub fn watch_buddies(&self, net: &NetworkHandle) {
        for buddy in &self.buddies {
            net.server(ServerRequest::WatchUser {
                user: buddy.clone(),
            });
        }
    }

    pub fn add_buddy(&mut self, net: &NetworkHandle, username: String) {
        if self.buddies.insert(username.clone()) {
            net.server(ServerRequest::WatchUser { user: username });
        }
    }

    pub fn remove_buddy(&mut self, net: &NetworkHandle, username: &str) {
        if self.buddies.remove(username) {
            net.server(ServerRequest::UnwatchUser {
                user: username.to_owned(),
            });
        }
    }

    pub fn handle_user_status(&mut self, username: &str, privileged: bool) {
        if privileged {
            self.privileged.insert(username.to_owned());
        } else {
            self.privileged.remove(username);
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
        users.handle_user_status("peer", true);
        assert!(users.is_privileged("peer"));
        users.handle_user_status("peer", false);
        assert!(!users.is_privileged("peer"));
        users.handle_privileged_users(vec!["peer".into()]);
        assert!(users.is_privileged("peer"));
        users.handle_privileged_users(Vec::new());
        assert!(!users.is_privileged("peer"));
    }
}
