use axum::Router;

use crate::state::AppState;

pub mod echo;
pub mod health;

pub fn router() -> Router<AppState> {
    Router::new().merge(health::router()).merge(echo::router())
}
