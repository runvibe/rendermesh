use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use super::*;
use crate::repositories::cdn::{
    CdnPurge, CdnPurgeMode, CdnPurgeRequest, CdnTenantConfig, CdnTenantReconcile,
    CdnTenantReconcileRequest, ManagedCertificateRequest,
};

#[test]
fn tenant_names_are_stable_distribution_specific_and_bounded() {
    assert_eq!(
        deterministic_tenant_name("DISTRIBUTION-1", "app.example.com"),
        deterministic_tenant_name("DISTRIBUTION-1", "app.example.com")
    );
    assert_ne!(
        deterministic_tenant_name("DISTRIBUTION-1", "app.example.com"),
        deterministic_tenant_name("DISTRIBUTION-2", "app.example.com")
    );

    for name in [
        deterministic_tenant_name("D", &"a".repeat(200)),
        deterministic_tenant_name("D", &"é /_".repeat(80)),
    ] {
        assert!(name.len() <= 128);
        assert!(name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.')));
        assert!(name
            .chars()
            .next()
            .expect("tenant name is non-empty")
            .is_ascii_alphanumeric());
        assert!(name
            .chars()
            .last()
            .expect("tenant name is non-empty")
            .is_ascii_alphanumeric());
    }
}

#[tokio::test]
async fn missing_domain_creates_one_enabled_tenant() {
    let client = FakeClient::default();
    client.lookup("app.example.com", None);
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    let result = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    assert_eq!((result.added, result.updated, result.unchanged), (1, 0, 0));
    assert_eq!(result.removed, 0);
    let operations = client.operations();
    assert_eq!(operations.len(), 2);
    assert_eq!(
        operations[0],
        Operation::Lookup("app.example.com".to_string())
    );
    let Operation::Create(created) = &operations[1] else {
        panic!("expected create operation, got {:?}", operations[1]);
    };
    assert_eq!(created.distribution_id, "DIST");
    assert_eq!(
        created.name,
        deterministic_tenant_name("DIST", "app.example.com")
    );
    assert_eq!(
        created.domains,
        BTreeSet::from(["app.example.com".to_string()])
    );
    assert_eq!(created.connection_group_id.as_deref(), Some("GROUP"));
    assert!(created.enabled);
}

#[tokio::test]
async fn matching_tenant_is_unchanged_without_fetching_an_etag() {
    let client = FakeClient::default();
    client.lookup(
        "app.example.com",
        Some(tenant("tenant-1", "app.example.com")),
    );
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    let result = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    assert_eq!((result.added, result.updated, result.unchanged), (0, 0, 1));
    assert_eq!(
        client.operations(),
        vec![Operation::Lookup("app.example.com".to_string())]
    );
}

#[tokio::test]
async fn unknown_enabled_state_does_not_cause_reconciliation_drift() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-1", "app.example.com");
    existing.enabled = None;
    client.lookup("app.example.com", Some(existing));
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    let result = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    assert_eq!((result.added, result.updated, result.unchanged), (0, 0, 1));
    assert_eq!(
        client.operations(),
        vec![Operation::Lookup("app.example.com".to_string())]
    );
}

#[tokio::test]
async fn unknown_enabled_state_is_preserved_when_other_drift_is_updated() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-1", "app.example.com");
    existing.connection_group_id = None;
    existing.enabled = None;
    client.lookup("app.example.com", Some(existing.clone()));
    client.current(existing, "etag-current");
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    let updated = client
        .operations()
        .into_iter()
        .find_map(|operation| match operation {
            Operation::Update(request) => Some(request),
            _ => None,
        })
        .expect("update operation");
    assert_eq!(updated.tenant.enabled, None);
}

#[tokio::test]
async fn changed_tenant_is_updated_with_its_current_etag() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-1", "app.example.com");
    existing.connection_group_id = None;
    existing.enabled = Some(false);
    client.lookup("app.example.com", Some(existing.clone()));
    client.current(existing, "etag-current");
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    let result = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    assert_eq!((result.added, result.updated, result.unchanged), (0, 1, 0));
    let operations = client.operations();
    assert_eq!(
        operations[..2],
        [
            Operation::Lookup("app.example.com".to_string()),
            Operation::Get("tenant-1".to_string())
        ]
    );
    let Operation::Update(updated) = &operations[2] else {
        panic!("expected update operation, got {:?}", operations[2]);
    };
    assert_eq!(updated.id, "tenant-1");
    assert_eq!(updated.if_match, "etag-current");
    assert_eq!(
        updated.tenant.domains,
        BTreeSet::from(["app.example.com".to_string()])
    );
    assert_eq!(updated.tenant.connection_group_id.as_deref(), Some("GROUP"));
    assert_eq!(updated.tenant.enabled, Some(true));
}

