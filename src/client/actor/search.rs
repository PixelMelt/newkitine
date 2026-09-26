use std::time::Duration;

use tokio::time::Instant;
use tracing::debug;

use super::ClientActor;
use crate::client::search::SearchQuery;
use crate::client::{ClientEvent, SearchResult, SearchScope};
use crate::network::NetworkCommand;
use crate::protocol::ServerRequest;

const DEFAULT_WISHLIST_INTERVAL: Duration = Duration::from_secs(720);

pub(super) struct ActiveSearch {
    query: String,
    filter: SearchQuery,
    shown: bool,
}

struct Wish {
    term: String,
    token: u32,
}

pub(super) struct Wishlist {
    wishes: Vec<Wish>,
    interval: Duration,
    cursor: usize,
    pub(super) at: Option<Instant>,
}

impl Wishlist {
    pub(super) fn new() -> Self {
        Self {
            wishes: Vec::new(),
            interval: DEFAULT_WISHLIST_INTERVAL,
            cursor: 0,
            at: None,
        }
    }
}

impl ClientActor {
    pub(super) fn start_search(&mut self, token: u32, query: String, scope: SearchScope) {
        self.net.send(NetworkCommand::AllowSearchToken(token));
        let filter = SearchQuery::parse(&query);
        let search_term = filter.transmitted.clone();
        self.emit(ClientEvent::SearchStarted {
            token,
            query: query.clone(),
        });
        self.searches.insert(
            token,
            ActiveSearch {
                query,
                filter,
                shown: true,
            },
        );
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

    pub(super) fn cancel_search(&mut self, token: u32) {
        self.net.send(NetworkCommand::DisallowSearchToken(token));
        self.searches.remove(&token);
    }

    pub(super) fn handle_search_response(&mut self, mut result: SearchResult) {
        let Some(search) = self.searches.get_mut(&result.token) else {
            debug!(
                token = result.token,
                "response for a cancelled search, dropping"
            );
            return;
        };
        result
            .results
            .retain(|file| search.filter.matches(&file.name));
        if result.results.is_empty() {
            return;
        }
        if !search.shown {
            search.shown = true;
            let query = search.query.clone();
            self.emit(ClientEvent::SearchStarted {
                token: result.token,
                query,
            });
        }
        self.emit(ClientEvent::SearchResults(result));
    }

    pub(super) fn add_wish(&mut self, term: String) {
        if !self.wishlist.wishes.iter().any(|wish| wish.term == term) {
            let token = self.next_token();
            self.wishlist.wishes.push(Wish { term, token });
            if self.wishlist.at.is_none() {
                self.schedule_wishlist();
            }
        }
    }

    pub(super) fn remove_wish(&mut self, term: &str) {
        let Some(index) = self
            .wishlist
            .wishes
            .iter()
            .position(|wish| wish.term == term)
        else {
            return;
        };
        let wish = self.wishlist.wishes.remove(index);
        if self
            .searches
            .get(&wish.token)
            .is_some_and(|search| !search.shown)
        {
            self.cancel_search(wish.token);
        }
        if self.wishlist.wishes.is_empty() {
            self.wishlist.at = None;
        }
    }

    pub(super) fn set_wishlist_interval(&mut self, seconds: u32) {
        if seconds == 0 {
            return;
        }
        self.wishlist.interval = Duration::from_secs(seconds as u64);
        self.schedule_wishlist();
    }

    pub(super) fn schedule_wishlist(&mut self) {
        self.wishlist.at = if self.session.logged_in && !self.wishlist.wishes.is_empty() {
            Some(Instant::now() + self.wishlist.interval)
        } else {
            None
        };
    }

    pub(super) fn do_wishlist_search(&mut self) {
        let wish = &self.wishlist.wishes[self.wishlist.cursor % self.wishlist.wishes.len()];
        let (token, term) = (wish.token, wish.term.clone());
        self.wishlist.cursor += 1;
        let filter = SearchQuery::parse(&term);
        if !filter.transmitted.is_empty() {
            self.net.send(NetworkCommand::AllowSearchToken(token));
            let search_term = filter.transmitted.clone();
            self.searches.entry(token).or_insert(ActiveSearch {
                query: term,
                filter,
                shown: false,
            });
            self.net
                .server(ServerRequest::WishlistSearch { token, search_term });
        }
        self.schedule_wishlist();
    }
}
