use super::*;

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

#[test]
fn rejects_whitespace_only_distribution_id_environment_value() {
    let _distribution = EnvVarGuard::set("FINAL_REVIEW_EMPTY_DISTRIBUTION_ID", " \t ");
    let config =
        cloudfront_saas_config("FINAL_REVIEW_EMPTY_DISTRIBUTION_ID", None, BTreeMap::new());

    let error = resolve_tenant_config(&config).expect_err("blank distribution id must fail");
    let rendered = format!("{error:#}");

    assert!(rendered.contains("cdn.distribution_id_env"));
    assert!(rendered.contains("FINAL_REVIEW_EMPTY_DISTRIBUTION_ID"));
    assert!(!rendered.contains("\\t"));
}

#[test]
fn rejects_whitespace_only_connection_group_environment_value() {
    let _distribution = EnvVarGuard::set("FINAL_REVIEW_DISTRIBUTION_ID", "DIST");
    let _connection_group = EnvVarGuard::set("FINAL_REVIEW_EMPTY_CONNECTION_GROUP_ID", "\r\n ");
    let config = cloudfront_saas_config(
        "FINAL_REVIEW_DISTRIBUTION_ID",
        Some("FINAL_REVIEW_EMPTY_CONNECTION_GROUP_ID"),
        BTreeMap::new(),
    );

    let error = resolve_tenant_config(&config).expect_err("blank connection group id must fail");
    let rendered = format!("{error:#}");

    assert!(rendered.contains("cdn.connection_group_id_env"));
    assert!(rendered.contains("FINAL_REVIEW_EMPTY_CONNECTION_GROUP_ID"));
    assert!(!rendered.contains("\\r"));
}

#[test]
fn rejects_whitespace_only_parameter_environment_value() {
    let _distribution = EnvVarGuard::set("FINAL_REVIEW_PARAMETER_DISTRIBUTION_ID", "DIST");
    let _parameter = EnvVarGuard::set("FINAL_REVIEW_EMPTY_PARAMETER", "   ");
    let config = cloudfront_saas_config(
        "FINAL_REVIEW_PARAMETER_DISTRIBUTION_ID",
        None,
        BTreeMap::from([(
            "origin-domain".to_string(),
            "FINAL_REVIEW_EMPTY_PARAMETER".to_string(),
        )]),
    );

    let error = resolve_tenant_config(&config).expect_err("blank parameter must fail");
    let rendered = format!("{error:#}");

    assert!(rendered.contains("cdn.parameters_env.origin-domain"));
    assert!(rendered.contains("FINAL_REVIEW_EMPTY_PARAMETER"));
}

#[tokio::test]
async fn repeated_tenant_pagination_marker_returns_contextual_error() {
    let client = FakeClient::default();
    client.page(
        None,
        DistributionTenantPage {
            tenants: Vec::new(),
            next_marker: Some("repeated".to_string()),
        },
    );
    client.page(
        Some("repeated"),
        DistributionTenantPage {
            tenants: Vec::new(),
            next_marker: Some("repeated".to_string()),
        },
    );
    client.limit_list_calls(2);
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    let error = repository
        .purge(CdnPurgeRequest {
            origin_id: "web".to_string(),
            generation: 1,
            mode: CdnPurgeMode::All,
        })
        .await
        .expect_err("repeated pagination marker must fail");
    let rendered = format!("{error:#}");

    assert!(rendered.contains("CloudFront SaaS tenant pagination"));
    assert!(rendered.contains("DIST"));
    assert_eq!(
        client
            .operations()
            .into_iter()
            .filter(|operation| matches!(operation, Operation::List { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn parameter_only_drift_does_not_resend_matching_managed_certificate() {
    let client = FakeClient::default();
    let mut existing = tenant("tenant-1", "app.example.com");
    existing
        .parameters
        .insert("origin-domain".to_string(), "old.example.com".to_string());
    client.lookup("app.example.com", Some(existing.clone()));
    client.certificate(
        "tenant-1",
        ManagedCertificateLookup::Found {
            validation_token_host: Some("cloudfront".to_string()),
        },
    );
    client.current(existing, "etag-parameter-drift");
    let mut desired = config();
    desired
        .parameters
        .insert("origin-domain".to_string(), "new.example.com".to_string());
    desired.managed_certificate = Some(ManagedCertificateRequest {
        validation_token_host: "cloudfront".to_string(),
    });
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
        updated.tenant.parameters.get("origin-domain"),
        Some(&"new.example.com".to_string())
    );
    assert_eq!(updated.managed_certificate, None);
}

#[tokio::test]
async fn caller_reference_changes_when_paths_change() {
    let client = FakeClient::default();
    client.page(
        None,
        DistributionTenantPage {
            tenants: vec![tenant("tenant-1", "app.example.com")],
            next_marker: None,
        },
    );
    let repository = repository(client.clone(), config(), ["app.example.com"]);

    for path in ["/first.css", "/second.css"] {
        repository
            .purge(CdnPurgeRequest {
                origin_id: "web".to_string(),
                generation: 9,
                mode: CdnPurgeMode::Paths(vec![path.to_string()]),
            })
            .await
            .expect("purge succeeds");
    }

    let references = invalidation_references(&client);
    assert_eq!(references.len(), 2);
    assert_ne!(references[0], references[1]);
}

#[tokio::test]
async fn caller_reference_namespace_separates_repository_instances() {
    let first_client = FakeClient::default();
    first_client.page(
        None,
        DistributionTenantPage {
            tenants: vec![tenant("tenant-1", "app.example.com")],
            next_marker: None,
        },
    );
    let second_client = FakeClient::default();
    second_client.page(
        None,
        DistributionTenantPage {
            tenants: vec![tenant("tenant-1", "app.example.com")],
            next_marker: None,
        },
    );
    let first = repository(first_client.clone(), config(), ["app.example.com"]);
    let second = repository(second_client.clone(), config(), ["app.example.com"]);
    let request = CdnPurgeRequest {
        origin_id: "web".to_string(),
        generation: 1,
        mode: CdnPurgeMode::Paths(vec!["/index.html".to_string()]),
    };

    first
        .purge(request.clone())
        .await
        .expect("first repository purge succeeds");
    second
        .purge(request)
        .await
        .expect("second repository purge succeeds");

    let first_reference = invalidation_references(&first_client)
        .into_iter()
        .next()
        .expect("first caller reference");
    let second_reference = invalidation_references(&second_client)
        .into_iter()
        .next()
        .expect("second caller reference");
    assert_ne!(first_reference, second_reference);
}

fn invalidation_references(client: &FakeClient) -> Vec<String> {
    client
        .operations()
        .into_iter()
        .filter_map(|operation| match operation {
            Operation::Invalidate(request) => Some(request.caller_reference),
            _ => None,
        })
        .collect()
}

fn cloudfront_saas_config(
    distribution_id_env: &str,
    connection_group_id_env: Option<&str>,
    parameters_env: BTreeMap<String, String>,
) -> CloudFrontSaasCdnConfig {
    CloudFrontSaasCdnConfig {
        distribution_id_env: distribution_id_env.to_string(),
        connection_group_id_env: connection_group_id_env.map(ToString::to_string),
        strategy: crate::dto::manifest::CdnRefreshStrategy::ChangedPaths,
        parameters_env,
        certificate: None,
    }
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = self.original.as_ref() {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}
