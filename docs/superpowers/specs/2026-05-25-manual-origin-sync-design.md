# Manual Origin Sync Design

## Summary

RenderMesh currently refreshes origins during startup and on each configured
background interval. That keeps runtime traffic served from local mirrors, but
operators cannot safely force an immediate refresh after urgent origin changes.
When a bucket receives a corrected `/_rendermesh/edge.yaml`, a new `index.html`,
or an updated HTML template, the only immediate operational workaround is to
restart RenderMesh and rely on startup sync.

RenderMesh should expose an administrative manual sync endpoint that runs the
same origin refresh pipeline used by startup and background sync. The endpoint
must preserve the current atomic activation behavior: list source files, stage
mirror changes, parse edge config, compile templates, activate the next
generation only when every required step succeeds, and keep the previous
generation active when refresh fails.

## Goals

- Allow operators to force an immediate refresh for one origin.
- Reuse the existing refresh pipeline instead of creating a separate path.
- Keep origin activation atomic and generation-based.
- Reload `/_rendermesh/edge.yaml`, `edge.yml`, or `edge.json` during manual sync.
- Recompile HTML template ASTs before activating the new generation.
- Submit configured CDN refresh after successful activation.
- Return a clear HTTP response with the new origin generation and sync summary.
- Reject unauthorized manual sync attempts.
- Prevent concurrent refreshes from mutating the same origin at the same time.
- Emit logs and OpenTelemetry spans for manual refresh attempts.

## Non-Goals

- No global "sync all origins" endpoint in the first implementation.
- No request-time source reads during normal rendering.
- No distributed lock across multiple RenderMesh replicas.
- No new persistence layer for sync history.
- No filesystem watcher.
- No replacement for interval-based background sync.
- No separate CDN-only purge endpoint.

## API

The first implementation should expose:

```http
POST /_rendermesh/origins/{origin_id}/sync
Authorization: Bearer <admin-token>
```

The endpoint refreshes only the requested `origin_id`.

Example:

```bash
curl -X POST \
  -H "Authorization: Bearer $RENDERMESH_ADMIN_TOKEN" \
  https://carpago.autobanking.com.br/_rendermesh/origins/floorplan/sync
```

## Authentication

Manual sync is an administrative operation.

RenderMesh should read an optional environment variable:

```text
RENDERMESH_ADMIN_TOKEN
```

Behavior:

- If `RENDERMESH_ADMIN_TOKEN` is configured, the endpoint requires
  `Authorization: Bearer <token>`.
- If the header is missing, return `401 Unauthorized`.
- If the token does not match, return `403 Forbidden`.
- If `RENDERMESH_ADMIN_TOKEN` is not configured, the endpoint should return
  `403 Forbidden` and include a machine-readable error indicating that manual
  sync is disabled.

The token must not be logged.

## Response Contract

Successful response:

```json
{
  "origin_id": "floorplan",
  "generation": 12,
  "activated_at": "2026-05-25T16:40:08.360098Z",
  "captured_at": "2026-05-25T16:40:07.912481Z",
  "known_files": 35,
  "added_files": 1,
  "modified_files": 2,
  "removed_files": 0,
  "unchanged_files": 32,
  "downloaded_files": 3,
  "cdn": {
    "provider": "cloudfront",
    "status": "submitted",
    "request_id": "I6FQ3BXICP8O8IM4KIT56EJ627",
    "submitted_items": 3
  }
}
```

If no CDN refresh is configured or no refresh is submitted:

```json
{
  "origin_id": "floorplan",
  "generation": 12,
  "activated_at": "2026-05-25T16:40:08.360098Z",
  "captured_at": "2026-05-25T16:40:07.912481Z",
  "known_files": 35,
  "added_files": 0,
  "modified_files": 0,
  "removed_files": 0,
  "unchanged_files": 35,
  "downloaded_files": 0,
  "cdn": null
}
```

Error response shape:

```json
{
  "error": {
    "code": "origin_sync_failed",
    "message": "failed to parse edge config as YAML or JSON"
  }
}
```

## Status Codes

- `200 OK`: refresh completed and the new generation was activated.
- `401 Unauthorized`: admin token is configured but the authorization header is
  missing.
- `403 Forbidden`: token is invalid or manual sync is disabled.
- `404 Not Found`: `origin_id` is not present in the global manifest.
- `409 Conflict`: another refresh for the same origin is already running.
- `500 Internal Server Error`: refresh failed. The previous active generation
  remains active.

## Refresh Semantics

Manual sync must run the same logical pipeline as startup and background sync:

1. Resolve the configured origin.
2. Acquire the per-origin refresh lock.
3. List source objects.
4. Normalize and validate source keys.
5. Build a new freshness index.
6. Diff against the current in-memory freshness index.
7. Stage the origin mirror.
8. Download added and modified files.
9. Remove files deleted from the source.
10. Load and validate edge config from the staged mirror.
11. Compile the next template registry from staged HTML files.
12. Activate the staged mirror.
13. Swap the edge config store.
14. Swap the template registry.
15. Swap the freshness index.
16. Update the origin runtime snapshot with the new generation.
17. Submit CDN refresh when configured and the diff requires it.
18. Release the per-origin refresh lock.

