# RenderMesh MVP Design

## Summary

RenderMesh MVP is a static edge gateway for serving frontend applications from
S3/R2-compatible buckets. It resolves incoming hosts to configured origins,
mirrors every configured bucket into a local project directory, loads edge
behavior from each origin mirror, applies CORS automatically, runs programmable
HTTP edge hooks, and serves static files from the local mirror with redirect,
rewrite, root object, auto-index, and missing-file behavior.

This MVP intentionally excludes prerender orchestration, distributed response
cache, dynamic global manifest reload, admin UI, and non-S3 providers. Those
features remain in the roadmap after the gateway contract is validated.

## Goals

- Serve multiple frontend applications from one RenderMesh instance.
- Map explicit and wildcard hosts to origins.
- Support multiple S3/R2-compatible buckets.
- Download every configured bucket into a local project directory before
  serving traffic.
- Periodically check each bucket for updates and refresh the local mirror.
- Keep bucket credentials and deployment secrets in environment variables.
- Keep edge behavior in YAML files that can live inside each bucket.
- Derive CORS from configured hosts without user-managed CORS rules.
- Provide programmable HTTP edge hooks before static file delivery.
- Render HTML with Handlebars only when an edge hook returns params.
- Keep the first version response-cacheless.

## Non-Goals

- No prerender job pipeline.
- No smart cache or distributed cache.
- No route-level edge hooks.
- No after-response edge hooks.
- No dynamic global manifest reload.
- No push-based edge config reload; edge config updates only through the
  periodic bucket mirror sync.
- No admin API or dashboard.
- No providers beyond S3/R2-compatible object storage.
- No request-time bucket reads for normal file delivery.

## Repository Fit

The existing Rust template already has the desired project shape:

- `routes/` handles HTTP transport and protocol wiring.
- `services/` owns business rules and use-case orchestration.
- `dto/` defines request and response contracts.
- `repositories/` owns external integrations such as storage and HTTP clients.

The MVP should preserve the flow:

```text
route -> dto -> service -> repository -> service -> dto -> route
```

Routes should not talk directly to object storage, local files, or edge APIs.
Bucket sync, local object reads, and edge HTTP calls should be repository
adapters. Host resolution, edge config handling, request flow, missing behavior,
rewrites, and render decisions should live in services.

## Global Manifest

RenderMesh starts from a local YAML manifest path provided by env:

```env
RENDERMESH_MANIFEST=./rendermesh.yaml
```

The manifest is the bootstrap configuration for the RenderMesh instance. It
defines origins, the bucket behind each origin, which env vars supply storage
connection data, and which hosts point to each origin.

Example:

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

  megaloja:
    type: s3
    bucket: e_commer_loja
    endpoint_env: MEGALOJA_STORAGE_ENDPOINT
    region_env: MEGALOJA_STORAGE_REGION
    access_key_id_env: MEGALOJA_ACCESS_KEY_ID
    secret_access_key_env: MEGALOJA_SECRET_ACCESS_KEY
    force_path_style_env: MEGALOJA_FORCE_PATH_STYLE

hosts:
  myapp.com:
    origin: my_app

  other.app:
    origin: megaloja

  megaloja.com.br:
    origin: megaloja

  "*.megaloja.com.br":
    origin: megaloja
