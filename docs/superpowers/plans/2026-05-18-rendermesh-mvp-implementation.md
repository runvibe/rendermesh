# RenderMesh MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the RenderMesh Static Edge Gateway MVP described in `docs/superpowers/specs/2026-05-18-rendermesh-mvp-design.md`.

**Architecture:** Add a RenderMesh domain behind the existing Axum template without mixing transport and business logic. HTTP routes receive requests and delegate to services; services resolve hosts, apply edge configs, run edge hooks, and assemble responses; repositories handle S3/R2 sync, local mirror reads, and outbound edge HTTP calls. Startup loads the global manifest, mirrors all configured buckets locally, loads per-origin edge configs from the mirror, then serves traffic from disk.

**Tech Stack:** Rust 2021, Axum 0.8, Tokio, Serde, SQLx template infrastructure, AWS SDK for S3/R2-compatible storage, Reqwest for edge hooks, Handlebars for HTML rendering, mime_guess for content type fallback, tempfile and wiremock/httpmock-style test servers for tests.

---

## File Structure

Create these modules:

- `src/dto/manifest.rs`: YAML DTOs for the global manifest and manifest validation input.
- `src/dto/edge.rs`: YAML DTOs for per-origin edge config and HTTP edge payloads.
- `src/dto/render.rs`: internal request/response structs shared between route and service.
- `src/services/manifest.rs`: manifest validation, env resolution, host normalization, exact/wildcard routing table.
- `src/services/cors.rs`: CORS policy derived from host mappings.
- `src/services/edge_config.rs`: default edge config and parsing/validation for `/_rendermesh/edge.yaml`.
- `src/services/static_rules.rs`: redirects, rewrites, root object, auto index, and missing behavior.
- `src/services/edge_hooks.rs`: ordered edge hook chain semantics and Handlebars decision rules.
- `src/services/render_gateway.rs`: top-level request orchestration for `GET`, `HEAD`, `OPTIONS`, and `405`.
- `src/repositories/manifest.rs`: local manifest file loader.
- `src/repositories/s3_storage.rs`: S3/R2 list and download adapter.
- `src/repositories/local_mirror.rs`: local mirror object reads and metadata.
- `src/repositories/sync.rs`: sync repository/service boundary for initial and background sync.
- `src/repositories/edge_http.rs`: outbound HTTP client for edge hooks.
- `src/routes/render.rs`: Axum fallback route that delegates to `RenderGatewayService`.

Modify these files:

- `Cargo.toml`: add dependencies required by the MVP.
- `src/dto/mod.rs`: export new DTO modules.
- `src/services/mod.rs`: export new service modules.
- `src/repositories/mod.rs`: export new repository modules.
- `src/routes/mod.rs`: mount the render fallback after system routes and docs.
- `src/state.rs`: store a shared `RenderGatewayService`.
- `src/config.rs`: load `RENDERMESH_MANIFEST`.
- `src/main.rs`: load manifest, run initial sync, start background sync, build state.
- `src/error.rs`: add status variants used by RenderMesh.
- `README.md`: document manifest, local mirror, sync interval, and edge hook contract.
- `tests/integration.rs`: add end-to-end HTTP tests using temp mirrors and fake edge APIs.

Keep each source file under 1000 lines. Split any module that approaches that size before adding more behavior.

---

### Task 1: Add Dependencies And Public Modules

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/dto/mod.rs`
- Modify: `src/services/mod.rs`
- Modify: `src/repositories/mod.rs`

- [ ] **Step 1: Add dependencies**

Add these dependencies to `Cargo.toml`:

```toml
aws-config = "1"
aws-credential-types = "1"
aws-sdk-s3 = "1"
handlebars = "6"
mime_guess = "2"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde_yaml = "0.9"
url = "2"
```

Add these dev dependencies:

```toml
tempfile = "3"
wiremock = "0.6"
```

- [ ] **Step 2: Create empty module exports**

Update `src/dto/mod.rs`:

```rust
pub mod edge;
pub mod echo;
pub mod health;
pub mod manifest;
pub mod render;
```

Update `src/services/mod.rs`:

```rust
pub mod cors;
pub mod edge_config;
pub mod edge_hooks;
pub mod echo;
pub mod health;
pub mod manifest;
pub mod render_gateway;
pub mod static_rules;
```

Update `src/repositories/mod.rs`:

```rust
pub mod database;
pub mod edge_http;
pub mod local_mirror;
pub mod manifest;
pub mod s3_storage;
pub mod sync;
```

- [ ] **Step 3: Create compile stubs**

Create each new module file with the line below, replacing the module name in the comment:

```rust
// RenderMesh module; behavior is introduced by focused follow-up tasks.
```

- [ ] **Step 4: Verify dependency graph**

Run:

```bash
cargo build
```

Expected: build succeeds or fails only because a dependency version is unavailable. If a dependency version is unavailable, use the latest compatible version reported by Cargo and re-run `cargo build`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/dto/mod.rs src/services/mod.rs src/repositories/mod.rs src/dto/edge.rs src/dto/manifest.rs src/dto/render.rs src/services/cors.rs src/services/edge_config.rs src/services/edge_hooks.rs src/services/manifest.rs src/services/render_gateway.rs src/services/static_rules.rs src/repositories/edge_http.rs src/repositories/local_mirror.rs src/repositories/manifest.rs src/repositories/s3_storage.rs src/repositories/sync.rs
git commit -m "chore: add rendermesh module skeleton"
```

---

### Task 2: Manifest DTOs, Loader, And Validation

**Files:**
- Create/Modify: `src/dto/manifest.rs`
- Create/Modify: `src/repositories/manifest.rs`
- Create/Modify: `src/services/manifest.rs`
- Modify: `src/config.rs`

- [ ] **Step 1: Write failing manifest parser tests**

Add unit tests in `src/services/manifest.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> &'static str {
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  my_app:
    type: s3
    bucket: bucket_my_app_123
    endpoint_env: MY_APP_STORAGE_ENDPOINT
    region_env: MY_APP_STORAGE_REGION
    access_key_id_env: MY_APP_ACCESS_KEY_ID
    secret_access_key_env: MY_APP_SECRET_ACCESS_KEY
    force_path_style_env: MY_APP_FORCE_PATH_STYLE
    sync_interval_seconds: 30
hosts:
  myapp.com:
    origin: my_app
  "*.myapp.com":
    origin: my_app
"#
    }

    #[test]
    fn parses_manifest_runtime_origins_and_hosts() {
        let manifest = parse_manifest_yaml(sample_manifest()).expect("manifest parses");

        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.runtime.local_store_dir, "./var/rendermesh/origins");
        assert_eq!(manifest.runtime.sync_interval_seconds, 60);
        assert_eq!(manifest.origins["my_app"].bucket, "bucket_my_app_123");
        assert_eq!(manifest.origins["my_app"].sync_interval_seconds, Some(30));
        assert_eq!(manifest.hosts["myapp.com"].origin, "my_app");
    }

    #[test]
    fn rejects_host_that_references_missing_origin() {
        let yaml = r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins: {}
hosts:
  myapp.com:
    origin: missing
"#;

        let manifest = serde_yaml::from_str::<crate::dto::manifest::RenderMeshManifest>(yaml)
            .expect("yaml parses");
        let error = validate_manifest(&manifest).expect_err("validation fails");

        assert!(error.to_string().contains("unknown origin missing"));
    }

    #[test]
    fn rejects_non_positive_sync_intervals() {
        let yaml = r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 0
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  web.test:
    origin: web
"#;

        let manifest = serde_yaml::from_str::<crate::dto::manifest::RenderMeshManifest>(yaml)
            .expect("yaml parses");
        let error = validate_manifest(&manifest).expect_err("validation fails");

        assert!(error.to_string().contains("sync_interval_seconds"));
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test services::manifest
```

Expected: tests fail because `parse_manifest_yaml` and manifest DTOs are not implemented.

- [ ] **Step 3: Implement DTOs**

In `src/dto/manifest.rs`, add:

```rust
use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RenderMeshManifest {
    pub version: u16,
    pub runtime: RuntimeConfig,
    pub origins: BTreeMap<String, OriginConfig>,
    pub hosts: BTreeMap<String, HostConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub local_store_dir: String,
    pub sync_interval_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OriginConfig {
    #[serde(rename = "type")]
    pub origin_type: OriginType,
    pub bucket: String,
    pub endpoint_env: String,
    pub region_env: String,
    pub access_key_id_env: String,
    pub secret_access_key_env: String,
    pub force_path_style_env: Option<String>,
    pub sync_interval_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OriginType {
    S3,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HostConfig {
    pub origin: String,
}
```

