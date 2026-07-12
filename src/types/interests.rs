use serde::Serialize;

use crate::types::SimilarUser;

pub type Recommendations = Vec<(String, i32)>;

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