```

Manifest validation rules:

- Every origin referenced by `hosts` must exist.
- Every origin must use `type: s3` in the MVP.
- Credentials and endpoints are read from env names referenced by the manifest.
- Secrets are never stored directly in YAML.
- `runtime.local_store_dir` defines where bucket mirrors are stored locally.
- `runtime.sync_interval_seconds` defines the default update check interval.
- `origin.sync_interval_seconds` can override the default for one origin.
- Sync intervals must be positive.
- Exact host matches have priority over wildcard matches.
- When multiple wildcards match, the most specific wildcard wins.
- Unknown hosts return `421 Misdirected Request`.

## Local Bucket Mirrors

All buckets listed in the global manifest are mirrored into a local project
directory. Runtime requests serve files from this local mirror, not directly
from the remote bucket.

Default local store:

```yaml
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
```

Rules:

- `local_store_dir` is resolved relative to the RenderMesh process working
  directory when it is not absolute.
- Each origin is stored under a deterministic child directory based on the
  origin id.
- Origin ids must be validated before being used as directory names.
- The first sync for every configured origin must complete before RenderMesh
  accepts traffic.
- If the first sync for any origin fails, startup fails.
- After startup, each origin is checked on its configured interval.
- A failed background sync logs an error and keeps serving the last successful
  local mirror.
- Sync downloads new and changed objects from the bucket.
- Sync removes local objects that no longer exist in the bucket.
- Sync should compare bucket object metadata such as ETag, last-modified, and
  size to avoid unnecessary downloads.
- Local metadata needed for response headers is persisted with the mirror.
- The origin edge config is read from the local mirror at
  `/_rendermesh/edge.yaml` after sync.

The local mirror is not a response cache. It is the required data plane for the
MVP. Smart cache rules and invalidation remain outside the MVP.

## Host Resolution

The incoming `Host` header is normalized before lookup:

- Lowercase host names.
- Strip port numbers.
- Reject empty or malformed hosts.

Resolution order:

1. Try exact host match.
2. Try wildcard host matches.
3. Choose the most specific wildcard if more than one matches.
4. Return `421 Misdirected Request` when no host matches.

Example:

```yaml
hosts:
  admin.megaloja.com.br:
    origin: admin
  "*.megaloja.com.br":
    origin: megaloja
```

`admin.megaloja.com.br` resolves to `admin` because exact hosts take priority.
`www.megaloja.com.br` resolves to `megaloja` through the wildcard.

## Automatic CORS

CORS is derived from the global manifest. Users do not configure `allow_origins`
manually.

For each origin, RenderMesh collects all exact and wildcard hosts that point to
that origin. A request `Origin` is allowed when it matches one of those host
rules using `https://` origin semantics.

Rules:

- Exact host `megaloja.com.br` allows `https://megaloja.com.br`.
- Wildcard host `*.megaloja.com.br` allows matching subdomains such as
  `https://admin.megaloja.com.br`.
- Wildcard CORS responses reflect the incoming `Origin` when it matches.
- RenderMesh does not emit literal invalid values such as
  `Access-Control-Allow-Origin: https://*.megaloja.com.br`.
- `OPTIONS` preflight is handled by RenderMesh without reading mirrored files.
- Default allowed methods are `GET`, `HEAD`, and `OPTIONS`.
- Default allowed headers include `content-type`, `authorization`,
  `if-none-match`, and `if-modified-since`.
- Default exposed headers include `etag`, `cache-control`, and `last-modified`.

## Edge Config Per Origin

Each origin attempts to load its edge behavior from the local mirror object:

```text
/_rendermesh/edge.yaml
```

If the object does not exist in the mirror, RenderMesh uses safe defaults and
logs a warning.

Default edge config:

```yaml
version: 1

edge:
  root_object: /index.html
  auto_rewrite_index: true

missing:
  action: not_found
  page: /index.html
```

Example edge config:

```yaml
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
  - name: auth-or-render
    url: https://api.example.com/rendermesh/edge
    timeout_ms: 800
```

## Static Rule Semantics

### Root Object

When a request path ends in `/`, RenderMesh resolves the configured root object
within that prefix.

Examples with `root_object: /index.html`:

- `/` resolves to `/index.html`.
- `/docs/` resolves to `/docs/index.html`.

### Missing Behavior

Supported missing actions:

- `not_found`: respond with `404`, using `page` as the response body if it
  exists.
- `serve`: respond with `200`, serving `path`; useful for SPA fallback.
- `redirect`: redirect to `to` using the configured status.

Default behavior:

```yaml
missing:
  action: not_found
  page: /index.html
```

This means `/unknown` returns status `404` with `/index.html` as the response
body when that object exists. If `/index.html` does not exist, RenderMesh returns
a simple textual `404`.

### Redirects

Redirects run before rewrites, root object resolution, object lookup, and
missing behavior.

Supported redirect statuses:

- `301`
- `302`
- `307`
- `308`

Rules:

- Exact path matches take priority over wildcard matches.
- Wildcard `*` captures the remaining path and can be inserted into `to` using
  `:splat`.
- Query string is preserved by default unless the target already contains a
  query string.

### Rewrites

