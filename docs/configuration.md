# Configuration

RenderMesh has two configuration layers:

- A **global manifest** loaded from `RENDERMESH_MANIFEST`.
- A per-origin **edge config** loaded from each origin source as YAML or JSON.

This document covers the global manifest and environment variables. See [Origin Edge Config](edge-config.md) for origin-level behavior.

## Environment Variables

Common runtime variables:

- `RENDERMESH_MANIFEST`: Path to the global manifest YAML or JSON. Defaults to `./rendermesh.yaml`.
- `APP_HOST`: Bind host. Defaults to `127.0.0.1`.
- `APP_PORT`: Bind port. Defaults to `8080`.
- `APP_BODY_LIMIT_BYTES`: Request body read limit for fallback rendering. Defaults to `1048576`.
- `OTEL_ENABLED`: Enables OpenTelemetry export when truthy. Defaults to enabled.

S3 origins reference storage connection settings by environment variable name:

- Endpoint: for example `MY_APP_STORAGE_ENDPOINT`.
- Region: for example `MY_APP_STORAGE_REGION`.
- Optional access key id: for example `MY_APP_ACCESS_KEY_ID`.
- Optional secret access key: for example `MY_APP_SECRET_ACCESS_KEY`.
- Optional force path style flag: for example `MY_APP_FORCE_PATH_STYLE`.
- Optional CloudFront distribution id: for example `MY_APP_CLOUDFRONT_DISTRIBUTION_ID`.
- Optional Cloudflare zone id and API token: for example `MY_APP_CLOUDFLARE_ZONE_ID` and `MY_APP_CLOUDFLARE_API_TOKEN`.

Truth values for `force_path_style` are `1`, `true`, `yes`, and `on`. False values are `0`, `false`, `no`, and `off`.

When `access_key_id_env` and `secret_access_key_env` are omitted, RenderMesh uses the AWS SDK default credential chain. This supports IAM roles for service accounts (IRSA) on EKS, EC2 instance roles, and the usual local AWS credential sources. If one static credential field is configured, both must be configured.

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
    force_path_style_env: MY_APP_FORCE_PATH_STYLE
    sync_interval_seconds: 30
    cdn:
      provider: cloudfront
      distribution_id_env: MY_APP_CLOUDFRONT_DISTRIBUTION_ID
      strategy: changed_paths

hosts:
  myapp.com:
    origin: my_app
  "*.myapp.com":
    origin: my_app
```

## `runtime`

- `local_store_dir`: Directory where RenderMesh stores local origin mirrors.
- `sync_interval_seconds`: Default background sync interval for origins that do not override it.

## `origins`

Each origin id must contain only ASCII letters, numbers, `_`, or `-`.

S3 origin fields:

- `type`: `s3`.
- `bucket`: Bucket name used by the storage provider.
- `endpoint_env`: Environment variable containing the S3/R2 endpoint.
- `region_env`: Environment variable containing the region.
- `access_key_id_env`: Optional environment variable containing the access key id for static credentials.
- `secret_access_key_env`: Optional environment variable containing the secret access key for static credentials.
- `force_path_style_env`: Optional environment variable controlling path-style S3 access.
- `sync_interval_seconds`: Optional origin-specific sync interval.

For EKS with IRSA or other AWS managed identities, omit `access_key_id_env` and `secret_access_key_env`:

```yaml
origins:
  my_app:
    type: s3
    bucket: bucket_my_app_123
    endpoint_env: MY_APP_STORAGE_ENDPOINT
    region_env: MY_APP_STORAGE_REGION
```

For S3-compatible providers that require static credentials, configure both fields:

```yaml
origins:
  my_app:
    type: s3
    bucket: bucket_my_app_123
    endpoint_env: MY_APP_STORAGE_ENDPOINT
    region_env: MY_APP_STORAGE_REGION
    access_key_id_env: MY_APP_ACCESS_KEY_ID
    secret_access_key_env: MY_APP_SECRET_ACCESS_KEY
```

Local origin fields:

- `type`: `local`.
- `path`: Source directory for the origin.
- `sync_interval_seconds`: Optional origin-specific sync interval.

```yaml
origins:
  docs:
    type: local
    path: ./examples/local/bucket
    sync_interval_seconds: 5
```

Absolute local paths are used as configured. Relative local paths are resolved from the directory containing the global manifest file. The path must exist and be a directory during startup.

Local origins do not accept S3 fields such as `bucket`, `endpoint_env`, `region_env`, `access_key_id_env`, `secret_access_key_env`, or `force_path_style_env`.

## `cdn`

Each origin can optionally configure CDN refresh. CDN refresh runs after a new origin generation is activated.

CloudFront:

```yaml
origins:
  my_app:
    type: s3
    bucket: bucket_my_app_123
    endpoint_env: MY_APP_STORAGE_ENDPOINT
    region_env: MY_APP_STORAGE_REGION
    cdn:
      provider: cloudfront
      distribution_id_env: MY_APP_CLOUDFRONT_DISTRIBUTION_ID
      strategy: changed_paths
```

Cloudflare:

```yaml
origins:
  docs:
    type: local
    path: ./docs
    cdn:
      provider: cloudflare
      zone_id_env: DOCS_CLOUDFLARE_ZONE_ID
      api_token_env: DOCS_CLOUDFLARE_API_TOKEN
      strategy: changed_paths
      url_prefixes:
        - https://docs.example.com
```

Fields:

- `provider`: `cloudfront` or `cloudflare`.
- `strategy`: `changed_paths` or `all`. Defaults to `changed_paths`.
- `distribution_id_env`: CloudFront distribution id env var.
- `zone_id_env`: Cloudflare zone id env var.
- `api_token_env`: Cloudflare API token env var.
- `url_prefixes`: Optional Cloudflare URL prefixes. When omitted, RenderMesh derives `https://<host>` from exact host mappings for the origin.
- `api_base_env`: Optional Cloudflare API base env var for tests or compatible proxies.

`changed_paths` invalidates added, modified, and removed paths from the freshness diff. `all` purges the whole configured CDN cache scope when a refresh has any changes.

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