#[tokio::test]
async fn tenant_on_another_distribution_errors_before_mutation() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-1", "app.example.com");
    existing.distribution_id = "OTHER".to_string();
    client.lookup("app.example.com", Some(existing));
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    let error = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect_err("other distribution must fail");

    assert!(error.to_string().contains("app.example.com"));
    assert!(error.to_string().contains("OTHER"));
    assert_eq!(
        client.operations(),
        vec![Operation::Lookup("app.example.com".to_string())]
    );
}

#[tokio::test]
async fn configured_parameters_replace_matching_names_and_preserve_others() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-1", "app.example.com");
    existing.parameters = BTreeMap::from([
        ("keep".to_string(), "existing".to_string()),
        ("replace".to_string(), "old".to_string()),
    ]);
    client.lookup("app.example.com", Some(existing.clone()));
    client.current(existing, "etag-parameters");
    let mut desired = config();
    desired.parameters = BTreeMap::from([("replace".to_string(), "new".to_string())]);
    let repository = repository(client.clone(), desired, ["app.example.com"]);

    repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    let updated = client
        .operations()
        .into_iter()
        .find_map(|operation| match operation {
            Operation::Update(request) => Some(request),
            _ => None,
        })
        .expect("update operation");
    assert_eq!(
        updated.tenant.parameters,
        BTreeMap::from([
            ("keep".to_string(), "existing".to_string()),
            ("replace".to_string(), "new".to_string()),
        ])
    );
}

#[tokio::test]
async fn managed_certificate_validation_host_drift_triggers_update() {
    let client = FakeClient::default();
    let existing = tenant("tenant-1", "app.example.com");
    client.lookup("app.example.com", Some(existing.clone()));
    client.certificate(
        "tenant-1",
        ManagedCertificateLookup::Found {
            validation_token_host: Some("self-hosted".to_string()),
        },
    );
    client.current(existing, "etag-certificate");
    let mut desired = config();
    desired.managed_certificate = Some(ManagedCertificateRequest {
        validation_token_host: "cloudfront".to_string(),
    });
    let repository = repository(client.clone(), desired, ["app.example.com"]);

    repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    let operations = client.operations();
    assert_eq!(
        operations[..3],
        [
            Operation::Lookup("app.example.com".to_string()),
            Operation::GetCertificate("tenant-1".to_string()),
            Operation::Get("tenant-1".to_string()),
        ]
    );
    let Operation::Update(updated) = &operations[3] else {
        panic!("expected update operation, got {:?}", operations[3]);
    };
    assert_eq!(
        updated
            .managed_certificate
            .as_ref()
            .map(|request| request.validation_token_host.as_str()),
        Some("cloudfront")
    );
}

#[tokio::test]
async fn managed_certificate_without_initial_validation_host_is_unchanged() {
    let client = FakeClient::default();
    let existing = tenant("tenant-1", "app.example.com");
    client.lookup("app.example.com", Some(existing));
    client.certificate(
        "tenant-1",
        ManagedCertificateLookup::Found {
            validation_token_host: None,
        },
    );
    let mut desired = config();
    desired.managed_certificate = Some(ManagedCertificateRequest {
        validation_token_host: "cloudfront".to_string(),
    });
    let repository = repository(client.clone(), desired, ["app.example.com"]);

    let result = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    assert_eq!((result.added, result.updated, result.unchanged), (0, 0, 1));
    assert_eq!(
        client.operations(),
        vec![
            Operation::Lookup("app.example.com".to_string()),
            Operation::GetCertificate("tenant-1".to_string()),
        ]
    );
}

