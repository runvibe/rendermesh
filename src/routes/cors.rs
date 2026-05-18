use axum::http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tracing::warn;

use crate::config::CorsConfig;

pub fn build_cors_layer(cors: &CorsConfig, methods: Option<&[Method]>) -> CorsLayer {
    match cors {
        CorsConfig::Permissive => CorsLayer::permissive(),
        CorsConfig::Restricted(origins) => {
            let mut values = Vec::new();
            for origin in origins {
                match HeaderValue::from_str(origin) {
                    Ok(value) => values.push(value),
                    Err(_) => warn!(origin = origin.as_str(), "invalid cors origin"),
                }
            }

            let layer = CorsLayer::new()
                .allow_origin(AllowOrigin::list(values))
                .allow_headers(Any);

            match methods {
                Some(methods) => layer.allow_methods(methods.to_vec()),
                None => layer.allow_methods(Any),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Method;

    use super::build_cors_layer;
    use crate::config::CorsConfig;

    #[test]
    fn permissive_config_builds_layer() {
        let _layer = build_cors_layer(&CorsConfig::Permissive, None);
    }

    #[test]
    fn restricted_config_builds_layer_with_explicit_methods() {
        let _layer = build_cors_layer(
            &CorsConfig::Restricted(vec!["http://localhost:3000".to_string()]),
            Some(&[Method::GET, Method::POST]),
        );
    }
}
