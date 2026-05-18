# RenderMesh

RenderMesh is a Rust edge gateway for serving frontend applications from S3/R2-compatible buckets. It maps incoming hosts to origins, mirrors each configured bucket to local disk, derives CORS from the host map, applies per-origin edge rules, calls programmable HTTP edge hooks, and serves static assets from the local mirror.

The current MVP keeps health, echo, and OpenTelemetry support, then adds the RenderMesh fallback renderer for application traffic.

## Getting Started

### Prerequisites

- Rust stable toolchain.
- S3/R2-compatible storage credentials for every configured origin.

### Run locally

1. Start local infrastructure if needed:
   - `docker compose up -d jaeger`
2. Create a RenderMesh manifest and point the service at it:
   - `export RENDERMESH_MANIFEST=./rendermesh.yaml`
3. Export the storage env vars referenced by the manifest.
4. Run:
   - `cargo run`

The service does not expose generated OpenAPI or Swagger UI routes.

### Local bucket lab

For a full local RenderMesh flow with MinIO, a seeded frontend bucket, and a programmable edge API, use [examples/local/README.md](examples/local/README.md).

The lab lets you test:

- bucket sync from MinIO
- `/_rendermesh/edge.yaml`
- edge params rendering `index.html` with Handlebars
- direct static delivery of `static.html` without templating
- direct edge body responses
- redirects, rewrites, and auto-index behavior

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
  "context": {
    "bucket": "bucket_my_app_123",
    "ip": "203.0.113.10",
    "origin": "my_app"
  },
  "request": {
    "url": "https://myapp.com/path?query=1",
    "method": "GET",
    "headers": {},
    "body": ""
  }
}
```

The edge API response status becomes the client status for terminal edge responses. Payload behavior:

- `{ "body": "..." }` returns that body directly.
- `{ "params": { ... } }` renders the resolved HTML file with Handlebars.
- `{ "file_path": "/other.html" }` serves that mirrored file.
- `{ "file_path": "/other.html", "params": { ... } }` serves and renders that HTML file.
- `{ "headers": { "x-edge": "yes" } }` adds safe response headers.

Templates are rendered only when `params` is present, and only for HTML.
HTML templates are compiled into an in-memory Handlebars registry after bucket sync and refreshed after successful background syncs. Non-HTML files are never compiled as templates; when served without `params`, HTML files are returned directly from the mirror without rendering.

## CORS

CORS is derived from the manifest host map. Exact hosts allow their matching `https://host` origins; wildcard hosts reflect matching subdomains. Users do not configure per-origin CORS manually.

## Observability

OpenTelemetry remains available through the existing tracing setup. RenderMesh emits structured spans for the full request path:

- `rendermesh.request`: incoming fallback request, final status, and total duration.
- `rendermesh.read_body`: request body read duration and byte count.
- `rendermesh.gateway`: gateway orchestration with host, path, origin, status, and duration.
- `rendermesh.resolve_host`, `rendermesh.cors`, `rendermesh.edge_config`, `rendermesh.redirect`, and `rendermesh.resolve_target`: routing and rule resolution steps.
- `rendermesh.edge_chain` and `rendermesh.edge_hook`: edge execution, outcome, status, timeout, and response time.
- `edge_http_request_start`, `edge_http_response_received`, and `edge_http_payload_decoded`: before/after markers around the external edge API call.
- `rendermesh.static`, `rendermesh.object`, `rendermesh.template_render`, and `rendermesh.missing`: local mirror lookup, HTML template render, auto-index, and missing-file handling.
- `rendermesh.response_build`: final response assembly timing.

## Environment

Common variables:

- `RENDERMESH_MANIFEST`
- `APP_HOST`, `APP_PORT`
- `APP_BODY_LIMIT_BYTES`
- `OTEL_ENABLED`

Each manifest origin references storage variables such as `MY_APP_STORAGE_ENDPOINT`, `MY_APP_STORAGE_REGION`, `MY_APP_ACCESS_KEY_ID`, `MY_APP_SECRET_ACCESS_KEY`, and optionally `MY_APP_FORCE_PATH_STYLE`.

## Testing

- Unit tests: `cargo test`
- Integration tests: `cargo test --test integration`
- If OpenTelemetry containers slow local runs: `OTEL_ENABLED=false cargo test`

Integration tests run in-process. If OpenTelemetry is enabled, Jaeger can still be used for local telemetry inspection.

## Architecture

The codebase follows:

- `routes/`: HTTP transport wiring
- `dto/`: request/response and YAML contracts
- `services/`: business rules and use-case orchestration
- `repositories/`: local mirror, S3/R2, sync, and HTTP adapters

Preferred flow:

`route -> dto -> service -> repository -> service -> dto -> route`
