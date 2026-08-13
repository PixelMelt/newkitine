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

    pub(super) fn place_of(&self, username: &str, virtual_path: &str) -> Option<u32> {
        self.entries
            .iter()
            .position(|key| key.0 == username && key.1 == virtual_path)
            .map(|place| place as u32 + 1)
    }

    pub(super) fn push(&mut self, key: TransferKey) {
        let username = key.0.clone();
        self.entries.push(key);
        self.record_user(&username);
    }

    pub(super) fn select_next(&self, users: &Users) -> Option<TransferKey> {
        let eligible = || {
            self.user_counters
                .iter()
                .filter(|(username, _)| {
                    !matches!(users.restriction(username), Some(Restriction::Hold))
                })
                .filter(|(username, _)| !self.is_active(username))
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
        self.record_user(&key.0);
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
        if has_queued {
            self.counter += 1;
            self.user_counters.insert(username.to_owned(), self.counter);
        } else {
            self.user_counters.remove(username);
        }
    }
}
