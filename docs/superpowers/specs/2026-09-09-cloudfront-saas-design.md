# CloudFront SaaS Manager support

## Status

Proposed design for native AWS CloudFront SaaS Manager support in RenderMesh.

## Context

RenderMesh currently supports standard CloudFront distributions through two
independent capabilities:

- cache invalidation with `CreateInvalidation`; and
- optional distribution domain reconciliation with
  `GetDistributionConfig` and `UpdateDistribution`.

CloudFront SaaS Manager has a different resource and API model. A
multi-tenant distribution is a shared template. Viewer traffic is attached to
distribution tenants through a connection group, and each tenant owns its
domains, parameters, certificate configuration, and cache invalidations.

The standard distribution adapter cannot safely infer or emulate these
semantics. SaaS support therefore needs an explicit provider contract and
dedicated repository adapters.

## Goals

- Add `provider: cloudfront_saas` without changing existing
  `provider: cloudfront` behavior.
- Automatically create and reconcile one distribution tenant for every exact
  manifest host associated with an origin.
- Submit tenant-specific invalidations after a new origin generation is
  activated.
- Preserve the existing activation guarantee: a CDN failure never rolls back
  an activated origin generation.
- Keep AWS transport details inside repositories and orchestration inside
  services.
- Expose multi-tenant invalidation results without losing individual AWS
  request IDs.
- Make reconciliation idempotent and safe to repeat.

## Non-goals

- Creating or updating the multi-tenant distribution template.
- Creating or updating connection groups.
- Managing Route 53 or other DNS providers.
- Automatically deleting distribution tenants.
- Grouping multiple host names into a single tenant.
- Creating tenants from `*.example.com` or the global `*` routing fallback.
- Supporting self-hosted CloudFront certificate validation tokens.
- Migrating a standard distribution to a multi-tenant distribution.

## Considered approaches

### 1. Extend the standard CloudFront adapter with a mode

This minimizes the number of types, but mixes incompatible API models in one
repository. Branches would be required for distribution updates, tenant
updates, invalidation requests, identifiers, and result handling. It also
makes future CloudFront changes more likely to regress the other mode.

### 2. Add a dedicated `cloudfront_saas` provider

This keeps the existing adapter unchanged and introduces focused tenant
reconciliation and invalidation repositories. Provider-neutral services retain
the activation and runtime-observability policies.

This is the selected approach because it follows the current layering,
preserves backward compatibility, and keeps each AWS adapter cohesive.

### 3. Require all tenants to be provisioned externally

RenderMesh would only discover tenants and invalidate them. This is simpler,
but it leaves host configuration split across systems and permits a manifest
host to become active without a matching tenant. It does not satisfy the
requested native SaaS feature.

## Manifest contract

The provider is configured per origin:

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

- `provider` must be `cloudfront_saas`.
- `distribution_id_env` is required and names the environment variable that
  contains the multi-tenant distribution ID.
- `connection_group_id_env` is optional and names the environment variable
  that contains the connection group ID. When omitted, CloudFront uses the
  distribution's default connection group.
- `strategy` keeps the existing values and default:
  - `changed_paths` submits paths derived from the activated generation diff.
  - `all` submits `/*` when the generation has changes.
- `parameters_env` is optional. Each key is a parameter name declared by the
  multi-tenant distribution and each value is the name of an environment
  variable containing the tenant parameter value.
- `certificate` is optional. When omitted, the tenant inherits the
  multi-tenant distribution certificate configuration.
- `certificate.mode: managed` requests a CloudFront-managed certificate.
- `certificate.validation_token_host` supports `cloudfront` in the first
  release and defaults to `cloudfront`.

`self-hosted` validation is rejected during manifest validation because
RenderMesh does not serve the CloudFront validation token endpoint.

The contract intentionally does not reuse the standard CloudFront `domains`
block. For SaaS, the manifest's exact `hosts` entries are authoritative tenant
domains rather than aliases on a distribution.

## Host-to-tenant mapping

Each normalized exact host maps to one tenant:

| Manifest host | SaaS action |
|---|---|
| `app.example.com` | Reconcile tenant `app.example.com` with domain `app.example.com` |
| `*.example.com` | No tenant is created |
| `*` | No tenant is created |

