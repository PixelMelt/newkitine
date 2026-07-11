use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use sqlx::MySqlPool;

use serde::Serialize;

use crate::types::{Recommendations, SimilarUser};

use super::api;
use super::contract::AppEvent;
use super::state::App;

#[derive(Default, Clone, Serialize)]
pub struct InterestsView {
    pub liked: Vec<String>,
    pub hated: Vec<String>,
    pub recommendations: Recommendations,
    pub unrecommendations: Recommendations,
    pub recommendations_for: Option<String>,
    pub recommendations_global: bool,
    pub similar_users: Vec<SimilarUser>,
    pub similar_users_for: Option<String>,
}

#[derive(Default)]
pub struct Interests {
    view: InterestsView,
}

impl Interests {
    pub fn load(liked: Vec<String>, hated: Vec<String>) -> Self {
        Self {
            view: InterestsView {
                liked,
                hated,
                ..Default::default()
            },
        }
    }

    pub fn view(&self) -> &InterestsView {
        &self.view
    }
}

fn publish(app: &App, apply: impl FnOnce(&mut InterestsView)) {
    let mut data = app.data.write().unwrap();
    apply(&mut data.interests.view);
    let event = AppEvent::Interests {
        interests: data.interests.view.clone(),
    };
    app.broadcast(&mut data, event);
}

pub async fn recommendations(
    app: &App,
    recommendations: Recommendations,
    unrecommendations: Recommendations,
) {
    if recommendations.is_empty() && unrecommendations.is_empty() {
        app.client.request_global_recommendations().await;
        return;
    }
    publish(app, |view| {
        view.recommendations = recommendations;
        view.unrecommendations = unrecommendations;
        view.recommendations_for = None;
        view.recommendations_global = false;
    });
}

pub fn global_recommendations(
    app: &App,
    recommendations: Recommendations,
    unrecommendations: Recommendations,
) {
    publish(app, |view| {
        view.recommendations = recommendations;
        view.unrecommendations = unrecommendations;
        view.recommendations_for = None;
        view.recommendations_global = true;
    });
}

pub fn item_recommendations_received(
    app: &App,
    thing: String,
    recommendations: Recommendations,
    unrecommendations: Recommendations,
) {
    publish(app, |view| {
        view.recommendations = recommendations;
        view.unrecommendations = unrecommendations;
        view.recommendations_for = Some(thing);
        view.recommendations_global = false;
    });
}

pub fn similar_users(app: &App, users: Vec<SimilarUser>) {
    publish(app, |view| {
        view.similar_users = users;
        view.similar_users_for = None;
    });
}

pub fn item_similar_users(app: &App, thing: String, usernames: Vec<String>) {
    publish(app, |view| {
        view.similar_users = usernames
            .into_iter()
            .map(|username| SimilarUser {
                username,
                rating: 0,
            })
            .collect();
        view.similar_users_for = Some(thing);
    });
}

pub async fn load_interests(pool: &MySqlPool, kind: &str) -> Vec<String> {
    use sqlx::Row;
    sqlx::query("SELECT thing FROM interests WHERE kind = ? ORDER BY thing")
        .bind(kind)
        .fetch_all(pool)
        .await
        .expect("load interests")
        .into_iter()
        .map(|row| row.get(0))
        .collect()
}

async fn insert_interest(pool: &MySqlPool, kind: &str, thing: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT IGNORE INTO interests (kind, thing) VALUES (?, ?)")
        .bind(kind)
        .bind(thing)
        .execute(pool)
        .await?;
    Ok(())
}

async fn delete_interest(pool: &MySqlPool, kind: &str, thing: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM interests WHERE kind = ? AND thing = ?")
        .bind(kind)
        .bind(thing)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn interests(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.data.read().unwrap();
    Json(json!({ "interests": data.interests.view() }))
}

#[derive(Deserialize)]
pub struct InterestBody {
    kind: String,
    thing: String,
}

pub async fn add_interest(
    State(app): State<Arc<App>>,
    Json(body): Json<InterestBody>,
) -> StatusCode {
    match body.kind.as_str() {
        "liked" => app.client.add_liked_interest(&body.thing).await,
        "hated" => app.client.add_hated_interest(&body.thing).await,
        _ => return StatusCode::BAD_REQUEST,
    }
    if let Err(error) = insert_interest(&app.db, &body.kind, &body.thing).await {
        return api::db_failed(error);
    }
    publish(&app, |view| {
        let list = if body.kind == "liked" {
            &mut view.liked
        } else {
            &mut view.hated
        };
        if !list.contains(&body.thing) {
            list.push(body.thing);
            list.sort();
        }
    });
    StatusCode::ACCEPTED
}

pub async fn remove_interest(
    State(app): State<Arc<App>>,
    Json(body): Json<InterestBody>,
) -> StatusCode {
    match body.kind.as_str() {
        "liked" => app.client.remove_liked_interest(&body.thing).await,
        "hated" => app.client.remove_hated_interest(&body.thing).await,
        _ => return StatusCode::BAD_REQUEST,
    }
    if let Err(error) = delete_interest(&app.db, &body.kind, &body.thing).await {
        return api::db_failed(error);
    }
    publish(&app, |view| {
        let list = if body.kind == "liked" {
            &mut view.liked
        } else {
            &mut view.hated
        };
        list.retain(|thing| thing != &body.thing);
    });
    StatusCode::ACCEPTED
}

pub async fn refresh_interests(State(app): State<Arc<App>>) -> StatusCode {
    let no_interests = {
        let data = app.data.read().unwrap();
        data.interests.view.liked.is_empty() && data.interests.view.hated.is_empty()
    };
    if no_interests {
        app.client.request_global_recommendations().await;
    } else {
        app.client.request_recommendations().await;
    }
    app.client.request_similar_users().await;
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
pub struct ItemBody {
    thing: String,
}

pub async fn item_recommendations(
    State(app): State<Arc<App>>,
    Json(body): Json<ItemBody>,
) -> StatusCode {
    app.client.request_item_recommendations(&body.thing).await;
    app.client.request_item_similar_users(&body.thing).await;
    StatusCode::ACCEPTED
}
