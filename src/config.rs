use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{anyhow, Result};

pub const DEFAULT_BODY_LIMIT_BYTES: usize = 1_048_576;
pub const DEFAULT_MCP_PATH: &str = "/mcp";

#[derive(Clone, Debug)]
pub enum CorsConfig {
    Permissive,
    Restricted(Vec<String>),
}

impl CorsConfig {
    fn from_env() -> Self {
        match std::env::var("APP_CORS_ALLOW_ORIGINS") {
            Ok(value) => {
                let origins = parse_csv_env(&value);
                if origins.iter().any(|origin| origin == "*") {
                    return CorsConfig::Permissive;
                }
                CorsConfig::Restricted(origins)
            }
            Err(_) => CorsConfig::Permissive,
        }
    }
}

#[derive(Clone, Debug)]
pub struct McpConfig {
    pub enabled: bool,
    pub path: String,
    pub cors: CorsConfig,
}

impl McpConfig {
    fn from_env() -> Self {
        let enabled = parse_env_bool("MCP_ENABLED").unwrap_or(false);
        let path = std::env::var("MCP_PATH")
            .ok()
            .map(|value| normalize_path(&value))
            .unwrap_or_else(|| DEFAULT_MCP_PATH.to_string());
        let cors = match std::env::var("MCP_ALLOWED_ORIGINS") {
            Ok(value) => {
                let origins = parse_csv_env(&value);
                if origins.iter().any(|origin| origin == "*") {
                    CorsConfig::Permissive
                } else {
                    CorsConfig::Restricted(origins)
                }
            }
            Err(_) => CorsConfig::Permissive,
        };

        Self {
            enabled,
            path,
            cors,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub database_url: String,
    pub host: IpAddr,
    pub port: u16,
    pub cors: CorsConfig,
    pub body_limit_bytes: usize,
    pub otel_enabled: bool,
    pub mcp: McpConfig,
    pub rendermesh_manifest: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow!("DATABASE_URL environment variable is required"))?;

        let host = std::env::var("APP_HOST")
            .ok()
            .and_then(|value| value.parse::<IpAddr>().ok())
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));

        let port = std::env::var("APP_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(8080);

        let cors = CorsConfig::from_env();
        let body_limit_bytes = std::env::var("APP_BODY_LIMIT_BYTES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_BODY_LIMIT_BYTES);
        let otel_enabled = otel_enabled_from_env();
        let mcp = McpConfig::from_env();
        let rendermesh_manifest = std::env::var("RENDERMESH_MANIFEST")
            .unwrap_or_else(|_| "./rendermesh.yaml".to_string());

        Ok(Self {
            database_url,
            host,
            port,
            cors,
            body_limit_bytes,
            otel_enabled,
            mcp,
            rendermesh_manifest,
        })
    }

    pub fn listen_addr(&self) -> Result<SocketAddr> {
        Ok(SocketAddr::new(self.host, self.port))
    }

    pub fn mcp_endpoint_url(&self) -> Option<String> {
        self.mcp
            .enabled
            .then(|| format!("http://{}:{}{}", self.host, self.port, self.mcp.path))
    }
}

pub fn otel_enabled_from_env() -> bool {
    parse_env_bool("OTEL_ENABLED").unwrap_or(true)
}

fn parse_env_bool(key: &str) -> Option<bool> {
    let value = std::env::var(key).ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_csv_env(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(|entry| {
            let trimmed = entry.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect()
}

fn normalize_path(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return DEFAULT_MCP_PATH.to_string();
    }

    let with_prefix = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };

    if with_prefix.len() > 1 && with_prefix.ends_with('/') {
        with_prefix.trim_end_matches('/').to_string()
    } else {
        with_prefix
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{LazyLock, Mutex};

    use super::{otel_enabled_from_env, AppConfig, CorsConfig, DEFAULT_MCP_PATH};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn otel_is_enabled_by_default() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _env_guard = EnvVarGuard::unset("OTEL_ENABLED");

        assert!(otel_enabled_from_env());
    }

    #[test]
    fn otel_can_be_disabled_via_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _env_guard = EnvVarGuard::set("OTEL_ENABLED", "false");

        assert!(!otel_enabled_from_env());
    }

    #[test]
    fn invalid_enable_value_keeps_otel_enabled() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _env_guard = EnvVarGuard::set("OTEL_ENABLED", "maybe");

        assert!(otel_enabled_from_env());
    }

    #[test]
    fn mcp_defaults_are_applied() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _database_url = EnvVarGuard::set("DATABASE_URL", "postgres://localhost/template");
        let _mcp_enabled = EnvVarGuard::unset("MCP_ENABLED");
        let _mcp_path = EnvVarGuard::unset("MCP_PATH");
        let _mcp_allowed_origins = EnvVarGuard::unset("MCP_ALLOWED_ORIGINS");
        let _app_host = EnvVarGuard::unset("APP_HOST");

        let config = AppConfig::from_env().expect("config should parse");

        assert!(!config.mcp.enabled);
        assert_eq!(config.mcp.path, DEFAULT_MCP_PATH);
        assert!(matches!(config.mcp.cors, CorsConfig::Permissive));
    }

    #[test]
    fn mcp_path_and_allowed_origins_are_normalized() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _database_url = EnvVarGuard::set("DATABASE_URL", "postgres://localhost/template");
        let _mcp_path = EnvVarGuard::set("MCP_PATH", " custom/mcp/ ");
        let _mcp_allowed_origins = EnvVarGuard::set(
            "MCP_ALLOWED_ORIGINS",
            " http://localhost:6274 , http://127.0.0.1:6274 ",
        );

        let config = AppConfig::from_env().expect("config should parse");

        assert_eq!(config.mcp.path, "/custom/mcp");
        match config.mcp.cors {
            CorsConfig::Restricted(origins) => assert_eq!(
                origins,
                vec![
                    "http://localhost:6274".to_string(),
                    "http://127.0.0.1:6274".to_string()
                ]
            ),
            CorsConfig::Permissive => panic!("expected restricted MCP origins"),
        }
    }

    #[test]
    fn mcp_wildcard_enables_permissive_cors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _database_url = EnvVarGuard::set("DATABASE_URL", "postgres://localhost/template");
        let _mcp_allowed_origins = EnvVarGuard::set("MCP_ALLOWED_ORIGINS", "*");

        let config = AppConfig::from_env().expect("config should parse");

        assert!(matches!(config.mcp.cors, CorsConfig::Permissive));
    }

    #[test]
    fn mcp_endpoint_url_is_reported_when_enabled() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _database_url = EnvVarGuard::set("DATABASE_URL", "postgres://localhost/template");
        let _mcp_enabled = EnvVarGuard::set("MCP_ENABLED", "true");
        let _app_host = EnvVarGuard::set("APP_HOST", "0.0.0.0");
        let _app_port = EnvVarGuard::set("APP_PORT", "3000");
        let _mcp_path = EnvVarGuard::set("MCP_PATH", "/custom-mcp");

        let config = AppConfig::from_env().expect("config should parse");

        assert_eq!(
            config.mcp_endpoint_url().as_deref(),
            Some("http://0.0.0.0:3000/custom-mcp")
        );
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, original }
        }

        fn unset(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.original.as_ref() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}
