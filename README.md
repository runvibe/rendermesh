# RenderMesh

RenderMesh is a Rust edge gateway for frontend applications served from S3/R2-compatible buckets.
It sits between users, frontend assets, and programmable rendering middleware, then routes requests by host, mirrors bucket contents locally, applies edge rules, calls HTTP edge hooks, renders HTML templates when requested, and serves the final response with low latency.

## Motivation

Modern frontend delivery often needs more than static file hosting. Teams need host-based routing, bucket-backed deployments, intelligent fallbacks, redirects, rewrites, HTML customization, observability, and programmable request-time decisions without coupling every frontend application to a specific infrastructure provider.

RenderMesh exists to provide that middle layer. The goal is to keep frontend artifacts portable in buckets while moving delivery concerns into a fast Rust gateway:

- Bucket contents remain the source of frontend files.
- Hosts map explicitly to origins.
- CORS is derived from the host map.
- Edge behavior lives beside each origin in YAML or JSON config files.
- External edge APIs can influence rendering through a stable HTTP contract.
- HTML templates are compiled in memory and rendered only when edge params are returned.
- Origin refresh keeps an in-memory freshness index and activates changed files only after edge config and template compilation succeed.
- OpenTelemetry spans make the request lifecycle observable from entrypoint to response.

## Project Status

This repository contains the RenderMesh MVP. It intentionally does not include PostgreSQL, MCP, OpenAPI, or Swagger UI. The current runtime focuses on the static edge gateway, local bucket mirroring, edge configuration, edge hooks, HTML-only template rendering, and OpenTelemetry.

## Summary

- [Overview](docs/overview.md): product concepts, request lifecycle, and MVP scope.
- [Configuration](docs/configuration.md): global manifest, environment variables, origins, hosts, and bucket credentials.
- [Origin Edge Config](docs/edge-config.md): `/_rendermesh/edge.yaml`, `edge.yml`, or `edge.json`, root object, auto-index, redirects, rewrites, and missing-file behavior.
- [Edge Hooks](docs/edge-hooks.md): HTTP middleware contract, `{ context, request }` payload, response payloads, status behavior, and headers.
- [Local Mirror And Sync](docs/local-mirror-and-sync.md): startup sync, background sync, freshness index, local filesystem layout, and refresh behavior.
- [Templates](docs/templates.md): HTML-only Handlebars compilation, in-memory registry, and render rules.
- [Observability](docs/observability.md): OpenTelemetry setup, span names, important fields, and local Jaeger usage.
- [Testing](docs/testing.md): unit tests, integration tests, manual local lab, and useful curl flows.
- [Release](docs/release.md): GitHub Actions release workflow, multi-arch Docker image, and `Dockerfile.artifact`.
- [Architecture](docs/architecture.md): code layers, module responsibilities, and request flow.
- [Local Example](examples/local/README.md): runnable MinIO + edge API lab using `test.com`.

## Quick Start

The fastest way to try RenderMesh locally is the local bucket lab:

```bash
docker compose up -d jaeger minio edge-api
docker compose run --rm minio-init
```

Run the gateway:

```bash
export RENDERMESH_MANIFEST=./examples/local/rendermesh.yaml
export LOCAL_APP_STORAGE_ENDPOINT=http://127.0.0.1:9000
export LOCAL_APP_STORAGE_REGION=us-east-1
export LOCAL_APP_ACCESS_KEY_ID=rendermesh
export LOCAL_APP_SECRET_ACCESS_KEY=rendermesh-secret
export LOCAL_APP_FORCE_PATH_STYLE=true
export APP_HOST=127.0.0.1
export APP_PORT=3000
export OTEL_ENABLED=false

cargo run
```

Test with curl:

```bash
curl -i -H 'Host: test.com' http://127.0.0.1:3000/
```

For browser testing, add this entry to `/etc/hosts`:

```text
127.0.0.1 test.com
```

Then open:

```text
http://test.com:3000/
```

See [examples/local/README.md](examples/local/README.md) for every route in the lab.

## Minimal Global Manifest

`RENDERMESH_MANIFEST` points to the global bootstrap config. YAML and JSON are both accepted. Bucket credentials stay in environment variables so the manifest can be reused safely across environments.

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
    force_path_style_env: MY_APP_FORCE_PATH_STYLE
    sync_interval_seconds: 30

hosts:
  myapp.com:
    origin: my_app
  "*.myapp.com":
    origin: my_app
```

Exact hosts take priority over wildcard hosts. Unknown hosts return `421 Misdirected Request`.

For AWS environments, omit `access_key_id_env` and `secret_access_key_env` to use the AWS SDK default credential chain, including EKS IRSA. For S3-compatible local labs or providers that require static credentials, configure both fields.

## Minimal Origin Edge Config

Each origin can include `/_rendermesh/edge.yaml`, `/_rendermesh/edge.yml`, or `/_rendermesh/edge.json` in its bucket:

```yaml
version: 1

edge:
  root_object: /index.html
  auto_rewrite_index: true

missing:
  action: not_found
  page: /index.html
```

If this file is missing, RenderMesh uses safe defaults. Invalid edge config marks only that origin as unavailable until a valid config is synced.

## Edge Hook Contract

When an origin defines edge hooks, RenderMesh sends a `POST` request to the configured hook URL:

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

The edge response status code becomes the client status for terminal edge responses. The edge response payload can return direct bodies, response headers, params for HTML template rendering, or a specific file path to serve from the origin mirror. See [Edge Hooks](docs/edge-hooks.md).

## Testing

Run the full suite:

```bash
cargo test
```

Run only integration tests:

```bash
cargo test --test integration
```

The local lab also validates the full bucket and edge flow with MinIO and a small Node.js edge API. See [Testing](docs/testing.md).

## Repository Layout

```text
src/routes/        HTTP transport wiring
src/dto/           request, response, manifest, and edge contracts
src/services/      business rules and orchestration
src/repositories/  local mirror, S3/R2, sync, and HTTP adapters
examples/local/    runnable local bucket lab
docs/              project documentation
```

Preferred application flow:

```text
route -> dto -> service -> repository -> service -> dto -> route
```