Rewrites change which object key is fetched without changing the browser URL.
They run after redirects and before root object resolution.

Rules:

- Exact path matches take priority over wildcard matches.
- Wildcard `*` captures the remaining path and can be inserted into `to` using
  `:splat`.

### Auto Rewrite Index

`edge.auto_rewrite_index` controls automatic directory-style rewrites.

When true, if the resolved object does not exist, RenderMesh tries:

```text
<path>/index.html
```

Examples:

- `/docs` can resolve to `/docs/index.html`.
- `/blog/post-1` can resolve to `/blog/post-1/index.html`.

Explicit rewrites have priority over auto-index rewrites. The MVP default is
`true`, and origins can disable it with:

```yaml
edge:
  auto_rewrite_index: false
```

## HTTP Edge Hooks

`edge.yaml` can define a global ordered chain of HTTP edge hooks for an origin.
There are no route-level hooks in the MVP.

Hooks run only for `GET` and `HEAD`. They run before redirects, rewrites, root
object resolution, and local mirror lookup.

Example:

```yaml
edges:
  - name: auth-or-render
    url: https://api.example.com/rendermesh/edge
    timeout_ms: 800

  - name: personalization
    url: https://api.example.com/rendermesh/personalization
    timeout_ms: 800
```

### Edge Request

RenderMesh sends the original request context:

```json
{
  "url": "https://megaloja.com.br/products/123?ref=ads",
  "method": "GET",
  "headers": {
    "accept": "text/html"
  },
  "body": ""
}
```

In the MVP, `body` is always an empty string because only `GET` and `HEAD`
execute edge hooks.

### Edge Response

RenderMesh reads the HTTP status code from the edge response, but only uses it
when the JSON payload produces a response.

Continue normal flow:

```json
{}
```

The edge HTTP status is ignored.

Accumulate headers and continue:

```json
{
  "headers": {
    "x-render-mode": "personalized"
  }
}
```

Return a direct response:

```json
{
  "body": "<html>ready</html>",
  "headers": {
    "content-type": "text/html"
  }
}
```

RenderMesh returns the payload body exactly and uses the edge HTTP status code.

Serve a specific mirrored file:

```json
{
  "file_path": "/products/show.html"
}
```

RenderMesh loads `file_path` from the current origin local mirror and uses the
edge HTTP status code.

Render the normal target file as Handlebars:

```json
{
  "params": {
    "title": "Produto X"
  }
}
```

Render a specific mirrored file as Handlebars:

```json
{
  "file_path": "/products/show.html",
  "params": {
    "title": "Produto X"
  }
}
```

### Edge Hook Rules

- Hooks execute in the configured order.
- `{}` continues to the next hook.
- Payloads containing only `headers` accumulate those headers and continue.
- Payloads containing `body`, `file_path`, or `params` stop the hook chain.
- Only `headers` from the JSON payload are propagated.
- HTTP headers from the edge API response are never propagated to the client.
- Without `params`, mirrored files are never rendered as Handlebars.
- Handlebars rendering is allowed only for HTML files.
- HTML detection prefers mirrored metadata `Content-Type: text/html` and falls
  back to `.html` or `.htm` file extensions.
- If `params` targets a non-HTML object, RenderMesh returns
  `415 Unsupported Media Type`.
- `file_path` must start with `/`, must not contain `..`, and must not contain
  control characters.
- Invalid `file_path` from an edge hook returns `502 Bad Gateway`.
- Edge timeout returns `504 Gateway Timeout`.
- Edge DNS, connection, TLS, or invalid JSON errors return `502 Bad Gateway`.

## Request Flow

Request handling order:

1. Normalize `Host`.
2. Resolve `Host` using exact match, then wildcard match.
3. Return `421 Misdirected Request` if no host matches.
4. Select the origin.
5. Load or reuse the origin edge config from the local mirror at
   `/_rendermesh/edge.yaml`.
6. Use defaults if the edge config object does not exist in the mirror.
7. Handle method:
   - `OPTIONS`: return automatic CORS preflight.
   - `GET` and `HEAD`: continue.
   - all others: return `405 Method Not Allowed`.
