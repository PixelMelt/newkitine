use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::MySqlPool;

use crate::types::{Recommendations, SimilarUser, USERNAME_LIMIT};

use super::api;
use super::contract::AppEvent;
use super::projection::set_member;
use super::state::App;

#[derive(Default, Clone, serde::Serialize)]
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

fn publish(app: &App, apply: impl FnOnce(&mut InterestsView)) {
    let mut data = app.projection.write();
    apply(&mut data.interests);
    let event = AppEvent::Interests {
        interests: data.interests.clone(),
    };
    data.broadcast(event);
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
    set_recommendations(app, recommendations, unrecommendations, None, false);
}

pub fn set_recommendations(
    app: &App,
    recommendations: Recommendations,
    unrecommendations: Recommendations,
    thing: Option<String>,
    global: bool,
) {
    publish(app, |view| {
        view.recommendations = recommendations;
        view.unrecommendations = unrecommendations;
        view.recommendations_for = thing;
        view.recommendations_global = global;
    });
}

pub fn similar_users(app: &App, users: Vec<SimilarUser>, thing: Option<String>) {
    publish(app, |view| {
        view.similar_users = users;
        view.similar_users_for = thing;
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

pub(super) fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/interests", get(interests))
        .route(
            "/api/interests/add",
            post(|app, body| set_interest(app, body, true)),
        )
        .route(
            "/api/interests/remove",
            post(|app, body| set_interest(app, body, false)),
        )
        .route("/api/interests/refresh", post(refresh_interests))
        .route("/api/interests/item", post(item_recommendations))
}

async fn interests(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let data = app.projection.read();
    Json(json!({ "interests": data.interests }))
}

#[derive(Deserialize)]
struct InterestBody {
    kind: String,
    thing: String,
}

pub(super) fn normalize_interest(thing: &str) -> Option<String> {
    let thing = thing.trim().to_lowercase();
    (!thing.is_empty()).then_some(thing)
}

async fn set_interest(
    State(app): State<Arc<App>>,
    Json(body): Json<InterestBody>,
    add: bool,
) -> Result<StatusCode, StatusCode> {
    let _mutation = app.list_mutation.lock().await;
    let liked = match body.kind.as_str() {
        "liked" => true,
        "hated" => false,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let thing = normalize_interest(&body.thing)
        .filter(|thing| !add || thing.len() <= USERNAME_LIMIT)
        .ok_or(StatusCode::BAD_REQUEST)?;
    let statement = if add {
        "INSERT INTO interests (kind, thing) VALUES (?, ?) ON DUPLICATE KEY UPDATE thing = thing"
    } else {
        "DELETE FROM interests WHERE kind = ? AND thing = ?"
    };
    sqlx::query(statement)
        .bind(&body.kind)
        .bind(&thing)
        .execute(&app.db)
        .await
        .map_err(api::db_failed)?;
    app.client.set_interest(&thing, !liked, add).await;
    publish(&app, |view| {
        let list = if liked {
            &mut view.liked
        } else {
            &mut view.hated
        };
        set_member(list, &thing, add);
    });
    Ok(StatusCode::ACCEPTED)
}

async fn refresh_interests(State(app): State<Arc<App>>) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    let no_interests = {
        let data = app.projection.read();
        data.interests.liked.is_empty() && data.interests.hated.is_empty()
    };
    if no_interests {
        app.client.request_global_recommendations().await;
    } else {
        app.client.request_recommendations().await;
    }
    app.client.request_similar_users().await;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct ItemBody {
    thing: String,
}

async fn item_recommendations(
    State(app): State<Arc<App>>,
    Json(body): Json<ItemBody>,
) -> Result<StatusCode, StatusCode> {
    api::require_login(&app)?;
    app.client.request_item_recommendations(&body.thing).await;
    app.client.request_item_similar_users(&body.thing).await;
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::normalize_interest;

    #[test]
    fn interests_are_trimmed_and_lowercased() {
        assert_eq!(
            normalize_interest("  Jazz Fusion \t").as_deref(),
            Some("jazz fusion")
        );
    }

    #[test]
    fn blank_interests_are_rejected() {
        assert_eq!(normalize_interest(" \n "), None);
        assert_eq!(normalize_interest(""), None);
    }
}
