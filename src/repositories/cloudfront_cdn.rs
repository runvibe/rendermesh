use std::collections::BTreeSet;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_cloudfront::{
    config::Region,
    types::{
        Aliases, DistributionConfig, InvalidationBatch, MinimumProtocolVersion, Origins, Paths,
        SslSupportMethod, ViewerCertificate,
    },
    Client,
};
use uuid::Uuid;

use crate::repositories::cdn::{
    ensure_paths_mode, CdnDomainReconcile, CdnDomainReconcileRequest, CdnDomainReconcileResult,
    CdnPurge, CdnPurgeRequest, CdnPurgeResult,
};

#[derive(Clone)]
pub struct CloudFrontCdnRepository {
    client: Client,
    distribution_id: String,
}

impl CloudFrontCdnRepository {
    pub async fn from_distribution_id_env(distribution_id_env: &str) -> Result<Self> {
        let distribution_id = std::env::var(distribution_id_env).with_context(|| {
            format!("read CloudFront distribution id env {distribution_id_env}")
        })?;
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .load()
            .await;

        Ok(Self {
            client: Client::new(&shared_config),
            distribution_id,
        })
    }
}

#[async_trait]
impl CdnPurge for CloudFrontCdnRepository {
    async fn purge(&self, request: CdnPurgeRequest) -> Result<CdnPurgeResult> {
        let paths = ensure_paths_mode(request.mode, "CloudFront")?;
        let quantity = paths.len() as i32;
        let caller_reference = format!(
            "rendermesh-{}-{}-{}",
            request.origin_id,
            request.generation,
            Uuid::new_v4()
        );
        let paths = Paths::builder()
            .quantity(quantity)
            .set_items(Some(paths))
            .build()?;
        let batch = InvalidationBatch::builder()
            .caller_reference(caller_reference)
            .paths(paths)
            .build()?;
        let response = self
            .client
            .create_invalidation()
            .distribution_id(&self.distribution_id)
            .invalidation_batch(batch)
            .send()
            .await?;
        let invalidation = response.invalidation();

        Ok(CdnPurgeResult {
            provider: "cloudfront".to_string(),
            request_id: invalidation.map(|invalidation| invalidation.id().to_string()),
            status: invalidation
                .map(|invalidation| invalidation.status())
                .unwrap_or("submitted")
                .to_string(),
            submitted_items: quantity as usize,
        })
    }
}

#[async_trait]
impl CdnDomainReconcile for CloudFrontCdnRepository {
    async fn reconcile_domains(
        &self,
        request: CdnDomainReconcileRequest,
    ) -> Result<CdnDomainReconcileResult> {
        let response = self
            .client
            .get_distribution_config()
            .id(&self.distribution_id)
            .send()
            .await?;
        let etag = response
            .e_tag()
            .ok_or_else(|| anyhow!("CloudFront get_distribution_config response missing ETag"))?;
        let config = response
            .distribution_config()
            .ok_or_else(|| {
                anyhow!("CloudFront get_distribution_config response missing DistributionConfig")
            })?
            .clone();
        let (config, result) = reconcile_distribution_config(config, &request)?;

        self.client
            .update_distribution()
            .id(&self.distribution_id)
            .if_match(etag)
            .distribution_config(config)
            .send()
            .await?;

        Ok(result)
    }
}