If any step before activation fails, the existing active mirror, edge config,
template registry, freshness index, and origin runtime snapshot must remain
active.

If CDN refresh fails after activation, the new generation remains active and the
runtime snapshot records the CDN error, matching the existing background refresh
policy.

## Concurrency

RenderMesh should keep an in-memory lock per origin.

Rules:

- A manual sync and background sync for the same origin must not run at the same
  time.
- Two manual sync requests for the same origin must not run at the same time.
- Syncs for different origins may run concurrently.
- The first implementation should return `409 Conflict` if the per-origin lock
  is already held.

This is an instance-local lock. Multi-replica distributed coordination is
outside the first implementation.

## Service Design

The refresh orchestration should move behind a reusable service, conceptually:

```rust
pub struct OriginRefreshService {
    // manifest origins, storage adapters, mirror syncer, stores, runtime state,
    // CDN refresh adapters, and per-origin locks
}

impl OriginRefreshService {
    pub async fn refresh_origin(
        &self,
        origin_id: &str,
        trigger: OriginRefreshTrigger,
    ) -> Result<OriginRefreshOutcome, OriginRefreshError>;
}

pub enum OriginRefreshTrigger {
    Startup,
    Background,
    Manual,
}
```

Startup sync, background sync, and manual sync should all call this service.
The route layer should not call storage adapters, mirror repositories, CDN
repositories, or template stores directly.

## Route Design

The existing system route module should add:

```text
POST /_rendermesh/origins/{origin_id}/sync
```

The route should:

1. Validate the admin token.
2. Delegate to `OriginRefreshService`.
3. Convert service errors into DTO error responses.
4. Return the sync outcome DTO.

The route must not contain business logic for listing storage, activating
mirrors, compiling templates, or refreshing CDN providers.

## Observability

Manual sync should emit an OpenTelemetry span:

```text
rendermesh.origin_sync.manual
```

Recommended fields:

- `origin_id`
- `generation`
- `added_files`
- `modified_files`
- `removed_files`
- `unchanged_files`
- `downloaded_files`
- `duration_ms`
- `result`
- `error_code`

Logs:

- Manual sync started.
- Manual sync activated generation.
- Manual sync rejected because origin is already refreshing.
- Manual sync failed before activation.
- Manual sync submitted CDN refresh.
- Manual sync activated generation but CDN refresh failed.

## Documentation

Update:

- `README.md`
- `docs/local-mirror-and-sync.md`
- `docs/observability.md`

The docs should explain:

- Manual sync is administrative.
- It uses the same activation semantics as background sync.
- It requires `RENDERMESH_ADMIN_TOKEN`.
- It is per-origin.
- It is useful after uploading `/_rendermesh/edge.yaml`, templates, or urgent
  static assets.
- Restarting the deployment is no longer required for a normal origin refresh.

## Testing Requirements

Unit tests:

- Admin auth accepts a valid bearer token.
- Admin auth rejects missing token with `401`.
- Admin auth rejects invalid token with `403`.
- Manual sync disabled returns `403` when no admin token is configured.
- Per-origin lock returns `409` when an origin refresh is already running.
- Syncs for different origins can run concurrently.

Service tests:

- Manual sync updates the origin snapshot generation.
- Manual sync downloads newly added source files.
- Manual sync updates modified source files.
- Manual sync removes deleted source files.
- Manual sync reloads a changed `/_rendermesh/edge.yaml`.
- Manual sync reloads a changed `/_rendermesh/edge.json`.
- Manual sync recompiles changed HTML templates.
- Manual sync removes deleted templates from the in-memory registry.
- Manual sync keeps the previous generation active when edge config parsing
  fails.
- Manual sync keeps the previous generation active when template compilation
  fails.
- Manual sync records CDN errors without rolling back an already activated
  generation.

Integration tests:

- `POST /_rendermesh/origins/{origin_id}/sync` returns `200` and sync summary.
- `POST /_rendermesh/origins/missing/sync` returns `404`.
- Unauthorized manual sync requests do not change runtime state.
- A successful manual sync makes a newly uploaded route visible without restart.
- A successful manual sync makes updated SPA fallback config visible without
  restart.

## Rollout Notes

The feature should be backwards-compatible:

- Existing configs without `RENDERMESH_ADMIN_TOKEN` continue to boot.
- The manual sync endpoint exists but is disabled without the token.
- Startup sync and background sync keep their existing behavior.
- Debug endpoints remain read-only.

Production environments should set `RENDERMESH_ADMIN_TOKEN` as a secret before
operators depend on the endpoint.
