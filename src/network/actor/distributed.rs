use std::collections::HashMap;
use std::net::SocketAddrV4;

use tracing::{debug, info};

use super::Actor;
use crate::network::ConnId;
use crate::network::NetworkEvent;
use crate::network::conn::ConnControl;
use crate::protocol::{DistributedMessage, ParentCandidate, ServerRequest};
use crate::types::ConnectionType;

const MAX_DISTRIB_CHILDREN_LIMIT: u32 = 10;

pub(super) struct PotentialParent {
    conn_id: Option<ConnId>,
    branch_level: Option<i32>,
    branch_root: Option<String>,
}

pub(super) struct Distributed {
    parent: Option<String>,
    potential_parents: HashMap<String, PotentialParent>,
    branch_level: u32,
    pub(super) branch_root: Option<String>,
    is_server_parent: bool,
    child_peers: HashMap<String, ConnId>,
    max_distrib_children: u32,
    upload_speed: u32,
    pub(super) parent_min_speed: u32,
    pub(super) parent_speed_ratio: u32,
}

impl Distributed {
    pub(super) fn new() -> Self {
        Self {
            parent: None,
            potential_parents: HashMap::new(),
            branch_level: 0,
            branch_root: None,
            is_server_parent: false,
            child_peers: HashMap::new(),
            max_distrib_children: 0,
            upload_speed: 0,
            parent_min_speed: 0,
            parent_speed_ratio: 1,
        }
    }

    pub(super) fn reset(&mut self) {
        self.parent = None;
        self.potential_parents.clear();
        self.branch_level = 0;
        self.branch_root = None;
        self.is_server_parent = false;
        self.child_peers.clear();
        self.max_distrib_children = 0;
        self.upload_speed = 0;
    }
}

impl Actor {
    pub(super) fn update_own_speed(&mut self, avgspeed: u32) {
        self.distributed.upload_speed = avgspeed;
        self.update_max_distrib_children();
    }

    pub(super) fn handle_possible_parents(&mut self, parents: &[ParentCandidate]) {
        if self.distributed.parent.is_some() {
            return;
        }
        self.close_parent_candidate_connections();
        self.distributed.potential_parents.clear();
        for parent in parents {
            let addr = SocketAddrV4::new(parent.ip_address, parent.port as u16);
            self.distributed.potential_parents.insert(
                parent.username.clone(),
                PotentialParent {
                    conn_id: None,
                    branch_level: None,
                    branch_root: None,
                },
            );
            self.initiate_peer_connection(
                parent.username.clone(),
                ConnectionType::Distributed,
                None,
                Some(addr),
            );
        }
    }

    pub(super) fn handle_reset_distributed(&mut self) {
        if let Some(parent) = self.distributed.parent.take()
            && let Some(init_id) = self
                .indirect
                .existing_attempt(&parent, ConnectionType::Distributed)
            && let Some(conn_id) = self.indirect.get(init_id).and_then(|init| init.conn_id)
        {
            self.close_conn(conn_id);
        }
        let children: Vec<ConnId> = self.distributed.child_peers.values().copied().collect();
        for conn_id in children {
            self.close_conn(conn_id);
        }
        self.send_have_no_parent();
    }

    pub(super) fn handle_embedded_message(&mut self, distrib_code: u8, distrib_message: &[u8]) {
        if self.distributed.parent.is_some() {
            return;
        }
        if let Ok(DistributedMessage::Search {
            identifier,
            search_username,
            token,
            search_term,
        }) = DistributedMessage::parse(distrib_code, distrib_message)
            && identifier == 49
        {
            if !self.distributed.is_server_parent {
                self.distributed.is_server_parent = true;
                self.distributed.branch_level = 0;
                self.distributed.branch_root = self.server.username().map(str::to_owned);
                if (self.distributed.child_peers.len() as u32)
                    < self.distributed.max_distrib_children
                {
                    self.send_to_server(ServerRequest::AcceptChildren { enabled: true });
                }
            }
            let forwarded = DistributedMessage::Search {
                identifier,
                search_username: search_username.clone(),
                token,
                search_term: search_term.clone(),
            };
            self.send_to_child_peers(&forwarded);
            self.emit(NetworkEvent::DistributedSearch {
                username: search_username,
                token,
                search_term,
            });
        }
    }

