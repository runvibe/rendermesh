use crate::dto::health::HealthStatus;

pub fn health_status() -> HealthStatus {
    HealthStatus {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}
