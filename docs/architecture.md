# Architecture

RenderMesh follows a layered Rust architecture. The goal is to keep transport details, business rules, contracts, and external integrations separated.

## Layers

```text
route -> dto -> service -> repository -> service -> dto -> route
```

## `routes/`

Routes own HTTP transport concerns:

- Extracting request data.
- Applying body limits.
- Calling application services.
- Turning service responses into HTTP responses.

Routes should not contain business rules or storage access.

Important files:

- `src/routes/mod.rs`
- `src/routes/render.rs`
- `src/routes/system/health.rs`
- `src/routes/system/echo.rs`

## `dto/`

DTOs define request, response, configuration, and edge contracts:

- Render request and response structs.
- Global manifest structs.
- Edge config structs.
- Edge hook request and response payloads.

Important files:

- `src/dto/render.rs`
- `src/dto/manifest.rs`
- `src/dto/edge.rs`

## `services/`

Services own business rules and orchestration:

- Host resolution.
- CORS policy.
- Edge config parsing and storage.
- Static redirect and rewrite rules.
- Edge payload application.
- Render gateway orchestration.
- Startup sync orchestration.
- Origin freshness indexing and staged refresh activation.
- Template compilation and rendering.

Important files:

- `src/services/render_gateway.rs`
- `src/services/startup.rs`
- `src/services/manifest.rs`
- `src/services/static_rules.rs`
- `src/services/edge_hooks.rs`
- `src/services/template_store.rs`

## `repositories/`

Repositories own external integrations and local storage adapters:

- S3/R2 access.
- Local mirror reads and metadata.
- Sync from remote storage to local mirrors.
- HTTP calls to edge hooks.
- Manifest file loading.

Important files:

- `src/repositories/s3_storage.rs`
- `src/repositories/local_mirror.rs`
- `src/repositories/sync.rs`
- `src/repositories/edge_http.rs`
- `src/repositories/manifest.rs`

## Runtime State

`AppState` stores the shared `RenderGatewayService`. The gateway holds references to:

- Host resolver.
- CORS policy.
- Local mirror repository.
- Edge config store.
- Template store.
- Edge HTTP repository.
- Origin bucket map.

## Request Flow

1. `routes::render` receives the fallback request.
2. It builds a `RenderRequest`.
3. `RenderGatewayService` resolves the host and origin.
4. CORS and edge config are loaded.
5. Edge hooks run.
6. Redirects and rewrites are applied.
7. Objects are read from the local mirror.
8. Templates render if params are present.
9. The route converts `RenderResponse` into an HTTP response.

## File Size Rule

Project guidance keeps source files below 1000 lines. If a module approaches that size, split it by responsibility while preserving the layer boundaries above.
