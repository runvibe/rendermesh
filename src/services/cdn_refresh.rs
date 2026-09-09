use std::{collections::BTreeSet, fmt, sync::Arc};

use anyhow::{anyhow, Result};

use crate::{
    dto::manifest::{CdnConfig, CdnRefreshStrategy},
    repositories::{
        cdn::{CdnPurge, CdnPurgeFailure, CdnPurgeMode, CdnPurgeRepository, CdnPurgeRequest},
        cloudflare_cdn::CloudflareCdnRepository,
        cloudfront_cdn::CloudFrontCdnRepository,
        cloudfront_saas_cdn::CloudFrontSaasCdnRepository,
    },
    services::freshness::OriginFreshnessDiff,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdnRefreshPlan {
    pub mode: CdnRefreshMode,
    pub changed_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CdnRefreshMode {
    All,
    Paths(Vec<String>),
    Urls(Vec<String>),
}

#[derive(Clone)]
pub struct OriginCdnRefresh {
    repository: Arc<dyn CdnPurge>,
    strategy: CdnRefreshStrategy,
    url_prefixes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdnRefreshOutcome {
    pub provider: String,
    pub request_ids: Vec<String>,
    pub status: String,
    pub submitted_items: usize,
    pub changed_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CdnRefreshFailure {
    pub provider: String,
    pub request_ids: Vec<String>,
    pub submitted_items: usize,
    message: String,
}

impl fmt::Display for CdnRefreshFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CdnRefreshFailure {}

impl OriginCdnRefresh {
    pub(crate) fn with_repository(
        repository: Arc<dyn CdnPurge>,
        strategy: CdnRefreshStrategy,
        url_prefixes: Vec<String>,
    ) -> Self {
        Self {
            repository,
            strategy,
            url_prefixes,
        }
    }

    pub(crate) fn from_cloudfront_saas(
        repository: CloudFrontSaasCdnRepository,
        strategy: CdnRefreshStrategy,
    ) -> Self {
        Self::with_repository(
            Arc::new(CdnPurgeRepository::CloudFrontSaas(repository)),
            strategy,
            Vec::new(),
        )
    }

    pub async fn from_config(
        config: &CdnConfig,
        derived_url_prefixes: Vec<String>,
    ) -> Result<Self> {
        match config {
            CdnConfig::CloudFront(config) => Ok(Self::with_repository(
                Arc::new(CdnPurgeRepository::CloudFront(
                    CloudFrontCdnRepository::from_distribution_id_env(&config.distribution_id_env)
                        .await?,
                )),
                config.strategy.clone(),
                Vec::new(),
            )),
            CdnConfig::Cloudflare(config) => {
                let url_prefixes = if config.url_prefixes.is_empty() {
                    derived_url_prefixes
                } else {
                    config.url_prefixes.clone()
                };
                if config.strategy == CdnRefreshStrategy::ChangedPaths && url_prefixes.is_empty() {
                    return Err(anyhow!(
                        "Cloudflare changed_paths CDN refresh requires url_prefixes or at least one exact host for the origin"
                    ));
                }
                Ok(Self::with_repository(
                    Arc::new(CdnPurgeRepository::Cloudflare(
                        CloudflareCdnRepository::from_config(config)?,
                    )),
                    config.strategy.clone(),
                    url_prefixes,
                ))
            }
            CdnConfig::CloudFrontSaas(_) => {
                Err(anyhow!("CloudFront SaaS CDN refresh is not implemented"))
            }
        }
    }

    pub async fn refresh_after_activation(
        &self,
        origin_id: &str,
        generation: u64,
        diff: &OriginFreshnessDiff,
    ) -> Result<Option<CdnRefreshOutcome>> {
        let Some(plan) = build_cdn_refresh_plan(&self.strategy, diff, &self.url_prefixes) else {
            return Ok(None);
        };
        let changed_count = plan.changed_count;
        let result = self
            .repository
            .purge(CdnPurgeRequest {
                origin_id: origin_id.to_string(),
                generation,
                mode: purge_mode_from_refresh_mode(plan.mode),
            })
            .await
            .map_err(translate_purge_error)?;

        Ok(Some(CdnRefreshOutcome {
            provider: result.provider,
            request_ids: result.request_ids,
            status: result.status,
            submitted_items: result.submitted_items,
            changed_count,
        }))
    }
}

fn translate_purge_error(error: anyhow::Error) -> anyhow::Error {
    match error.downcast::<CdnPurgeFailure>() {
        Ok(failure) => {
            let message = failure.to_string();
            anyhow::Error::new(CdnRefreshFailure {
                provider: failure.provider,
                request_ids: failure.request_ids,
                submitted_items: failure.submitted_items,
                message,
            })
        }
        Err(error) => error,
    }
}

pub fn build_cdn_refresh_plan(
    strategy: &CdnRefreshStrategy,
    diff: &OriginFreshnessDiff,
    url_prefixes: &[String],
) -> Option<CdnRefreshPlan> {
    let paths = changed_paths(diff);
    if paths.is_empty() {
        return None;
    }

    match strategy {
        CdnRefreshStrategy::All => Some(CdnRefreshPlan {
            mode: CdnRefreshMode::All,
            changed_count: paths.len(),
        }),
        CdnRefreshStrategy::ChangedPaths if url_prefixes.is_empty() => Some(CdnRefreshPlan {
            mode: CdnRefreshMode::Paths(paths),
            changed_count: diff_changed_count(diff),
        }),
        CdnRefreshStrategy::ChangedPaths => {
            let urls = url_prefixes
                .iter()
                .flat_map(|prefix| paths.iter().map(|path| format_url(prefix, path)))
                .collect();
            Some(CdnRefreshPlan {
                mode: CdnRefreshMode::Urls(urls),
                changed_count: diff_changed_count(diff),
            })
        }
    }
}

fn purge_mode_from_refresh_mode(mode: CdnRefreshMode) -> CdnPurgeMode {
    match mode {
        CdnRefreshMode::All => CdnPurgeMode::All,
        CdnRefreshMode::Paths(paths) => CdnPurgeMode::Paths(paths),
        CdnRefreshMode::Urls(urls) => CdnPurgeMode::Urls(urls),
    }
}

fn changed_paths(diff: &OriginFreshnessDiff) -> Vec<String> {
    let paths = diff
        .added
        .iter()
        .chain(diff.modified.iter())
        .chain(diff.removed.iter())
        .map(|path| format_cdn_path(path))
        .collect::<BTreeSet<_>>();

    paths.into_iter().collect()
}

fn diff_changed_count(diff: &OriginFreshnessDiff) -> usize {
    diff.added.len() + diff.modified.len() + diff.removed.len()
}

fn format_cdn_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn format_url(prefix: &str, path: &str) -> String {
    format!("{}{}", prefix.trim_end_matches('/'), format_cdn_path(path))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use crate::{
        dto::manifest::CdnRefreshStrategy,
        repositories::cdn::{CdnPurge, CdnPurgeFailure, CdnPurgeRequest, CdnPurgeResult},
        services::{
            cdn_refresh::{
                build_cdn_refresh_plan, CdnRefreshFailure, CdnRefreshMode, OriginCdnRefresh,
            },
            freshness::OriginFreshnessDiff,
        },
    };
    use anyhow::anyhow;
    use async_trait::async_trait;

    struct PartialFanoutPurge;

    #[async_trait]
    impl CdnPurge for PartialFanoutPurge {
        async fn purge(&self, _request: CdnPurgeRequest) -> anyhow::Result<CdnPurgeResult> {
            Err(anyhow!(CdnPurgeFailure {
                provider: "cloudfront_saas".to_string(),
                request_ids: vec!["INV-1".to_string()],
                submitted_items: 2,
                message: "invalidate tenant-b: service unavailable".to_string(),
            }))
        }
    }

    #[tokio::test]
    async fn repository_partial_failure_maps_to_service_owned_error() {
        let refresh = OriginCdnRefresh {
            repository: Arc::new(PartialFanoutPurge),
            strategy: CdnRefreshStrategy::ChangedPaths,
            url_prefixes: Vec::new(),
        };
        let diff = OriginFreshnessDiff {
            added: BTreeSet::from(["index.html".to_string(), "app.js".to_string()]),
            ..OriginFreshnessDiff::default()
        };

        let error = refresh
            .refresh_after_activation("web", 7, &diff)
            .await
            .expect_err("partial fanout must remain a refresh failure");
        let failure = error
            .downcast_ref::<CdnRefreshFailure>()
            .expect("repository failure is translated at the service boundary");

        assert_eq!(failure.provider, "cloudfront_saas");
        assert_eq!(failure.request_ids, ["INV-1"]);
        assert_eq!(failure.submitted_items, 2);
        assert!(failure.to_string().contains("tenant-b"));
    }

    #[test]
    fn changed_paths_strategy_collects_added_modified_and_removed_paths() {
        let diff = OriginFreshnessDiff {
            added: BTreeSet::from(["index.html".to_string()]),
            modified: BTreeSet::from(["docs/index.html".to_string()]),
            removed: BTreeSet::from(["old.html".to_string()]),
            unchanged: BTreeSet::from(["same.css".to_string()]),
        };

        let plan = build_cdn_refresh_plan(&CdnRefreshStrategy::ChangedPaths, &diff, &[])
            .expect("plan is created");

        assert_eq!(
            plan.mode,
            CdnRefreshMode::Paths(vec![
                "/docs/index.html".to_string(),
                "/index.html".to_string(),
                "/old.html".to_string(),
            ])
        );
        assert_eq!(plan.changed_count, 3);
    }

    #[test]
    fn changed_paths_strategy_can_build_urls_for_cloudflare() {
        let diff = OriginFreshnessDiff {
            added: BTreeSet::from(["index.html".to_string()]),
            ..OriginFreshnessDiff::default()
        };

        let plan = build_cdn_refresh_plan(
            &CdnRefreshStrategy::ChangedPaths,
            &diff,
            &[
                "https://docs.test".to_string(),
                "https://www.docs.test/".to_string(),
            ],
        )
        .expect("plan is created");

        assert_eq!(
            plan.mode,
            CdnRefreshMode::Urls(vec![
                "https://docs.test/index.html".to_string(),
                "https://www.docs.test/index.html".to_string(),
            ])
        );
    }

    #[test]
    fn all_strategy_skips_when_there_are_no_changes() {
        let plan = build_cdn_refresh_plan(
            &CdnRefreshStrategy::All,
            &OriginFreshnessDiff::default(),
            &[],
        );

        assert_eq!(plan, None);
    }
}
