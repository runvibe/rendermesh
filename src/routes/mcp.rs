use std::sync::Arc;

use axum::{http::Method, Router};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        session::never::NeverSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    Json, ServerHandler,
};

use crate::{
    config::McpConfig,
    dto::{
        echo::{EchoRequestInput, EchoResponse},
        health::HealthStatus,
    },
    routes::cors,
    services::{echo, health},
    state::AppState,
};

#[derive(Debug, Clone)]
struct McpServer {
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                format!("{}-mcp", env!("CARGO_PKG_NAME")),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions("Available tools: health_check and echo_request.")
    }
}

#[tool_router(router = tool_router)]
impl McpServer {
    #[tool(
        name = "health_check",
        description = "Return the current service health status and version."
    )]
    async fn health_check(&self) -> Json<HealthStatus> {
        Json(health::health_status())
    }

    #[tool(
        name = "echo_request",
        description = "Echo a method, path, headers, and body payload."
    )]
    async fn echo_request(
        &self,
        Parameters(input): Parameters<EchoRequestInput>,
    ) -> Json<EchoResponse> {
        Json(echo::echo(input))
    }
}

pub fn router(config: &McpConfig) -> Router<AppState> {
    let service = StreamableHttpService::new(
        || Ok(McpServer::new()),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig {
            stateful_mode: false,
            json_response: true,
            sse_keep_alive: None,
            sse_retry: None,
            ..Default::default()
        },
    );

    Router::new()
        .nest_service(config.path.as_str(), service)
        .layer(cors::build_cors_layer(
            &config.cors,
            Some(&[Method::POST, Method::OPTIONS, Method::GET]),
        ))
}

#[cfg(test)]
mod tests {
    use axum::http::Method;

    use crate::{config::CorsConfig, routes::cors};

    #[test]
    fn restricted_mcp_cors_config_is_supported() {
        let _layer = cors::build_cors_layer(
            &CorsConfig::Restricted(vec![
                "http://localhost:6274".to_string(),
                "http://127.0.0.1:6274".to_string(),
            ]),
            Some(&[Method::POST, Method::OPTIONS, Method::GET]),
        );
    }
}