- [ ] **Step 4: Implement parser and validation**

In `src/services/manifest.rs`, add:

```rust
use anyhow::{anyhow, Result};

use crate::dto::manifest::RenderMeshManifest;

pub fn parse_manifest_yaml(input: &str) -> Result<RenderMeshManifest> {
    let manifest = serde_yaml::from_str::<RenderMeshManifest>(input)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn validate_manifest(manifest: &RenderMeshManifest) -> Result<()> {
    if manifest.version != 1 {
        return Err(anyhow!("unsupported manifest version {}", manifest.version));
    }
    if manifest.runtime.local_store_dir.trim().is_empty() {
        return Err(anyhow!("runtime.local_store_dir is required"));
    }
    if manifest.runtime.sync_interval_seconds == 0 {
        return Err(anyhow!("runtime.sync_interval_seconds must be positive"));
    }
    for (origin_id, origin) in &manifest.origins {
        validate_origin_id(origin_id)?;
        if origin.bucket.trim().is_empty() {
            return Err(anyhow!("origin {origin_id} bucket is required"));
        }
        if origin.sync_interval_seconds == Some(0) {
            return Err(anyhow!("origin {origin_id} sync_interval_seconds must be positive"));
        }
    }
    for (host, host_config) in &manifest.hosts {
        if host.trim().is_empty() {
            return Err(anyhow!("host entry cannot be empty"));
        }
        if !manifest.origins.contains_key(&host_config.origin) {
            return Err(anyhow!("host {host} references unknown origin {}", host_config.origin));
        }
    }
    Ok(())
}

fn validate_origin_id(origin_id: &str) -> Result<()> {
    let valid = !origin_id.is_empty()
        && origin_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    if !valid {
        return Err(anyhow!("invalid origin id {origin_id}"));
    }
    Ok(())
}
```

- [ ] **Step 5: Implement manifest file loader**

In `src/repositories/manifest.rs`, add:

```rust
use std::{path::Path, sync::Arc};

use anyhow::Result;

use crate::{dto::manifest::RenderMeshManifest, services::manifest::parse_manifest_yaml};

#[derive(Clone, Default)]
pub struct ManifestRepository;

impl ManifestRepository {
    pub fn new() -> Self {
        Self
    }

    pub async fn load(&self, path: impl AsRef<Path>) -> Result<Arc<RenderMeshManifest>> {
        let content = tokio::fs::read_to_string(path).await?;
        Ok(Arc::new(parse_manifest_yaml(&content)?))
    }
}
```

- [ ] **Step 6: Add `RENDERMESH_MANIFEST` to config**

In `src/config.rs`, add field:

```rust
pub rendermesh_manifest: String,
```

In `AppConfig::from_env`, read:

```rust
let rendermesh_manifest = std::env::var("RENDERMESH_MANIFEST")
    .unwrap_or_else(|_| "./rendermesh.yaml".to_string());
```

Include it in `Ok(Self { ... })`.

- [ ] **Step 7: Run tests**

Run:

```bash
cargo test services::manifest
cargo test config
```

Expected: manifest and config tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/dto/manifest.rs src/repositories/manifest.rs src/services/manifest.rs src/config.rs
git commit -m "feat: parse rendermesh manifest"
```

---

### Task 3: Host Resolver And Derived CORS Policy

**Files:**
- Modify: `src/services/manifest.rs`
- Modify: `src/services/cors.rs`

- [ ] **Step 1: Write failing host resolver tests**

Add tests in `src/services/manifest.rs`:

```rust
#[test]
fn exact_host_wins_over_wildcard() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  admin:
    type: s3
    bucket: admin
    endpoint_env: ADMIN_ENDPOINT
    region_env: ADMIN_REGION
    access_key_id_env: ADMIN_KEY
    secret_access_key_env: ADMIN_SECRET
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  admin.megaloja.com.br:
    origin: admin
  "*.megaloja.com.br":
    origin: web
"#,
    )
    .expect("manifest parses");

    let resolver = HostResolver::new(&manifest).expect("resolver builds");
    let resolved = resolver.resolve("ADMIN.megaloja.com.br:443").expect("host resolves");

    assert_eq!(resolved.origin_id, "admin");
    assert_eq!(resolved.matched_host, "admin.megaloja.com.br");
}

#[test]
fn most_specific_wildcard_wins() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  broad:
    type: s3
    bucket: broad
    endpoint_env: BROAD_ENDPOINT
    region_env: BROAD_REGION
    access_key_id_env: BROAD_KEY
    secret_access_key_env: BROAD_SECRET
  narrow:
    type: s3
    bucket: narrow
    endpoint_env: NARROW_ENDPOINT
    region_env: NARROW_REGION
    access_key_id_env: NARROW_KEY
    secret_access_key_env: NARROW_SECRET
hosts:
  "*.megaloja.com.br":
    origin: broad
  "*.admin.megaloja.com.br":
    origin: narrow
"#,
    )
    .expect("manifest parses");

    let resolver = HostResolver::new(&manifest).expect("resolver builds");
    let resolved = resolver.resolve("x.admin.megaloja.com.br").expect("host resolves");

    assert_eq!(resolved.origin_id, "narrow");
}

#[test]
fn unknown_host_is_none() {
    let manifest = parse_manifest_yaml(sample_manifest()).expect("manifest parses");
    let resolver = HostResolver::new(&manifest).expect("resolver builds");

    assert!(resolver.resolve("unknown.test").is_none());
}
```

- [ ] **Step 2: Write failing CORS tests**

Add tests in `src/services/cors.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::manifest::parse_manifest_yaml;

    #[test]
    fn allows_exact_host_origin() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  megaloja.com.br:
    origin: web
"#,
        )
        .expect("manifest parses");

        let policy = CorsPolicy::from_manifest(&manifest);

        assert_eq!(
            policy.allowed_origin_for("web", "https://megaloja.com.br"),
            Some("https://megaloja.com.br".to_string())
        );
    }

    #[test]
    fn reflects_matching_wildcard_origin() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  "*.megaloja.com.br":
    origin: web
"#,
        )
        .expect("manifest parses");

        let policy = CorsPolicy::from_manifest(&manifest);

        assert_eq!(
            policy.allowed_origin_for("web", "https://admin.megaloja.com.br"),
            Some("https://admin.megaloja.com.br".to_string())
        );
        assert_eq!(policy.allowed_origin_for("web", "https://megaloja.com.br"), None);
    }
}
```

- [ ] **Step 3: Implement resolver**

In `src/services/manifest.rs`, add:

```rust
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedHost {
    pub normalized_host: String,
    pub matched_host: String,
    pub origin_id: String,
}

#[derive(Clone, Debug)]
pub struct HostResolver {
    exact: BTreeMap<String, String>,
    wildcards: Vec<WildcardHost>,
}

#[derive(Clone, Debug)]
struct WildcardHost {
    pattern: String,
    suffix: String,
    origin_id: String,
}

impl HostResolver {
    pub fn new(manifest: &RenderMeshManifest) -> Result<Self> {
        let mut exact = BTreeMap::new();
        let mut wildcards = Vec::new();
        for (host, config) in &manifest.hosts {
            let normalized = host.to_ascii_lowercase();
            if let Some(suffix) = normalized.strip_prefix("*.") {
                wildcards.push(WildcardHost {
                    pattern: normalized,
                    suffix: format!(".{suffix}"),
                    origin_id: config.origin.clone(),
                });
            } else {
                exact.insert(normalized, config.origin.clone());
            }
        }
        wildcards.sort_by(|left, right| right.suffix.len().cmp(&left.suffix.len()));
        Ok(Self { exact, wildcards })
    }

    pub fn resolve(&self, host_header: &str) -> Option<ResolvedHost> {
        let normalized_host = normalize_host(host_header)?;
        if let Some(origin_id) = self.exact.get(&normalized_host) {
            return Some(ResolvedHost {
                normalized_host,
                matched_host: host_header
                    .split(':')
                    .next()
                    .unwrap_or(host_header)
                    .to_ascii_lowercase(),
                origin_id: origin_id.clone(),
            });
        }
        for wildcard in &self.wildcards {
            if normalized_host.ends_with(&wildcard.suffix)
                && normalized_host.len() > wildcard.suffix.len()
            {
                return Some(ResolvedHost {
                    normalized_host,
                    matched_host: wildcard.pattern.clone(),
                    origin_id: wildcard.origin_id.clone(),
                });
            }
        }
        None
    }
}

