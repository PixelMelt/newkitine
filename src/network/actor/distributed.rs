use std::collections::HashMap;
use std::net::SocketAddrV4;

use tracing::{debug, info};

use super::Actor;
use crate::network::ConnId;
use crate::network::NetworkEvent;
use crate::network::conn::ConnControl;
use crate::protocol::{DistributedMessage, DistributedSearch, ParentCandidate, ServerRequest};
use crate::types::ConnectionType;

const MAX_DISTRIB_CHILDREN_LIMIT: u32 = 10;
const SEARCH_IDENTIFIER: u32 = 49;

pub(super) struct PotentialParent {
    conn_id: Option<ConnId>,
    branch_level: Option<i32>,
    branch_root: Option<String>,
}

pub(super) struct Distributed {
    parent: Option<ConnId>,
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
            let Some(port) = u16::try_from(parent.port).ok().filter(|&port| port != 0) else {
                debug!(
                    username = parent.username,
                    port = parent.port,
                    "skipping parent candidate, invalid port"
                );
                continue;
            };
            let addr = SocketAddrV4::new(parent.ip_address, port);
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
                Vec::new(),
                Some(addr),
            );
        }
    }

    pub(super) fn handle_reset_distributed(&mut self) {
        if let Some(parent) = self.distributed.parent.take() {
            self.close_conn(parent);
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
        if distrib_code != DistributedSearch::CODE {
            debug!(
                distrib_code,
                "ignoring embedded distributed message that is not a search"
            );
            return;
        }
        let search = match DistributedSearch::parse(distrib_message) {
            Ok(search) => search,
            Err(error) => {
                debug!(%error, "dropping unparsable embedded distributed search");
                return;
            }
        };
        if search.identifier != SEARCH_IDENTIFIER {
            return;
        }
        if !self.distributed.is_server_parent {
            self.distributed.is_server_parent = true;
            self.distributed.branch_level = 0;
            self.distributed.branch_root = self.server.username().map(str::to_owned);
            if (self.distributed.child_peers.len() as u32) < self.distributed.max_distrib_children {
                self.send_to_server(ServerRequest::AcceptChildren { enabled: true });
            }
        }
        self.forward_search(search);
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
            DistributedMessage::Search(search) => {
                if search.identifier != SEARCH_IDENTIFIER {
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
                self.forward_search(search);
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
                    self.send_to_child_peers(forwarded.to_bytes());
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
                    self.send_to_child_peers(forwarded.to_bytes());
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

    fn forward_search(&mut self, search: DistributedSearch) {
        self.send_to_child_peers(search.to_bytes());
        self.emit(NetworkEvent::DistributedSearch {
            username: search.search_username,
            token: search.token,
            search_term: search.search_term,
        });
    }

    fn is_parent_conn(&self, conn_id: ConnId) -> bool {
        self.distributed.parent == Some(conn_id)
    }

    fn adopt_parent(&mut self, username: &str) {
        let Some(candidate) = self.distributed.potential_parents.get_mut(username) else {
            return;
        };
        let (Some(parent), Some(branch_level), Some(branch_root)) = (
            candidate.conn_id,
            candidate.branch_level,
            candidate.branch_root.clone(),
        ) else {
            return;
        };
        candidate.branch_level = None;
        candidate.branch_root = None;
        info!(
            username,
            branch_level, branch_root, "adopting distributed parent"
        );
        self.distributed.parent = Some(parent);
        self.distributed.branch_level = branch_level as u32 + 1;
        self.distributed.branch_root = Some(branch_root.clone());
        self.distributed.is_server_parent = false;

        self.close_parent_candidate_connections();
        if let Some(former_child) = self.distributed.child_peers.remove(username)
            && former_child != parent
        {
            self.close_conn(former_child);
        }

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
        self.send_to_child_peers(level.to_bytes());
        self.send_to_child_peers(root.to_bytes());
    }

    fn close_parent_candidate_connections(&mut self) {
        let parent = self.distributed.parent;
        let mut to_close = Vec::new();
        for candidate in self.distributed.potential_parents.values_mut() {
            if let Some(conn_id) = candidate.conn_id
                && Some(conn_id) != parent
            {
                candidate.conn_id = None;
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

    fn send_to_child_peers(&mut self, bytes: Vec<u8>) {
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
        if self.is_parent_conn(conn_id) {
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

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::super::peers::{Conn, PeerIdentity};
    use super::*;
    use crate::network::NetworkCommand;
    use crate::protocol::MessageWriter;

    struct Harness {
        actor: Actor,
        _commands: mpsc::Sender<NetworkCommand>,
        events: mpsc::Receiver<NetworkEvent>,
    }

    impl Harness {
        fn new() -> Self {
            let (commands_tx, commands) = mpsc::channel(8);
            let (events_tx, events) = mpsc::channel(64);
            let mut actor = Actor::new(commands, events_tx);
            actor.distributed.max_distrib_children = 10;
            Self {
                actor,
                _commands: commands_tx,
                events,
            }
        }

        fn conn(&mut self, username: &str) -> (ConnId, mpsc::Receiver<ConnControl>) {
            let (control, rx) = mpsc::channel(64);
            let conn_id = self.actor.peers.add(Conn::established(
                control,
                PeerIdentity {
                    username: username.to_owned(),
                    conn_type: ConnectionType::Distributed,
                },
            ));
            (conn_id, rx)
        }

        fn candidate(&mut self, username: &str) {
            self.actor.distributed.potential_parents.insert(
                username.to_owned(),
                PotentialParent {
                    conn_id: None,
                    branch_level: None,
                    branch_root: None,
                },
            );
        }

        fn adopt(&mut self, username: &str) -> (ConnId, mpsc::Receiver<ConnControl>) {
            self.candidate(username);
            let (conn_id, rx) = self.conn(username);
            self.actor
                .handle_distrib_message(conn_id, DistributedMessage::BranchLevel { level: 0 });
            self.actor
                .handle_distrib_message(conn_id, search(b"first", &[]));
            assert_eq!(self.actor.distributed.parent, Some(conn_id));
            (conn_id, rx)
        }
    }

    fn search_payload(term: &[u8], trailing: &[u8]) -> Vec<u8> {
        let mut w = MessageWriter::new();
        w.write_u32(SEARCH_IDENTIFIER);
        w.write_string("searcher");
        w.write_u32(7);
        w.write_bytes(term);
        w.write_raw(trailing);
        w.into_bytes()
    }

    fn search(term: &[u8], trailing: &[u8]) -> DistributedMessage {
        DistributedMessage::parse(DistributedSearch::CODE, &search_payload(term, trailing)).unwrap()
    }

    fn drain(rx: &mut mpsc::Receiver<ConnControl>) -> Vec<ConnControl> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn closed(controls: &[ConnControl]) -> bool {
        controls
            .iter()
            .any(|control| matches!(control, ConnControl::Close))
    }

    #[test]
    fn parent_is_tracked_by_conn_not_username() {
        let mut h = Harness::new();
        let (parent, mut parent_rx) = h.adopt("parent");
        let (secondary, mut secondary_rx) = h.conn("parent");

        h.actor
            .handle_distrib_message(secondary, search(b"stray", &[]));
        assert!(closed(&drain(&mut secondary_rx)));
        assert_eq!(h.actor.distributed.parent, Some(parent));

        h.actor.handle_distributed_conn_closed("parent", secondary);
        assert_eq!(h.actor.distributed.parent, Some(parent));

        h.actor.handle_distributed_conn_closed("parent", parent);
        assert_eq!(h.actor.distributed.parent, None);
        assert!(!closed(&drain(&mut parent_rx)));
    }

    #[test]
    fn reset_closes_the_parent_conn() {
        let mut h = Harness::new();
        let (_parent, mut parent_rx) = h.adopt("parent");
        h.actor.handle_reset_distributed();
        assert!(closed(&drain(&mut parent_rx)));
        assert_eq!(h.actor.distributed.parent, None);
    }

    #[test]
    fn search_between_branch_level_and_root_keeps_the_level() {
        let mut h = Harness::new();
        h.candidate("parent");
        let (conn_id, _rx) = h.conn("parent");

        h.actor
            .handle_distrib_message(conn_id, DistributedMessage::BranchLevel { level: 3 });
        h.actor
            .handle_distrib_message(conn_id, search(b"early", &[]));
        assert_eq!(h.actor.distributed.parent, None);

        h.actor.handle_distrib_message(
            conn_id,
            DistributedMessage::BranchRoot {
                root_username: "root".into(),
            },
        );
        h.actor
            .handle_distrib_message(conn_id, search(b"late", &[]));
        assert_eq!(h.actor.distributed.parent, Some(conn_id));
        assert_eq!(h.actor.distributed.branch_level, 4);
        assert_eq!(h.actor.distributed.branch_root.as_deref(), Some("root"));
    }

    #[test]
    fn searches_are_forwarded_with_their_original_bytes() {
        let mut h = Harness::new();
        let (parent, _parent_rx) = h.adopt("parent");
        let (child, mut child_rx) = h.conn("child");
        h.actor.accept_child_peer(child, "child");
        drain(&mut child_rx);
        while h.events.try_recv().is_ok() {}

        let payload = search_payload(b"caf\xe9", b"\x01\x02");
        h.actor.handle_distrib_message(
            parent,
            DistributedMessage::parse(DistributedSearch::CODE, &payload).unwrap(),
        );

        let mut expected = (payload.len() as u32 + 1).to_le_bytes().to_vec();
        expected.push(DistributedSearch::CODE);
        expected.extend_from_slice(&payload);
        let sent: Vec<Vec<u8>> = drain(&mut child_rx)
            .into_iter()
            .filter_map(|control| match control {
                ConnControl::Send(bytes) => Some(bytes),
                _ => None,
            })
            .collect();
        assert_eq!(sent, vec![expected]);
        assert!(matches!(
            h.events.try_recv(),
            Ok(NetworkEvent::DistributedSearch { search_term, .. }) if search_term == "caf\u{e9}"
        ));
    }

    #[test]
    fn server_embedded_non_search_is_ignored() {
        let mut h = Harness::new();
        h.actor
            .handle_embedded_message(93, &[DistributedSearch::CODE, 0, 0, 0, 0]);
        assert!(!h.actor.distributed.is_server_parent);

        h.actor
            .handle_embedded_message(DistributedSearch::CODE, &search_payload(b"q", &[]));
        assert!(h.actor.distributed.is_server_parent);
    }

    #[test]
    fn adopting_a_former_child_closes_its_child_conn() {
        let mut h = Harness::new();
        let (first, _first_rx) = h.adopt("parent");
        let (former, mut former_rx) = h.conn("other");
        h.actor.accept_child_peer(former, "other");
        drain(&mut former_rx);
        h.actor.handle_distributed_conn_closed("parent", first);

        h.candidate("other");
        let (new_parent, _new_rx) = h.conn("other");
        h.actor
            .handle_distrib_message(new_parent, DistributedMessage::BranchLevel { level: 0 });
        h.actor
            .handle_distrib_message(new_parent, search(b"q", &[]));

        assert_eq!(h.actor.distributed.parent, Some(new_parent));
        assert!(!h.actor.distributed.child_peers.contains_key("other"));
        assert!(closed(&drain(&mut former_rx)));
    }
}
