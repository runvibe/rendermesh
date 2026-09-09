use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_cloudfront::{
    config::Region,
    operation::RequestId,
    types::{
        Certificate as AwsCertificate, CustomizationActionType,
        Customizations as AwsCustomizations, DistributionTenantAssociationFilter, DomainItem,
        GeoRestrictionCustomization as AwsGeoRestrictionCustomization, GeoRestrictionType,
        InvalidationBatch, ManagedCertificateRequest as AwsManagedCertificateRequest, Parameter,
        Paths, ValidationTokenHost, WebAclCustomization as AwsWebAclCustomization,
    },
    Client,
};
use sha2::{Digest, Sha256};

use crate::{
    dto::manifest::{
        CloudFrontSaasCdnConfig, CloudFrontSaasCertificateConfig, CloudFrontSaasValidationTokenHost,
    },
    repositories::cdn::{
        ensure_paths_mode, CdnDomainReconcileResult, CdnPurge, CdnPurgeRequest, CdnPurgeResult,
        CdnTenantConfig, CdnTenantReconcile, CdnTenantReconcileRequest, ManagedCertificateRequest,
    },
};

#[derive(Clone)]
pub struct CloudFrontSaasCdnRepository {
    client: Arc<dyn CloudFrontSaasClient>,
    config: CdnTenantConfig,
    exact_hosts: BTreeSet<String>,
}

impl CloudFrontSaasCdnRepository {
    pub async fn from_config(
        config: &CloudFrontSaasCdnConfig,
        exact_hosts: BTreeSet<String>,
    ) -> Result<Self> {
        let tenant_config = resolve_tenant_config(config)?;
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .load()
            .await;

        Ok(Self::with_client(
            Arc::new(AwsCloudFrontSaasClient {
                client: Client::new(&shared_config),
            }),
            tenant_config,
            exact_hosts,
        ))
    }

    fn with_client(
        client: Arc<dyn CloudFrontSaasClient>,
        config: CdnTenantConfig,
        exact_hosts: BTreeSet<String>,
    ) -> Self {
        Self {
            client,
            config,
            exact_hosts,
        }
    }

    fn merged_tenant(&self, existing: &DistributionTenant, domain: &str) -> DistributionTenant {
        let mut parameters = existing.parameters.clone();
        parameters.extend(self.config.parameters.clone());

        DistributionTenant {
            id: existing.id.clone(),
            name: existing.name.clone(),
            distribution_id: self.config.distribution_id.clone(),
            domains: BTreeSet::from([domain.to_string()]),
            connection_group_id: self
                .config
                .connection_group_id
                .clone()
                .or_else(|| existing.connection_group_id.clone()),
            parameters,
            customizations: existing.customizations.clone(),
            enabled: existing.enabled.map(|_| true),
        }
    }

    fn tenant_matches(
        &self,
        existing: &DistributionTenant,
        domain: &str,
        managed_certificate: Option<&ManagedCertificateLookup>,
    ) -> bool {
        let desired = self.merged_tenant(existing, domain);
        let certificate_matches =
            self.config.managed_certificate.as_ref().is_none_or(
                |request| match managed_certificate {
                    Some(ManagedCertificateLookup::Found {
                        validation_token_host: Some(existing),
                    }) => existing == &request.validation_token_host,
                    Some(ManagedCertificateLookup::Found {
                        validation_token_host: None,
                    }) => true,
                    Some(ManagedCertificateLookup::NotFound) | None => false,
                },
            );

        existing == &desired && certificate_matches
    }

    fn verify_distribution(&self, tenant: &DistributionTenant, domain: &str) -> Result<()> {
        if tenant.distribution_id != self.config.distribution_id {
            return Err(anyhow!(
                "CloudFront SaaS tenant {} for domain {domain} belongs to distribution {}, not {}",
                tenant.id,
                tenant.distribution_id,
                self.config.distribution_id
            ));
        }
        Ok(())
    }

