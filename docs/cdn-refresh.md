# CDN Refresh And Domains

RenderMesh can refresh CDN caches after an origin generation is activated. It can also reconcile CDN-facing domains from the global `hosts` map during startup.

CDN refresh is part of the origin sync lifecycle, but it runs after the local mirror, edge config, freshness index, and template registry have already been activated. If the CDN call fails, RenderMesh keeps the activated generation and records the CDN error in the runtime snapshot.

## Supported Providers

- CloudFront: creates invalidations by path.
- Cloudflare: purges cache by URL or purges everything.

## Lifecycle

```text
origin listing
  -> freshness diff
  -> staged mirror update
  -> edge config parse
  -> HTML template compile
  -> generation activation
  -> CDN refresh
```

## Strategies

`changed_paths`:

- Uses added, modified, and removed files from the freshness diff.
- CloudFront receives paths such as `/index.html`.
- Cloudflare receives URLs such as `https://example.com/index.html`.

`all`:

- Runs only when the freshness diff contains at least one changed path.
- CloudFront invalidates `/*`.
- Cloudflare sends `purge_everything`.

## CloudFront

```yaml
origins:
  web:
    type: s3
    bucket: web-bucket
    endpoint_env: WEB_STORAGE_ENDPOINT
    region_env: WEB_STORAGE_REGION
    cdn:
      provider: cloudfront
      distribution_id_env: WEB_CLOUDFRONT_DISTRIBUTION_ID
      strategy: changed_paths
```

CloudFront credentials use the AWS SDK default credential chain.

## Cloudflare

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

When `url_prefixes` is omitted, RenderMesh derives URL prefixes from exact host mappings:

```yaml
hosts:
  docs.example.com:
    origin: docs
```

Wildcard hosts are not used for URL derivation. If an origin only uses wildcard hosts, configure `url_prefixes` explicitly for Cloudflare `changed_paths`.

## Runtime Snapshot

The origin debug endpoints include CDN fields:

```text
GET /_rendermesh/origins
GET /_rendermesh/origins/{origin_id}/snapshot
GET /_rendermesh/origins/{origin_id}/freshness
```

CDN fields include provider, status, request id, refresh timestamp, submitted item count, domain reconciliation counts, and last CDN errors.

## Domain Reconciliation

Domain reconciliation is opt-in through `cdn.domains.enabled`. It runs during startup after the initial origin generation is active.

RenderMesh computes desired domains from `hosts`:

- Exact hosts are included by default.
- Wildcard hosts are skipped by default.
- Wildcard hosts are included when `include_wildcards: true`.
- Extra provider-side domains are preserved by default.
- Extra provider-side domains are removed only when `remove_extra_domains: true`.

### CloudFront Domains

CloudFront domain reconciliation updates:

- distribution aliases/CNAMEs;
- viewer certificate using `certificate_arn_env`;
- the default origin domain using `origin_domain_env`.

```yaml
cdn:
  provider: cloudfront
  distribution_id_env: WEB_CLOUDFRONT_DISTRIBUTION_ID
  domains:
    enabled: true
    origin_domain_env: RENDERMESH_PUBLIC_ORIGIN
    certificate_arn_env: WEB_CLOUDFRONT_CERTIFICATE_ARN
```

CloudFront custom domains require a certificate that covers the aliases. ACM certificates for CloudFront must be in `us-east-1`.

### Cloudflare DNS Records

Cloudflare domain reconciliation currently supports `mode: dns_records`. RenderMesh creates or updates CNAME records in the configured zone.

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
```

`mode: custom_hostnames` is reserved for a future Cloudflare for SaaS implementation and is rejected by the current runtime.
