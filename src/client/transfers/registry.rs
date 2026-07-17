use std::collections::HashMap;

use crate::network::ConnId;
use crate::types::TransferId;

pub(super) type TransferKey = (String, String);

pub(super) struct Detached {
    pub(super) token: Option<u32>,
    pub(super) conn_id: Option<ConnId>,
}

struct Entry<T> {
    id: TransferId,
    token: Option<u32>,
    conn_id: Option<ConnId>,
    transfer: T,
}

pub(super) struct Registry<T> {
    entries: HashMap<TransferKey, Entry<T>>,
    ids: HashMap<TransferId, TransferKey>,
    tokens: HashMap<(String, u32), TransferKey>,
    conns: HashMap<ConnId, TransferKey>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            ids: HashMap::new(),
            tokens: HashMap::new(),
            conns: HashMap::new(),
        }
    }
}

impl<T> Registry<T> {
    pub(super) fn insert(&mut self, id: TransferId, key: TransferKey, transfer: T) {
        if let Some(previous) = self.entries.get_mut(&key) {
            if let Some(token) = previous.token.take() {
                self.tokens.remove(&(key.0.clone(), token));
            }
            if let Some(conn_id) = previous.conn_id.take() {
                self.conns.remove(&conn_id);
            }
            self.ids.remove(&previous.id);
        }
        self.ids.insert(id, key.clone());
        self.entries.insert(
            key,
            Entry {
                id,
                token: None,
                conn_id: None,
                transfer,
            },
        );
    }

    pub(super) fn get(&self, key: &TransferKey) -> Option<&T> {
        self.entries.get(key).map(|entry| &entry.transfer)
    }

    pub(super) fn get_mut(&mut self, key: &TransferKey) -> Option<&mut T> {
        self.entries.get_mut(key).map(|entry| &mut entry.transfer)
    }

    pub(super) fn key_of(&self, id: TransferId) -> Option<&TransferKey> {
        self.ids.get(&id)
    }

    pub(super) fn key_by_token(&self, username: &str, token: u32) -> Option<&TransferKey> {
        self.tokens.get(&(username.to_owned(), token))
    }

    pub(super) fn key_by_conn(&self, conn_id: ConnId) -> Option<&TransferKey> {
        self.conns.get(&conn_id)
    }

    pub(super) fn owns_token(&self, username: &str, token: u32) -> bool {
        self.tokens.contains_key(&(username.to_owned(), token))
    }

    pub(super) fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub(super) fn conn_of(&self, key: &TransferKey) -> Option<ConnId> {
        self.entries.get(key).and_then(|entry| entry.conn_id)
    }

    pub(super) fn attach_token(&mut self, key: &TransferKey, token: u32) {
        let entry = self
            .entries
            .get_mut(key)
            .expect("attach token to unknown transfer");
        if let Some(previous) = entry.token.replace(token) {
            self.tokens.remove(&(key.0.clone(), previous));
        }
        self.tokens.insert((key.0.clone(), token), key.clone());
    }

    pub(super) fn attach_conn(&mut self, key: &TransferKey, conn_id: ConnId) {
        let entry = self
            .entries
            .get_mut(key)
            .expect("attach connection to unknown transfer");
        if let Some(previous) = entry.conn_id.replace(conn_id) {
            self.conns.remove(&previous);
        }
        self.conns.insert(conn_id, key.clone());
    }

    pub(super) fn detach_token(&mut self, key: &TransferKey) -> Option<u32> {
        let token = self.entries.get_mut(key)?.token.take()?;
        self.tokens.remove(&(key.0.clone(), token));
        Some(token)
    }

    pub(super) fn detach_conn(&mut self, key: &TransferKey) -> Option<ConnId> {
        let conn_id = self.entries.get_mut(key)?.conn_id.take()?;
        self.conns.remove(&conn_id);
        Some(conn_id)
    }

    pub(super) fn detach(&mut self, key: &TransferKey) -> Detached {
        Detached {
            token: self.detach_token(key),
            conn_id: self.detach_conn(key),
        }
    }

    pub(super) fn remove(&mut self, id: TransferId) -> Option<(TransferKey, Detached)> {
        let key = self.ids.remove(&id)?;
        let detached = self.detach(&key);
        self.entries.remove(&key);
        Some((key, detached))
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &T> {
        self.entries.values().map(|entry| &entry.transfer)
    }
}
