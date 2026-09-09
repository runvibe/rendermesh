# CloudFront SaaS Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add opt-in CloudFront SaaS Manager tenant reconciliation and tenant-specific cache invalidation while preserving standard CloudFront behavior.

**Architecture:** Introduce a distinct `cloudfront_saas` manifest variant, a provider-neutral tenant reconciliation contract, and a dedicated AWS adapter. Existing origin activation and CDN refresh services remain policy owners; the SaaS adapter encapsulates CloudFront tenant APIs and fans one purge request out to matching tenants.

**Tech Stack:** Rust 2021, Tokio, async-trait, AWS SDK for Rust (`aws-sdk-cloudfront`), serde, anyhow, sha2, existing unit/integration test harness.

## Global Constraints

- Existing `provider: cloudfront` and `provider: cloudflare` manifests and behavior must remain compatible.
- Create one tenant per normalized exact host; never create tenants for `*.example.com` or `*`.
- Never delete extra distribution tenants in the first release.
- Tenant creation and reconciliation must be idempotent and use ETags for updates.
- CloudFront-managed certificates support only `validation_token_host: cloudfront`; reject `self_hosted`.
- CDN failures after activation must not roll back an activated origin generation.
- Preserve `request_id` and add `request_ids` for multi-tenant invalidation compatibility.
- Do not log resolved tenant parameter values.
- Keep routes free of business rules, services free of AWS SDK types, and AWS transport details in repositories.
- No source file may reach 1000 lines.

---

## File Structure

- `src/dto/manifest.rs`: deserialize the SaaS provider contract.
- `src/repositories/cdn.rs`: provider-neutral purge and tenant reconciliation interfaces.
- `src/repositories/cloudfront_saas_cdn.rs`: CloudFront SaaS API adapter, reconciliation, deterministic tenant names, and invalidation fan-out.
- `src/repositories/mod.rs`: export the SaaS repository module.
- `src/services/cdn_tenants.rs`: derive exact hosts and orchestrate tenant reconciliation.
- `src/services/cdn_refresh.rs`: build SaaS purge services and carry multiple request IDs.
- `src/services/origin_refresh.rs`: serialize compatible single and multiple request-ID fields.
- `src/services/origin_runtime.rs`: retain multi-request invalidation evidence.
- `src/services/startup/cdn.rs`: build and invoke SaaS tenant reconcilers.
- `src/services/startup/initial_sync.rs`: reconcile SaaS tenants after initial origin activation.
- `src/services/mod.rs`: export the tenant service.
- `README.md`: summarize provider capabilities and point to configuration.
- `docs/configuration.md`: document the complete manifest contract and environment variables.
- `docs/cdn-refresh.md`: document tenant invalidation and startup ordering.

### Task 1: Manifest contract and validation

**Files:**
- Modify: `src/dto/manifest.rs`
- Modify: `src/services/manifest.rs`

**Interfaces:**
- Produces: `CdnConfig::CloudFrontSaas(CloudFrontSaasCdnConfig)`.
- Produces: `CloudFrontSaasCdnConfig { distribution_id_env, connection_group_id_env, strategy, parameters_env, certificate }`.
- Produces: `CloudFrontSaasCertificateConfig::Managed { validation_token_host }`.
- Produces: `CloudFrontSaasValidationTokenHost::CloudFront`.

- [ ] **Step 1: Write failing parsing and validation tests**

Add tests under `src/services/manifest.rs` that parse:

```rust
cdn:
  provider: cloudfront_saas
  distribution_id_env: APP_CLOUDFRONT_DISTRIBUTION_ID
  connection_group_id_env: APP_CLOUDFRONT_CONNECTION_GROUP_ID
  strategy: changed_paths
  parameters_env:
    origin-domain: APP_CLOUDFRONT_ORIGIN_DOMAIN
  certificate:
    mode: managed
    validation_token_host: cloudfront
```

Assert the exact DTO values, the default `strategy`, default
`validation_token_host`, optional `certificate`, and rejection of:

