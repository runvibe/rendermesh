use std::collections::BTreeSet;

use anyhow::{anyhow, Result};

use crate::{
    dto::manifest::{CdnConfig, CdnRefreshStrategy},
    repositories::{
        cdn::{CdnPurge, CdnPurgeMode, CdnPurgeRepository, CdnPurgeRequest},
        cloudflare_cdn::CloudflareCdnRepository,
        cloudfront_cdn::CloudFrontCdnRepository,
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
    repository: CdnPurgeRepository,
    strategy: CdnRefreshStrategy,
    url_prefixes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdnRefreshOutcome {
    pub provider: String,
    pub request_id: Option<String>,
    pub status: String,
    pub submitted_items: usize,
    pub changed_count: usize,
}

impl OriginCdnRefresh {
    pub async fn from_config(
        config: &CdnConfig,
        derived_url_prefixes: Vec<String>,
    ) -> Result<Self> {
        match config {
            CdnConfig::CloudFront(config) => Ok(Self {
                repository: CdnPurgeRepository::CloudFront(
                    CloudFrontCdnRepository::from_distribution_id_env(&config.distribution_id_env)
                        .await?,
                ),
                strategy: config.strategy.clone(),
                url_prefixes: Vec::new(),
            }),
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
                Ok(Self {
                    repository: CdnPurgeRepository::Cloudflare(
                        CloudflareCdnRepository::from_config(config)?,
                    ),
                    strategy: config.strategy.clone(),
                    url_prefixes,
                })
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
            .await?;

        Ok(Some(CdnRefreshOutcome {
            provider: result.provider,
            request_id: result.request_id,
            status: result.status,
            submitted_items: result.submitted_items,
            changed_count,
        }))
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
    use std::collections::BTreeSet;

    use crate::{
        dto::manifest::CdnRefreshStrategy,
        services::{
            cdn_refresh::{build_cdn_refresh_plan, CdnRefreshMode},
            freshness::OriginFreshnessDiff,
        },
    };

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
