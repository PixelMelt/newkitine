use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::MySqlPool;

use crate::client::{SearchResult, SearchScope};
use crate::types::{FileInfo, USERNAME_LIMIT};

use super::api;
use super::contract::AppEvent;
use super::projection::set_member;
use super::state::App;

#[derive(Clone, serde::Serialize)]
pub struct SearchView {
    pub token: u32,
    pub query: String,
    pub results: Vec<SearchResponseView>,
}

impl SearchView {
    fn accepts(&self, username: &str, max_responses: usize) -> bool {
        self.results.len() < max_responses
            && !self
                .results
                .iter()
                .any(|response| response.username == username)
    }
}

#[derive(Clone, serde::Serialize)]
pub struct SearchResponseView {
    pub username: String,
    pub free_upload_slots: bool,
    pub upload_speed: u32,
    pub queue_size: u32,
    pub files: Vec<FileInfo>,
}

#[derive(Default)]
pub struct SearchState {
    searches: Vec<SearchView>,
    wishlist: Vec<String>,
}

impl SearchState {
    pub fn load(wishlist: Vec<String>) -> Self {
        Self {
            searches: Vec::new(),
            wishlist,
        }
    }

    pub fn searches(&self) -> &[SearchView] {
        &self.searches
    }

    fn insert(&mut self, search: SearchView) -> Vec<u32> {
        self.searches.push(search);
        let excess = self.searches.len().saturating_sub(MAX_RETAINED_SEARCHES);
        self.searches
            .drain(..excess)
            .map(|search| search.token)
            .collect()
    }

    pub fn wishlist(&self) -> &[String] {
        &self.wishlist
    }
}

const MAX_RETAINED_SEARCHES: usize = 25;

pub async fn search_started(app: &App, token: u32, query: String) {
    let evicted = {
        let mut data = app.projection.write();
        let search = SearchView {
            token,
            query,
            results: Vec::new(),
        };
        let event = AppEvent::SearchAdded {
            search: search.clone(),
        };
        let evicted = data.search.insert(search);
        data.broadcast(event);
        for token in &evicted {
            data.broadcast(AppEvent::SearchRemoved { token: *token });
        }
        evicted
    };
    for token in evicted {
        app.client.cancel_search(token).await;
    }
}

pub fn apply_results(app: &App, result: SearchResult) {
    let token = result.token;
    let response = SearchResponseView {
        username: result.username,
        free_upload_slots: result.free_upload_slots,
        upload_speed: result.upload_speed,
        queue_size: result.queue_size,
        files: result.results,
    };
    let max_search_responses = app.settings.max_search_responses();
    let mut data = app.projection.write();
    let Some(index) = data
        .search
        .searches
        .iter()
        .position(|search| search.token == token)
    else {
        tracing::warn!(token, "results for a removed search, dropping");
        return;
    };
    let search = &mut data.search.searches[index];
    if search.accepts(&response.username, max_search_responses) {
        search.results.push(response.clone());
        data.broadcast(AppEvent::SearchResults { token, response });
    }
}

pub async fn load_wishlist(pool: &MySqlPool) -> Vec<String> {
    use sqlx::Row;
    sqlx::query("SELECT term FROM wishlist ORDER BY term")
        .fetch_all(pool)
        .await
        .expect("load wishlist")
        .into_iter()
        .map(|row| row.get(0))
        .collect()
}

pub(super) fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/searches", get(list_searches).post(start_search))
        .route(
            "/api/searches/{token}",
            get(search_results).delete(remove_search),
        )
        .route(
            "/api/wishlist",
            get(list_wishlist).post(|app, body| set_wish(app, body, true)),
        )
        .route(
            "/api/wishlist/remove",
            post(|app, body| set_wish(app, body, false)),
        )
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default = "default_search_mode")]
    mode: String,
    room: Option<String>,
    user: Option<String>,
}

