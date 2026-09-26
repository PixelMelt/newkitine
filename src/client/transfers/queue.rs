use std::collections::HashMap;

use crate::types::Restriction;

use super::registry::TransferKey;
use crate::client::users::Users;

pub(super) struct UploadQueue {
    entries: Vec<TransferKey>,
    active_users: HashMap<String, u32>,
    user_counters: HashMap<String, u64>,
    counter: u64,
}

impl UploadQueue {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            active_users: HashMap::new(),
            user_counters: HashMap::new(),
            counter: 0,
        }
    }

    pub(super) fn is_active(&self, username: &str) -> bool {
        self.active_users.contains_key(username)
    }

    pub(super) fn active_user_count(&self) -> usize {
        self.active_users.len()
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn queued_for(&self, username: &str) -> usize {
        self.entries
            .iter()
            .filter(|(user, _)| user == username)
            .count()
    }

    pub(super) fn place_of(
        &self,
        username: &str,
        virtual_path: &str,
        users: &Users,
    ) -> Option<u32> {
        let own = self
            .entries
            .iter()
            .filter(|(user, _)| user == username)
            .position(|(_, path)| path == virtual_path)? as u32
            + 1;
        let mut queued: HashMap<&str, u32> = HashMap::new();
        for (user, _) in &self.entries {
            *queued.entry(user).or_default() += 1;
        }
        let privileged = queued.iter().filter(|(user, _)| users.is_privileged(user));
        let ahead = if users.is_privileged(username) {
            privileged.count() as u32
        } else {
            privileged.map(|(_, count)| count).sum::<u32>() + queued.len() as u32
        };
        Some(ahead + own)
    }

    pub(super) fn push(&mut self, key: TransferKey) {
        let username = key.0.clone();
        self.entries.push(key);
        if !self.is_active(&username) && !self.user_counters.contains_key(&username) {
            self.advance(username);
        }
    }

    pub(super) fn select_next(&self, users: &Users) -> Option<TransferKey> {
        let eligible = || {
            self.user_counters.iter().filter(|(username, _)| {
                !matches!(users.restriction(username), Some(Restriction::Hold))
            })
        };
        eligible()
            .filter(|(username, _)| users.is_privileged(username))
            .min_by_key(|(_, counter)| *counter)
            .or_else(|| eligible().min_by_key(|(_, counter)| *counter))
            .and_then(|(username, _)| {
                self.entries
                    .iter()
                    .find(|(user, _)| user == username)
                    .cloned()
            })
    }

    pub(super) fn mark_active(&mut self, key: &TransferKey, token: u32) {
        self.entries.retain(|queued| queued != key);
        self.active_users.insert(key.0.clone(), token);
        self.user_counters.remove(&key.0);
    }

    pub(super) fn release(&mut self, key: &TransferKey, token: Option<u32>) {
        if let Some(token) = token
            && self.active_users.get(&key.0) == Some(&token)
        {
            self.active_users.remove(&key.0);
        }
        self.entries.retain(|queued| queued != key);
        self.record_user(&key.0);
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.active_users.clear();
        self.user_counters.clear();
    }

    fn record_user(&mut self, username: &str) {
        let has_queued = self.entries.iter().any(|(user, _)| user == username);
        if !has_queued {
            self.user_counters.remove(username);
        } else if !self.is_active(username) {
            self.advance(username.to_owned());
        }
    }

    fn advance(&mut self, username: String) {
        self.counter += 1;
        self.user_counters.insert(username, self.counter);
    }
}