RenderMesh looks up existing tenants by domain, so it can safely adopt a
pre-provisioned tenant regardless of its name. A newly created tenant receives
a deterministic account-unique name composed from a readable, length-limited
host prefix and the first 16 hexadecimal characters of
`SHA-256(distribution_id + NUL + normalized_host)`. This keeps the name within
CloudFront's 128-character limit and avoids collisions when distribution
lifecycles reuse a host in the same AWS account.

Before creating or adopting an existing tenant, RenderMesh verifies that it
belongs to the configured multi-tenant distribution. A same-name tenant on
another distribution is an error and is never modified.

RenderMesh owns only the desired fields on adopted tenants:

- multi-tenant distribution ID;
- connection group ID when configured;
- the single exact-host domain;
- enabled state;
- parameter values configured through `parameters_env`; and
- certificate configuration when configured.

Unconfigured parameters are not invented. Extra tenants and tenants for hosts
removed from the manifest are preserved. This avoids destructive cleanup and
allows a safe rollback during the first release.

## Reconciliation flow

For each origin using `cloudfront_saas`:

1. Derive normalized exact hosts assigned to the origin.
2. Resolve all configured environment variables before making AWS calls.
3. Look up each tenant with `GetDistributionTenantByDomain`.
4. Create a missing tenant with
   `CreateDistributionTenant`.
5. For an existing tenant, verify its distribution ID, fetch its current ETag,
   and compare the desired managed fields. When a managed certificate is
   configured, inspect its validation host with
   `GetManagedCertificateDetails`.
6. If fields differ, update it with `UpdateDistributionTenant` and the ETag
   returned by `GetDistributionTenant`.
7. Leave an already matching tenant unchanged.
8. Record aggregate `added`, `updated`, and `unchanged` counts. `removed` is
   always zero in the first release.

Reconciliation is not transactional across tenants. It stops at the first
error, reports the affected host, and keeps already completed AWS operations.
The next startup safely retries the complete desired set.

As with current domain reconciliation, tenant reconciliation runs after the
initial origin activation. This prevents a newly created viewer endpoint from
targeting an origin generation that has not been activated.

## Invalidation flow

After every successful origin generation activation:

1. Build the invalidation path plan with the existing strategy.
2. List distribution tenants for the configured multi-tenant distribution.
3. Select enabled tenants whose domains equal the origin's normalized exact
   hosts.
4. Submit one `CreateInvalidationForDistributionTenant` request per selected
   tenant.
5. Use the existing deterministic caller reference per generation, extended
   with the tenant ID so retries remain idempotent.
6. Return all submitted AWS request IDs.

When no matching tenant exists, the refresh returns a successful
`skipped_no_tenants` outcome with zero submitted items. This is expected during
the first startup because origin activation precedes tenant creation, and no
tenant means there is no tenant cache or viewer traffic to invalidate.

Invalidation failures are reported after activation and do not roll back the
origin generation. A failure identifying one tenant stops further submissions
for that refresh. Already submitted invalidations remain valid and the next
origin refresh can retry with a new generation caller reference.

## Result contract and observability

A standard distribution produces one invalidation request ID while a SaaS
origin can produce many. The provider-neutral purge result will therefore use
`request_ids: Vec<String>`.

To preserve the existing HTTP response contract:

- `request_id` remains and contains the first request ID, or `null`;
- `request_ids` is added and contains every request ID in deterministic tenant
  order.

The origin runtime debug snapshot follows the same compatibility rule with
`last_cdn_request_id` and a new `last_cdn_request_ids` field.

Logs include:

- origin ID;
- provider;
- multi-tenant distribution ID;
- tenant name and ID;
- operation (`created`, `updated`, `unchanged`, or `invalidated`);
- submitted item count; and
- AWS request ID.

No resolved parameter value is logged because parameter values can contain
sensitive origin information.

## Architecture

### DTO

`src/dto/manifest.rs` adds:

- `CdnConfig::CloudFrontSaas`;
- `CloudFrontSaasCdnConfig`;
- `CloudFrontSaasCertificateConfig`; and
- an enum for managed certificate validation.

DTO validation rejects empty environment-variable names, unsupported
self-hosted validation, and configuration fields that do not apply to the
selected certificate mode.

### Repositories

`src/repositories/cloudfront_saas_cdn.rs` owns AWS SDK calls and AWS-to-domain
model conversion:

- tenant lookup, list, create, and optimistic-concurrency update;
- managed certificate inspection;
- tenant-specific invalidation; and
- CloudFront error classification needed to distinguish not-found from
  transport or authorization failures.

