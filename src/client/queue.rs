use std::collections::{HashMap, HashSet};

use crate::types::Restriction;

use super::users::Users;

pub(super) struct UploadQueue {
    entries: Vec<(String, String)>,
    active_users: HashMap<String, HashSet<u32>>,
    user_counters: HashMap<String, u64>,
    counter: u64,
    max_per_user: usize,
}

impl UploadQueue {
    pub(super) fn new(max_per_user: usize) -> Self {
        Self {
            entries: Vec::new(),
            active_users: HashMap::new(),
            user_counters: HashMap::new(),
            counter: 0,
            max_per_user,
        }
    }

    pub(super) fn set_max_per_user(&mut self, max_per_user: usize) {
        self.max_per_user = max_per_user;
    }

    pub(super) fn active_count(&self, username: &str) -> usize {
        self.active_users.get(username).map_or(0, HashSet::len)
    }

    fn under_per_user_cap(&self, username: &str) -> bool {
        self.max_per_user == 0 || self.active_count(username) < self.max_per_user
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

    #[cfg(test)]
    pub(super) fn has_active(&self, username: &str) -> bool {
        self.active_users.contains_key(username)
    }

    pub(super) fn push(&mut self, key: (String, String)) {
        let username = key.0.clone();
        self.entries.push(key);
        self.record_user(&username);
    }

    pub(super) fn select_next(&self, users: &Users) -> Option<(String, String)> {
        let eligible: Vec<&String> = self
            .user_counters
            .keys()
            .filter(|username| !matches!(users.restriction(username), Some(Restriction::Hold)))
            .filter(|username| self.under_per_user_cap(username))
            .collect();
        let privileged: Vec<&&String> = eligible
            .iter()
            .filter(|username| users.is_privileged(username))
            .collect();
        let normal: Vec<&&String> = eligible
            .iter()
            .filter(|username| {
                !matches!(
                    users.restriction(username),
                    Some(Restriction::Deprioritized)
                )
            })
            .collect();
        let tier: Vec<&String> = if !privileged.is_empty() {
            privileged.into_iter().copied().collect()
        } else if !normal.is_empty() {
            normal.into_iter().copied().collect()
        } else {
            eligible
        };
        tier.into_iter()
            .min_by_key(|username| self.user_counters[*username])
            .cloned()
            .and_then(|username| {
                self.entries
                    .iter()
                    .find(|(user, _)| *user == username)
                    .cloned()
            })
    }

    pub(super) fn mark_active(&mut self, key: &(String, String), token: u32) {
        self.entries.retain(|queued| queued != key);
        self.active_users
            .entry(key.0.clone())
            .or_default()
            .insert(token);
        self.record_user(&key.0);
    }

    pub(super) fn release(&mut self, key: &(String, String), token: Option<u32>) {
        if let Some(token) = token
            && let Some(tokens) = self.active_users.get_mut(&key.0)
        {
            tokens.remove(&token);
            if tokens.is_empty() {
                self.active_users.remove(&key.0);
            }
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
