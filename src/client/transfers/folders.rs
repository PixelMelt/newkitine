use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::network::{NetworkCommand, NetworkHandle};
use crate::protocol::PeerMessage;
use crate::types::{FileInfo, FolderContents};

const FOLDER_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const SHORT_FOLDER_NAME_CHARS: usize = 12;

type FolderKey = (String, String);

pub(in crate::client) fn destination_root(directory: &str) -> &str {
    let (parent, basename) = directory.rsplit_once('\\').unwrap_or(("", directory));
    if !parent.is_empty() && basename.chars().count() < SHORT_FOLDER_NAME_CHARS {
        return parent;
    }
    directory
}

struct RequestedFolder {
    requested_at: Instant,
    has_retried: bool,
    legacy_attempt: bool,
}

pub(in crate::client) struct FolderRequests {
    net: NetworkHandle,
    requests: HashMap<FolderKey, RequestedFolder>,
    token: u32,
}

impl FolderRequests {
    pub fn new(net: NetworkHandle) -> Self {
        Self {
            net,
            requests: HashMap::new(),
            token: 0,
        }
    }

    pub fn request(&mut self, username: String, directory: String) {
        let key = (username, directory);
        let request = self
            .requests
            .entry(key.clone())
            .or_insert_with(|| RequestedFolder {
                requested_at: Instant::now(),
                has_retried: false,
                legacy_attempt: false,
            });
        request.requested_at = Instant::now();
        let legacy_client = request.legacy_attempt;
        self.send(key, legacy_client);
    }

    fn send(&mut self, key: FolderKey, legacy_client: bool) {
        let (username, directory) = key;
        self.token = self.token.wrapping_add(1);
        info!(username, directory, "requesting folder contents");
        self.net.send(NetworkCommand::AllowFolderContents {
            username: username.clone(),
            directory: directory.clone(),
        });
        self.net.peer(
            username,
            PeerMessage::FolderContentsRequest {
                token: self.token,
                directory,
                legacy_client,
            },
        );
    }

    pub fn accept(
        &mut self,
        username: &str,
        directory: &str,
        folders: Vec<FolderContents>,
    ) -> Option<Vec<FileInfo>> {
        let key = (username.to_owned(), directory.to_owned());
        let request = self.requests.get_mut(&key)?;
        request.requested_at = Instant::now();
        if folders.is_empty() && !request.legacy_attempt {
            debug!(
                username,
                directory, "folder contents response is empty, retrying as a legacy client"
            );
            request.legacy_attempt = true;
            self.send(key, true);
            return None;
        }
        self.forget(&key);
        Some(
            folders
                .into_iter()
                .filter(|folder| folder.directory == directory)
                .flat_map(|folder| folder.files)
                .collect(),
        )
    }

    pub fn sweep(&mut self) -> Vec<FolderKey> {
        let expired: Vec<FolderKey> = self
            .requests
            .iter()
            .filter(|(_, request)| request.requested_at.elapsed() >= FOLDER_REQUEST_TIMEOUT)
            .map(|(key, _)| key.clone())
            .collect();
        expired
            .into_iter()
            .filter_map(|key| self.time_out(key))
            .collect()
    }

    pub fn time_out(&mut self, key: FolderKey) -> Option<FolderKey> {
        let request = self.requests.get_mut(&key)?;
        if request.has_retried {
            info!(
                username = key.0,
                directory = key.1,
                "folder contents request timed out, giving up"
            );
            self.forget(&key);
            return Some(key);
        }
        info!(
            username = key.0,
            directory = key.1,
            "folder contents request timed out, retrying"
        );
        request.has_retried = true;
        request.requested_at = Instant::now();
        let legacy_client = request.legacy_attempt;
        self.send(key, legacy_client);
        None
    }

    fn forget(&mut self, key: &FolderKey) {
        self.requests.remove(key);
        self.net.send(NetworkCommand::DisallowFolderContents {
            username: key.0.clone(),
            directory: key.1.clone(),
        });
    }

    pub fn clear(&mut self) {
        for key in self.requests.keys() {
            self.net.send(NetworkCommand::DisallowFolderContents {
                username: key.0.clone(),
                directory: key.1.clone(),
            });
        }
        self.requests.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_folder_names_keep_their_parent() {
        assert_eq!(destination_root("Music\\Artist\\CD1"), "Music\\Artist");
        assert_eq!(destination_root("Music\\Artist\\Disc 2"), "Music\\Artist");
        assert_eq!(
            destination_root("Music\\Artist\\A Long Album Name"),
            "Music\\Artist\\A Long Album Name"
        );
        assert_eq!(destination_root("CD1"), "CD1");
    }
}
