# Local Origin Design

## Summary

RenderMesh should support `type: local` origins in addition to S3-compatible
bucket origins. A local origin points at a directory on the RenderMesh host and
uses the same render pipeline as bucket-backed origins: host resolution, CORS,
edge config loading, edge hooks, redirects, rewrites, HTML template compilation,
and static response assembly.

The implementation should keep local origins as another storage source that can
feed the existing mirror and refresh pipeline. The render gateway should not
learn whether an origin came from S3 or from a local directory.

## Goals

- Allow users to serve an origin from a local filesystem path.
- Keep the manifest format explicit with `type: local`.
- Reuse the existing local mirror, edge config store, and template store.
- Support local origins in YAML and JSON manifests.
- Support `/_rendermesh/edge.yaml`, `edge.yml`, and `edge.json` inside local
  origins.
- Keep path traversal protections for local source reads and mirror writes.
- Make local origins useful for development, tests, self-hosted deployments, and
  environments where the frontend artifact is already mounted as a volume.

## Non-Goals

- No file watcher in this first iteration.
- No request-time direct reads from arbitrary source paths.
- No symlink traversal outside the configured local source root.
- No per-origin response cache behavior.
- No mixed origin where one origin reads from both S3 and local path.
- No admin API for changing local paths at runtime.

## Manifest Contract

The global manifest continues to define origins under `origins`. S3 origins keep
their existing fields. Local origins use a new `path` field and do not configure
bucket or S3 connection fields.

YAML example:

```yaml
version: 1

runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60

origins:
  local_docs:
    type: local
    path: ./examples/local/bucket
    sync_interval_seconds: 5

hosts:
  docs.test:
    origin: local_docs
```

JSON example:

```json
{
  "version": 1,
  "runtime": {
    "local_store_dir": "./var/rendermesh/origins",
    "sync_interval_seconds": 60
  },
  "origins": {
    "local_docs": {
      "type": "local",
      "path": "./examples/local/bucket",
      "sync_interval_seconds": 5
    }
  },
  "hosts": {
    "docs.test": {
      "origin": "local_docs"
    }
  }
}
```

## Field Rules

`type: local` requires:

- `path`: The source directory for the origin.

`type: local` allows:

- `sync_interval_seconds`: Optional refresh interval. When omitted,
  `runtime.sync_interval_seconds` is used.

`type: local` rejects:

- `bucket`
- `endpoint_env`
- `region_env`
- `access_key_id_env`
- `secret_access_key_env`
- `force_path_style_env`

`type: s3` keeps the existing validation rules and rejects `path`.

## Path Resolution

Local origin `path` should be resolved as follows:

- Absolute paths are used as configured.
- Relative paths are resolved relative to the directory containing the global
  manifest file.
- The resolved path must exist during startup.
- The resolved path must be a directory.
- The source root should be canonicalized before reads.
- Files outside the canonical source root must never be read.

Resolving relative paths from the manifest directory makes examples and mounted
configuration directories portable. This intentionally differs from
`runtime.local_store_dir`, which currently follows the existing process working
directory behavior for compatibility.

## Runtime Behavior

Local origins should still be mirrored into `runtime.local_store_dir/<origin_id>`
before traffic is served. RenderMesh should not serve directly from the
configured source path.

This keeps one data plane:

```text
local path -> local origin storage adapter -> mirror sync -> local mirror -> render gateway
S3 bucket  -> S3 storage adapter          -> mirror sync -> local mirror -> render gateway
```

Benefits:

- Existing `LocalMirrorRepository` remains the only static file read path for
  request handling.
- Existing template loading remains unchanged.
- Existing edge config refresh remains unchanged.
- Atomic activation behavior remains consistent across origin types.
- Future providers can use the same source adapter pattern.

## Local Directory Storage Adapter

Add a repository adapter, tentatively `LocalDirectoryStorageRepository`, that
implements the existing `RemoteStorage` trait used by `MirrorSyncService`.

Responsibilities:

- Walk the configured source directory recursively.
- Ignore directories.
- Return object summaries for regular files.
- Read object bodies by normalized key.
- Produce metadata compatible with the mirror:
  - `content_type` from `mime_guess`.
  - `etag` from a stable local fingerprint, such as file size plus modified
    timestamp, or a content hash if needed.
  - `last_modified` from filesystem metadata when available.
  - `cache_control` as `None` in the first iteration.
- Reject keys that escape the source root.
- Reject source entries under reserved mirror metadata names such as
  `.rendermesh-meta`.

The adapter should not follow symlinks outside the canonical source root. The
first implementation may either skip symlinks or resolve them and reject targets
outside the source root. Skipping symlinks is the safer MVP default.

## Startup Flow

Startup should become provider-aware without leaking provider details into
routes or the render gateway:

1. Load the global manifest from YAML or JSON.
2. Validate each origin according to its type.
3. Build a storage adapter per origin:
   - `S3StorageRepository` for `type: s3`.
   - `LocalDirectoryStorageRepository` for `type: local`.
4. Sync every origin into the mirror before serving.
5. Load each origin edge config from the mirror.
6. Compile HTML templates from the mirror.
7. Spawn background refresh tasks for every origin.