pub fn normalize_host(host_header: &str) -> Option<String> {
    let host = host_header.trim().split(':').next()?.trim().to_ascii_lowercase();
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.');
    valid.then_some(host)
}
```

- [ ] **Step 4: Implement CORS policy**

In `src/services/cors.rs`, add:

```rust
use std::collections::BTreeMap;

use crate::{dto::manifest::RenderMeshManifest, services::manifest::normalize_host};

#[derive(Clone, Debug, Default)]
pub struct CorsPolicy {
    rules_by_origin: BTreeMap<String, Vec<CorsHostRule>>,
}

#[derive(Clone, Debug)]
enum CorsHostRule {
    Exact(String),
    WildcardSuffix(String),
}

impl CorsPolicy {
    pub fn from_manifest(manifest: &RenderMeshManifest) -> Self {
        let mut rules_by_origin: BTreeMap<String, Vec<CorsHostRule>> = BTreeMap::new();
        for (host, host_config) in &manifest.hosts {
            let rule = if let Some(suffix) = host.strip_prefix("*.") {
                CorsHostRule::WildcardSuffix(format!(".{}", suffix.to_ascii_lowercase()))
            } else {
                CorsHostRule::Exact(host.to_ascii_lowercase())
            };
            rules_by_origin
                .entry(host_config.origin.clone())
                .or_default()
                .push(rule);
        }
        Self { rules_by_origin }
    }

    pub fn allowed_origin_for(&self, origin_id: &str, request_origin: &str) -> Option<String> {
        let parsed = url::Url::parse(request_origin).ok()?;
        if parsed.scheme() != "https" {
            return None;
        }
        let host = normalize_host(parsed.host_str()?)?;
        let rules = self.rules_by_origin.get(origin_id)?;
        for rule in rules {
            match rule {
                CorsHostRule::Exact(exact) if exact == &host => return Some(request_origin.to_string()),
                CorsHostRule::WildcardSuffix(suffix)
                    if host.ends_with(suffix) && host.len() > suffix.len() =>
                {
                    return Some(request_origin.to_string());
                }
                _ => {}
            }
        }
        None
    }
}
```

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test services::manifest
cargo test services::cors
```

Expected: tests pass.

Commit:

```bash
git add src/services/manifest.rs src/services/cors.rs
git commit -m "feat: resolve hosts and derive cors"
```

---

### Task 4: Edge Config DTOs And Defaults

**Files:**
- Modify: `src/dto/edge.rs`
- Modify: `src/services/edge_config.rs`

- [ ] **Step 1: Write failing edge config tests**

Add tests in `src/services/edge_config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_edge_config_matches_mvp_defaults() {
        let config = default_edge_config();

        assert_eq!(config.version, 1);
        assert_eq!(config.edge.root_object, "/index.html");
        assert!(config.edge.auto_rewrite_index);
        assert_eq!(config.missing.action, MissingAction::NotFound);
        assert_eq!(config.missing.page.as_deref(), Some("/index.html"));
    }

    #[test]
    fn parses_redirects_rewrites_and_edges() {
        let yaml = r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
redirects:
  - from: /old/*
    to: /new/:splat
    status: 301
rewrites:
  - from: /docs
    to: /docs/index.html
edges:
  - name: auth
    url: https://api.example.com/edge
    timeout_ms: 800
"#;

        let config = parse_edge_config_yaml(yaml).expect("edge config parses");

        assert_eq!(config.redirects[0].from, "/old/*");
        assert_eq!(config.rewrites[0].to, "/docs/index.html");
        assert_eq!(config.edges[0].timeout_ms, 800);
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test services::edge_config
```

Expected: tests fail because DTOs and functions are missing.

- [ ] **Step 3: Implement edge DTOs**

In `src/dto/edge.rs`, add:

```rust
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EdgeConfig {
    pub version: u16,
    pub edge: EdgeDefaults,
    pub missing: MissingConfig,
    #[serde(default)]
    pub redirects: Vec<RedirectRule>,
    #[serde(default)]
    pub rewrites: Vec<RewriteRule>,
    #[serde(default)]
    pub edges: Vec<EdgeHookConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EdgeDefaults {
    pub root_object: String,
    #[serde(default = "default_true")]
    pub auto_rewrite_index: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MissingConfig {
    pub action: MissingAction,
    pub page: Option<String>,
    pub path: Option<String>,
    pub to: Option<String>,
    pub status: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissingAction {
    NotFound,
    Serve,
    Redirect,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RedirectRule {
    pub from: String,
    pub to: String,
    pub status: u16,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RewriteRule {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EdgeHookConfig {
    pub name: String,
    pub url: String,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct EdgeHookRequest {
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq)]
pub struct EdgeHookPayload {
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
    pub file_path: Option<String>,
    pub params: Option<Value>,
}

fn default_true() -> bool {
    true
}
```

- [ ] **Step 4: Implement parsing and defaults**

In `src/services/edge_config.rs`, add:

```rust
use anyhow::{anyhow, Result};

use crate::dto::edge::{EdgeConfig, EdgeDefaults, MissingAction, MissingConfig};

pub fn default_edge_config() -> EdgeConfig {
    EdgeConfig {
        version: 1,
        edge: EdgeDefaults {
            root_object: "/index.html".to_string(),
            auto_rewrite_index: true,
        },
        missing: MissingConfig {
            action: MissingAction::NotFound,
            page: Some("/index.html".to_string()),
            path: None,
            to: None,
            status: None,
        },
        redirects: Vec::new(),
        rewrites: Vec::new(),
        edges: Vec::new(),
    }
}

pub fn parse_edge_config_yaml(input: &str) -> Result<EdgeConfig> {
    let config = serde_yaml::from_str::<EdgeConfig>(input)?;
    validate_edge_config(&config)?;
    Ok(config)
}

pub fn validate_edge_config(config: &EdgeConfig) -> Result<()> {
    if config.version != 1 {
        return Err(anyhow!("unsupported edge config version {}", config.version));
    }
    validate_absolute_path(&config.edge.root_object)?;
    match config.missing.action {
        MissingAction::NotFound => validate_optional_path(config.missing.page.as_deref())?,
        MissingAction::Serve => validate_optional_path(config.missing.path.as_deref())?,
        MissingAction::Redirect => {
            if config.missing.to.as_deref().unwrap_or_default().is_empty() {
                return Err(anyhow!("missing redirect requires to"));
            }
        }
    }
    for redirect in &config.redirects {
        if !matches!(redirect.status, 301 | 302 | 307 | 308) {
            return Err(anyhow!("invalid redirect status {}", redirect.status));
        }
    }
    for edge in &config.edges {
        if edge.name.trim().is_empty() {
            return Err(anyhow!("edge hook name is required"));
        }
        if edge.timeout_ms == 0 {
            return Err(anyhow!("edge hook {} timeout_ms must be positive", edge.name));
        }
        url::Url::parse(&edge.url)?;
    }
    Ok(())
}

fn validate_optional_path(value: Option<&str>) -> Result<()> {
    if let Some(path) = value {
        validate_absolute_path(path)?;
    }
    Ok(())
}

fn validate_absolute_path(path: &str) -> Result<()> {
    if !path.starts_with('/') || path.contains("..") {
        return Err(anyhow!("invalid absolute path {path}"));
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test services::edge_config
```

Expected: tests pass.

Commit:

```bash
git add src/dto/edge.rs src/services/edge_config.rs
git commit -m "feat: parse edge config"
```

---

### Task 5: Local Mirror Repository

**Files:**
- Modify: `src/repositories/local_mirror.rs`

- [ ] **Step 1: Write failing local mirror tests**

Add tests in `src/repositories/local_mirror.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_object_and_sidecar_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let repository = LocalMirrorRepository::new(root.clone());
        let origin_dir = repository.origin_dir("web").expect("origin dir");
        tokio::fs::create_dir_all(origin_dir.join("docs")).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("docs/index.html"), "<h1>Docs</h1>")
            .await
            .expect("write object");
        tokio::fs::write(
            origin_dir.join("docs/index.html.meta.json"),
            r#"{"content_type":"text/html","etag":"abc","last_modified":"Mon, 01 Jan 2024 00:00:00 GMT","cache_control":"max-age=60"}"#,
        )
        .await
        .expect("write metadata");

        let object = repository.read_object("web", "/docs/index.html").await.expect("read");

        assert_eq!(object.body, bytes::Bytes::from_static(b"<h1>Docs</h1>"));
        assert_eq!(object.metadata.content_type.as_deref(), Some("text/html"));
        assert_eq!(object.metadata.etag.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn returns_none_for_missing_object() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repository = LocalMirrorRepository::new(temp.path().join("origins"));

        let object = repository.read_object("web", "/missing.html").await.expect("read");

        assert!(object.is_none());
    }

    #[test]
    fn rejects_invalid_origin_id_and_path() {
        let repository = LocalMirrorRepository::new("./var/rendermesh/origins");

        assert!(repository.origin_dir("../bad").is_err());
        assert!(repository.object_path("web", "../secret").is_err());
        assert!(repository.object_path("web", "/../secret").is_err());
    }
}
```