fn reconcile_distribution_config(
    mut config: DistributionConfig,
    request: &CdnDomainReconcileRequest,
) -> Result<(DistributionConfig, CdnDomainReconcileResult)> {
    if !request.desired_domains.is_empty() && request.certificate_arn.is_none() {
        return Err(anyhow!(
            "CloudFront domain reconciliation requires certificate_arn"
        ));
    }

    let existing_aliases = config
        .aliases
        .as_ref()
        .map(|aliases| aliases.items().iter().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let desired_aliases = request.desired_domains.clone();
    let final_aliases = if request.remove_extra_domains {
        desired_aliases.clone()
    } else {
        existing_aliases
            .union(&desired_aliases)
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    let aliases = final_aliases.iter().cloned().collect::<Vec<_>>();

    config.aliases = Some(
        Aliases::builder()
            .quantity(aliases.len() as i32)
            .set_items((!aliases.is_empty()).then_some(aliases))
            .build()?,
    );

    if let Some(certificate_arn) = request.certificate_arn.as_ref() {
        config.viewer_certificate = Some(
            ViewerCertificate::builder()
                .cloud_front_default_certificate(false)
                .acm_certificate_arn(certificate_arn)
                .ssl_support_method(SslSupportMethod::SniOnly)
                .minimum_protocol_version(MinimumProtocolVersion::TlSv122021)
                .build(),
        );
    }

    update_default_origin_domain(&mut config, &request.origin_domain)?;

    Ok((
        config,
        CdnDomainReconcileResult {
            provider: "cloudfront".to_string(),
            status: "submitted".to_string(),
            added: desired_aliases.difference(&existing_aliases).count(),
            updated: 0,
            removed: if request.remove_extra_domains {
                existing_aliases.difference(&desired_aliases).count()
            } else {
                0
            },
            unchanged: desired_aliases.intersection(&existing_aliases).count(),
        },
    ))
}

fn update_default_origin_domain(
    config: &mut DistributionConfig,
    origin_domain: &str,
) -> Result<()> {
    let Some(origins) = config.origins.as_mut() else {
        return Ok(());
    };
    let target_origin_id = config
        .default_cache_behavior
        .as_ref()
        .map(|behavior| behavior.target_origin_id().to_string());
    let Some(origin) = origins.items.iter_mut().find(|origin| {
        target_origin_id
            .as_ref()
            .is_none_or(|target_origin_id| origin.id() == target_origin_id)
    }) else {
        return Ok(());
    };

    origin.domain_name = origin_domain.to_string();
    origins.quantity = origins.items.len() as i32;
    let rebuilt = Origins::builder()
        .quantity(origins.quantity)
        .set_items(Some(origins.items.clone()))
        .build()?;
    config.origins = Some(rebuilt);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use aws_sdk_cloudfront::types::{
        AllowedMethods, CachedMethods, DefaultCacheBehavior, ForwardedValues, GeoRestriction,
        Method, Origin, Restrictions, SslProtocol, ViewerProtocolPolicy,
    };

    use super::*;

    #[test]
    fn cloudfront_distribution_config_reconcile_updates_aliases_and_origin() {
        let config = minimal_distribution_config(vec!["old.example.com"], "old-origin.example.com");

        let (updated, result) = reconcile_distribution_config(
            config,
            &CdnDomainReconcileRequest {
                origin_id: "web".to_string(),
                desired_domains: BTreeSet::from([
                    "new.example.com".to_string(),
                    "old.example.com".to_string(),
                ]),
                origin_domain: "rendermesh.example.com".to_string(),
                certificate_arn: Some("arn:aws:acm:us-east-1:123:certificate/abc".to_string()),
                proxied: false,
                remove_extra_domains: false,
            },
        )
        .expect("config reconciles");

        assert_eq!(result.added, 1);
        assert_eq!(result.unchanged, 1);
        assert_eq!(
            updated.aliases.expect("aliases").items(),
            &["new.example.com".to_string(), "old.example.com".to_string()]
        );
        assert_eq!(
            updated.origins.expect("origins").items()[0].domain_name(),
            "rendermesh.example.com"
        );
    }

    #[allow(deprecated)]
    fn minimal_distribution_config(aliases: Vec<&str>, origin_domain: &str) -> DistributionConfig {
        let cached_methods = CachedMethods::builder()
            .quantity(2)
            .items(Method::Get)
            .items(Method::Head)
            .build()
            .expect("cached methods");
        let allowed_methods = AllowedMethods::builder()
            .quantity(2)
            .items(Method::Get)
            .items(Method::Head)
            .cached_methods(cached_methods)
            .build()
            .expect("allowed methods");
        let forwarded_values = ForwardedValues::builder()
            .query_string(false)
            .cookies(
                aws_sdk_cloudfront::types::CookiePreference::builder()
                    .forward(aws_sdk_cloudfront::types::ItemSelection::None)
                    .build()
                    .expect("cookies"),
            )
            .build()
            .expect("forwarded values");
        DistributionConfig::builder()
            .caller_reference("test")
            .aliases(
                Aliases::builder()
                    .quantity(aliases.len() as i32)
                    .set_items(Some(aliases.into_iter().map(ToString::to_string).collect()))
                    .build()
                    .expect("aliases"),
            )
            .origins(
                Origins::builder()
                    .quantity(1)
                    .items(
                        Origin::builder()
                            .id("origin-1")
                            .domain_name(origin_domain)
                            .custom_origin_config(
                                aws_sdk_cloudfront::types::CustomOriginConfig::builder()
                                    .http_port(80)
                                    .https_port(443)
                                    .origin_protocol_policy(
                                        aws_sdk_cloudfront::types::OriginProtocolPolicy::HttpsOnly,
                                    )
                                    .origin_ssl_protocols(
                                        aws_sdk_cloudfront::types::OriginSslProtocols::builder()
                                            .quantity(1)
                                            .items(SslProtocol::TlSv12)
                                            .build()
                                            .expect("protocols"),
                                    )
                                    .build()
                                    .expect("custom origin"),
                            )
                            .build()
                            .expect("origin"),
                    )
                    .build()
                    .expect("origins"),
            )
            .default_cache_behavior(
                DefaultCacheBehavior::builder()
                    .target_origin_id("origin-1")
                    .viewer_protocol_policy(ViewerProtocolPolicy::RedirectToHttps)
                    .allowed_methods(allowed_methods)
                    .forwarded_values(forwarded_values)
                    .min_ttl(0)
                    .build()
                    .expect("cache behavior"),
            )
            .comment("test")
            .enabled(true)
            .restrictions(
                Restrictions::builder()
                    .geo_restriction(
                        GeoRestriction::builder()
                            .restriction_type(aws_sdk_cloudfront::types::GeoRestrictionType::None)
                            .quantity(0)
                            .build()
                            .expect("geo restriction"),
                    )
                    .build(),
            )
            .viewer_certificate(
                ViewerCertificate::builder()
                    .cloud_front_default_certificate(true)
                    .build(),
            )
            .build()
            .expect("distribution config")
    }
}