#[tokio::test]
async fn missing_managed_certificate_triggers_update() {
    let client = FakeClient::default();
    let existing = tenant("tenant-1", "app.example.com");
    client.lookup("app.example.com", Some(existing.clone()));
    client.certificate("tenant-1", ManagedCertificateLookup::NotFound);
    client.current(existing, "etag-certificate");
    let mut desired = config();
    desired.managed_certificate = Some(ManagedCertificateRequest {
        validation_token_host: "cloudfront".to_string(),
    });
    let repository = repository(client.clone(), desired, ["app.example.com"]);

    let result = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    assert_eq!((result.added, result.updated, result.unchanged), (0, 1, 0));
    assert!(client
        .operations()
        .iter()
        .any(|operation| matches!(operation, Operation::Update(_))));
}

#[tokio::test]
async fn adopted_tenant_customizations_are_preserved_on_update() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-1", "app.example.com");
    existing.connection_group_id = None;
    existing.customizations = Some(TenantCustomizations {
        certificate_arn: Some("arn:aws:acm:us-east-1:123:certificate/adopted".to_string()),
        geo_restrictions: Some(TenantGeoRestrictionCustomization {
            restriction_type: "whitelist".to_string(),
            locations: Some(vec!["BR".to_string(), "US".to_string()]),
        }),
        web_acl: Some(TenantWebAclCustomization {
            action: "override".to_string(),
            arn: Some("arn:aws:wafv2:us-east-1:123:global/webacl/adopted".to_string()),
        }),
    });
    client.lookup("app.example.com", Some(existing.clone()));
    client.current(existing.clone(), "etag-customizations");
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    let updated = client
        .operations()
        .into_iter()
        .find_map(|operation| match operation {
            Operation::Update(request) => Some(request),
            _ => None,
        })
        .expect("update operation");
    assert_eq!(updated.tenant.customizations, existing.customizations);
}

#[tokio::test]
async fn adopted_tenant_with_a_sibling_domain_is_rejected_without_mutation() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-1", "app.example.com");
    existing.domains.insert("sibling.example.com".to_string());
    client.lookup("app.example.com", Some(existing));
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    let error = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect_err("multi-domain tenant must not be adopted");

    assert!(error.to_string().contains("app.example.com"));
    assert_eq!(
        client.operations(),
        vec![Operation::Lookup("app.example.com".to_string())]
    );
}

#[tokio::test]
async fn extra_tenants_are_never_removed() {
    let client = FakeClient::default();
    client.lookup(
        "app.example.com",
        Some(tenant("tenant-1", "app.example.com")),
    );
    client.lookup(
        "extra.example.com",
        Some(tenant("tenant-2", "extra.example.com")),
    );
    let repository = repository(
        client.clone(),
        config(),
        ["app.example.com", "extra.example.com"],
    );

    let result = repository
        .reconcile_tenants(reconcile_request(["app.example.com"]))
        .await
        .expect("tenant reconciles");

    assert_eq!(result.removed, 0);
    assert_eq!(
        client.operations(),
        vec![Operation::Lookup("app.example.com".to_string())]
    );
}

#[tokio::test]
async fn invalidation_paginates_filters_sorts_and_submits_per_tenant() {
    let client = FakeClient::default();
    client.page(
        None,
        DistributionTenantPage {
            tenants: vec![
                tenant("tenant-z", "b.example.com"),
                disabled_tenant("tenant-disabled", "a.example.com"),
                tenant("tenant-other", "other.example.com"),
            ],
            next_marker: Some("page-2".to_string()),
        },
    );
    client.page(
        Some("page-2"),
        DistributionTenantPage {
            tenants: vec![tenant("tenant-a", "a.example.com")],
            next_marker: None,
        },
    );
    client.invalidation_id("tenant-a", "request-a");
    client.invalidation_id("tenant-z", "request-z");
    let repository = repository(client.clone(), config(), ["a.example.com", "b.example.com"]);

    let result = repository
        .purge(CdnPurgeRequest {
            origin_id: "web".to_string(),
            generation: 7,
            mode: CdnPurgeMode::Paths(vec!["/a.css".to_string(), "/b.js".to_string()]),
        })
        .await
        .expect("purge succeeds");

    assert_eq!(result.provider, "cloudfront_saas");
    assert_eq!(result.status, "submitted");
    assert_eq!(result.request_ids, ["request-a", "request-z"]);
    assert_eq!(result.submitted_items, 4);
    assert_eq!(
        client.operations(),
        vec![
            Operation::List {
                distribution_id: "DIST".to_string(),
                marker: None,
            },
            Operation::List {
                distribution_id: "DIST".to_string(),
                marker: Some("page-2".to_string()),
            },
            Operation::Invalidate(TenantInvalidationRequest {
                tenant_id: "tenant-a".to_string(),
                caller_reference: "rendermesh-web-7-tenant-a".to_string(),
                paths: vec!["/a.css".to_string(), "/b.js".to_string()],
            }),
            Operation::Invalidate(TenantInvalidationRequest {
                tenant_id: "tenant-z".to_string(),
                caller_reference: "rendermesh-web-7-tenant-z".to_string(),
                paths: vec!["/a.css".to_string(), "/b.js".to_string()],
            }),
        ]
    );
}