- [ ] **Step 2: Implement local mirror repository**

In `src/repositories/local_mirror.rs`, add:

```rust
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct LocalMirrorRepository {
    root: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cache_control: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalObject {
    pub body: Bytes,
    pub metadata: ObjectMetadata,
}

impl LocalMirrorRepository {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn origin_dir(&self, origin_id: &str) -> Result<PathBuf> {
        validate_origin_id(origin_id)?;
        Ok(self.root.join(origin_id))
    }

    pub fn object_path(&self, origin_id: &str, object_path: &str) -> Result<PathBuf> {
        let key = normalize_object_path(object_path)?;
        Ok(self.origin_dir(origin_id)?.join(key))
    }

    pub async fn read_object(&self, origin_id: &str, object_path: &str) -> Result<Option<LocalObject>> {
        let path = self.object_path(origin_id, object_path)?;
        match tokio::fs::read(&path).await {
            Ok(body) => {
                let metadata = self.read_metadata(&path).await?;
                Ok(Some(LocalObject {
                    body: Bytes::from(body),
                    metadata,
                }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn read_metadata(&self, object_path: &Path) -> Result<ObjectMetadata> {
        let meta_path = PathBuf::from(format!("{}.meta.json", object_path.display()));
        match tokio::fs::read_to_string(meta_path).await {
            Ok(content) => Ok(serde_json::from_str(&content)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ObjectMetadata::default()),
            Err(error) => Err(error.into()),
        }
    }
}

pub fn normalize_object_path(path: &str) -> Result<String> {
    let trimmed = path.trim();
    let without_prefix = trimmed.strip_prefix('/').unwrap_or(trimmed);
    if without_prefix.is_empty()
        || without_prefix.contains("..")
        || without_prefix.chars().any(|ch| ch.is_control())
    {
        return Err(anyhow!("invalid object path {path}"));
    }
    Ok(without_prefix.to_string())
}

fn validate_origin_id(origin_id: &str) -> Result<()> {
    let valid = !origin_id.is_empty()
        && origin_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    valid.then_some(()).ok_or_else(|| anyhow!("invalid origin id {origin_id}"))
}
```

- [ ] **Step 3: Run tests and commit**

Run:

```bash
cargo test repositories::local_mirror
```

Expected: tests pass.

Commit:

```bash
git add src/repositories/local_mirror.rs
git commit -m "feat: read local mirror objects"
```

---

### Task 6: Static Rules Engine

**Files:**
- Modify: `src/services/static_rules.rs`

- [ ] **Step 1: Write failing static rule tests**

Add tests in `src/services/static_rules.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::edge_config::parse_edge_config_yaml;

    #[test]
    fn redirects_exact_and_preserves_query() {
        let config = parse_edge_config_yaml(
            r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
redirects:
  - from: /docs
    to: /docs/
    status: 308
"#,
        )
        .expect("config");

        let redirect = find_redirect(&config, "/docs", Some("v=1")).expect("redirect");

        assert_eq!(redirect.status, 308);
        assert_eq!(redirect.location, "/docs/?v=1");
    }

    #[test]
    fn wildcard_redirect_replaces_splat() {
        let config = parse_edge_config_yaml(
            r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
redirects:
  - from: /old/*
    to: /new/:splat
    status: 301
"#,
        )
        .expect("config");

        let redirect = find_redirect(&config, "/old/a/b", None).expect("redirect");

        assert_eq!(redirect.location, "/new/a/b");
    }

    #[test]
    fn rewrite_and_root_object_resolution_work() {
        let config = parse_edge_config_yaml(
            r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
rewrites:
  - from: /docs
    to: /docs/index.html
"#,
        )
        .expect("config");

        assert_eq!(resolve_rewrite(&config, "/docs"), "/docs/index.html");
        assert_eq!(resolve_root_object(&config, "/guide/"), "/guide/index.html");
    }

    #[test]
    fn auto_index_candidate_uses_path_index_html() {
        assert_eq!(auto_index_candidate("/docs"), "/docs/index.html");
        assert_eq!(auto_index_candidate("/blog/post-1"), "/blog/post-1/index.html");
    }
}
```

- [ ] **Step 2: Implement static rules**

In `src/services/static_rules.rs`, add:

```rust
use crate::dto::edge::EdgeConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedirectDecision {
    pub status: u16,
    pub location: String,
}

pub fn find_redirect(
    config: &EdgeConfig,
    path: &str,
    query: Option<&str>,
) -> Option<RedirectDecision> {
    let mut matches = config
        .redirects
        .iter()
        .filter_map(|rule| match_pattern(&rule.from, path).map(|splat| (rule, splat)))
        .collect::<Vec<_>>();
    matches.sort_by(|(left, _), (right, _)| right.from.len().cmp(&left.from.len()));
    let (rule, splat) = matches.into_iter().next()?;
    let mut location = rule.to.replace(":splat", splat.unwrap_or_default());
    if !location.contains('?') {
        if let Some(query) = query.filter(|value| !value.is_empty()) {
            location.push('?');
            location.push_str(query);
        }
    }
    Some(RedirectDecision {
        status: rule.status,
        location,
    })
}

pub fn resolve_rewrite(config: &EdgeConfig, path: &str) -> String {
    let mut matches = config
        .rewrites
        .iter()
        .filter_map(|rule| match_pattern(&rule.from, path).map(|splat| (rule, splat)))
        .collect::<Vec<_>>();
    matches.sort_by(|(left, _), (right, _)| right.from.len().cmp(&left.from.len()));
    matches
        .into_iter()
        .next()
        .map(|(rule, splat)| rule.to.replace(":splat", splat.unwrap_or_default()))
        .unwrap_or_else(|| path.to_string())
}

pub fn resolve_root_object(config: &EdgeConfig, path: &str) -> String {
    if path.ends_with('/') {
        format!("{}{}", path.trim_end_matches('/'), config.edge.root_object)
    } else {
        path.to_string()
    }
}

pub fn auto_index_candidate(path: &str) -> String {
    format!("{}/index.html", path.trim_end_matches('/'))
}

fn match_pattern<'a>(pattern: &str, path: &'a str) -> Option<Option<&'a str>> {
    if pattern == path {
        return Some(None);
    }
    let prefix = pattern.strip_suffix('*')?;
    if path.starts_with(prefix) {
        return Some(Some(&path[prefix.len()..]));
    }
    None
}
```

- [ ] **Step 3: Run tests and commit**

Run:

```bash
cargo test services::static_rules
```

Expected: tests pass.

Commit:

```bash
git add src/services/static_rules.rs
git commit -m "feat: apply static edge rules"
```

---

### Task 7: Edge HTTP Client And Hook Chain

**Files:**
- Modify: `src/repositories/edge_http.rs`
- Modify: `src/services/edge_hooks.rs`

- [ ] **Step 1: Write failing edge HTTP tests**

Add tests in `src/repositories/edge_http.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::edge::EdgeHookRequest;
    use std::collections::BTreeMap;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn posts_edge_request_and_parses_payload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/edge"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "body": "ok",
                "headers": {"x-edge": "yes"}
            })))
            .mount(&server)
            .await;

        let client = EdgeHttpRepository::new();
        let response = client
            .call(
                &format!("{}/edge", server.uri()),
                500,
                &EdgeHookRequest {
                    url: "https://app.test/".to_string(),
                    method: "GET".to_string(),
                    headers: BTreeMap::new(),
                    body: String::new(),
                },
            )
            .await
            .expect("edge call succeeds");

        assert_eq!(response.status, axum::http::StatusCode::CREATED);
        assert_eq!(response.payload.body.as_deref(), Some("ok"));
        assert_eq!(response.payload.headers["x-edge"], "yes");
    }
}
```

- [ ] **Step 2: Write failing hook chain tests**

