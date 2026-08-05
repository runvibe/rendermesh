# Startup Refactor Spec

## Status

Implemented.

## Summary

Split `src/services/startup.rs` into smaller startup-focused modules while preserving the public startup API and runtime behavior. The current file owns manifest loading, origin repository assembly, CDN setup, startup sync, background sync, runtime construction, and many tests. It is over the project file size limit and should be decomposed by responsibility.

## Goals

- Keep source files below the 1000-line project limit.
- Preserve the existing public API:
  - `RenderRuntime`
  - `build_render_gateway`
  - `build_render_runtime`
- Keep startup behavior unchanged.
- Improve cohesion by grouping startup origin wiring, CDN wiring, initial sync, and background sync separately.
- Keep route, service, DTO, and repository boundaries intact.
- Move tests near the responsibilities they validate.

## Non-goals

- Do not change manifest semantics.
- Do not change origin sync, activation barrier, CDN refresh, CDN domain reconciliation, edge config loading, or template compilation behavior.
- Do not introduce persistence or a database layer.
- Do not change public HTTP routes or edge hook contracts.
- Do not perform unrelated cleanup in `render_gateway`, repositories, or DTOs.

## Current Problem

`src/services/startup.rs` currently exceeds 1000 lines and mixes several responsibilities:

- Loading and validating the global manifest through service/repository wiring.
- Resolving manifest-relative local origin paths.
- Building per-origin storage adapters.
- Building activation barrier maps.
- Building origin bucket and edge context maps for edge hook context.
- Building CDN refresh adapters.
- Building CDN domain reconciliation adapters.
- Running startup sync for every origin.
- Reconciling CDN domains after startup sync.
- Spawning background sync loops.
- Hosting tests for multiple startup subdomains.

This makes the module harder to navigate and violates the file size rule.

## Target Module Layout

Keep `src/services/startup.rs` as the facade module and add submodules under `src/services/startup/`.

```text
src/services/startup.rs
src/services/startup/background.rs
src/services/startup/cdn.rs
src/services/startup/initial_sync.rs
src/services/startup/origins.rs
```

Rust supports this shape by declaring submodules from `startup.rs`:

```rust
mod background;
mod cdn;
mod initial_sync;
mod origins;
```

## Module Responsibilities

### `startup.rs`

Public facade and high-level orchestration only.

Keep:

- `RenderRuntime`
- `build_render_gateway`
- `build_render_runtime`

Expected orchestration:

1. Load manifest.
2. Build origin startup state.
3. Build CDN startup state.
4. Build shared stores.
5. Build `OriginRefreshService`.
6. Run initial sync.
7. Spawn background sync.
8. Build `RenderGatewayService`.
9. Return `RenderRuntime`.

### `startup/origins.rs`

Origin-specific startup wiring.

Responsibilities:

- Resolve `manifest_dir`.
- Build `OriginStorageRepository` for each origin.
- Build activation barrier map.
- Build origin bucket map.
- Build origin edge context map.

Suggested API:

```rust
pub struct StartupOrigins {
    pub storage_by_origin: BTreeMap<String, OriginStorageRepository>,
    pub activation_barrier_by_origin: BTreeMap<String, String>,
    pub origin_buckets: BTreeMap<String, String>,
    pub origin_edge_contexts: BTreeMap<String, serde_json::Value>,
}

pub async fn build_startup_origins(
    manifest: &RenderMeshManifest,
    manifest_path: &str,
) -> Result<StartupOrigins>;
```

### `startup/cdn.rs`

CDN adapter and domain reconciliation setup.

Responsibilities:

- Build `OriginCdnRefresh` map.
- Build `OriginCdnDomains` map.
- Reconcile CDN domains for an origin.
- Derive exact URL prefixes for Cloudflare refresh.

Suggested API:

```rust
pub struct StartupCdn {
    pub refresh_by_origin: BTreeMap<String, OriginCdnRefresh>,
    pub domains_by_origin: BTreeMap<String, OriginCdnDomains>,
}

pub async fn build_startup_cdn(manifest: &RenderMeshManifest) -> Result<StartupCdn>;

pub async fn reconcile_origin_cdn_domains(
    origin_id: &str,
    manifest: &RenderMeshManifest,
    cdn_domains: &OriginCdnDomains,
    origin_runtime: &OriginRuntimeStore,
);
```

### `startup/initial_sync.rs`

Initial origin activation flow.

Responsibilities:

- Convert `OriginRefreshError` into startup `anyhow::Error`.
- Run `OriginRefreshService::refresh_origin(..., Startup)` for all origins.
- Log startup sync result.
- Trigger CDN domain reconciliation after successful initial sync.

Suggested API:

```rust
pub async fn sync_origins_at_startup(
    manifest: &RenderMeshManifest,
    origin_refresh: &OriginRefreshService,
    cdn_domains_by_origin: &BTreeMap<String, OriginCdnDomains>,
    origin_runtime: &OriginRuntimeStore,
) -> Result<()>;
```

### `startup/background.rs`

Background refresh scheduling.

Responsibilities:

- Spawn one background sync loop per origin.
- Respect origin-specific `sync_interval_seconds`.
- Fall back to `manifest.runtime.sync_interval_seconds`.
- Log background sync failures.

Suggested API:

```rust
pub fn spawn_background_sync(
    manifest: Arc<RenderMeshManifest>,
    origin_refresh: OriginRefreshService,
);
```

## Test Migration Plan

Prefer moving tests based on responsibility:

- Origin path and local origin startup tests -> `startup/origins.rs`.
- CDN refresh/domain startup tests -> `startup/cdn.rs`.
- Startup sync, edge config store, activation barrier, and template refresh tests -> `startup/initial_sync.rs`.
- Facade-level and cross-module runtime construction tests can remain in `startup.rs` when they validate the integrated startup flow.

Keep test names stable where possible to reduce review noise.

## Implementation Order

1. Create empty submodules and move imports gradually.
2. Extract origin wiring first because it has the least behavioral coupling.
3. Extract CDN setup and domain reconciliation.
4. Extract startup sync orchestration.
5. Extract background sync.
6. Move tests to matching modules.
7. Run `cargo fmt`.
8. Run `cargo build`.
9. Run `cargo test`.
10. Check line counts for all `src/**/*.rs` files.

## Acceptance Criteria

- `src/services/startup.rs` is below 1000 lines.
- No new source file exceeds 1000 lines.
- `build_render_gateway` and `build_render_runtime` remain publicly available from `rendermesh::services::startup`.
- Startup behavior remains equivalent:
  - startup sync still runs before serving;
  - invalid edge config still keeps an origin unavailable;
  - failed template compilation still rejects activation;
  - activation barrier behavior remains unchanged;
  - CDN refresh still runs after activation;
  - CDN domain reconciliation still runs after startup sync;
  - background sync still starts for each origin.
- `cargo build` passes.
- `cargo test` passes.

## Risk Controls

- Prefer moving existing functions unchanged before renaming or reshaping APIs.
- Keep new structs as internal startup implementation details unless tests require crate visibility.
- Avoid changing data flow between services and repositories.
- Do not alter route behavior, manifest validation rules, or edge hook DTOs during this refactor.