8. Apply automatic CORS headers derived from manifest hosts.
9. Execute global HTTP edge hooks.
10. If an edge hook produces a response, return it.
11. Apply redirects.
12. Apply explicit rewrites.
13. Resolve root object for paths ending in `/`.
14. Fetch the object from the origin local mirror.
15. If the object is missing and `auto_rewrite_index: true`, try
    `<path>/index.html`.
16. If still missing, apply `missing`.
17. Return the final response.

`HEAD` follows the same resolution flow as `GET`, but does not return a response
body.

## Response Headers

The final response includes:

- Relevant object metadata from the mirror, such as content type, ETag,
  cache-control, and last-modified.
- Automatic CORS headers when the request origin is allowed.
- Accumulated headers from edge hook payloads.

Edge hook payload headers may override static response headers except for
headers RenderMesh reserves for connection safety. Reserved header names should
include `connection`, `transfer-encoding`, and other hop-by-hop headers.

## Error Handling

- Unknown host: `421 Misdirected Request`.
- Unsupported method: `405 Method Not Allowed`.
- Missing object with no configured missing page available: `404 Not Found`.
- Invalid manifest: startup failure.
- Missing env var referenced by manifest: startup failure.
- Initial sync failure for any configured origin: startup failure.
- Invalid edge config YAML: origin config load failure; request returns `500`
  for that origin until fixed.
- Edge timeout: `504 Gateway Timeout`.
- Edge technical failure or invalid JSON: `502 Bad Gateway`.
- Edge `file_path` validation failure: `502 Bad Gateway`.
- Handlebars requested for non-HTML: `415 Unsupported Media Type`.
- Background sync failure: log error and keep serving the last successful
  mirror.
- Local mirror read failure: `500 Internal Server Error`.

## Observability

The MVP should log structured events with:

- host
- matched host rule
- origin
- path
- object key
- method
- status
- duration
- whether edge config was defaulted
- edge hook name and duration when hooks run
- sync result, object counts, and sync duration for each origin

Warnings:

- Missing `/_rendermesh/edge.yaml` for an origin.
- Edge hook returns headers-only payload and continues.

Errors:

- Manifest validation failure.
- Missing env vars.
- Initial sync failures.
- Background sync failures.
- Local mirror read failures.
- Edge hook technical failures.
- Invalid edge payloads.

## Testing Requirements

The MVP should include tests for:

- Manifest parsing and validation.
- Missing env var handling.
- Local mirror directory configuration.
- Initial sync must complete before traffic.
- Startup failure when first sync fails.
- Background sync interval handling.
- Background sync keeps last successful mirror on failure.
- New, changed, and deleted bucket objects updating the local mirror.
- Exact host resolution.
- Wildcard host resolution.
- Exact host priority over wildcard host.
- Most-specific wildcard priority.
- Unknown host returning `421`.
- Automatic CORS for exact hosts.
- Automatic CORS for wildcard hosts by reflecting matching origins.
- `OPTIONS` preflight without local file lookup.
- Missing `edge.yaml` defaults.
- Invalid edge YAML behavior.
- Redirect exact and wildcard rules.
- Rewrite exact and wildcard rules.
- Root object resolution.
- Auto rewrite index enabled and disabled.
- Missing `not_found` returning `404` with `/index.html` body.
- Missing `serve` returning `200`.
- Missing `redirect`.
- `GET`, `HEAD`, `OPTIONS`, and `405` handling.
- Edge hook `{}` continue behavior.
- Edge hook headers-only accumulation.
- Edge hook direct `body`.
- Edge hook `file_path`.
- Edge hook `params` rendering the normal target.
- Edge hook `file_path + params`.
- No Handlebars rendering when `params` is absent.
- Rejection of invalid `file_path`.
- `415` when `params` targets non-HTML.
- Edge timeout returning `504`.
- Edge invalid JSON returning `502`.

## Roadmap

After this MVP, likely next steps are:

- Cache rules and cache invalidation.
- Prerender orchestration.
- Render job queue and revalidation controls.
- Dynamic global manifest reload.
- Push-based edge config refresh.
- Admin API and MCP tools for inspection and invalidation.
- Per-route edge hooks.
- After-response edge hooks.
- Provider adapters beyond S3/R2.
- Metrics by host, origin, edge hook, and storage operation.