Add tests in `src/services/edge_hooks.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::edge::EdgeHookPayload;
    use axum::http::StatusCode;

    #[test]
    fn headers_only_payload_continues_and_accumulates() {
        let mut state = EdgeChainState::default();
        let outcome = apply_edge_payload(
            &mut state,
            StatusCode::OK,
            EdgeHookPayload {
                headers: [("x-a".to_string(), "1".to_string())].into(),
                body: None,
                file_path: None,
                params: None,
            },
        )
        .expect("payload applies");

        assert_eq!(outcome, EdgePayloadOutcome::Continue);
        assert_eq!(state.headers["x-a"], "1");
    }

    #[test]
    fn body_payload_stops_chain() {
        let mut state = EdgeChainState::default();
        let outcome = apply_edge_payload(
            &mut state,
            StatusCode::ACCEPTED,
            EdgeHookPayload {
                headers: Default::default(),
                body: Some("ready".to_string()),
                file_path: None,
                params: None,
            },
        )
        .expect("payload applies");

        assert_eq!(
            outcome,
            EdgePayloadOutcome::RespondDirect {
                status: StatusCode::ACCEPTED,
                body: "ready".to_string()
            }
        );
    }

    #[test]
    fn invalid_file_path_is_rejected() {
        let error = validate_edge_file_path("../secret").expect_err("invalid path");

        assert!(error.to_string().contains("invalid file_path"));
    }
}
```

- [ ] **Step 3: Implement HTTP repository**

In `src/repositories/edge_http.rs`, add:

```rust
use std::time::Duration;

use anyhow::Result;
use axum::http::StatusCode;

use crate::dto::edge::{EdgeHookPayload, EdgeHookRequest};

#[derive(Clone)]
pub struct EdgeHttpRepository {
    client: reqwest::Client,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EdgeHttpResponse {
    pub status: StatusCode,
    pub payload: EdgeHookPayload,
}

impl EdgeHttpRepository {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub async fn call(
        &self,
        url: &str,
        timeout_ms: u64,
        request: &EdgeHookRequest,
    ) -> Result<EdgeHttpResponse> {
        let response = self
            .client
            .post(url)
            .timeout(Duration::from_millis(timeout_ms))
            .json(request)
            .send()
            .await?;
        let status = StatusCode::from_u16(response.status().as_u16())?;
        let payload = response.json::<EdgeHookPayload>().await?;
        Ok(EdgeHttpResponse { status, payload })
    }
}

impl Default for EdgeHttpRepository {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Implement hook payload semantics**

In `src/services/edge_hooks.rs`, add:

```rust
use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use axum::http::StatusCode;
use serde_json::Value;