```rust
assert!(parse_manifest_yaml(manifest_with_empty_distribution_env)
    .unwrap_err()
    .to_string()
    .contains("cdn.distribution_id_env is required"));
assert!(parse_manifest_yaml(manifest_with_empty_parameter_env)
    .unwrap_err()
    .to_string()
    .contains("cdn.parameters_env.origin-domain is required"));
```

Also use `serde_norway::from_str::<RenderMeshManifest>` to assert that
`validation_token_host: self_hosted` is rejected as an unknown enum value.

- [ ] **Step 2: Run the targeted tests and confirm the new provider is rejected**

Run:

```powershell
cargo test services::manifest::tests -- --nocapture
```

Expected: the SaaS tests fail because `cloudfront_saas` is not a `CdnConfig`
variant.

- [ ] **Step 3: Add the DTO types**

Add:

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudFrontSaasCdnConfig {
    pub distribution_id_env: String,
    pub connection_group_id_env: Option<String>,
    #[serde(default)]
    pub strategy: CdnRefreshStrategy,
    #[serde(default)]
    pub parameters_env: BTreeMap<String, String>,
    pub certificate: Option<CloudFrontSaasCertificateConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CloudFrontSaasCertificateConfig {
    Managed {
        #[serde(default)]
        validation_token_host: CloudFrontSaasValidationTokenHost,
    },
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudFrontSaasValidationTokenHost {
    #[default]
    CloudFront,
}
```

Add `CloudFrontSaas(CloudFrontSaasCdnConfig)` to `CdnConfig`, and return
`None` from `CdnConfig::domains()` for this variant.

- [ ] **Step 4: Validate all environment-variable names**

Extend `validate_cdn_config` so the SaaS branch rejects blank
`distribution_id_env`, blank `connection_group_id_env` when present, blank
parameter names, and blank environment-variable names. Use errors containing
the origin ID and exact manifest field.

- [ ] **Step 5: Run manifest tests**

Run:

```powershell
cargo test services::manifest::tests -- --nocapture
```

Expected: all manifest tests pass.

- [ ] **Step 6: Commit the contract**

```powershell
git add src/dto/manifest.rs src/services/manifest.rs
git commit -m "feat: add CloudFront SaaS manifest contract" -m "Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: Multi-request purge result compatibility

**Files:**
- Modify: `src/repositories/cdn.rs`
- Modify: `src/repositories/cloudfront_cdn.rs`
- Modify: `src/repositories/cloudflare_cdn.rs`
- Modify: `src/services/cdn_refresh.rs`
- Modify: `src/dto/origin_sync.rs`
- Modify: `src/services/origin_refresh.rs`
- Modify: `src/services/origin_runtime.rs`

**Interfaces:**
- Produces: `CdnPurgeResult { provider, request_ids: Vec<String>, status, submitted_items }`.
- Produces: `CdnRefreshOutcome::request_ids: Vec<String>`.
- Produces: `OriginSyncCdnResponse::request_ids: Vec<String>`.
- Preserves: `OriginSyncCdnResponse::request_id: Option<String>` as the first ID.
- Produces: `OriginSnapshotDebug::last_cdn_request_ids: Vec<String>`.

- [ ] **Step 1: Write failing compatibility tests**

Add a serialization test that expects:

```rust
let value = serde_json::to_value(OriginSyncCdnResponse {
    provider: "cloudfront_saas".to_string(),
    status: "submitted".to_string(),
    request_id: Some("INV-1".to_string()),
    request_ids: vec!["INV-1".to_string(), "INV-2".to_string()],
    submitted_items: 4,
})?;
assert_eq!(value["request_id"], "INV-1");
assert_eq!(value["request_ids"], json!(["INV-1", "INV-2"]));
```

Extend origin runtime tests to assert both request-ID fields survive a later
snapshot replacement.

- [ ] **Step 2: Run targeted tests and confirm missing fields fail**

Run:

```powershell
cargo test dto::origin_sync services::origin_runtime -- --nocapture
```

Expected: compilation fails because `request_ids` and
`last_cdn_request_ids` do not exist.

- [ ] **Step 3: Change the provider-neutral result**

Replace `CdnPurgeResult::request_id` with:

```rust
pub request_ids: Vec<String>,
```

Update standard CloudFront and Cloudflare adapters to return zero or one
element. Update `CdnRefreshOutcome` to carry the vector.

- [ ] **Step 4: Preserve the external compatibility fields**

Build responses with:

```rust
let request_id = outcome.request_ids.first().cloned();
let request_ids = outcome.request_ids.clone();
```

Store both in `OriginRuntimeStore::set_cdn_result`, and preserve
`last_cdn_request_ids` in `set_snapshot`.

- [ ] **Step 5: Run CDN and runtime tests**

Run:

```powershell
cargo test repositories::cloudfront_cdn repositories::cloudflare_cdn services::cdn_refresh services::origin_runtime -- --nocapture
```

Expected: all selected tests pass.

- [ ] **Step 6: Commit multi-request support**

```powershell
git add src/repositories/cdn.rs src/repositories/cloudfront_cdn.rs src/repositories/cloudflare_cdn.rs src/services/cdn_refresh.rs src/dto/origin_sync.rs src/services/origin_refresh.rs src/services/origin_runtime.rs
git commit -m "refactor: support multiple CDN request ids" -m "Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: Provider-neutral tenant orchestration

**Files:**
- Modify: `src/repositories/cdn.rs`
- Create: `src/services/cdn_tenants.rs`
- Modify: `src/services/mod.rs`

**Interfaces:**
- Produces:

```rust
pub struct CdnTenantConfig {
    pub distribution_id: String,
    pub connection_group_id: Option<String>,
    pub parameters: BTreeMap<String, String>,
    pub managed_certificate: Option<ManagedCertificateRequest>,
}

pub struct ManagedCertificateRequest {
    pub validation_token_host: String,
}

pub struct CdnTenantReconcileRequest {
    pub origin_id: String,
    pub desired_domains: BTreeSet<String>,
}

#[async_trait]
pub trait CdnTenantReconcile: Send + Sync {
    async fn reconcile_tenants(
        &self,
        request: CdnTenantReconcileRequest,
    ) -> Result<CdnDomainReconcileResult>;
}
```

- Produces: `exact_hosts_for_origin(manifest, origin_id) -> BTreeSet<String>`.
- Produces: `OriginCdnTenants::reconcile(manifest, origin_id)`.

- [ ] **Step 1: Write exact-host derivation tests**

Create tests proving:

```rust
assert_eq!(
    exact_hosts_for_origin(&manifest, "app"),
    BTreeSet::from(["app.example.com".to_string()])
);
```

Use a manifest containing uppercase exact input, `*.example.com`, `*`, and an
exact host assigned to another origin. Only the normalized exact host for
`app` may remain.

- [ ] **Step 2: Run the new service tests and confirm the module is missing**

Run:

```powershell
cargo test services::cdn_tenants -- --nocapture
```

Expected: compilation fails until the service is exported and implemented.

- [ ] **Step 3: Add the tenant repository contract**

Add the exact structs and trait from the Interfaces block to
`src/repositories/cdn.rs`. Keep tenant configuration on the repository rather
than adding SaaS-only fields to `CdnDomainReconcileRequest`.

- [ ] **Step 4: Implement exact-host derivation and orchestration**

Implement:

```rust
pub fn exact_hosts_for_origin(
    manifest: &RenderMeshManifest,
    origin_id: &str,
) -> BTreeSet<String> {
    manifest.hosts.iter()
        .filter(|(host, config)| config.origin == origin_id && !host.trim().starts_with('*'))
        .filter_map(|(host, _)| normalize_host(host))
        .collect()
}
```

`OriginCdnTenants` owns an `Arc<dyn CdnTenantReconcile>`, calls it with the
derived set, and converts `CdnDomainReconcileResult` into the existing
`CdnDomainReconcileOutcome`.

- [ ] **Step 5: Run tenant service tests**

Run:

```powershell
cargo test services::cdn_tenants -- --nocapture
```

Expected: all tenant derivation and orchestration tests pass.

- [ ] **Step 6: Commit tenant orchestration**

```powershell
git add src/repositories/cdn.rs src/services/cdn_tenants.rs src/services/mod.rs
git commit -m "feat: add CDN tenant reconciliation service" -m "Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>"
```

### Task 4: CloudFront SaaS repository

**Files:**
- Create: `src/repositories/cloudfront_saas_cdn.rs`
- Modify: `src/repositories/mod.rs`
- Modify: `src/repositories/cdn.rs`

**Interfaces:**
- Produces: `CloudFrontSaasCdnRepository::from_config(&CloudFrontSaasCdnConfig, BTreeSet<String>) -> Result<Self>`.
- Implements: `CdnTenantReconcile`.
- Implements: `CdnPurge`.
- Produces: `deterministic_tenant_name(distribution_id, host) -> String`.
- Consumes: `CdnTenantReconcileRequest`, `CdnPurgeRequest`, and
  `ensure_paths_mode`.

- [ ] **Step 1: Write repository policy tests against a fake client**

Define a private async `CloudFrontSaasClient` trait whose domain-level methods
return repository-owned `DistributionTenant` values rather than AWS SDK types.
Test:

```rust
assert_eq!(
    deterministic_tenant_name("DISTRIBUTION-1", "app.example.com"),
    deterministic_tenant_name("DISTRIBUTION-1", "app.example.com")
);
assert_ne!(
    deterministic_tenant_name("DISTRIBUTION-1", "app.example.com"),
    deterministic_tenant_name("DISTRIBUTION-2", "app.example.com")
);
assert!(deterministic_tenant_name("D", &"a".repeat(200)).len() <= 128);
```

Fake-client operation logs must prove:

- a missing domain creates one enabled tenant;
- a matching tenant is unchanged;
- a changed tenant updates with its ETag;
- a tenant on another distribution returns an error before mutation;
- configured parameters replace same-name values but preserve other values;
- extra tenants are never deleted;
- invalidation lists all pages, selects enabled exact-domain tenants, sorts by
  tenant ID, and submits once per tenant;
- no matching tenants returns `skipped_no_tenants`;
- caller references differ by tenant and path set, remain stable for identical
  calls through one repository, and differ between independently constructed
  repository namespaces.

- [ ] **Step 2: Run repository tests and confirm the module is missing**

Run:

```powershell
cargo test repositories::cloudfront_saas_cdn -- --nocapture
```

Expected: compilation fails because the module and repository do not exist.

- [ ] **Step 3: Implement domain models and deterministic naming**

Use SHA-256:

```rust
fn deterministic_tenant_name(distribution_id: &str, host: &str) -> String {
    let digest = Sha256::digest(format!("{distribution_id}\0{host}").as_bytes());
    let hash = format!("{digest:x}");
    let host_prefix = &host[..host.floor_char_boundary(100.min(host.len()))];
    format!("rendermesh-{host_prefix}-{}", &hash[..16])
}
```

Apply a final length cap before returning and ensure the final character is
alphanumeric. Keep AWS model conversion in the concrete AWS client.

- [ ] **Step 4: Implement tenant reconciliation**

For each desired domain in sorted order:

1. Call `get_distribution_tenant_by_domain`.
2. On not-found, call `create_distribution_tenant`.
3. On found, reject a different distribution ID.
4. Merge configured parameters over existing parameters.
5. Compare domain, connection group, enabled state, parameters, and managed
   certificate validation host.
6. Fetch the current ETag and call `update_distribution_tenant` only on drift.
   Include the managed-certificate request only when inspection reports a
   missing or mismatched certificate.
7. Accumulate `added`, `updated`, and `unchanged`; always return `removed: 0`.

- [ ] **Step 5: Implement tenant invalidation**

Convert modes with `ensure_paths_mode`. List every tenant page using the
distribution association filter, select enabled tenants matching the
repository's exact-host set, sort by tenant ID, and call
`create_invalidation_for_distribution_tenant`.

Generate a random namespace once per repository construction. Hash that
namespace with `origin_id`, `generation`, tenant ID, and the ordered paths, and
prefix the hexadecimal digest with `rendermesh-`. Repository clones retain the
same namespace, while a restarted process constructs a new namespace. Keep the
result within the AWS caller-reference limit.

Return `submitted_items = paths.len() * matching_tenant_count` and all
invalidation IDs.

- [ ] **Step 6: Implement the AWS SDK client**

Map repository operations to:

```rust
client.get_distribution_tenant_by_domain()
client.get_distribution_tenant()
client.get_managed_certificate_details()
client.list_distribution_tenants()
client.create_distribution_tenant()
client.update_distribution_tenant()
client.create_invalidation_for_distribution_tenant()
```

Use `DistributionTenantAssociationFilter` when listing, pass `if_match` on
updates, and classify only `EntityNotFound` as absence. Add operation and
tenant context with `anyhow::Context`.

- [ ] **Step 7: Run repository tests**

Run:

```powershell
cargo test repositories::cloudfront_saas_cdn -- --nocapture
```

Expected: all policy tests pass.

- [ ] **Step 8: Commit the repository**

```powershell
git add src/repositories/cloudfront_saas_cdn.rs src/repositories/mod.rs src/repositories/cdn.rs
git commit -m "feat: reconcile CloudFront SaaS tenants" -m "Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>"
```

### Task 5: Startup and refresh wiring

**Files:**
- Modify: `src/services/cdn_refresh.rs`
- Modify: `src/services/startup/cdn.rs`
- Modify: `src/services/startup/initial_sync.rs`
- Modify: `src/services/startup.rs`

**Interfaces:**
- Consumes: `CloudFrontSaasCdnRepository`, `OriginCdnTenants`,
  `CdnConfig::CloudFrontSaas`.
- Produces: `StartupCdn::tenants_by_origin: BTreeMap<String, OriginCdnTenants>`.
- Produces: SaaS `OriginCdnRefresh` using exact hosts and the configured
  strategy.

- [ ] **Step 1: Write failing startup tests**

Add a startup construction test with a SaaS origin and environment variables:

```rust
std::env::set_var("APP_CLOUDFRONT_DISTRIBUTION_ID", "D123");
std::env::set_var("APP_CLOUDFRONT_CONNECTION_GROUP_ID", "CG123");
std::env::set_var("APP_CLOUDFRONT_ORIGIN_DOMAIN", "origin.internal");
```

Assert one refresh adapter and one tenant reconciler are built. Add a
regression test that `exact_url_prefixes_for_origin` excludes both
`*.example.com` and `*`.

- [ ] **Step 2: Run startup tests and confirm missing variant branches**

Run:

```powershell
cargo test services::startup -- --nocapture
```

Expected: compilation or assertion failure because SaaS is not wired.

- [ ] **Step 3: Build one shared SaaS repository per origin**

In `build_startup_cdn`, resolve the exact-host set once. Construct one
`CloudFrontSaasCdnRepository` clone and place it behind both:

- the `CdnPurgeRepository::CloudFrontSaas` refresh path; and
- `OriginCdnTenants`.

Resolve `parameters_env` values with field-specific errors and never log their
values.

- [ ] **Step 4: Reconcile tenants after initial activation**

Extend `sync_origins_at_startup` with
`cdn_tenants_by_origin: &BTreeMap<String, OriginCdnTenants>`. After
`refresh_origin` returns, invoke tenant reconciliation and store its counts
through the existing CDN domain runtime fields.

Keep errors non-fatal:

```rust
if let Err(error) = cdn_tenants.reconcile(manifest, origin_id).await {
    origin_runtime.set_cdn_domain_error(origin_id, error.to_string());
    tracing::error!(origin = %origin_id, "cdn tenant reconciliation failed: {error}");
}
```

- [ ] **Step 5: Run startup and refresh tests**

Run:

```powershell
cargo test services::startup services::cdn_refresh services::origin_refresh -- --nocapture
```

Expected: all selected tests pass, including the initial
`skipped_no_tenants` behavior.

- [ ] **Step 6: Commit runtime wiring**

```powershell
git add src/services/cdn_refresh.rs src/services/startup/cdn.rs src/services/startup/initial_sync.rs src/services/startup.rs
git commit -m "feat: wire CloudFront SaaS at startup" -m "Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>"
```

### Task 6: User documentation

**Files:**
- Modify: `README.md`
- Modify: `docs/configuration.md`
- Modify: `docs/cdn-refresh.md`
- Modify: `docs/superpowers/specs/2026-09-09-cloudfront-saas-design.md`

**Interfaces:**
- Documents the final manifest fields and operational prerequisites implemented
  by Tasks 1-5.

- [ ] **Step 1: Add the provider summary**

In `README.md`, add `cloudfront_saas` beside current CDN providers and state
that it automatically reconciles one tenant per exact host and performs
tenant-specific invalidations.

- [ ] **Step 2: Add the full configuration example**

In `docs/configuration.md`, include the exact YAML from the design spec and a
field table covering required/optional status, defaults, and environment
variable indirection.

- [ ] **Step 3: Document runtime behavior**

In `docs/cdn-refresh.md`, document:

- initial activation before tenant creation;
- `skipped_no_tenants`;
- exact-host-only selection;
- one invalidation per tenant;
- `request_id` plus `request_ids`;
- no tenant deletion;
- required IAM actions; and
- external DNS and managed-certificate prerequisites.

- [ ] **Step 4: Align the design spec with implementation**

Update only concrete names or behavior that changed during compiler-driven AWS
SDK integration. Do not broaden scope.

- [ ] **Step 5: Validate documentation references**

Run:

```powershell
rg -n "cloudfront_saas|request_ids|CreateInvalidationForDistributionTenant" README.md docs src
git diff --check
```

Expected: the provider, response field, and API are described consistently and
there are no whitespace errors.

- [ ] **Step 6: Commit documentation**

```powershell
git add README.md docs/configuration.md docs/cdn-refresh.md docs/superpowers/specs/2026-09-09-cloudfront-saas-design.md
git commit -m "docs: document CloudFront SaaS operations" -m "Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>"
```

### Task 7: Full verification and review

**Files:**
- Test: all modified Rust modules and existing integration suites.

**Interfaces:**
- Consumes the complete feature.
- Produces a clean, reviewed, buildable branch.

- [ ] **Step 1: Format**

Run:

```powershell
cargo fmt --all -- --check
```

If it fails, run `cargo fmt --all`, inspect the formatting-only diff, and rerun
the check.

- [ ] **Step 2: Run the required build**

Run:

```powershell
cargo build
```

Expected: exit code 0.

- [ ] **Step 3: Run the full test suite**

Run:

```powershell
cargo test
```

Expected: every unit and integration test passes.

- [ ] **Step 4: Check repository constraints**

Run:

```powershell
git diff HEAD~6 --check
Get-ChildItem src -Recurse -Filter *.rs | ForEach-Object {
  $lines = (Get-Content $_.FullName).Count
  if ($lines -ge 1000) { Write-Error "$($_.FullName): $lines lines" }
}
```

Expected: no whitespace errors and no source file at or above 1000 lines.

- [ ] **Step 5: Request focused code review**

Review the complete branch diff against
`docs/superpowers/specs/2026-09-09-cloudfront-saas-design.md`. Address only
high-confidence correctness, security, or regression findings, then rerun the
smallest affected test plus `cargo build` and `cargo test`.

- [ ] **Step 6: Commit review fixes if needed**

```powershell
git add src README.md docs/configuration.md docs/cdn-refresh.md docs/superpowers/specs/2026-09-09-cloudfront-saas-design.md
git commit -m "fix: address CloudFront SaaS review findings" -m "Co-authored-by: Copilot App <223556219+Copilot@users.noreply.github.com>"
```

Skip this commit when review finds no issues.