    fn verify_exact_host(&self, tenant: &DistributionTenant, domain: &str) -> Result<()> {
        if tenant.domains != BTreeSet::from([domain.to_string()]) {
            return Err(anyhow!(
                "CloudFront SaaS tenant {} found for desired exact host {domain} also owns other domains; refusing adoption",
                tenant.id
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl CdnTenantReconcile for CloudFrontSaasCdnRepository {
    async fn reconcile_tenants(
        &self,
        request: CdnTenantReconcileRequest,
    ) -> Result<CdnDomainReconcileResult> {
        let mut added = 0;
        let mut updated = 0;
        let mut unchanged = 0;

        for domain in request.desired_domains {
            let lookup = self
                .client
                .get_distribution_tenant_by_domain(&domain)
                .await
                .with_context(|| format!("get CloudFront SaaS tenant for exact domain {domain}"))?;
            let Some(existing) = lookup.value else {
                let tenant_name = deterministic_tenant_name(&self.config.distribution_id, &domain);
                let created = self
                    .client
                    .create_distribution_tenant(CreateDistributionTenantRequest {
                        distribution_id: self.config.distribution_id.clone(),
                        name: tenant_name.clone(),
                        domains: BTreeSet::from([domain.clone()]),
                        connection_group_id: self.config.connection_group_id.clone(),
                        parameters: self.config.parameters.clone(),
                        managed_certificate: self.config.managed_certificate.clone(),
                        enabled: true,
                    })
                    .await
                    .with_context(|| {
                        format!("create CloudFront SaaS tenant for exact domain {domain}")
                    })?;
                trace_tenant_operation(
                    "created",
                    &tenant_name,
                    &created.value.id,
                    created.aws_request_id.as_deref(),
                );
                added += 1;
                continue;
            };

            self.verify_distribution(&existing, &domain)?;
            self.verify_exact_host(&existing, &domain)?;
            let mut observation_request_id = lookup.aws_request_id;
            let managed_certificate = if self.config.managed_certificate.is_some() {
                let certificate = self
                    .client
                    .get_managed_certificate_validation_token_host(&existing.id)
                    .await
                    .with_context(|| {
                        format!(
                            "get managed certificate details for CloudFront SaaS tenant {}",
                            existing.id
                        )
                    })?;
                observation_request_id = certificate.aws_request_id.or(observation_request_id);
                Some(certificate.value)
            } else {
                None
            };

            if self.tenant_matches(&existing, &domain, managed_certificate.as_ref()) {
                trace_tenant_operation(
                    "unchanged",
                    &existing.name,
                    &existing.id,
                    observation_request_id.as_deref(),
                );
                unchanged += 1;
                continue;
            }

            let current_response = self
                .client
                .get_distribution_tenant(&existing.id)
                .await
                .with_context(|| {
                    format!(
                        "get current CloudFront SaaS tenant {} for exact domain {domain}",
                        existing.id
                    )
                })?;
            let current = current_response.value;
            self.verify_distribution(&current.tenant, &domain)?;
            self.verify_exact_host(&current.tenant, &domain)?;
            if self.tenant_matches(&current.tenant, &domain, managed_certificate.as_ref()) {
                trace_tenant_operation(
                    "unchanged",
                    &current.tenant.name,
                    &current.tenant.id,
                    current_response.aws_request_id.as_deref(),
                );
                unchanged += 1;
                continue;
            }

            let current_id = current.tenant.id.clone();
            let updated_tenant = self
                .client
                .update_distribution_tenant(UpdateDistributionTenantRequest {
                    id: current_id.clone(),
                    if_match: current.etag,
                    tenant: self.merged_tenant(&current.tenant, &domain),
                    managed_certificate: self.config.managed_certificate.clone(),
                })
                .await
                .with_context(|| {
                    format!(
                        "update CloudFront SaaS tenant {} for exact domain {domain}",
                        current.tenant.id
                    )
                })?;
            trace_tenant_operation(
                "updated",
                &updated_tenant.value.name,
                &updated_tenant.value.id,
                updated_tenant.aws_request_id.as_deref(),
            );
            updated += 1;
        }

        Ok(CdnDomainReconcileResult {
            provider: "cloudfront_saas".to_string(),
            status: "submitted".to_string(),
            added,
            updated,
            removed: 0,
            unchanged,
        })
    }
}

#[async_trait]
impl CdnPurge for CloudFrontSaasCdnRepository {
    async fn purge(&self, request: CdnPurgeRequest) -> Result<CdnPurgeResult> {
        let paths = ensure_paths_mode(request.mode, "CloudFront SaaS")?;
        let mut marker = None;
        let mut tenants = Vec::new();

        loop {
            let page = self
                .client
                .list_distribution_tenants(&self.config.distribution_id, marker.as_deref())
                .await
                .with_context(|| {
                    format!(
                        "list CloudFront SaaS tenants for distribution {}",
                        self.config.distribution_id
                    )
                })?;
            tenants.extend(page.tenants);
            let Some(next_marker) = page.next_marker else {
                break;
            };
            marker = Some(next_marker);
        }

        tenants.retain(|tenant| {
            tenant.enabled != Some(false)
                && tenant.distribution_id == self.config.distribution_id
                && tenant
                    .domains
                    .iter()
                    .any(|domain| self.exact_hosts.contains(domain))
        });
        tenants.sort_by(|left, right| left.id.cmp(&right.id));

        if tenants.is_empty() {
            return Ok(CdnPurgeResult {
                provider: "cloudfront_saas".to_string(),
                request_ids: Vec::new(),
                status: "skipped_no_tenants".to_string(),
                submitted_items: 0,
            });
        }

        let submitted_items = paths.len() * tenants.len();
        let mut request_ids = Vec::with_capacity(tenants.len());
        for tenant in tenants {
            let caller_reference = format!(
                "rendermesh-{}-{}-{}",
                request.origin_id, request.generation, tenant.id
            );
            let tenant_id = tenant.id;
            let invalidation = self
                .client
                .create_invalidation_for_distribution_tenant(TenantInvalidationRequest {
                    tenant_id: tenant_id.clone(),
                    caller_reference,
                    paths: paths.clone(),
                })
                .await
                .with_context(|| format!("invalidate CloudFront SaaS tenant {tenant_id}"))?;
            trace_tenant_operation(
                "invalidated",
                &tenant.name,
                &tenant_id,
                invalidation.aws_request_id.as_deref(),
            );
            request_ids.push(invalidation.value);
        }

        Ok(CdnPurgeResult {
            provider: "cloudfront_saas".to_string(),
            request_ids,
            status: "submitted".to_string(),
            submitted_items,
        })
    }
}

pub fn deterministic_tenant_name(distribution_id: &str, host: &str) -> String {
    let digest = Sha256::digest(format!("{distribution_id}\0{host}").as_bytes());
    let hash = format!("{digest:x}");
    let host_prefix = host
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '.') {
                character
            } else {
                '-'
            }
        })
        .take(100)
        .collect::<String>();
    format!("rendermesh-{host_prefix}-{}", &hash[..16])
}

fn resolve_tenant_config(config: &CloudFrontSaasCdnConfig) -> Result<CdnTenantConfig> {
    let distribution_id = read_environment_variable(&config.distribution_id_env)
        .context("resolve CloudFront SaaS cdn.distribution_id_env")?;
    let connection_group_id = config
        .connection_group_id_env
        .as_ref()
        .map(|env_name| {
            read_environment_variable(env_name)
                .context("resolve CloudFront SaaS cdn.connection_group_id_env")
        })
        .transpose()?;
    let parameters = config
        .parameters_env
        .iter()
        .map(|(name, env_name)| {
            read_environment_variable(env_name)
                .with_context(|| format!("resolve CloudFront SaaS cdn.parameters_env.{name}"))
                .map(|value| (name.clone(), value))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let managed_certificate = config
        .certificate
        .as_ref()
        .map(|certificate| match certificate {
            CloudFrontSaasCertificateConfig::Managed {
                validation_token_host,
            } => ManagedCertificateRequest {
                validation_token_host: match validation_token_host {
                    CloudFrontSaasValidationTokenHost::CloudFront => "cloudfront".to_string(),
                },
            },
        });

    Ok(CdnTenantConfig {
        distribution_id,
        connection_group_id,
        parameters,
        managed_certificate,
    })
}

fn read_environment_variable(name: &str) -> Result<String> {
    std::env::var(name).map_err(|error| environment_variable_error(name, error))
}

fn environment_variable_error(name: &str, error: std::env::VarError) -> anyhow::Error {
    let error_class = match error {
        std::env::VarError::NotPresent => "NotPresent",
        std::env::VarError::NotUnicode(_) => "NotUnicode",
    };
    anyhow!("environment variable {name}: {error_class}")
}

fn trace_tenant_operation(
    action: &'static str,
    tenant_name: &str,
    tenant_id: &str,
    aws_request_id: Option<&str>,
) {
    tracing::info!(
        action,
        tenant_name,
        tenant_id,
        aws_request_id = aws_request_id.unwrap_or("unknown"),
        "CloudFront SaaS tenant operation"
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DistributionTenant {
    id: String,
    name: String,
    distribution_id: String,
    domains: BTreeSet<String>,
    connection_group_id: Option<String>,
    parameters: BTreeMap<String, String>,
    customizations: Option<TenantCustomizations>,
    enabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VersionedDistributionTenant {
    tenant: DistributionTenant,
    etag: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DistributionTenantPage {
    tenants: Vec<DistributionTenant>,
    next_marker: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CloudFrontResponse<T> {
    value: T,
    aws_request_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ManagedCertificateLookup {
    Found {
        validation_token_host: Option<String>,
    },
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TenantCustomizations {
    certificate_arn: Option<String>,
    geo_restrictions: Option<TenantGeoRestrictionCustomization>,
    web_acl: Option<TenantWebAclCustomization>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TenantGeoRestrictionCustomization {
    restriction_type: String,
    locations: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TenantWebAclCustomization {
    action: String,
    arn: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CreateDistributionTenantRequest {
    distribution_id: String,
    name: String,
    domains: BTreeSet<String>,
    connection_group_id: Option<String>,
    parameters: BTreeMap<String, String>,
    managed_certificate: Option<ManagedCertificateRequest>,
    enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UpdateDistributionTenantRequest {
    id: String,
    if_match: String,
    tenant: DistributionTenant,
    managed_certificate: Option<ManagedCertificateRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TenantInvalidationRequest {
    tenant_id: String,
    caller_reference: String,
    paths: Vec<String>,
}

#[async_trait]
trait CloudFrontSaasClient: Send + Sync {
    async fn get_distribution_tenant_by_domain(
        &self,
        domain: &str,
    ) -> Result<CloudFrontResponse<Option<DistributionTenant>>>;

    async fn get_distribution_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<CloudFrontResponse<VersionedDistributionTenant>>;

    async fn get_managed_certificate_validation_token_host(
        &self,
        tenant_id: &str,
    ) -> Result<CloudFrontResponse<ManagedCertificateLookup>>;

    async fn list_distribution_tenants(
        &self,
        distribution_id: &str,
        marker: Option<&str>,
    ) -> Result<DistributionTenantPage>;

    async fn create_distribution_tenant(
        &self,
        request: CreateDistributionTenantRequest,
    ) -> Result<CloudFrontResponse<DistributionTenant>>;

    async fn update_distribution_tenant(
        &self,
        request: UpdateDistributionTenantRequest,
    ) -> Result<CloudFrontResponse<DistributionTenant>>;

    async fn create_invalidation_for_distribution_tenant(
        &self,
        request: TenantInvalidationRequest,
    ) -> Result<CloudFrontResponse<String>>;
}

struct AwsCloudFrontSaasClient {
    client: Client,
}

#[async_trait]
impl CloudFrontSaasClient for AwsCloudFrontSaasClient {
    async fn get_distribution_tenant_by_domain(
        &self,
        domain: &str,
    ) -> Result<CloudFrontResponse<Option<DistributionTenant>>> {
        let response = self
            .client
            .get_distribution_tenant_by_domain()
            .domain(domain)
            .send()
            .await;
        match response {
            Ok(response) => {
                let aws_request_id = response.request_id().map(ToString::to_string);
                let tenant = response
                    .distribution_tenant()
                    .ok_or_else(|| {
                        anyhow!(
                            "CloudFront get_distribution_tenant_by_domain response missing tenant"
                        )
                    })
                    .and_then(distribution_tenant_from_aws)?;
                Ok(CloudFrontResponse {
                    value: Some(tenant),
                    aws_request_id,
                })
            }
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|service_error| service_error.is_entity_not_found()) =>
            {
                Ok(CloudFrontResponse {
                    value: None,
                    aws_request_id: error.request_id().map(ToString::to_string),
                })
            }
            Err(error) => Err(error).with_context(|| {
                format!("CloudFront get_distribution_tenant_by_domain for domain {domain}")
            }),
        }
    }

    async fn get_distribution_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<CloudFrontResponse<VersionedDistributionTenant>> {
        let response = self
            .client
            .get_distribution_tenant()
            .identifier(tenant_id)
            .send()
            .await
            .with_context(|| {
                format!("CloudFront get_distribution_tenant for tenant {tenant_id}")
            })?;
        let aws_request_id = response.request_id().map(ToString::to_string);
        let tenant = response
            .distribution_tenant()
            .ok_or_else(|| anyhow!("CloudFront get_distribution_tenant response missing tenant"))
            .and_then(distribution_tenant_from_aws)?;
        let etag = response
            .e_tag()
            .ok_or_else(|| {
                anyhow!(
                    "CloudFront get_distribution_tenant response missing ETag for tenant {tenant_id}"
                )
            })?
            .to_string();

        Ok(CloudFrontResponse {
            value: VersionedDistributionTenant { tenant, etag },
            aws_request_id,
        })
    }

    async fn get_managed_certificate_validation_token_host(
        &self,
        tenant_id: &str,
    ) -> Result<CloudFrontResponse<ManagedCertificateLookup>> {
        let response = self
            .client
            .get_managed_certificate_details()
            .identifier(tenant_id)
            .send()
            .await;
        match response {
            Ok(response) => {
                let aws_request_id = response.request_id().map(ToString::to_string);
                let details = response.managed_certificate_details().ok_or_else(|| {
                    anyhow!(
                        "CloudFront get_managed_certificate_details response missing details for tenant {tenant_id}"
                    )
                })?;
                Ok(CloudFrontResponse {
                    value: ManagedCertificateLookup::Found {
                        validation_token_host: details
                            .validation_token_host()
                            .map(|host| host.as_str().to_string()),
                    },
                    aws_request_id,
                })
            }
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|service_error| service_error.is_entity_not_found()) =>
            {
                Ok(CloudFrontResponse {
                    value: ManagedCertificateLookup::NotFound,
                    aws_request_id: error.request_id().map(ToString::to_string),
                })
            }
            Err(error) => Err(error).with_context(|| {
                format!("CloudFront get_managed_certificate_details for tenant {tenant_id}")
            }),
        }
    }

    async fn list_distribution_tenants(
        &self,
        distribution_id: &str,
        marker: Option<&str>,
    ) -> Result<DistributionTenantPage> {
        let filter = DistributionTenantAssociationFilter::builder()
            .distribution_id(distribution_id)
            .build();
        let response = self
            .client
            .list_distribution_tenants()
            .association_filter(filter)
            .set_marker(marker.map(ToString::to_string))
            .send()
            .await
            .with_context(|| {
                format!("CloudFront list_distribution_tenants for distribution {distribution_id}")
            })?;
        let tenants = response
            .distribution_tenant_list()
            .iter()
            .map(distribution_tenant_summary_from_aws)
            .collect::<Vec<_>>();

        Ok(DistributionTenantPage {
            tenants,
            next_marker: response.next_marker().map(ToString::to_string),
        })
    }

    async fn create_distribution_tenant(
        &self,
        request: CreateDistributionTenantRequest,
    ) -> Result<CloudFrontResponse<DistributionTenant>> {
        let domains = domain_items(&request.domains)?;
        let parameters = parameters(&request.parameters)?;
        let managed_certificate = request
            .managed_certificate
            .as_ref()
            .map(aws_managed_certificate_request)
            .transpose()?;
        let response = self
            .client
            .create_distribution_tenant()
            .distribution_id(&request.distribution_id)
            .name(&request.name)
            .set_domains(Some(domains))
            .set_connection_group_id(request.connection_group_id)
            .set_parameters(Some(parameters))
            .set_managed_certificate_request(managed_certificate)
            .enabled(request.enabled)
            .send()
            .await
            .with_context(|| {
                format!(
                    "CloudFront create_distribution_tenant named {} for distribution {}",
                    request.name, request.distribution_id
                )
            })?;
        let aws_request_id = response.request_id().map(ToString::to_string);
        let tenant = response
            .distribution_tenant()
            .ok_or_else(|| anyhow!("CloudFront create_distribution_tenant response missing tenant"))
            .and_then(distribution_tenant_from_aws)?;
        Ok(CloudFrontResponse {
            value: tenant,
            aws_request_id,
        })
    }

    async fn update_distribution_tenant(
        &self,
        request: UpdateDistributionTenantRequest,
    ) -> Result<CloudFrontResponse<DistributionTenant>> {
        let domains = domain_items(&request.tenant.domains)?;
        let parameters = parameters(&request.tenant.parameters)?;
        let customizations = request
            .tenant
            .customizations
            .as_ref()
            .map(aws_customizations)
            .transpose()?;
        let managed_certificate = request
            .managed_certificate
            .as_ref()
            .map(aws_managed_certificate_request)
            .transpose()?;
        let response = self
            .client
            .update_distribution_tenant()
            .id(&request.id)
            .distribution_id(&request.tenant.distribution_id)
            .set_domains(Some(domains))
            .set_customizations(customizations)
            .set_connection_group_id(request.tenant.connection_group_id)
            .set_parameters(Some(parameters))
            .set_managed_certificate_request(managed_certificate)
            .set_enabled(request.tenant.enabled)
            .if_match(&request.if_match)
            .send()
            .await
            .with_context(|| {
                format!(
                    "CloudFront update_distribution_tenant for tenant {}",
                    request.id
                )
            })?;
        let aws_request_id = response.request_id().map(ToString::to_string);
        let tenant = response
            .distribution_tenant()
            .ok_or_else(|| anyhow!("CloudFront update_distribution_tenant response missing tenant"))
            .and_then(distribution_tenant_from_aws)?;
        Ok(CloudFrontResponse {
            value: tenant,
            aws_request_id,
        })
    }

    async fn create_invalidation_for_distribution_tenant(
        &self,
        request: TenantInvalidationRequest,
    ) -> Result<CloudFrontResponse<String>> {
        let paths = Paths::builder()
            .quantity(request.paths.len() as i32)
            .set_items(Some(request.paths))
            .build()
            .context("build CloudFront SaaS invalidation paths")?;
        let batch = InvalidationBatch::builder()
            .caller_reference(request.caller_reference)
            .paths(paths)
            .build()
            .context("build CloudFront SaaS invalidation batch")?;
        let response = self
            .client
            .create_invalidation_for_distribution_tenant()
            .id(&request.tenant_id)
            .invalidation_batch(batch)
            .send()
            .await
            .with_context(|| {
                format!(
                    "CloudFront create_invalidation_for_distribution_tenant for tenant {}",
                    request.tenant_id
                )
            })?;
        let aws_request_id = response.request_id().map(ToString::to_string);
        let invalidation_id = response
            .invalidation()
            .map(|invalidation| invalidation.id().to_string())
            .ok_or_else(|| {
                anyhow!(
                    "CloudFront create_invalidation_for_distribution_tenant response missing invalidation for tenant {}",
                    request.tenant_id
                )
            })?;
        Ok(CloudFrontResponse {
            value: invalidation_id,
            aws_request_id,
        })
    }
}

fn distribution_tenant_from_aws(
    tenant: &aws_sdk_cloudfront::types::DistributionTenant,
) -> Result<DistributionTenant> {
    Ok(DistributionTenant {
        id: tenant
            .id()
            .ok_or_else(|| anyhow!("CloudFront distribution tenant missing ID"))?
            .to_string(),
        name: tenant
            .name()
            .ok_or_else(|| anyhow!("CloudFront distribution tenant missing name"))?
            .to_string(),
        distribution_id: tenant
            .distribution_id()
            .ok_or_else(|| anyhow!("CloudFront distribution tenant missing distribution ID"))?
            .to_string(),
        domains: tenant
            .domains()
            .iter()
            .map(|domain| domain.domain().to_string())
            .collect(),
        connection_group_id: tenant.connection_group_id().map(ToString::to_string),
        parameters: tenant
            .parameters()
            .iter()
            .map(|parameter| (parameter.name().to_string(), parameter.value().to_string()))
            .collect(),
        customizations: tenant.customizations().map(tenant_customizations_from_aws),
        enabled: tenant.enabled(),
    })
}

fn distribution_tenant_summary_from_aws(
    tenant: &aws_sdk_cloudfront::types::DistributionTenantSummary,
) -> DistributionTenant {
    DistributionTenant {
        id: tenant.id().to_string(),
        name: tenant.name().to_string(),
        distribution_id: tenant.distribution_id().to_string(),
        domains: tenant
            .domains()
            .iter()
            .map(|domain| domain.domain().to_string())
            .collect(),
        connection_group_id: tenant.connection_group_id().map(ToString::to_string),
        parameters: BTreeMap::new(),
        customizations: tenant.customizations().map(tenant_customizations_from_aws),
        enabled: tenant.enabled(),
    }
}

fn tenant_customizations_from_aws(customizations: &AwsCustomizations) -> TenantCustomizations {
    TenantCustomizations {
        certificate_arn: customizations
            .certificate()
            .map(|certificate| certificate.arn().to_string()),
        geo_restrictions: customizations.geo_restrictions().map(|restrictions| {
            TenantGeoRestrictionCustomization {
                restriction_type: restrictions.restriction_type().as_str().to_string(),
                locations: restrictions.locations.clone(),
            }
        }),
        web_acl: customizations
            .web_acl()
            .map(|web_acl| TenantWebAclCustomization {
                action: web_acl.action().as_str().to_string(),
                arn: web_acl.arn().map(ToString::to_string),
            }),
    }
}

fn aws_customizations(customizations: &TenantCustomizations) -> Result<AwsCustomizations> {
    let certificate = customizations
        .certificate_arn
        .as_ref()
        .map(|arn| {
            AwsCertificate::builder()
                .arn(arn)
                .build()
                .context("build CloudFront SaaS certificate customization")
        })
        .transpose()?;
    let geo_restrictions = customizations
        .geo_restrictions
        .as_ref()
        .map(|restrictions| {
            AwsGeoRestrictionCustomization::builder()
                .restriction_type(GeoRestrictionType::from(
                    restrictions.restriction_type.as_str(),
                ))
                .set_locations(restrictions.locations.clone())
                .build()
                .context("build CloudFront SaaS geo restriction customization")
        })
        .transpose()?;
    let web_acl = customizations
        .web_acl
        .as_ref()
        .map(|web_acl| {
            AwsWebAclCustomization::builder()
                .action(CustomizationActionType::from(web_acl.action.as_str()))
                .set_arn(web_acl.arn.clone())
                .build()
                .context("build CloudFront SaaS web ACL customization")
        })
        .transpose()?;

    Ok(AwsCustomizations::builder()
        .set_certificate(certificate)
        .set_geo_restrictions(geo_restrictions)
        .set_web_acl(web_acl)
        .build())
}

fn domain_items(domains: &BTreeSet<String>) -> Result<Vec<DomainItem>> {
    domains
        .iter()
        .map(|domain| {
            DomainItem::builder()
                .domain(domain)
                .build()
                .context("build CloudFront SaaS tenant domain")
        })
        .collect()
}

fn parameters(parameters: &BTreeMap<String, String>) -> Result<Vec<Parameter>> {
    parameters
        .iter()
        .map(|(name, value)| {
            Parameter::builder()
                .name(name)
                .value(value)
                .build()
                .context("build CloudFront SaaS tenant parameter")
        })
        .collect()
}

fn aws_managed_certificate_request(
    request: &ManagedCertificateRequest,
) -> Result<AwsManagedCertificateRequest> {
    AwsManagedCertificateRequest::builder()
        .validation_token_host(ValidationTokenHost::from(
            request.validation_token_host.as_str(),
        ))
        .build()
        .context("build CloudFront SaaS managed certificate request")
}

#[cfg(test)]
mod tests;