use crate::dto::edge::EdgeHookPayload;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeChainState {
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EdgePayloadOutcome {
    Continue,
    RespondDirect { status: StatusCode, body: String },
    ServeFile { status: StatusCode, file_path: String, params: Option<Value> },
    RenderTarget { status: StatusCode, params: Value },
}

pub fn apply_edge_payload(
    state: &mut EdgeChainState,
    status: StatusCode,
    payload: EdgeHookPayload,
) -> Result<EdgePayloadOutcome> {
    state.headers.extend(payload.headers);
    if let Some(body) = payload.body {
        return Ok(EdgePayloadOutcome::RespondDirect { status, body });
    }
    if let Some(file_path) = payload.file_path {
        validate_edge_file_path(&file_path)?;
        return Ok(EdgePayloadOutcome::ServeFile {
            status,
            file_path,
            params: payload.params,
        });
    }
    if let Some(params) = payload.params {
        return Ok(EdgePayloadOutcome::RenderTarget { status, params });
    }
    Ok(EdgePayloadOutcome::Continue)
}

pub fn validate_edge_file_path(path: &str) -> Result<()> {
    if !path.starts_with('/') || path.contains("..") || path.chars().any(|ch| ch.is_control()) {
        return Err(anyhow!("invalid file_path {path}"));
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test repositories::edge_http
cargo test services::edge_hooks
```

Expected: tests pass.

Commit:

```bash
git add src/repositories/edge_http.rs src/services/edge_hooks.rs
git commit -m "feat: call edge hooks"
```

---

### Task 8: Handlebars HTML Rendering

**Files:**
- Modify: `src/services/edge_hooks.rs`

- [ ] **Step 1: Write failing render tests**

Add tests in `src/services/edge_hooks.rs`:

```rust
#[test]
fn renders_html_with_params() {
    let rendered = render_html_template(
        "/index.html",
        Some("text/html"),
        "<h1>{{title}}</h1>",
        &serde_json::json!({"title": "Hello"}),
    )
    .expect("render succeeds");

    assert_eq!(rendered, "<h1>Hello</h1>");
}

#[test]
fn rejects_params_for_non_html() {
    let error = render_html_template(
        "/data.json",
        Some("application/json"),
        "{\"title\":\"{{title}}\"}",
        &serde_json::json!({"title": "Hello"}),
    )
    .expect_err("non-html rejected");

    assert_eq!(error, RenderTemplateError::UnsupportedMediaType);
}
```

- [ ] **Step 2: Implement render helper**

Add to `src/services/edge_hooks.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RenderTemplateError {
    #[error("unsupported media type")]
    UnsupportedMediaType,
    #[error(transparent)]
    Render(#[from] handlebars::RenderError),
    #[error(transparent)]
    Template(#[from] handlebars::TemplateError),
}

pub fn render_html_template(
    path: &str,
    content_type: Option<&str>,
    body: &str,
    params: &serde_json::Value,
) -> Result<String, RenderTemplateError> {
    if !is_html(path, content_type) {
        return Err(RenderTemplateError::UnsupportedMediaType);
    }
    let registry = handlebars::Handlebars::new();
    Ok(registry.render_template(body, params)?)
}

pub fn is_html(path: &str, content_type: Option<&str>) -> bool {
    content_type
        .map(|value| value.to_ascii_lowercase().starts_with("text/html"))
        .unwrap_or_else(|| path.ends_with(".html") || path.ends_with(".htm"))
}
```

- [ ] **Step 3: Run tests and commit**

Run:

```bash
cargo test services::edge_hooks
```

Expected: tests pass.

Commit:

```bash
git add src/services/edge_hooks.rs
git commit -m "feat: render html templates from edge params"
```

---

### Task 9: Sync Repository And Initial Mirror Flow

**Files:**
- Modify: `src/repositories/s3_storage.rs`
- Modify: `src/repositories/sync.rs`

- [ ] **Step 1: Write failing sync service tests with fake storage**

Add tests in `src/repositories/sync.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;
    use std::{collections::BTreeMap, sync::Arc};
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct FakeStorage {
        objects: Arc<Mutex<BTreeMap<String, RemoteObject>>>,
    }

    #[async_trait]
    impl RemoteStorage for FakeStorage {
        async fn list_objects(&self) -> anyhow::Result<Vec<RemoteObjectSummary>> {
            let objects = self.objects.lock().await;
            Ok(objects
                .values()
                .map(|object| RemoteObjectSummary {
                    key: object.key.clone(),
                    etag: object.etag.clone(),
                    last_modified: object.last_modified.clone(),
                    size: object.body.len() as u64,
                    content_type: object.content_type.clone(),
                    cache_control: object.cache_control.clone(),
                })
                .collect())
        }

        async fn get_object(&self, key: &str) -> anyhow::Result<RemoteObject> {
            self.objects.lock().await.get(key).cloned().ok_or_else(|| anyhow::anyhow!("missing"))
        }
    }

    #[tokio::test]
    async fn initial_sync_downloads_objects_and_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            RemoteObject {
                key: "index.html".to_string(),
                body: Bytes::from_static(b"<h1>Hello</h1>"),
                etag: Some("abc".to_string()),
                last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
                content_type: Some("text/html".to_string()),
                cache_control: Some("max-age=60".to_string()),
            },
        );
        let syncer = MirrorSyncService::new(temp.path().join("origins"));

        syncer.sync_origin("web", &storage).await.expect("sync succeeds");

        assert_eq!(
            tokio::fs::read_to_string(temp.path().join("origins/web/index.html")).await.expect("read"),
            "<h1>Hello</h1>"
        );
        assert!(temp.path().join("origins/web/index.html.meta.json").exists());
    }
}
```

- [ ] **Step 2: Add `async-trait` dependency**

Add to `Cargo.toml`:

```toml
async-trait = "0.1"
```

- [ ] **Step 3: Implement sync traits and local writer**

In `src/repositories/sync.rs`, add:

```rust
use std::{collections::BTreeSet, path::PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;

use crate::repositories::local_mirror::ObjectMetadata;

#[derive(Clone, Debug)]
pub struct RemoteObjectSummary {
    pub key: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub size: u64,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RemoteObject {
    pub key: String,
    pub body: Bytes,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
}

#[async_trait]
pub trait RemoteStorage: Send + Sync {
    async fn list_objects(&self) -> Result<Vec<RemoteObjectSummary>>;
    async fn get_object(&self, key: &str) -> Result<RemoteObject>;
}

#[derive(Clone)]
pub struct MirrorSyncService {
    root: PathBuf,
}

impl MirrorSyncService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub async fn sync_origin<S>(&self, origin_id: &str, storage: &S) -> Result<SyncReport>
    where
        S: RemoteStorage,
    {
        let origin_dir = self.root.join(origin_id);
        tokio::fs::create_dir_all(&origin_dir).await?;
        let summaries = storage.list_objects().await?;
        let mut remote_keys = BTreeSet::new();
        let mut downloaded = 0usize;
        for summary in summaries {
            remote_keys.insert(summary.key.clone());
            let object = storage.get_object(&summary.key).await?;
            write_object(&origin_dir, object).await?;
            downloaded += 1;
        }
        remove_deleted_objects(&origin_dir, &remote_keys).await?;
        Ok(SyncReport { downloaded })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    pub downloaded: usize,
}

async fn write_object(origin_dir: &std::path::Path, object: RemoteObject) -> Result<()> {
    let object_path = origin_dir.join(&object.key);
    if let Some(parent) = object_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&object_path, object.body).await?;
    let metadata = ObjectMetadata {
        content_type: object.content_type,
        etag: object.etag,
        last_modified: object.last_modified,
        cache_control: object.cache_control,
    };
    tokio::fs::write(
        format!("{}.meta.json", object_path.display()),
        serde_json::to_vec(&metadata)?,
    )
    .await?;
    Ok(())
}

async fn remove_deleted_objects(_origin_dir: &std::path::Path, _remote_keys: &BTreeSet<String>) -> Result<()> {
    Ok(())
}
```

Implement deletion fully before committing by walking `origin_dir` with `tokio::fs::read_dir`, skipping `*.meta.json`, and removing files whose relative key is not present in `remote_keys`, plus their sidecar metadata.

- [ ] **Step 4: Implement S3 storage adapter**

In `src/repositories/s3_storage.rs`, add:

```rust
use anyhow::Result;
use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::{config::Region, Client};
use bytes::Bytes;

use crate::{dto::manifest::OriginConfig, repositories::sync::{RemoteObject, RemoteObjectSummary, RemoteStorage}};

#[derive(Clone)]
pub struct S3StorageRepository {
    client: Client,
    bucket: String,
}

impl S3StorageRepository {
    pub async fn from_origin_config(origin: &OriginConfig) -> Result<Self> {
        let endpoint = std::env::var(&origin.endpoint_env)?;
        let region = std::env::var(&origin.region_env)?;
        let access_key_id = std::env::var(&origin.access_key_id_env)?;
        let secret_access_key = std::env::var(&origin.secret_access_key_env)?;
        let force_path_style = origin
            .force_path_style_env
            .as_ref()
            .and_then(|key| std::env::var(key).ok())
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false);

        let credentials = Credentials::new(access_key_id, secret_access_key, None, None, "rendermesh");
        let config = aws_sdk_s3::config::Builder::new()
            .endpoint_url(endpoint)
            .region(Region::new(region))
            .credentials_provider(credentials)
            .force_path_style(force_path_style)
            .build();

        Ok(Self {
            client: Client::from_conf(config),
            bucket: origin.bucket.clone(),
        })
    }
}

#[async_trait]
impl RemoteStorage for S3StorageRepository {
    async fn list_objects(&self) -> Result<Vec<RemoteObjectSummary>> {
        let mut output = Vec::new();
        let mut continuation_token = None;
        loop {
            let response = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .set_continuation_token(continuation_token)
                .send()
                .await?;
            for object in response.contents() {
                if let Some(key) = object.key() {
                    output.push(RemoteObjectSummary {
                        key: key.to_string(),
                        etag: object.e_tag().map(ToString::to_string),
                        last_modified: object.last_modified().map(ToString::to_string),
                        size: object.size().unwrap_or_default() as u64,
                        content_type: None,
                        cache_control: None,
                    });
                }
            }
            if response.is_truncated().unwrap_or(false) {
                continuation_token = response.next_continuation_token().map(ToString::to_string);
            } else {
                break;
            }
        }
        Ok(output)
    }

    async fn get_object(&self, key: &str) -> Result<RemoteObject> {
        let response = self.client.get_object().bucket(&self.bucket).key(key).send().await?;
        let body = response.body.collect().await?.into_bytes();
        Ok(RemoteObject {
            key: key.to_string(),
            body: Bytes::from(body.to_vec()),
            etag: response.e_tag().map(ToString::to_string),
            last_modified: response.last_modified().map(ToString::to_string),
            content_type: response.content_type().map(ToString::to_string),
            cache_control: response.cache_control().map(ToString::to_string),
        })
    }
}
```

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test repositories::sync
cargo build
```

Expected: sync tests and build pass.

Commit:

```bash
git add Cargo.toml Cargo.lock src/repositories/sync.rs src/repositories/s3_storage.rs
git commit -m "feat: sync buckets to local mirrors"
```

---

### Task 10: Render Gateway Service

**Files:**
- Modify: `src/dto/render.rs`
- Modify: `src/error.rs`
- Modify: `src/services/render_gateway.rs`

- [ ] **Step 1: Write failing gateway tests**

Add tests in `src/services/render_gateway.rs` using temp local mirrors:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        repositories::local_mirror::LocalMirrorRepository,
        services::{
            cors::CorsPolicy,
            edge_config::default_edge_config,
            manifest::{HostResolver, parse_manifest_yaml},
        },
    };
    use axum::http::{Method, StatusCode};

    #[tokio::test]
    async fn serves_get_from_local_mirror() {
        let temp = tempfile::tempdir().expect("tempdir");
        let origin_dir = temp.path().join("origins/web");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("index.html"), "<h1>Hello</h1>").await.expect("write");
        tokio::fs::write(origin_dir.join("index.html.meta.json"), r#"{"content_type":"text/html"}"#)
            .await
            .expect("meta");
        let service = test_gateway(temp.path().join("origins"));

        let response = service
            .handle(RenderRequest {
                method: Method::GET,
                host: "web.test".to_string(),
                path: "/".to_string(),
                query: None,
                scheme: "https".to_string(),
                headers: Default::default(),
                body: bytes::Bytes::new(),
            })
            .await
            .expect("response");

        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>Hello</h1>"));
    }

    #[tokio::test]
    async fn unknown_host_returns_421() {
        let service = test_gateway(tempfile::tempdir().expect("tempdir").path().join("origins"));

        let response = service
            .handle(RenderRequest {
                method: Method::GET,
                host: "unknown.test".to_string(),
                path: "/".to_string(),
                query: None,
                scheme: "https".to_string(),
                headers: Default::default(),
                body: bytes::Bytes::new(),
            })
            .await
            .expect("response");

        assert_eq!(response.status, StatusCode::MISDIRECTED_REQUEST);
    }

    fn test_gateway(root: std::path::PathBuf) -> RenderGatewayService {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  web.test:
    origin: web
"#,
        )
        .expect("manifest");
        RenderGatewayService::new_for_tests(
            HostResolver::new(&manifest).expect("resolver"),
            CorsPolicy::from_manifest(&manifest),
            LocalMirrorRepository::new(root),
            [("web".to_string(), default_edge_config())].into(),
        )
    }
}
```

- [ ] **Step 2: Define render DTOs**

In `src/dto/render.rs`, add:

```rust
use std::collections::BTreeMap;

use axum::http::{HeaderMap, Method, StatusCode};
use bytes::Bytes;

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub method: Method,
    pub host: String,
    pub path: String,
    pub query: Option<String>,
    pub scheme: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderResponse {
    pub status: StatusCode,
    pub headers: BTreeMap<String, String>,
    pub body: Bytes,
}