    pub(super) fn handle_distrib_message(&mut self, conn_id: ConnId, message: DistributedMessage) {
        let Some(conn) = self.peers.get(conn_id) else {
            return;
        };
        let Some(identity) = &conn.identity else {
            tracing::warn!(
                conn_id,
                "distributed message on unidentified connection, closing"
            );
            self.close_conn(conn_id);
            return;
        };
        let username = identity.username.clone();
        match message {
            DistributedMessage::Search {
                identifier,
                search_username,
                token,
                search_term,
            } => {
                if identifier != 49 {
                    return;
                }
                if self.distributed.parent.is_none() {
                    self.adopt_parent(&username);
                }
                if !self.is_parent_conn(conn_id) {
                    if self.distributed.parent.is_some() {
                        self.close_conn(conn_id);
                    }
                    return;
                }
                let forwarded = DistributedMessage::Search {
                    identifier,
                    search_username: search_username.clone(),
                    token,
                    search_term: search_term.clone(),
                };
                self.send_to_child_peers(&forwarded);
                self.emit(NetworkEvent::DistributedSearch {
                    username: search_username,
                    token,
                    search_term,
                });
            }
            DistributedMessage::EmbeddedMessage {
                distrib_code,
                distrib_message,
            } => {
                if let Ok(unpacked) = DistributedMessage::parse(distrib_code, &distrib_message) {
                    self.handle_distrib_message(conn_id, unpacked);
                }
            }
            DistributedMessage::BranchLevel { level } => {
                if level < 0 {
                    self.close_conn(conn_id);
                    return;
                }
                if self.is_parent_conn(conn_id) {
                    self.distributed.branch_level = level as u32 + 1;
                    self.send_to_server(ServerRequest::BranchLevel {
                        value: self.distributed.branch_level,
                    });
                    let forwarded = DistributedMessage::BranchLevel {
                        level: self.distributed.branch_level as i32,
                    };
                    self.send_to_child_peers(&forwarded);
                } else if self.distributed.parent.is_none()
                    && let Some(candidate) = self.distributed.potential_parents.get_mut(&username)
                {
                    candidate.conn_id = Some(conn_id);
                    candidate.branch_level = Some(level);
                    if level == 0 {
                        candidate.branch_root = Some(username);
                    }
                } else if self.distributed.parent.is_some() {
                    self.close_conn(conn_id);
                }
            }
            DistributedMessage::BranchRoot { root_username } => {
                if root_username.is_empty() {
                    self.close_conn(conn_id);
                    return;
                }
                if self.is_parent_conn(conn_id) {
                    self.distributed.branch_root = Some(root_username.clone());
                    self.send_to_server(ServerRequest::BranchRoot {
                        user: root_username.clone(),
                    });
                    let forwarded = DistributedMessage::BranchRoot { root_username };
                    self.send_to_child_peers(&forwarded);
                } else if self.distributed.parent.is_none()
                    && let Some(candidate) = self.distributed.potential_parents.get_mut(&username)
                {
                    candidate.conn_id = Some(conn_id);
                    candidate.branch_root = Some(root_username);
                } else if self.distributed.parent.is_some() {
                    self.close_conn(conn_id);
                }
            }
            DistributedMessage::Ping | DistributedMessage::ChildDepth { .. } => {}
        }
    }

    fn is_parent_conn(&self, conn_id: ConnId) -> bool {
        let Some(parent) = &self.distributed.parent else {
            return false;
        };
        self.peers
            .get(conn_id)
            .and_then(|conn| conn.identity.as_ref())
            .is_some_and(|identity| &identity.username == parent)
    }

    fn adopt_parent(&mut self, username: &str) {
        let Some(candidate) = self.distributed.potential_parents.get_mut(username) else {
            return;
        };
        if candidate.conn_id.is_none() {
            return;
        }
        let (Some(branch_level), Some(branch_root)) =
            (candidate.branch_level.take(), candidate.branch_root.take())
        else {
            return;
        };
        info!(
            username,
            branch_level, branch_root, "adopting distributed parent"
        );
        self.distributed.parent = Some(username.to_owned());
        self.distributed.branch_level = branch_level as u32 + 1;
        self.distributed.branch_root = Some(branch_root.clone());
        self.distributed.is_server_parent = false;

        self.close_parent_candidate_connections();

        self.send_to_server(ServerRequest::HaveNoParent { no_parent: false });
        self.send_to_server(ServerRequest::BranchRoot {
            user: branch_root.clone(),
        });
        self.send_to_server(ServerRequest::BranchLevel {
            value: self.distributed.branch_level,
        });
        if (self.distributed.child_peers.len() as u32) < self.distributed.max_distrib_children {
            self.send_to_server(ServerRequest::AcceptChildren { enabled: true });
        }
        let level = DistributedMessage::BranchLevel {
            level: self.distributed.branch_level as i32,
        };
        let root = DistributedMessage::BranchRoot {
            root_username: branch_root,
        };
        self.send_to_child_peers(&level);
        self.send_to_child_peers(&root);
        self.distributed.child_peers.remove(username);
    }

