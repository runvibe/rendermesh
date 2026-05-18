use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::Result;

pub const DEFAULT_BODY_LIMIT_BYTES: usize = 1_048_576;

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
pub struct AppConfig {
    pub host: IpAddr,
    pub port: u16,
    pub cors: CorsConfig,
    pub body_limit_bytes: usize,
    pub otel_enabled: bool,
    pub rendermesh_manifest: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
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
        let rendermesh_manifest = std::env::var("RENDERMESH_MANIFEST")
            .unwrap_or_else(|_| "./rendermesh.yaml".to_string());

        Ok(Self {
            host,
            port,
            cors,
            body_limit_bytes,
            otel_enabled,
            rendermesh_manifest,
        })
    }

    pub fn listen_addr(&self) -> Result<SocketAddr> {
        Ok(SocketAddr::new(self.host, self.port))
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

#[cfg(test)]
mod tests {
    use std::sync::{LazyLock, Mutex};

    use super::{otel_enabled_from_env, AppConfig};

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
    fn app_config_does_not_require_external_database_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let _database_url = EnvVarGuard::unset("DATABASE_URL");

        let config = AppConfig::from_env().expect("config should parse without a database");

        assert_eq!(config.rendermesh_manifest, "./rendermesh.yaml");
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
