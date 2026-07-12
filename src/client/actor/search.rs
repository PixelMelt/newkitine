use std::time::Duration;

use tokio::time::Instant;

use super::ClientActor;
use crate::client::search::sanitize_search_term;
use crate::protocol::ServerRequest;
use crate::types::{ClientEvent, NetworkCommand, SearchScope};

const DEFAULT_WISHLIST_INTERVAL: Duration = Duration::from_secs(720);

pub(super) struct Wishlist {
    terms: Vec<String>,
    interval: Duration,
    cursor: usize,
    pub(super) at: Option<Instant>,
}

impl Wishlist {
    pub(super) fn new(terms: Vec<String>) -> Self {
        Self {
            terms,
            interval: DEFAULT_WISHLIST_INTERVAL,
            cursor: 0,
            at: None,
        }
    }
}

impl ClientActor {
    pub(super) fn start_search(&mut self, token: u32, query: String, scope: SearchScope) {
        self.searches.add(token);
        self.net.send(NetworkCommand::AllowSearchToken(token));
        let search_term = sanitize_search_term(&query);
        self.emit(ClientEvent::SearchStarted { token, query });
        match scope {
            SearchScope::Global => {
                self.net
                    .server(ServerRequest::FileSearch { token, search_term });
            }
            SearchScope::Room(room) => {
                self.net.server(ServerRequest::RoomSearch {
                    room,
                    token,
                    search_term,
                });
            }
            SearchScope::Buddies => {
                for buddy in &self.users.buddies {
                    self.net.server(ServerRequest::UserSearch {
                        search_username: buddy.clone(),
                        token,
                        search_term: search_term.clone(),
                    });
                }
            }
            SearchScope::User(username) => {
                self.net.server(ServerRequest::UserSearch {
                    search_username: username,
                    token,
                    search_term,
                });
            }
        }
    }

    pub(super) fn add_wish(&mut self, term: String) {
        if !self.wishlist.terms.contains(&term) {
            self.wishlist.terms.push(term);
            if self.wishlist.at.is_none() {
                self.schedule_wishlist();
            }
        }
    }

    pub(super) fn remove_wish(&mut self, term: &str) {
        self.wishlist.terms.retain(|wish| wish != term);
        if self.wishlist.terms.is_empty() {
            self.wishlist.at = None;
        }
    }

    pub(super) fn set_wishlist_interval(&mut self, seconds: u32) {
        self.wishlist.interval = Duration::from_secs(seconds.max(1) as u64);
        self.schedule_wishlist();
    }

    pub(super) fn schedule_wishlist(&mut self) {
        self.wishlist.at = if self.session.logged_in && !self.wishlist.terms.is_empty() {
            Some(Instant::now() + self.wishlist.interval)
        } else {
            None
        };
    }

    pub(super) fn do_wishlist_search(&mut self) {
        if self.wishlist.terms.is_empty() {
            self.wishlist.at = None;
            return;
        }
        let term = self.wishlist.terms[self.wishlist.cursor % self.wishlist.terms.len()].clone();
        self.wishlist.cursor += 1;
        let token = self.next_token();
        self.searches.add(token);
        self.net.send(NetworkCommand::AllowSearchToken(token));
        self.emit(ClientEvent::SearchStarted {
            token,
            query: term.clone(),
        });
        self.net.server(ServerRequest::WishlistSearch {
            token,
            search_term: sanitize_search_term(&term),
        });
        self.schedule_wishlist();
    }
}