It implements provider-neutral purge and domain-reconciliation interfaces. The
standard `cloudfront_cdn.rs` adapter remains unchanged except for adopting the
new multi-request result shape.

### Services

`src/services/cdn_domains.rs` derives SaaS desired tenants from exact hosts
only and orchestrates reconciliation through the repository interface.

`src/services/cdn_refresh.rs` retains path planning and delegates a single
provider-neutral purge request. The SaaS repository fans that request out to
matching tenants.

`src/services/startup/cdn.rs` selects the correct adapters from the tagged
provider variant and resolves environment-backed configuration without
exposing AWS SDK types to services.

The dependency direction remains:

```text
startup/route -> dto -> service -> repository -> AWS SDK
```

## Error behavior

- Missing or empty required environment variables fail startup before AWS
  calls.
- Missing optional connection group configuration delegates selection to
  CloudFront.
- An exact host assigned to more than one origin remains a manifest error under
  existing host validation.
- A deterministic tenant-name collision fails reconciliation and does not
  modify the colliding tenant.
- ETag conflicts are surfaced. They are not overwritten with an unconditional
  update.
- AWS authorization, throttling, validation, and transport failures retain
  their operation and tenant context.
- Origin activation remains successful when a post-activation invalidation
  fails.
- Initial startup still fails when tenant reconciliation fails, matching
  current domain-reconciliation startup behavior.

## IAM permissions

The RenderMesh AWS identity needs the existing storage permissions plus:

```text
cloudfront:CreateDistributionTenant
cloudfront:GetDistributionTenant
cloudfront:GetDistributionTenantByDomain
cloudfront:GetManagedCertificateDetails
cloudfront:ListDistributionTenants
cloudfront:UpdateDistributionTenant
cloudfront:CreateInvalidationForDistributionTenant
```

No delete permission is required.

## DNS and certificate prerequisites

RenderMesh does not create DNS records. Before a CloudFront-managed certificate
can become active with `validation_token_host: cloudfront`, each exact host
must resolve to the configured connection group's routing endpoint according
to CloudFront SaaS Manager onboarding requirements.

When certificate configuration is omitted, operators are responsible for
ensuring the multi-tenant distribution's inherited certificate configuration
covers each tenant domain.

## Testing

### DTO tests

- Parse the complete `cloudfront_saas` contract.
- Apply strategy and managed-certificate defaults.
- Parse inherited certificate behavior when `certificate` is omitted.
- Reject empty environment-variable names.
- Reject self-hosted validation.
- Keep existing `cloudfront` manifests unchanged.

### Service tests

- Derive one desired tenant per normalized exact host.
- Exclude suffix wildcard hosts and global `*`.
- Keep tenants isolated by origin.
- Report aggregate create, update, and unchanged counts.
- Preserve extra tenants and report zero removals.
- Expose all invalidation request IDs while retaining the first compatibility
  ID.

### Repository tests

AWS SDK operations are exercised through a narrow client interface with a fake
implementation:

- create a missing tenant;
- leave a matching tenant unchanged;
- update a changed tenant with its ETag;
- reject a tenant owned by another distribution;
- preserve unconfigured parameters;
- list and invalidate matching enabled tenants in deterministic order;
- skip successfully when no tenants match;
- include tenant context in AWS errors; and
- generate distinct idempotent caller references per tenant and generation.

### Integration tests

- Startup builds both standard and SaaS providers.
- Initial activation can skip invalidation before tenant creation and then
  reconcile the tenant.
- A later refresh submits tenant-specific invalidation.
- Tenant reconciliation failure follows existing startup failure behavior.
- Invalidation failure after activation is observable and does not roll back
  the origin generation.

## Documentation

`README.md`, `docs/configuration.md`, and `docs/cdn-refresh.md` will document:

- the new manifest contract;
- exact-host-only tenant creation;
- DNS and certificate prerequisites;
- required IAM actions;
- startup reconciliation semantics;
- multi-request invalidation results; and
- the explicit absence of automatic tenant deletion.

## Rollout

The feature is opt-in through `provider: cloudfront_saas`. Existing
`provider: cloudfront` configurations and behavior are unchanged. Operators
should first grant the new IAM actions, create the multi-tenant distribution
and connection group, configure DNS, and then change the origin provider.
