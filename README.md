# RenderMesh

RenderMesh is a Rust edge gateway for serving frontend applications from S3/R2-compatible buckets. It maps incoming hosts to origins, mirrors each configured bucket to local disk, derives CORS from the host map, applies per-origin edge rules, calls programmable HTTP edge hooks, and serves static assets from the local mirror.

The current MVP keeps the original template health, echo, docs, PostgreSQL, OpenTelemetry, and optional MCP endpoints, then adds the RenderMesh fallback renderer for application traffic.

## Getting Started

### Prerequisites

- Rust stable toolchain.
- PostgreSQL access for the template database layer.
- S3/R2-compatible storage credentials for every configured origin.

### Run locally

1. Start local infrastructure if needed:
   - `docker compose up -d postgres jaeger`
2. Set the database URL:
   - `export DATABASE_URL=postgres://postgres:postgres@localhost:5453/postgres`
3. Create a RenderMesh manifest and point the service at it:
   - `export RENDERMESH_MANIFEST=./rendermesh.yaml`
4. Export the storage env vars referenced by the manifest.
5. Run:
   - `cargo run`

Migrations are managed by SQLx and run on startup from `migrations/`. Swagger UI remains available at `/docs`.

## Global Manifest

`RENDERMESH_MANIFEST` points to the bootstrap YAML file. Bucket connection values stay in environment variables so the same manifest shape can be reused safely.

```yaml
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
```

Exact hosts take priority over wildcards. Unknown hosts return `421 Misdirected Request`.

## Local Mirrors

On startup, RenderMesh syncs every configured origin into `runtime.local_store_dir/<origin_id>`. Startup fails if any initial sync fails. Runtime requests are served from this local mirror, not directly from the bucket.

After startup, each origin syncs periodically using `origin.sync_interval_seconds` or the runtime default. Failed background syncs log an error and keep the last successful mirror contents. Changed `/_rendermesh/edge.yaml` files are reparsed after successful syncs.

## Edge Config

Each origin may include `/_rendermesh/edge.yaml` in its bucket. If the file is missing, safe defaults are used:

```yaml
version: 1

edge:
  root_object: /index.html
  auto_rewrite_index: true

missing:
  action: not_found
  page: /index.html
```

Supported rules include redirects, rewrites, missing-file behavior, root object resolution, and automatic `/docs -> /docs/index.html` rewrites when enabled. Invalid edge config marks only that origin as unavailable and requests for it return `500` until a valid config is synced.

## Edge Hooks

Edge hooks are global per origin and run before static file delivery. RenderMesh sends:

```json
{
  "url": "https://myapp.com/path?query=1",
  "method": "GET",
  "headers": {},
  "body": ""
}
```

The edge API response status becomes the client status for terminal edge responses. Payload behavior:

- `{ "body": "..." }` returns that body directly.
- `{ "params": { ... } }` renders the resolved HTML file with Handlebars.
- `{ "file_path": "/other.html" }` serves that mirrored file.
- `{ "file_path": "/other.html", "params": { ... } }` serves and renders that HTML file.
- `{ "headers": { "x-edge": "yes" } }` adds safe response headers.

Templates are rendered only when `params` is present, and only for HTML.

## CORS

CORS is derived from the manifest host map. Exact hosts allow their matching `https://host` origins; wildcard hosts reflect matching subdomains. Users do not configure per-origin CORS manually.

## Environment

Common variables:

- `DATABASE_URL`
- `RENDERMESH_MANIFEST`
- `APP_HOST`, `APP_PORT`
- `APP_BODY_LIMIT_BYTES`
- `OTEL_ENABLED`
- `MCP_ENABLED`, `MCP_PATH`, `MCP_ALLOWED_ORIGINS`

Each manifest origin references storage variables such as `MY_APP_STORAGE_ENDPOINT`, `MY_APP_STORAGE_REGION`, `MY_APP_ACCESS_KEY_ID`, `MY_APP_SECRET_ACCESS_KEY`, and optionally `MY_APP_FORCE_PATH_STYLE`.

## Testing

- Unit tests: `cargo test`
- Integration tests: `cargo test --test integration`
- If OpenTelemetry containers slow local runs: `OTEL_ENABLED=false cargo test`

Integration tests use Docker testcontainers for PostgreSQL and, unless OTEL is disabled, Jaeger.

## Architecture

The codebase follows:

- `routes/`: HTTP and MCP transport wiring
- `dto/`: request/response and YAML contracts
- `services/`: business rules and use-case orchestration
- `repositories/`: database, local mirror, S3/R2, sync, and HTTP adapters

Preferred flow:

`route -> dto -> service -> repository -> service -> dto -> route`
