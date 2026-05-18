use utoipa_axum::router::OpenApiRouter;

use crate::state::AppState;

pub mod echo;
pub mod health;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(health::router())
        .merge(echo::router())
}
