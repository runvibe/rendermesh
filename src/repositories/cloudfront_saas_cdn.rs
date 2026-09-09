use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::{
    dto::manifest::{
        CloudFrontSaasCdnConfig, CloudFrontSaasCertificateConfig, CloudFrontSaasValidationTokenHost,
    },
    repositories::cdn::{
        ensure_paths_mode, CdnDomainReconcileResult, CdnPurge, CdnPurgeFailure, CdnPurgeRequest,
        CdnPurgeResult, CdnTenantConfig, CdnTenantReconcile, CdnTenantReconcileRequest,
        ManagedCertificateRequest,
    },
};

#[cfg(test)]
use self::aws_client::{
    aws_customizations, tenant_customizations_from_aws, CloudFrontResponse, DistributionTenantPage,
    TenantCustomizations, TenantGeoRestrictionCustomization, TenantWebAclCustomization,
    VersionedDistributionTenant,
};
use self::aws_client::{
    AwsCloudFrontSaasClient, CloudFrontSaasClient, CreateDistributionTenantRequest,
    DistributionTenant, ManagedCertificateLookup, TenantInvalidationRequest,
    UpdateDistributionTenantRequest,
};

mod aws_client;

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
        let client = AwsCloudFrontSaasClient::from_default_config().await;

        Ok(Self::with_client(
            Arc::new(client),
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
                .map_err(|error| CdnPurgeFailure {
                    provider: "cloudfront_saas".to_string(),
                    submitted_items: paths.len() * request_ids.len(),
                    request_ids: request_ids.clone(),
                    message: format!("invalidate CloudFront SaaS tenant {tenant_id}: {error:#}"),
                })?;
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

#[cfg(test)]
mod tests;
