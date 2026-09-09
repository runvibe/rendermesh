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
- Optional CloudFront SaaS distribution id and connection group id: for example `MY_APP_CLOUDFRONT_MULTI_TENANT_DISTRIBUTION_ID` and `MY_APP_CLOUDFRONT_CONNECTION_GROUP_ID`.
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
- `edge_context`: Optional static JSON/YAML value sent to edge hooks as `context.edge_context`.

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
- `edge_context`: Optional static JSON/YAML value sent to edge hooks as `context.edge_context`.

```yaml
origins:
  docs:
    type: local
    path: ./examples/local/bucket
    sync_interval_seconds: 5
```

Absolute local paths are used as configured. Relative local paths are resolved from the directory containing the global manifest file. The path must exist and be a directory during startup.

Local origins do not accept S3 fields such as `bucket`, `endpoint_env`, `region_env`, `access_key_id_env`, `secret_access_key_env`, or `force_path_style_env`.

### `edge_context`

Use `edge_context` for non-secret origin metadata that every edge hook for that origin should receive.

```yaml
origins:
  storefront:
    type: s3
    bucket: storefront-prod
    endpoint_env: STOREFRONT_STORAGE_ENDPOINT
    region_env: STOREFRONT_STORAGE_REGION
    edge_context:
      tenant_id: loja-123
      environment: production
      feature_flags:
        checkout_v2: true
```

RenderMesh does not interpolate environment variables inside `edge_context`. Do not put credentials, API tokens, private keys, session tokens, or end-user personal data in this field. See [Edge Context](edge-context.md) for payload examples and edge API usage.

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

CloudFront SaaS:

```yaml
origins:
  app:
    type: s3
    bucket: app-assets
    endpoint_env: APP_STORAGE_ENDPOINT
    region_env: APP_STORAGE_REGION
    cdn:
      provider: cloudfront_saas
      distribution_id_env: APP_CLOUDFRONT_MULTI_TENANT_DISTRIBUTION_ID
      connection_group_id_env: APP_CLOUDFRONT_CONNECTION_GROUP_ID
      strategy: changed_paths
      parameters_env:
        origin-domain: APP_CLOUDFRONT_ORIGIN_DOMAIN
      certificate:
        mode: managed
        validation_token_host: cloudfront
```

Fields:

- `provider`: `cloudfront`, `cloudfront_saas`, or `cloudflare`.
- `strategy`: `changed_paths` or `all`. Defaults to `changed_paths`.
- `distribution_id_env`: CloudFront distribution id env var.
- `connection_group_id_env`: Optional CloudFront SaaS connection group id env var. When omitted, CloudFront uses the distribution's default connection group.
- `parameters_env`: Optional CloudFront SaaS map of tenant parameter names to environment variable names. RenderMesh resolves the environment variables during startup and passes the resulting values to CloudFront.
- `certificate`: Optional CloudFront SaaS tenant certificate block. When omitted, the tenant inherits the multi-tenant distribution certificate behavior.
- `validation_token_host`: CloudFront SaaS managed-certificate validation host. Defaults to `cloudfront`.
- `zone_id_env`: Cloudflare zone id env var.
- `api_token_env`: Cloudflare API token env var.
- `url_prefixes`: Optional Cloudflare URL prefixes. When omitted, RenderMesh derives canonical `https://<normalized-host>` prefixes from exact host mappings for the origin.
- `api_base_env`: Optional Cloudflare API base env var for tests or compatible proxies.

`changed_paths` invalidates added, modified, and removed paths from the freshness diff. `all` purges the whole configured CDN cache scope when a refresh has any changes.

### `cdn.provider: cloudfront_saas`

Use `cloudfront_saas` when one RenderMesh origin should drive CloudFront SaaS Manager tenants instead of a standard distribution alias list. This provider is separate from `cdn.domains`: it derives tenants from exact `hosts` entries and never creates tenants from wildcard rules.

Fields are resolved through environment-variable indirection. Any field ending in `_env` contains the name of an environment variable, not the distribution id, connection group id, or parameter value itself.