    fn close_parent_candidate_connections(&mut self) {
        let parent = self.distributed.parent.clone();
        let mut to_close = Vec::new();
        for (username, candidate) in &mut self.distributed.potential_parents {
            if Some(username) == parent.as_ref() {
                continue;
            }
            if let Some(conn_id) = candidate.conn_id.take() {
                to_close.push(conn_id);
            }
        }
        for conn_id in to_close {
            self.close_conn(conn_id);
        }
    }

    pub(super) fn send_have_no_parent(&mut self) {
        self.distributed.parent = None;
        self.distributed.branch_level = 0;
        self.distributed.branch_root = self.server.username().map(str::to_owned);
        self.send_to_server(ServerRequest::HaveNoParent { no_parent: true });
        if let Some(root) = self.distributed.branch_root.clone() {
            self.send_to_server(ServerRequest::BranchRoot { user: root });
        }
        self.send_to_server(ServerRequest::BranchLevel { value: 0 });
        self.send_to_server(ServerRequest::AcceptChildren { enabled: false });
    }

    fn send_to_child_peers(&mut self, message: &DistributedMessage) {
        let bytes = message.to_bytes();
        let children: Vec<ConnId> = self.distributed.child_peers.values().copied().collect();
        for conn_id in children {
            self.push_conn(conn_id, ConnControl::Send(bytes.clone()));
        }
    }

    pub(super) fn accept_child_peer(&mut self, conn_id: ConnId, username: &str) {
        if Some(username) == self.server.username() {
            return;
        }
        if self.distributed.potential_parents.contains_key(username) {
            return;
        }
        let reject = (self.distributed.parent.is_none() && !self.distributed.is_server_parent)
            || self.distributed.child_peers.contains_key(username)
            || self.distributed.child_peers.len() as u32 >= self.distributed.max_distrib_children;
        if reject {
            debug!(username, "rejecting distributed child peer");
            self.close_conn(conn_id);
            return;
        }
        self.distributed
            .child_peers
            .insert(username.to_owned(), conn_id);
        let level = DistributedMessage::BranchLevel {
            level: self.distributed.branch_level as i32,
        };
        let root = DistributedMessage::BranchRoot {
            root_username: self
                .distributed
                .branch_root
                .clone()
                .expect("accepting distributed child without a branch root"),
        };
        self.push_conn(conn_id, ConnControl::Send(level.to_bytes()));
        self.push_conn(conn_id, ConnControl::Send(root.to_bytes()));
        if self.distributed.child_peers.len() as u32 >= self.distributed.max_distrib_children {
            self.send_to_server(ServerRequest::AcceptChildren { enabled: false });
        }
    }

    pub(super) fn update_max_distrib_children(&mut self) {
        let previous = self.distributed.max_distrib_children;
        let num_children = self.distributed.child_peers.len() as u32;
        if self.distributed.upload_speed >= self.distributed.parent_min_speed
            && self.distributed.parent_speed_ratio > 0
        {
            self.distributed.max_distrib_children =
                (self.distributed.upload_speed / self.distributed.parent_speed_ratio / 100)
                    .min(MAX_DISTRIB_CHILDREN_LIMIT);
        } else {
            self.distributed.max_distrib_children = 0;
        }
        if self.distributed.max_distrib_children <= num_children && num_children < previous {
            self.send_to_server(ServerRequest::AcceptChildren { enabled: false });
        }
    }

    pub(super) fn handle_distributed_conn_closed(&mut self, username: &str, conn_id: ConnId) {
        if self.distributed.parent.as_deref() == Some(username) {
            self.distributed.parent = None;
            self.send_have_no_parent();
        }
        if self.distributed.child_peers.get(username) == Some(&conn_id) {
            self.distributed.child_peers.remove(username);
            if self.distributed.child_peers.len() as u32 + 1
                == self.distributed.max_distrib_children
            {
                self.send_to_server(ServerRequest::AcceptChildren { enabled: true });
            }
        }
        if let Some(candidate) = self.distributed.potential_parents.get_mut(username)
            && candidate.conn_id == Some(conn_id)
        {
            candidate.conn_id = None;
        }
    }
}
