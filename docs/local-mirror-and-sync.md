# Local Mirror And Sync

RenderMesh serves runtime traffic from local bucket mirrors. It does not fetch bucket objects directly during normal request handling.

## Startup Sync

At startup, RenderMesh:

1. Loads the global manifest.
2. Creates one S3-compatible storage adapter per origin.
3. Syncs each configured origin into `runtime.local_store_dir/<origin_id>`.
4. Loads each origin's edge config from `/_rendermesh/edge.yaml`, `edge.yml`, or `edge.json`.
5. Compiles HTML templates into the in-memory template store.
6. Starts background sync tasks.
7. Starts serving traffic.

Startup fails if any initial origin sync fails.

## Local Layout

For this manifest:

```yaml
runtime:
  local_store_dir: ./var/rendermesh/origins
origins:
  my_app:
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

Failed background syncs are logged and the previous local mirror remains active.

## Atomic Activation

Sync writes into a staging directory first. After a successful sync, the staging directory replaces the active origin mirror. If activation fails, RenderMesh attempts to preserve the previous mirror.

## Refresh Behavior

After a successful sync, RenderMesh refreshes:

- The origin edge config store.
- The origin HTML template registry.

This means changes to `/_rendermesh/edge.yaml`, `edge.yml`, `edge.json`, and HTML templates become active after the next successful sync.

## Deleted Objects

Objects missing from the remote bucket are removed from the local mirror during sync. Removed HTML templates are also removed from the in-memory template registry after template reload.
