use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;

use crate::network::NetworkHandle;
use crate::protocol::ServerRequest;
use crate::types::{Restriction, UserStats, UserStatus};

#[derive(Debug, Clone, Default)]
pub struct WatchedUser {
    pub status: Option<UserStatus>,
    pub stats: Option<UserStats>,
    pub privileged: bool,
}

pub struct Users {
    pub buddies: HashSet<String>,
    pub banned: HashSet<String>,
    pub ignored: HashSet<String>,
    ip_bans: Vec<String>,
    restrictions: HashMap<String, Restriction>,
    watched: HashMap<String, WatchedUser>,
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
            watched: HashMap::new(),
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
            || self
                .watched
                .get(username)
                .is_some_and(|user| user.privileged)
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

    pub fn handle_watch_user(
        &mut self,
        username: &str,
        user_exists: bool,
        status: Option<u32>,
        stats: Option<UserStats>,
    ) {
        if !user_exists {
            self.watched.remove(username);
            return;
        }
        let user = self.watched.entry(username.to_owned()).or_default();
        if let Some(status) = status {
            user.status = UserStatus::from_u32(status);
        }
        if let Some(stats) = stats {
            user.stats = Some(stats);
        }
    }

    pub fn handle_user_status(&mut self, username: &str, status: u32, privileged: bool) {
        if let Some(user) = self.watched.get_mut(username) {
            user.status = UserStatus::from_u32(status);
            user.privileged = privileged;
        }
        if privileged {
            self.privileged.insert(username.to_owned());
        }
    }

    pub fn handle_user_stats(&mut self, username: &str, stats: UserStats) {
        if let Some(user) = self.watched.get_mut(username) {
            user.stats = Some(stats);
        }
    }

    pub fn handle_privileged_users(&mut self, users: Vec<String>) {
        self.privileged = users.into_iter().collect();
    }

    pub fn reset(&mut self) {
        self.watched.clear();
        self.privileged.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