If a local origin path is missing, not a directory, or unreadable, startup fails.
If a background refresh fails, RenderMesh logs the error and keeps the last
successful mirror, matching the existing S3 behavior.

## Background Refresh

Local origins should participate in the same periodic refresh loop as S3
origins.

This means edits to local files become visible after:

1. The next successful sync interval.
2. Edge config refresh.
3. Template store refresh.

The MVP should not add filesystem watchers. A watcher can be added later as an
optimization for development environments.

## Edge Config And Templates

Local origins use the same bucket-internal paths, now interpreted relative to
the local source root:

- `/_rendermesh/edge.yaml`
- `/_rendermesh/edge.yml`
- `/_rendermesh/edge.json`

Only HTML files from the mirrored local origin are compiled into the Handlebars
template store. Non-HTML files remain static objects and are never compiled as
templates.

## Edge Hook Context

The edge hook context currently sends:

```json
{
  "context": {
    "bucket": "bucket-name",
    "ip": "203.0.113.10",
    "origin": "origin_id"
  }
}
```

For `type: local`, `context.bucket` should be an empty string in the first
implementation only if keeping the current DTO unchanged is the smallest safe
step. Prefer adding a provider-neutral field before exposing local origins
publicly:

```json
{
  "context": {
    "origin": "local_docs",
    "origin_type": "local",
    "source": "local_docs",
    "ip": "203.0.113.10"
  }
}
```

To avoid breaking the current hook contract in this iteration, the recommended
implementation is:

- Keep `bucket` for compatibility.
- Set `bucket` to the configured bucket for S3 origins.
- Set `bucket` to the origin id for local origins.
- Add `origin_type` in a follow-up contract update if needed.

## DTO Shape

The current `OriginConfig` has S3-specific required fields. Local origin support
should split provider-specific fields so invalid combinations are impossible or
at least rejected clearly.

Recommended Rust shape:

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OriginConfig {
    S3(S3OriginConfig),
    Local(LocalOriginConfig),
}
```

Shared helpers can expose:

- `sync_interval_seconds() -> Option<u64>`
- `source_label() -> String`
- provider-specific accessors where needed

If a smaller migration is needed, `OriginConfig` may temporarily keep optional
fields and validation can enforce the matrix. The enum form is preferred because
it better preserves provider boundaries as more origins are added.

## Repository And Service Changes

Expected module changes:

- `src/dto/manifest.rs`
  - Add `local` origin variant.
  - Add local path field.
  - Preserve YAML and JSON deserialization.
- `src/services/manifest.rs`
  - Validate S3 and local origin field rules.
  - Resolve local paths relative to manifest directory or expose enough
    metadata for startup to do so.
- `src/repositories/local_directory_storage.rs`
  - Implement local directory source adapter.
- `src/repositories/mod.rs`
  - Export the new repository.
- `src/services/startup.rs`
  - Build storage adapters by origin type.
  - Keep sync, edge config refresh, template loading, and background refresh
    shared.
- `src/services/render_gateway.rs`
  - Avoid provider-specific behavior. At most, adjust origin source labels used
    for edge hook context.
- Documentation
  - Update root README, configuration docs, overview docs, testing docs, and
    local example docs.

## Testing Plan

Unit tests:

- Manifest parses `type: local` from YAML.
- Manifest parses `type: local` from JSON.
- Local origin requires `path`.
- Local origin rejects S3-only fields.
- S3 origin rejects `path`.
- Relative local path resolves against the manifest directory.
- Missing local path fails startup or adapter construction.
- File path traversal is rejected.
- Symlink outside source root is skipped or rejected.
- Local directory adapter lists nested files.
- Local directory adapter reads object content and metadata.
- Local directory adapter excludes `.rendermesh-meta`.

Service tests:

- Startup syncs local origin into `runtime.local_store_dir/<origin_id>`.
- Local `edge.yaml` is loaded after sync.
- Local `edge.json` is loaded after sync.
- HTML templates from local origin compile into memory.
- Background refresh updates changed local files.
- Background refresh removes deleted local files.
- Failed local background refresh keeps the previous mirror.

Integration tests:

- Render route serves a file from a local origin.
- Render route applies rewrite rules from a local origin edge config.
- Render route calls edge hook for a local origin.
- Edge hook `file_path` can select a file from the local origin mirror.
- Missing-file behavior works for a local origin.

Local lab:

- Add a second origin using `type: local`.
- Map a host such as `local.test` to the local origin.
- Document curl/browser flows for the local origin.

## Acceptance Criteria

- A manifest can define both S3 and local origins in the same RenderMesh
  instance.
- Local origins serve through the same request lifecycle as S3 origins.
- Local origins support YAML and JSON edge config files.
- Local origins support HTML template rendering through edge hook params.
- Local origin refresh is interval-based and uses the existing mirror activation
  behavior.
- The full test suite passes with new coverage for local origin parsing,
  startup, sync, rendering, and docs.

## Open Questions

- Should the edge hook contract add `origin_type` immediately, or should this be
  deferred to a separate backward-compatible change?
- Should symlinks inside the source root be skipped entirely, or allowed when
  their canonical target remains inside the source root?
- Should local origins support an optional `cache_control` map later, or should
  headers stay entirely driven by metadata and edge hooks?
