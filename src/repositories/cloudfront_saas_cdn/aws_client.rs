use std::collections::{BTreeMap, BTreeSet};

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

use crate::repositories::cdn::ManagedCertificateRequest;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DistributionTenant {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) distribution_id: String,
    pub(super) domains: BTreeSet<String>,
    pub(super) connection_group_id: Option<String>,
    pub(super) parameters: BTreeMap<String, String>,
    pub(super) customizations: Option<TenantCustomizations>,
    pub(super) enabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VersionedDistributionTenant {
    pub(super) tenant: DistributionTenant,
    pub(super) etag: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DistributionTenantPage {
    pub(super) tenants: Vec<DistributionTenant>,
    pub(super) next_marker: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CloudFrontResponse<T> {
    pub(super) value: T,
    pub(super) aws_request_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ManagedCertificateLookup {
    Found {
        validation_token_host: Option<String>,
    },
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TenantCustomizations {
    pub(super) certificate_arn: Option<String>,
    pub(super) geo_restrictions: Option<TenantGeoRestrictionCustomization>,
    pub(super) web_acl: Option<TenantWebAclCustomization>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TenantGeoRestrictionCustomization {
    pub(super) restriction_type: String,
    pub(super) locations: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TenantWebAclCustomization {
    pub(super) action: String,
    pub(super) arn: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CreateDistributionTenantRequest {
    pub(super) distribution_id: String,
    pub(super) name: String,
    pub(super) domains: BTreeSet<String>,
    pub(super) connection_group_id: Option<String>,
    pub(super) parameters: BTreeMap<String, String>,
    pub(super) managed_certificate: Option<ManagedCertificateRequest>,
    pub(super) enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UpdateDistributionTenantRequest {
    pub(super) id: String,
    pub(super) if_match: String,
    pub(super) tenant: DistributionTenant,
    pub(super) managed_certificate: Option<ManagedCertificateRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TenantInvalidationRequest {
    pub(super) tenant_id: String,
    pub(super) caller_reference: String,
    pub(super) paths: Vec<String>,
}

#[async_trait]
pub(super) trait CloudFrontSaasClient: Send + Sync {
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

pub(super) struct AwsCloudFrontSaasClient {
    client: Client,
}

impl AwsCloudFrontSaasClient {
    pub(super) async fn from_default_config() -> Self {
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .load()
            .await;
        Self {
            client: Client::new(&shared_config),
        }
    }
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

pub(super) fn tenant_customizations_from_aws(
    customizations: &AwsCustomizations,
) -> TenantCustomizations {
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

pub(super) fn aws_customizations(
    customizations: &TenantCustomizations,
) -> Result<AwsCustomizations> {
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
