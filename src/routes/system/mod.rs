use axum::Router;

use crate::state::AppState;

pub mod echo;
pub mod health;
pub mod origins;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(health::router())
        .merge(echo::router())
        .merge(origins::router())
}
