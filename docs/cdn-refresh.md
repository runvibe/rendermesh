# CDN Refresh

RenderMesh can refresh CDN caches after an origin generation is activated.

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

CDN fields include provider, status, request id, refresh timestamp, submitted item count, and last CDN error.