#[tokio::test]
async fn invalidation_includes_tenants_with_unknown_enabled_state() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-unknown", "app.example.com");
    existing.enabled = None;
    client.page(
        None,
        DistributionTenantPage {
            tenants: vec![existing],
            next_marker: None,
        },
    );
    client.invalidation_id("tenant-unknown", "request-unknown");
    let repository = repository(client, config(), ["app.example.com"]);

    let result = repository
        .purge(CdnPurgeRequest {
            origin_id: "web".to_string(),
            generation: 7,
            mode: CdnPurgeMode::All,
        })
        .await
        .expect("purge succeeds");

    assert_eq!(result.request_ids, ["request-unknown"]);
    assert_eq!(result.submitted_items, 1);
}

#[test]
fn customizations_round_trip_through_aws_models() {
    use aws_sdk_cloudfront::types::{
        Certificate, CustomizationActionType, Customizations, GeoRestrictionCustomization,
        GeoRestrictionType, WebAclCustomization,
    };

    let aws = Customizations::builder()
        .certificate(
            Certificate::builder()
                .arn("arn:aws:acm:us-east-1:123:certificate/adopted")
                .build()
                .expect("certificate builds"),
        )
        .geo_restrictions(
            GeoRestrictionCustomization::builder()
                .restriction_type(GeoRestrictionType::Whitelist)
                .set_locations(Some(vec!["BR".to_string(), "US".to_string()]))
                .build()
                .expect("geo restrictions build"),
        )
        .web_acl(
            WebAclCustomization::builder()
                .action(CustomizationActionType::Override)
                .arn("arn:aws:wafv2:us-east-1:123:global/webacl/adopted")
                .build()
                .expect("web ACL builds"),
        )
        .build();

    let owned = tenant_customizations_from_aws(&aws);
    let round_tripped = aws_customizations(&owned).expect("customizations build");

    assert_eq!(
        round_tripped
            .certificate()
            .map(|certificate| certificate.arn()),
        Some("arn:aws:acm:us-east-1:123:certificate/adopted")
    );
    let geo = round_tripped
        .geo_restrictions()
        .expect("geo restrictions preserved");
    assert_eq!(geo.restriction_type().as_str(), "whitelist");
    assert_eq!(
        geo.locations.as_deref(),
        Some(["BR".to_string(), "US".to_string()].as_slice())
    );
    let web_acl = round_tripped.web_acl().expect("web ACL preserved");
    assert_eq!(web_acl.action().as_str(), "override");
    assert_eq!(
        web_acl.arn(),
        Some("arn:aws:wafv2:us-east-1:123:global/webacl/adopted")
    );
}

#[test]
fn non_unicode_environment_errors_do_not_expose_values() {
    use std::{env::VarError, ffi::OsString};

    let error = environment_variable_error(
        "APP_CLOUDFRONT_PARAMETER",
        VarError::NotUnicode(OsString::from("sensitive-value")),
    );
    let rendered = format!("{error:#}");

    assert_eq!(
        rendered,
        "environment variable APP_CLOUDFRONT_PARAMETER: NotUnicode"
    );
    assert!(!rendered.contains("sensitive-value"));
}

#[tokio::test]
async fn no_matching_tenants_skips_invalidation() {
    let client = FakeClient::default();
    client.page(
        None,
        DistributionTenantPage {
            tenants: vec![tenant("tenant-other", "other.example.com")],
            next_marker: None,
        },
    );
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    let result = repository
        .purge(CdnPurgeRequest {
            origin_id: "web".to_string(),
            generation: 7,
            mode: CdnPurgeMode::All,
        })
        .await
        .expect("purge succeeds");

    assert_eq!(result.status, "skipped_no_tenants");
    assert!(result.request_ids.is_empty());
    assert_eq!(result.submitted_items, 0);
    assert_eq!(
        client.operations(),
        vec![Operation::List {
            distribution_id: "DIST".to_string(),
            marker: None,
        }]
    );
}