fn default_search_mode() -> String {
    "global".into()
}

async fn list_searches(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    let searches: Vec<serde_json::Value> = data
        .search
        .searches
        .iter()
        .map(|search| {
            json!({
                "token": search.token,
                "query": search.query,
                "responses": search.results.len(),
            })
        })
        .collect();
    Json(json!({ "searches": searches }))
}

async fn start_search(
    State(app): State<Arc<App>>,
    Json(body): Json<SearchBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    api::require_login(&app)?;
    let scope = match body.mode.as_str() {
        "global" => SearchScope::Global,
        "buddies" => SearchScope::Buddies,
        "rooms" => SearchScope::Room(body.room.ok_or(StatusCode::BAD_REQUEST)?),
        "user" => SearchScope::User(body.user.ok_or(StatusCode::BAD_REQUEST)?),
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let token = app.client.search(&body.query, scope).await;
    let token = token.ok_or(StatusCode::BAD_REQUEST)?;
    Ok(Json(json!({ "token": token })))
}

async fn search_results(State(app): State<Arc<App>>, Path(token): Path<u32>) -> impl IntoResponse {
    let data = app.projection.read();
    match data
        .search
        .searches
        .iter()
        .find(|search| search.token == token)
    {
        Some(search) => Json(serde_json::to_value(search).unwrap()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn remove_search(State(app): State<Arc<App>>, Path(token): Path<u32>) -> StatusCode {
    app.client.cancel_search(token).await;
    let mut data = app.projection.write();
    data.search.searches.retain(|search| search.token != token);
    data.broadcast(AppEvent::SearchRemoved { token });
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct WishBody {
    term: String,
}

async fn list_wishlist(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "wishlist": data.search.wishlist() }))
}

async fn set_wish(
    State(app): State<Arc<App>>,
    Json(body): Json<WishBody>,
    wanted: bool,
) -> Result<StatusCode, StatusCode> {
    if wanted && (body.term.trim().is_empty() || body.term.len() > USERNAME_LIMIT) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let _mutation = app.list_mutation.lock().await;
    let statement = if wanted {
        "INSERT INTO wishlist (term) VALUES (?) ON DUPLICATE KEY UPDATE term = term"
    } else {
        "DELETE FROM wishlist WHERE term = ?"
    };
    sqlx::query(statement)
        .bind(&body.term)
        .execute(&app.db)
        .await
        .map_err(api::db_failed)?;
    app.client.set_wish(&body.term, wanted).await;
    let mut data = app.projection.write();
    set_member(&mut data.search.wishlist, &body.term, wanted);
    let event = AppEvent::Wishlist {
        wishlist: data.search.wishlist.clone(),
    };
    data.broadcast(event);
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oldest_searches_evict_beyond_retention_cap() {
        let mut state = SearchState::default();
        for token in 0..MAX_RETAINED_SEARCHES as u32 + 2 {
            let evicted = state.insert(SearchView {
                token,
                query: format!("query {token}"),
                results: Vec::new(),
            });
            match token as usize {
                t if t < MAX_RETAINED_SEARCHES => assert!(evicted.is_empty()),
                t => assert_eq!(evicted, vec![(t - MAX_RETAINED_SEARCHES) as u32]),
            }
        }
        assert_eq!(state.searches.len(), MAX_RETAINED_SEARCHES);
        assert_eq!(state.searches[0].token, 2);
    }

    #[test]
    fn a_peer_is_retained_once_per_search() {
        let response = |username: &str| SearchResponseView {
            username: username.into(),
            free_upload_slots: true,
            upload_speed: 0,
            queue_size: 0,
            files: Vec::new(),
        };
        let search = SearchView {
            token: 1,
            query: "query".into(),
            results: vec![response("alice")],
        };
        assert!(!search.accepts("alice", 10));
        assert!(search.accepts("bob", 10));
        assert!(!search.accepts("bob", 1));
    }
}