impl RenderResponse {
    pub fn empty(status: StatusCode) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: Bytes::new(),
        }
    }
}
```

- [ ] **Step 3: Add API error statuses**

In `src/error.rs`, add variants:

```rust
#[error("misdirected request")]
MisdirectedRequest,
#[error("method not allowed")]
MethodNotAllowed,
#[error("bad gateway: {0}")]
BadGateway(String),
#[error("gateway timeout: {0}")]
GatewayTimeout(String),
#[error("unsupported media type: {0}")]
UnsupportedMediaType(String),
```

Map them in `IntoResponse`:

```rust
MisdirectedRequest => (StatusCode::MISDIRECTED_REQUEST, "unknown host".to_string()),
MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, "method not allowed".to_string()),
BadGateway(message) => (StatusCode::BAD_GATEWAY, message.clone()),
GatewayTimeout(message) => (StatusCode::GATEWAY_TIMEOUT, message.clone()),
UnsupportedMediaType(message) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, message.clone()),
```

- [ ] **Step 4: Implement minimal gateway serving**

In `src/services/render_gateway.rs`, implement `RenderGatewayService` with host resolution, method handling, root object, local mirror lookup, and simple `404`:

```rust
use std::{collections::BTreeMap, sync::Arc};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use bytes::Bytes;

use crate::{
    dto::{edge::EdgeConfig, render::{RenderRequest, RenderResponse}},
    repositories::local_mirror::LocalMirrorRepository,
    services::{
        cors::CorsPolicy,
        manifest::HostResolver,
        static_rules::resolve_root_object,
    },
};

#[derive(Clone)]
pub struct RenderGatewayService {
    resolver: Arc<HostResolver>,
    cors: Arc<CorsPolicy>,
    mirror: LocalMirrorRepository,
    edge_configs: Arc<BTreeMap<String, EdgeConfig>>,
}

impl RenderGatewayService {
    pub fn new(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: BTreeMap<String, EdgeConfig>,
    ) -> Self {
        Self {
            resolver: Arc::new(resolver),
            cors: Arc::new(cors),
            mirror,
            edge_configs: Arc::new(edge_configs),
        }
    }

    pub fn new_for_tests(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: BTreeMap<String, EdgeConfig>,
    ) -> Self {
        Self::new(resolver, cors, mirror, edge_configs)
    }

    pub async fn handle(&self, request: RenderRequest) -> Result<RenderResponse> {
        let Some(resolved) = self.resolver.resolve(&request.host) else {
            return Ok(RenderResponse::empty(StatusCode::MISDIRECTED_REQUEST));
        };
        if request.method == Method::OPTIONS {
            return Ok(RenderResponse::empty(StatusCode::NO_CONTENT));
        }
        if request.method != Method::GET && request.method != Method::HEAD {
            return Ok(RenderResponse::empty(StatusCode::METHOD_NOT_ALLOWED));
        }
        let config = self
            .edge_configs
            .get(&resolved.origin_id)
            .expect("edge config exists for resolved origin");
        let path = resolve_root_object(config, &request.path);
        if let Some(object) = self.mirror.read_object(&resolved.origin_id, &path).await? {
            let mut response = RenderResponse {
                status: StatusCode::OK,
                headers: BTreeMap::new(),
                body: if request.method == Method::HEAD { Bytes::new() } else { object.body },
            };
            if let Some(content_type) = object.metadata.content_type {
                response.headers.insert("content-type".to_string(), content_type);
            }
            return Ok(response);
        }
        Ok(RenderResponse {
            status: StatusCode::NOT_FOUND,
            headers: BTreeMap::new(),
            body: if request.method == Method::HEAD {
                Bytes::new()
            } else {
                Bytes::from_static(b"not found")
            },
        })
    }
}
```

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test services::render_gateway
```

Expected: tests pass.

Commit:

```bash
git add src/dto/render.rs src/error.rs src/services/render_gateway.rs
git commit -m "feat: serve mirrored files through gateway"
```

---

### Task 11: Complete Gateway Rules, Missing, CORS, Edge Hooks

**Files:**
- Modify: `src/services/render_gateway.rs`
- Modify: `src/services/static_rules.rs`
- Modify: `src/services/cors.rs`

- [ ] **Step 1: Add gateway tests for redirects, rewrites, auto index, missing, CORS, and edge outcomes**

Add tests in `src/services/render_gateway.rs`:

```rust
#[tokio::test]
async fn missing_not_found_uses_index_body_with_404() {
    let temp = tempfile::tempdir().expect("tempdir");
    let origin_dir = temp.path().join("origins/web");
    tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
    tokio::fs::write(origin_dir.join("index.html"), "<h1>Shell</h1>").await.expect("write");
    let service = test_gateway(temp.path().join("origins"));

    let response = service.handle(RenderRequest {
        method: Method::GET,
        host: "web.test".to_string(),
        path: "/missing".to_string(),
        query: None,
        scheme: "https".to_string(),
        headers: Default::default(),
        body: bytes::Bytes::new(),
    }).await.expect("response");

    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>Shell</h1>"));
}

#[tokio::test]
async fn auto_index_serves_directory_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let origin_dir = temp.path().join("origins/web/docs");
    tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
    tokio::fs::write(origin_dir.join("index.html"), "<h1>Docs</h1>").await.expect("write");
    let service = test_gateway(temp.path().join("origins"));

    let response = service.handle(RenderRequest {
        method: Method::GET,
        host: "web.test".to_string(),
        path: "/docs".to_string(),
        query: None,
        scheme: "https".to_string(),
        headers: Default::default(),
        body: bytes::Bytes::new(),
    }).await.expect("response");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>Docs</h1>"));
}
```

Add route tests for CORS after Task 12; service-level tests should check `CorsPolicy` headers through a helper in this task.

- [ ] **Step 2: Implement all flow branches**

Extend `RenderGatewayService::handle` so the order is:

```rust
// 1. Resolve host or 421.
// 2. OPTIONS returns 204 with derived CORS headers.
// 3. Reject non GET/HEAD with 405.
// 4. Execute edge hooks.
// 5. If edge returns body, file_path, params, or file_path+params, produce response.
// 6. Redirects.
// 7. Explicit rewrites.
// 8. Root object.
// 9. Local mirror lookup.
// 10. Auto index.
// 11. Missing behavior.
```

Use existing helpers from `static_rules`, `edge_hooks`, `LocalMirrorRepository`, and `CorsPolicy`. Convert local object metadata into response headers. For `params`, call `render_html_template`. For `HEAD`, compute headers/status normally and return an empty body.

- [ ] **Step 3: Run gateway tests**

Run:

```bash
cargo test services::render_gateway
```

Expected: all gateway tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/services/render_gateway.rs src/services/static_rules.rs src/services/cors.rs
git commit -m "feat: complete render gateway flow"
```

---

### Task 12: Axum Route, App State, Startup Wiring, And Background Sync

**Files:**
- Create/Modify: `src/routes/render.rs`
- Modify: `src/routes/mod.rs`
- Modify: `src/state.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write failing integration tests for HTTP behavior**

Add tests in `tests/integration.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_route_serves_host_mapped_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let origin_dir = temp.path().join("origins/web");
    tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
    tokio::fs::write(origin_dir.join("index.html"), "<h1>Hello</h1>").await.expect("write");

    let router = setup_render_router(temp.path().join("origins")).await;
    let response = router
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "web.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_bytes(response).await, bytes::Bytes::from_static(b"<h1>Hello</h1>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_route_rejects_unknown_host() {
    let router = setup_render_router(tempfile::tempdir().expect("tempdir").path().join("origins")).await;
    let response = router
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "unknown.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}
```

Implement `setup_render_router` in the test file using the same `AppState` constructor that production uses, but with a temp `RenderGatewayService`.

- [ ] **Step 2: Implement route conversion**

In `src/routes/render.rs`, add:

```rust
use axum::{
    body::Body,
    extract::{OriginalUri, State},
    http::{HeaderMap, Request},
    response::{IntoResponse, Response},
};

use crate::{dto::render::RenderRequest, state::AppState};

pub async fn render(State(state): State<AppState>, OriginalUri(uri): OriginalUri, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let body = match http_body_util::BodyExt::collect(body).await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return axum::http::StatusCode::BAD_REQUEST.into_response(),
    };
    let host = parts
        .headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let render_request = RenderRequest {
        method: parts.method,
        host,
        path: uri.path().to_string(),
        query: uri.query().map(ToString::to_string),
        scheme: "https".to_string(),
        headers: parts.headers,
        body,
    };
    match state.render_gateway().handle(render_request).await {
        Ok(render_response) => response_from_render(render_response),
        Err(error) => {
            tracing::error!("render gateway failed: {error}");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn response_from_render(render_response: crate::dto::render::RenderResponse) -> Response {
    let mut builder = Response::builder().status(render_response.status);
    for (name, value) in render_response.headers {
        builder = builder.header(name, value);
    }
    builder.body(Body::from(render_response.body)).expect("response builds")
}
```

