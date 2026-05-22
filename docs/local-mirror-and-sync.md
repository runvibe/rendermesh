# Local Mirror And Sync

RenderMesh serves runtime traffic from local origin mirrors. It does not fetch source objects directly during normal request handling.

## Startup Sync

At startup, RenderMesh:

1. Loads the global manifest.
2. Creates one storage adapter per origin.
3. Lists each configured origin and builds an in-memory freshness index.
4. Stages changed origin files under `.rendermesh-sync`.
5. Loads each staged origin's edge config from `/_rendermesh/edge.yaml`, `edge.yml`, or `edge.json`.
6. Compiles staged HTML templates into the in-memory template store.
7. Activates the staged mirror and runtime state after validation succeeds.
8. Starts background sync tasks.
9. Starts serving traffic.

Startup fails if any initial origin refresh, edge config parse, or template compilation fails.

## Local Layout

For this manifest:

```yaml
runtime:
  local_store_dir: ./var/rendermesh/origins
origins:
  my_app:
    type: s3
    bucket: bucket_my_app_123
```

Objects are mirrored under:

```text
./var/rendermesh/origins/my_app/
```

RenderMesh also writes metadata sidecars under `.rendermesh-meta/`.

## Background Sync

Each origin syncs periodically. The interval comes from:

1. `origins.<origin>.sync_interval_seconds` when present.
2. `runtime.sync_interval_seconds` otherwise.

Failed background syncs are logged and the previous local mirror, edge config, freshness index, and template registry remain active.

## Freshness Index

RenderMesh keeps an in-memory freshness index per origin. The index records each known source path plus metadata such as size, ETag, last-modified value, content type, cache-control value, optional creation time, and the time RenderMesh captured the listing.

Local directory origins generate content-hash ETags (`sha256:<digest>`) during listing, so same-size edits are still detected on the next refresh.

On each refresh, RenderMesh lists the source again and classifies paths as:

- `added`
- `modified`
- `removed`
- `unchanged`

Only added and modified paths are fetched from the source provider. Removed paths are removed from the staged mirror.

## Atomic Activation

Refresh writes into a staging directory first. RenderMesh parses edge config and compiles template updates from the staging directory before activation. If any required step fails, the previous generation remains active.

Each successful activation increments the origin generation exposed by the runtime debug API.

## Runtime Debug API

RenderMesh exposes the current in-memory snapshot for origin refresh state:

```text
GET /_rendermesh/origins
GET /_rendermesh/origins/{origin_id}/snapshot
GET /_rendermesh/origins/{origin_id}/freshness
```

Snapshots include the origin id, generation, activation time, capture time, known file count, added/modified/removed/unchanged counts, downloaded file count, and the last background refresh error when present.

## Refresh Behavior

After a successful refresh, RenderMesh activates:

- The local origin mirror.
- The origin edge config.
- The origin freshness index.
- The origin HTML template registry.

This means changes to `/_rendermesh/edge.yaml`, `edge.yml`, `edge.json`, and HTML templates become active after the next successful sync.

Template compilation is incremental. Added or modified HTML candidates are compiled, removed templates are dropped, and files that stop being HTML are removed from the registry.

## Deleted Objects

Objects missing from the source are removed from the local mirror during refresh. Removed HTML templates are also removed from the in-memory template registry after successful activation.
