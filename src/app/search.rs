use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use sqlx::MySqlPool;

use crate::types::{SearchFileView, SearchResponseView, SearchResult, SearchScope, SearchView};

use super::api;
use super::contract::AppEvent;
use super::state::App;

const MAX_SEARCH_RESPONSES: usize = 500;

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

    pub fn wishlist(&self) -> &[String] {
        &self.wishlist
    }
}

pub fn search_started(app: &App, token: u32, query: String) {
    let mut data = app.projection.write();
    let search = SearchView {
        token,
        query,
        results: Vec::new(),
    };
    let event = AppEvent::SearchAdded {
        search: search.clone(),
    };
    data.search.searches.push(search);
    data.broadcast(event);
}

pub fn apply_results(app: &App, result: SearchResult) {
    let SearchResult {
        token,
        username,
        results,
        free_upload_slots,
        upload_speed,
        queue_size,
    } = result;
    let response = SearchResponseView {
        username,
        free_upload_slots,
        upload_speed,
        queue_size,
        files: results
            .into_iter()
            .map(|file| SearchFileView {
                name: file.name,
                size: file.size,
                attributes: file.attributes,
            })
            .collect(),
    };
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
    if search.results.len() < MAX_SEARCH_RESPONSES {
        search.results.push(response.clone());
        data.broadcast(AppEvent::SearchResults { token, response });
    }
}

async fn insert_wish(pool: &MySqlPool, term: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT IGNORE INTO wishlist (term) VALUES (?)")
        .bind(term)
        .execute(pool)
        .await?;
    Ok(())
}

async fn delete_wish(pool: &MySqlPool, term: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM wishlist WHERE term = ?")
        .bind(term)
        .execute(pool)
        .await?;
    Ok(())
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

#[derive(Deserialize)]
pub struct SearchBody {
    query: String,
    #[serde(default = "default_search_mode")]
    mode: String,
    room: Option<String>,
    user: Option<String>,
}

fn default_search_mode() -> String {
    "global".into()
}

pub async fn list_searches(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
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

pub async fn start_search(
    State(app): State<Arc<App>>,
    Json(body): Json<SearchBody>,
) -> impl IntoResponse {
    let scope = match body.mode.as_str() {
        "global" => SearchScope::Global,
        "buddies" => SearchScope::Buddies,
        "rooms" => match body.room {
            Some(room) => SearchScope::Room(room),
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        "user" => match body.user {
            Some(user) => SearchScope::User(user),
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let token = app.client.search(&body.query, scope).await;
    Json(json!({ "token": token })).into_response()
}

pub async fn search_results(
    State(app): State<Arc<App>>,
    Path(token): Path<u32>,
) -> impl IntoResponse {
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

pub async fn remove_search(State(app): State<Arc<App>>, Path(token): Path<u32>) -> StatusCode {
    app.client.cancel_search(token).await;
    let mut data = app.projection.write();
    data.search.searches.retain(|search| search.token != token);
    data.broadcast(AppEvent::SearchRemoved { token });
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
pub struct WishBody {
    term: String,
}

pub async fn list_wishlist(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "wishlist": data.search.wishlist() }))
}

pub async fn add_wish(State(app): State<Arc<App>>, Json(body): Json<WishBody>) -> StatusCode {
    if let Err(error) = insert_wish(&app.db, &body.term).await {
        return api::db_failed(error);
    }
    app.client.add_wish(&body.term).await;
    let mut data = app.projection.write();
    if !data.search.wishlist.contains(&body.term) {
        data.search.wishlist.push(body.term);
        data.search.wishlist.sort();
    }
    let event = AppEvent::Wishlist {
        wishlist: data.search.wishlist.clone(),
    };
    data.broadcast(event);
    StatusCode::ACCEPTED
}

pub async fn remove_wish(State(app): State<Arc<App>>, Json(body): Json<WishBody>) -> StatusCode {
    if let Err(error) = delete_wish(&app.db, &body.term).await {
        return api::db_failed(error);
    }
    app.client.remove_wish(&body.term).await;
    let mut data = app.projection.write();
    data.search.wishlist.retain(|term| term != &body.term);
    let event = AppEvent::Wishlist {
        wishlist: data.search.wishlist.clone(),
    };
    data.broadcast(event);
    StatusCode::ACCEPTED
}