- [ ] **Step 3: Mount fallback route**

In `src/routes/mod.rs`, add:

```rust
pub mod render;
```

Mount the fallback after docs and MCP decisions:

```rust
let router = router.fallback(render::render);
```

- [ ] **Step 4: Update AppState**

Modify `src/state.rs` so `SharedState` contains:

```rust
pub render_gateway: crate::services::render_gateway::RenderGatewayService,
```

Add:

```rust
pub fn render_gateway(&self) -> crate::services::render_gateway::RenderGatewayService {
    self.inner.render_gateway.clone()
}
```

Update constructors and tests to provide a gateway.

- [ ] **Step 5: Wire startup**

In `src/main.rs`, add these imports:

```rust
use std::{collections::BTreeMap, sync::Arc, time::Duration};
```

Add helper functions near the bottom of `src/main.rs`:

```rust
async fn build_render_gateway(
    manifest_path: &str,
) -> anyhow::Result<rendermesh::services::render_gateway::RenderGatewayService> {
    use rendermesh::{
        dto::{edge::EdgeConfig, manifest::RenderMeshManifest},
        repositories::{
            local_mirror::LocalMirrorRepository,
            manifest::ManifestRepository,
            s3_storage::S3StorageRepository,
            sync::MirrorSyncService,
        },
        services::{
            cors::CorsPolicy,
            edge_config::{default_edge_config, parse_edge_config_yaml},
            manifest::HostResolver,
            render_gateway::RenderGatewayService,
        },
    };

    let manifest = ManifestRepository::new().load(manifest_path).await?;
    let mirror = LocalMirrorRepository::new(&manifest.runtime.local_store_dir);
    let syncer = MirrorSyncService::new(&manifest.runtime.local_store_dir);

    let mut storage_by_origin = BTreeMap::new();
    for (origin_id, origin) in &manifest.origins {
        let storage = S3StorageRepository::from_origin_config(origin).await?;
        syncer.sync_origin(origin_id, &storage).await?;
        storage_by_origin.insert(origin_id.clone(), storage);
    }

    let edge_configs = load_edge_configs(&manifest, &mirror).await?;
    spawn_background_sync(manifest.clone(), syncer, storage_by_origin);

    Ok(RenderGatewayService::new(
        HostResolver::new(&manifest)?,
        CorsPolicy::from_manifest(&manifest),
        mirror,
        edge_configs,
    ))
}

async fn load_edge_configs(
    manifest: &Arc<rendermesh::dto::manifest::RenderMeshManifest>,
    mirror: &rendermesh::repositories::local_mirror::LocalMirrorRepository,
) -> anyhow::Result<BTreeMap<String, rendermesh::dto::edge::EdgeConfig>> {
    use rendermesh::services::edge_config::{default_edge_config, parse_edge_config_yaml};

    let mut configs = BTreeMap::new();
    for origin_id in manifest.origins.keys() {
        let config = match mirror.read_object(origin_id, "/_rendermesh/edge.yaml").await? {
            Some(object) => {
                let content = String::from_utf8(object.body.to_vec())?;
                parse_edge_config_yaml(&content)?
            }
            None => {
                tracing::warn!(origin = origin_id, "origin has no /_rendermesh/edge.yaml; using defaults");
                default_edge_config()
            }
        };
        configs.insert(origin_id.clone(), config);
    }
    Ok(configs)
}

fn spawn_background_sync(
    manifest: Arc<rendermesh::dto::manifest::RenderMeshManifest>,
    syncer: rendermesh::repositories::sync::MirrorSyncService,
    storage_by_origin: BTreeMap<String, rendermesh::repositories::s3_storage::S3StorageRepository>,
) {
    for (origin_id, storage) in storage_by_origin {
        let syncer = syncer.clone();
        let interval_seconds = manifest
            .origins
            .get(&origin_id)
            .and_then(|origin| origin.sync_interval_seconds)
            .unwrap_or(manifest.runtime.sync_interval_seconds);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_seconds));
            loop {
                interval.tick().await;
                match syncer.sync_origin(&origin_id, &storage).await {
                    Ok(report) => tracing::info!(
                        origin = origin_id,
                        downloaded = report.downloaded,
                        "background sync completed"
                    ),
                    Err(error) => tracing::error!(
                        origin = origin_id,
                        "background sync failed: {error}"
                    ),
                }
            }
        });
    }
}
```

Then replace the existing state construction after migrations with:

```rust
let render_gateway = build_render_gateway(&config.rendermesh_manifest).await?;
let state = AppState::new(DatabaseRepository::new(pool), render_gateway);
```

- [ ] **Step 6: Run integration tests and commit**

Run:

```bash
cargo test --test integration render_route
cargo test
```

Expected: integration tests and full suite pass.

Commit:

```bash
git add src/routes/render.rs src/routes/mod.rs src/state.rs src/main.rs tests/integration.rs
git commit -m "feat: route requests through rendermesh gateway"
```

---

### Task 13: Documentation And Final Verification

**Files:**
- Modify: `README.md`
- Modify: `.env.example`

- [ ] **Step 1: Update README**

Add a `RenderMesh MVP` section to `README.md` with:

```markdown
## RenderMesh MVP

RenderMesh serves host-mapped frontend applications from local mirrors of S3/R2-compatible buckets.

Set `RENDERMESH_MANIFEST` to a local manifest:

```env
RENDERMESH_MANIFEST=./rendermesh.yaml
```

Example manifest:

```yaml
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: my-web-bucket
    endpoint_env: WEB_STORAGE_ENDPOINT
    region_env: WEB_STORAGE_REGION
    access_key_id_env: WEB_STORAGE_ACCESS_KEY_ID
    secret_access_key_env: WEB_STORAGE_SECRET_ACCESS_KEY
    force_path_style_env: WEB_STORAGE_FORCE_PATH_STYLE
hosts:
  example.com:
    origin: web
  "*.example.com":
    origin: web
```

Each origin may include `/_rendermesh/edge.yaml` in the bucket. If it is missing, RenderMesh uses:

```yaml
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
```
```

- [ ] **Step 2: Update `.env.example`**

Add:

```env
RENDERMESH_MANIFEST=./rendermesh.yaml
WEB_STORAGE_ENDPOINT=https://s3.us-east-1.amazonaws.com
WEB_STORAGE_REGION=us-east-1
WEB_STORAGE_ACCESS_KEY_ID=
WEB_STORAGE_SECRET_ACCESS_KEY=
WEB_STORAGE_FORCE_PATH_STYLE=false
```

- [ ] **Step 3: Run final checks**

Run:

```bash
cargo fmt
cargo build
cargo test
```

Expected: all commands pass.

- [ ] **Step 4: Commit**

```bash
git add README.md .env.example
git commit -m "docs: document rendermesh mvp configuration"
```

---

## Implementation Order

1. Task 1: dependencies and module skeleton.
2. Task 2: manifest parsing and validation.
3. Task 3: host resolver and CORS policy.
4. Task 4: edge config parsing and defaults.
5. Task 5: local mirror reads.
6. Task 6: static rules.
7. Task 7: edge HTTP and hook chain.
8. Task 8: Handlebars HTML rendering.
9. Task 9: S3/R2 sync to local mirror.
10. Task 10: gateway service.
11. Task 11: complete gateway flow.
12. Task 12: Axum route and startup wiring.
13. Task 13: documentation and final verification.

This order keeps the core domain logic testable before transport wiring and keeps S3/R2 integration behind a repository boundary.

## Self-Review

Spec coverage:

- Multi-origin manifest: Tasks 2 and 3.
- Exact and wildcard host resolution: Task 3.
- CORS derived from hosts: Task 3 and Task 11.
- Local mirror and periodic sync: Task 9 and Task 12.
- Edge config defaults: Task 4 and Task 10.
- Redirects, rewrites, root object, auto index, missing behavior: Task 6 and Task 11.
- Edge HTTP contract: Task 7 and Task 11.
- Handlebars only for HTML when params exist: Task 8 and Task 11.
- GET, HEAD, OPTIONS, 405, and 421 behavior: Task 10 through Task 12.
- README and env docs: Task 13.

No open-ended implementation steps remain in required behavior. Startup wiring is covered by concrete helper functions in Task 12, and each helper depends only on types introduced by earlier tasks.
