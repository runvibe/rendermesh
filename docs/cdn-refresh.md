# CDN Refresh And Domains

RenderMesh can refresh CDN caches after an origin generation is activated. It can also reconcile CDN-facing domains from the global `hosts` map during startup.

CDN refresh is part of the origin sync lifecycle, but it runs after the local mirror, edge config, freshness index, and template registry have already been activated. If the CDN call fails, RenderMesh keeps the activated generation and records the CDN error in the runtime snapshot.

For `cloudfront_saas`, tenant reconciliation is a separate startup-time step that also runs after the initial origin generation is active. This preserves the same activation guarantee for first boot and means the first post-activation purge can legitimately return `skipped_no_tenants` before tenants exist.

## Supported Providers

- CloudFront: creates invalidations by path.
- CloudFront SaaS (`cloudfront_saas`): reconciles exact-host tenants during startup and invalidates each matching tenant separately.
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

## CloudFront SaaS

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

### Startup ordering

For origins using `cloudfront_saas`, startup order is:

```text
initial origin activation
  -> CloudFront SaaS tenant reconciliation
  -> later syncs can invalidate matching tenants
```

RenderMesh never creates tenants before the first origin generation is active. If the first activation triggers a purge before reconciliation has created or adopted tenants, the purge succeeds with `status: skipped_no_tenants`.

### Exact-host tenant reconciliation

RenderMesh derives desired CloudFront SaaS tenants from normalized exact `hosts` entries only:

- `app.example.com` creates or reconciles one tenant for `app.example.com`.
- `*.example.com` does not create a tenant.
- `*` does not create a tenant.

When RenderMesh finds an existing tenant by domain, it only adopts that tenant when all of the following are true:

- the tenant belongs to the configured multi-tenant distribution;
- the tenant owns only that single exact host; and
- any configured managed-certificate validation settings match.

Tenant reconciliation owns the `enabled` state. A matching tenant that is
explicitly disabled is updated and re-enabled.

RenderMesh preserves provider-side state that it does not manage, including existing AWS customizations. It does not delete extra tenants, and it does not delete tenants for hosts removed from the manifest. `removed` therefore remains `0` for this provider.

### Invalidation behavior

After each successful origin activation, RenderMesh:

1. Builds paths from the configured `strategy`.
2. Lists tenants for the configured multi-tenant distribution.
3. Filters to matching exact-host tenants that are not explicitly disabled.
4. Submits one `CreateInvalidationForDistributionTenant` request per matching tenant.

Each tenant invalidation reuses the same activated path set, but has its own caller reference and its own invalidation id. A single refresh therefore returns:

- `request_id`: the first invalidation id, for compatibility with existing clients; and
- `request_ids`: the complete ordered invalidation id list for every submitted tenant request.

The runtime snapshot follows the same pattern with `last_cdn_request_id` and `last_cdn_request_ids`.
If a later tenant invalidation fails, the sync response omits `cdn`, as it does
for other post-activation CDN failures. The runtime snapshot uses
`partial_failure`, retains the request ids and submitted item count from
tenants completed before the failure, and records the failure in
`last_cdn_error`.

### Managed certificates, DNS, and IAM

CloudFront SaaS support does **not** create the multi-tenant distribution, the connection group, DNS records, or ACM resources. Operators must provision the CloudFront SaaS environment first.

- `certificate.mode: managed` is the only supported certificate mode.
- `validation_token_host` currently supports only `cloudfront`.
- When `certificate` is omitted, RenderMesh leaves tenant certificate behavior inherited from the multi-tenant distribution.
- Before a CloudFront-managed certificate can become active, each exact host must already resolve to the configured connection group's routing endpoint according to CloudFront SaaS Manager requirements.

CloudFront SaaS uses the AWS SDK default credential chain and requires these CloudFront IAM actions:

```text
cloudfront:CreateDistributionTenant
cloudfront:GetDistributionTenant
cloudfront:GetDistributionTenantByDomain
cloudfront:GetManagedCertificateDetails
cloudfront:ListDistributionTenants
cloudfront:UpdateDistributionTenant
cloudfront:CreateInvalidationForDistributionTenant
```

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

CDN fields include provider, status, request id, request ids, refresh timestamp, submitted item count, domain reconciliation counts, and last CDN errors.

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
