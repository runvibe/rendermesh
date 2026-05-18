# Configuration

RenderMesh has two configuration layers:

- A **global manifest** loaded from `RENDERMESH_MANIFEST`.
- A per-origin **edge config** loaded from each bucket as YAML or JSON.

This document covers the global manifest and environment variables. See [Origin Edge Config](edge-config.md) for bucket-level behavior.

## Environment Variables

Common runtime variables:

- `RENDERMESH_MANIFEST`: Path to the global manifest YAML or JSON. Defaults to `./rendermesh.yaml`.
- `APP_HOST`: Bind host. Defaults to `127.0.0.1`.
- `APP_PORT`: Bind port. Defaults to `8080`.
- `APP_BODY_LIMIT_BYTES`: Request body read limit for fallback rendering. Defaults to `1048576`.
- `OTEL_ENABLED`: Enables OpenTelemetry export when truthy. Defaults to enabled.

Each origin references storage credentials by environment variable name:

- Endpoint: for example `MY_APP_STORAGE_ENDPOINT`.
- Region: for example `MY_APP_STORAGE_REGION`.
- Access key id: for example `MY_APP_ACCESS_KEY_ID`.
- Secret access key: for example `MY_APP_SECRET_ACCESS_KEY`.
- Optional force path style flag: for example `MY_APP_FORCE_PATH_STYLE`.

Truth values for `force_path_style` are `1`, `true`, `yes`, and `on`. False values are `0`, `false`, `no`, and `off`.

## Global Manifest Example

The global manifest can be written as YAML or JSON. The fields are the same in both formats.

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

## `runtime`

- `local_store_dir`: Directory where RenderMesh stores local bucket mirrors.
- `sync_interval_seconds`: Default background sync interval for origins that do not override it.

## `origins`

Each origin id must contain only ASCII letters, numbers, `_`, or `-`.

Fields:

- `type`: Currently `s3`.
- `bucket`: Bucket name used by the storage provider.
- `endpoint_env`: Environment variable containing the S3/R2 endpoint.
- `region_env`: Environment variable containing the region.
- `access_key_id_env`: Environment variable containing the access key id.
- `secret_access_key_env`: Environment variable containing the secret access key.
- `force_path_style_env`: Optional environment variable controlling path-style S3 access.
- `sync_interval_seconds`: Optional origin-specific sync interval.

## `hosts`

Hosts map incoming domains to origin ids.

Exact hosts:

```yaml
hosts:
  myapp.com:
    origin: my_app
```

Wildcard hosts:

```yaml
hosts:
  "*.myapp.com":
    origin: my_app
```

Exact hosts take priority over wildcard hosts. Unknown hosts return `421 Misdirected Request`.

## Local Lab Manifest

The local example uses [examples/local/rendermesh.yaml](../examples/local/rendermesh.yaml), which maps `test.com` and `*.test.com` to a MinIO-backed origin named `local_app`.