#[tokio::test]
async fn caller_references_are_tenant_specific_and_retry_stable() {
    let client = FakeClient::default();
    client.page(
        None,
        DistributionTenantPage {
            tenants: vec![
                tenant("tenant-b", "b.example.com"),
                tenant("tenant-a", "a.example.com"),
            ],
            next_marker: None,
        },
    );
    let repository = repository(client.clone(), config(), ["a.example.com", "b.example.com"]);
    let request = CdnPurgeRequest {
        origin_id: "web".to_string(),
        generation: 9,
        mode: CdnPurgeMode::Paths(vec!["/index.html".to_string()]),
    };

    repository
        .purge(request.clone())
        .await
        .expect("first purge succeeds");
    repository
        .purge(request)
        .await
        .expect("retry purge succeeds");

    let references = client
        .operations()
        .into_iter()
        .filter_map(|operation| match operation {
            Operation::Invalidate(request) => Some(request.caller_reference),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        references,
        [
            "rendermesh-web-9-tenant-a",
            "rendermesh-web-9-tenant-b",
            "rendermesh-web-9-tenant-a",
            "rendermesh-web-9-tenant-b",
        ]
    );
    assert_ne!(references[0], references[1]);
    assert_eq!(references[0], references[2]);
    assert_eq!(references[1], references[3]);
}

fn config() -> CdnTenantConfig {
    CdnTenantConfig {
        distribution_id: "DIST".to_string(),
        connection_group_id: Some("GROUP".to_string()),
        parameters: BTreeMap::new(),
        managed_certificate: None,
    }
}

fn reconcile_request<const N: usize>(domains: [&str; N]) -> CdnTenantReconcileRequest {
    CdnTenantReconcileRequest {
        origin_id: "web".to_string(),
        desired_domains: domains.into_iter().map(ToString::to_string).collect(),
    }
}

fn repository<const N: usize>(
    client: FakeClient,
    config: CdnTenantConfig,
    exact_hosts: [&str; N],
) -> CloudFrontSaasCdnRepository {
    CloudFrontSaasCdnRepository::with_client(
        Arc::new(client),
        config,
        exact_hosts.into_iter().map(ToString::to_string).collect(),
    )
}

fn tenant(id: &str, domain: &str) -> DistributionTenant {
    DistributionTenant {
        id: id.to_string(),
        name: format!("name-{id}"),
        distribution_id: "DIST".to_string(),
        domains: BTreeSet::from([domain.to_string()]),
        connection_group_id: Some("GROUP".to_string()),
        parameters: BTreeMap::new(),
        customizations: None,
        enabled: Some(true),
    }
}

fn disabled_tenant(id: &str, domain: &str) -> DistributionTenant {
    let mut tenant = tenant(id, domain);
    tenant.enabled = Some(false);
    tenant
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Operation {
    Lookup(String),
    Get(String),
    GetCertificate(String),
    List {
        distribution_id: String,
        marker: Option<String>,
    },
    Create(CreateDistributionTenantRequest),
    Update(UpdateDistributionTenantRequest),
    Invalidate(TenantInvalidationRequest),
}

#[derive(Clone, Default)]
struct FakeClient {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    lookups: BTreeMap<String, Option<DistributionTenant>>,
    current: BTreeMap<String, VersionedDistributionTenant>,
    certificates: BTreeMap<String, ManagedCertificateLookup>,
    pages: BTreeMap<Option<String>, DistributionTenantPage>,
    invalidation_ids: BTreeMap<String, String>,
    operations: Vec<Operation>,
}

impl FakeClient {
    fn lookup(&self, domain: &str, tenant: Option<DistributionTenant>) {
        self.state
            .lock()
            .expect("fake lock")
            .lookups
            .insert(domain.to_string(), tenant);
    }

    fn current(&self, tenant: DistributionTenant, etag: &str) {
        self.state.lock().expect("fake lock").current.insert(
            tenant.id.clone(),
            VersionedDistributionTenant {
                tenant,
                etag: etag.to_string(),
            },
        );
    }

    fn certificate(&self, tenant_id: &str, certificate: ManagedCertificateLookup) {
        self.state
            .lock()
            .expect("fake lock")
            .certificates
            .insert(tenant_id.to_string(), certificate);
    }

    fn page(&self, marker: Option<&str>, page: DistributionTenantPage) {
        self.state
            .lock()
            .expect("fake lock")
            .pages
            .insert(marker.map(ToString::to_string), page);
    }

    fn invalidation_id(&self, tenant_id: &str, invalidation_id: &str) {
        self.state
            .lock()
            .expect("fake lock")
            .invalidation_ids
            .insert(tenant_id.to_string(), invalidation_id.to_string());
    }

    fn operations(&self) -> Vec<Operation> {
        self.state.lock().expect("fake lock").operations.clone()
    }
}

#[async_trait]
impl CloudFrontSaasClient for FakeClient {
    async fn get_distribution_tenant_by_domain(
        &self,
        domain: &str,
    ) -> Result<CloudFrontResponse<Option<DistributionTenant>>> {
        let mut state = self.state.lock().expect("fake lock");
        state.operations.push(Operation::Lookup(domain.to_string()));
        Ok(CloudFrontResponse {
            value: state.lookups.get(domain).cloned().unwrap_or(None),
            aws_request_id: Some(format!("lookup-request-{domain}")),
        })
    }

    async fn get_distribution_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<CloudFrontResponse<VersionedDistributionTenant>> {
        let mut state = self.state.lock().expect("fake lock");
        state.operations.push(Operation::Get(tenant_id.to_string()));
        let value = state
            .current
            .get(tenant_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing current tenant {tenant_id}"))?;
        Ok(CloudFrontResponse {
            value,
            aws_request_id: Some(format!("get-request-{tenant_id}")),
        })
    }

    async fn get_managed_certificate_validation_token_host(
        &self,
        tenant_id: &str,
    ) -> Result<CloudFrontResponse<ManagedCertificateLookup>> {
        let mut state = self.state.lock().expect("fake lock");
        state
            .operations
            .push(Operation::GetCertificate(tenant_id.to_string()));
        Ok(CloudFrontResponse {
            value: state
                .certificates
                .get(tenant_id)
                .cloned()
                .unwrap_or(ManagedCertificateLookup::NotFound),
            aws_request_id: Some(format!("certificate-request-{tenant_id}")),
        })
    }

    async fn list_distribution_tenants(
        &self,
        distribution_id: &str,
        marker: Option<&str>,
    ) -> Result<DistributionTenantPage> {
        let mut state = self.state.lock().expect("fake lock");
        let marker = marker.map(ToString::to_string);
        state.operations.push(Operation::List {
            distribution_id: distribution_id.to_string(),
            marker: marker.clone(),
        });
        Ok(state.pages.get(&marker).cloned().unwrap_or_default())
    }

    async fn create_distribution_tenant(
        &self,
        request: CreateDistributionTenantRequest,
    ) -> Result<CloudFrontResponse<DistributionTenant>> {
        self.state
            .lock()
            .expect("fake lock")
            .operations
            .push(Operation::Create(request.clone()));
        Ok(CloudFrontResponse {
            value: DistributionTenant {
                id: "created-tenant".to_string(),
                name: request.name,
                distribution_id: request.distribution_id,
                domains: request.domains,
                connection_group_id: request.connection_group_id,
                parameters: request.parameters,
                customizations: None,
                enabled: Some(request.enabled),
            },
            aws_request_id: Some("create-request".to_string()),
        })
    }

    async fn update_distribution_tenant(
        &self,
        request: UpdateDistributionTenantRequest,
    ) -> Result<CloudFrontResponse<DistributionTenant>> {
        self.state
            .lock()
            .expect("fake lock")
            .operations
            .push(Operation::Update(request.clone()));
        Ok(CloudFrontResponse {
            value: request.tenant,
            aws_request_id: Some("update-request".to_string()),
        })
    }

    async fn create_invalidation_for_distribution_tenant(
        &self,
        request: TenantInvalidationRequest,
    ) -> Result<CloudFrontResponse<String>> {
        let mut state = self.state.lock().expect("fake lock");
        state
            .operations
            .push(Operation::Invalidate(request.clone()));
        Ok(CloudFrontResponse {
            value: state
                .invalidation_ids
                .get(&request.tenant_id)
                .cloned()
                .unwrap_or_else(|| format!("request-{}", request.tenant_id)),
            aws_request_id: Some(format!("aws-request-{}", request.tenant_id)),
        })
    }
}