| Field | Required | Default | Notes |
|---|---|---|---|
| `provider` | Yes | None | Must be `cloudfront_saas`. |
| `distribution_id_env` | Yes | None | Names the environment variable containing the multi-tenant distribution id. Startup fails if the field is blank or the referenced variable is missing, empty, or whitespace-only. |
| `connection_group_id_env` | No | CloudFront distribution default connection group | Names the environment variable containing the connection group id. If present, the field itself and its resolved value must be non-empty after trimming. |
| `strategy` | No | `changed_paths` | `changed_paths` invalidates changed paths only. `all` invalidates `/*` when the generation has changes. |
| `parameters_env` | No | Empty map | Each key is the literal CloudFront tenant parameter name. Each value is the name of an environment variable whose resolved value is sent to CloudFront. Blank parameter names, blank env-var names, and empty or whitespace-only resolved values are rejected. |
| `certificate` | No | Inherit the multi-tenant distribution certificate behavior | When omitted, RenderMesh does not send a tenant certificate request and CloudFront keeps the inherited certificate behavior. |
| `certificate.mode` | Yes, when `certificate` is present | None | Only `managed` is implemented. |
| `certificate.validation_token_host` | No, when `certificate.mode` is `managed` | `cloudfront` | Only `cloudfront` is supported. `self_hosted` is not implemented. |

Unknown fields in the `certificate` block are rejected during manifest parsing.

RenderMesh resolves `distribution_id_env`, `connection_group_id_env`, and every `parameters_env` value during startup before making AWS calls. Resolution errors identify the manifest field and environment key but never include the resolved value. This keeps manifests portable across environments and avoids placing CloudFront identifiers or tenant parameter values directly in source-controlled YAML.

CloudFront SaaS tenant reconciliation owns the `enabled` state. If a matching
tenant is explicitly disabled, the next reconciliation updates it and
re-enables it.

### `cdn.domains`

`cdn.domains` reconciles CDN-facing domains from the global `hosts` map during startup.

CloudFront:

```yaml
cdn:
  provider: cloudfront
  distribution_id_env: WEB_CLOUDFRONT_DISTRIBUTION_ID
  domains:
    enabled: true
    origin_domain_env: RENDERMESH_PUBLIC_ORIGIN
    certificate_arn_env: WEB_CLOUDFRONT_CERTIFICATE_ARN
    include_wildcards: false
    remove_extra_domains: false
```

Cloudflare DNS records:

```yaml
cdn:
  provider: cloudflare
  zone_id_env: WEB_CLOUDFLARE_ZONE_ID
  api_token_env: WEB_CLOUDFLARE_API_TOKEN
  domains:
    enabled: true
    mode: dns_records
    origin_domain_env: RENDERMESH_PUBLIC_ORIGIN
    proxied: true
    include_wildcards: false
    remove_extra_domains: false
```

Domain fields:

- `enabled`: Enables domain reconciliation.
- `mode`: Cloudflare mode. `dns_records` is implemented. `custom_hostnames` is reserved for a future Cloudflare for SaaS implementation.
- `origin_domain_env`: Env var containing the upstream DNS name that CDN domains should point to, such as the public RenderMesh load balancer hostname.
- `certificate_arn_env`: CloudFront ACM certificate ARN env var. Required for CloudFront domain reconciliation.
- `proxied`: Cloudflare DNS proxied flag. Defaults to `true`.
- `include_wildcards`: Include wildcard hosts such as `*.example.com`. Defaults to `false`.
- `remove_extra_domains`: Remove provider domains that point to this origin but are no longer in RenderMesh `hosts`. Defaults to `false`.

RenderMesh reconciles exact hosts by default. Wildcard hosts are included only when `include_wildcards` is true.

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

Global fallback:

```yaml
hosts:
  "*":
    origin: my_app
```

Rules can be combined:

```yaml
hosts:
  admin.myapp.com:
    origin: admin
  "*.myapp.com":
    origin: tenant
  "*":
    origin: default_app
```

Resolution uses the following precedence:

```text
exact host -> most specific domain wildcard -> global * fallback
```

The global fallback matches only syntactically valid incoming hosts. The
resolved request preserves the normalized incoming host and identifies `*` as
the matched manifest rule. Missing or invalid hosts remain unresolved. When no
global fallback is configured, unknown hosts return `421 Misdirected Request`.

The global wildcard affects routing only. It does not allow arbitrary CORS
origins and is excluded from CDN domain reconciliation regardless of
`include_wildcards`.

## Local Lab Manifest

The local example uses [examples/local/rendermesh.yaml](../examples/local/rendermesh.yaml), which maps `test.com` and `*.test.com` to a MinIO-backed origin named `local_app`.
