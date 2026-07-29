use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::Result;
use tracing::Instrument;

use crate::{
    dto::origin_sync::{OriginSyncCdnResponse, OriginSyncResponse},
    repositories::{
        local_mirror::LocalMirrorRepository, origin_storage::OriginStorageRepository,
        sync::MirrorSyncService, sync::RemoteStorage,
    },
    services::{
        cdn_refresh::OriginCdnRefresh,
        edge_config::{default_edge_config, parse_edge_config},
        edge_config_store::EdgeConfigStore,
        freshness::OriginFreshnessIndex,
        origin_runtime::{OriginRuntimeStore, OriginSnapshotDebug},
        template_store::TemplateStore,
    },
};

struct EdgeConfigPath {
    path: &'static str,
    deprecated: bool,
}

const EDGE_CONFIG_PATHS: [EdgeConfigPath; 6] = [
    EdgeConfigPath {
        path: "/.rendermesh/edge.yaml",
        deprecated: false,
    },
    EdgeConfigPath {
        path: "/.rendermesh/edge.yml",
        deprecated: false,
    },
    EdgeConfigPath {
        path: "/.rendermesh/edge.json",
        deprecated: false,
    },
    EdgeConfigPath {
        path: "/_rendermesh/edge.yaml",
        deprecated: true,
    },
    EdgeConfigPath {
        path: "/_rendermesh/edge.yml",
        deprecated: true,
    },
    EdgeConfigPath {
        path: "/_rendermesh/edge.json",
        deprecated: true,
    },
];

pub(crate) type OriginFreshnessIndexes = Arc<RwLock<BTreeMap<String, OriginFreshnessIndex>>>;

#[derive(Clone)]
pub struct OriginRefreshService {
    syncer: MirrorSyncService,
    edge_configs: EdgeConfigStore,
    template_store: TemplateStore,
    freshness_indexes: OriginFreshnessIndexes,
    origin_runtime: OriginRuntimeStore,
    storage_by_origin: Arc<BTreeMap<String, OriginStorageRepository>>,
    activation_barrier_by_origin: Arc<BTreeMap<String, String>>,
    cdn_by_origin: Arc<BTreeMap<String, OriginCdnRefresh>>,
    locks: OriginRefreshLocks,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginRefreshTrigger {
    Startup,
    Background,
    Manual,
}

#[derive(Debug)]
pub enum OriginRefreshError {
    NotFound,
    AlreadyRunning,
    Failed(anyhow::Error),
}

impl fmt::Display for OriginRefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("origin not found"),
            Self::AlreadyRunning => formatter.write_str("origin sync already running"),
            Self::Failed(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for OriginRefreshError {}

#[derive(Clone, Default)]
struct OriginRefreshLocks {
    active: Arc<Mutex<BTreeSet<String>>>,
}

struct OriginRefreshGuard {
    origin_id: String,
    locks: OriginRefreshLocks,
}

impl OriginRefreshLocks {
    fn try_acquire(&self, origin_id: &str) -> Option<OriginRefreshGuard> {
        let mut active = self.active.lock().expect("origin refresh lock");
        if active.contains(origin_id) {
            return None;
        }
        active.insert(origin_id.to_string());
        Some(OriginRefreshGuard {
            origin_id: origin_id.to_string(),
            locks: self.clone(),
        })
    }
}

impl Drop for OriginRefreshGuard {
    fn drop(&mut self) {
        self.locks
            .active
            .lock()
            .expect("origin refresh lock")
            .remove(&self.origin_id);
    }
}

impl OriginRefreshService {
    pub fn new(
        syncer: MirrorSyncService,
        edge_configs: EdgeConfigStore,
        template_store: TemplateStore,
        freshness_indexes: OriginFreshnessIndexes,
        origin_runtime: OriginRuntimeStore,
        storage_by_origin: BTreeMap<String, OriginStorageRepository>,
        activation_barrier_by_origin: BTreeMap<String, String>,
        cdn_by_origin: BTreeMap<String, OriginCdnRefresh>,
    ) -> Self {
        Self {
            syncer,
            edge_configs,
            template_store,
            freshness_indexes,
            origin_runtime,
            storage_by_origin: Arc::new(storage_by_origin),
            activation_barrier_by_origin: Arc::new(activation_barrier_by_origin),
            cdn_by_origin: Arc::new(cdn_by_origin),
            locks: OriginRefreshLocks::default(),
        }
    }

    pub fn origin_runtime(&self) -> OriginRuntimeStore {
        self.origin_runtime.clone()
    }

    pub fn edge_configs(&self) -> EdgeConfigStore {
        self.edge_configs.clone()
    }

    pub fn template_store(&self) -> TemplateStore {
        self.template_store.clone()
    }

    pub fn has_origin(&self, origin_id: &str) -> bool {
        self.storage_by_origin.contains_key(origin_id)
    }

    pub async fn refresh_origin(
        &self,
        origin_id: &str,
        trigger: OriginRefreshTrigger,
    ) -> Result<OriginSyncResponse, OriginRefreshError> {
        let storage = self
            .storage_by_origin
            .get(origin_id)
            .ok_or(OriginRefreshError::NotFound)?;
        let _guard = self
            .locks
            .try_acquire(origin_id)
            .ok_or(OriginRefreshError::AlreadyRunning)?;
        let cdn_refresh = self.cdn_by_origin.get(origin_id);
        let span = tracing::info_span!(
            "rendermesh.origin_sync",
            origin = %origin_id,
            trigger = ?trigger,
            generation = tracing::field::Empty,
            added_files = tracing::field::Empty,
            modified_files = tracing::field::Empty,
            removed_files = tracing::field::Empty,
            downloaded_files = tracing::field::Empty,
            result = tracing::field::Empty,
        );

        async {
            tracing::info!(origin = %origin_id, trigger = ?trigger, "origin sync started");
            let outcome = refresh_origin_snapshot(
                origin_id,
                &self.syncer,
                storage,
                cdn_refresh,
                &self.edge_configs,
                &self.template_store,
                &self.freshness_indexes,
                &self.origin_runtime,
                self.activation_barrier_by_origin
                    .get(origin_id)
                    .map(String::as_str),
            )
            .await
            .map_err(OriginRefreshError::Failed)?;

            span.record("generation", outcome.generation);
            span.record("added_files", outcome.added_files);
            span.record("modified_files", outcome.modified_files);
            span.record("removed_files", outcome.removed_files);
            span.record("downloaded_files", outcome.downloaded_files);
            span.record("result", "activated");
            tracing::info!(
                origin = %origin_id,
                trigger = ?trigger,
                generation = outcome.generation,
                downloaded = outcome.downloaded_files,
                "origin sync completed"
            );
            Ok(outcome)
        }
        .instrument(span.clone())
        .await
        .inspect_err(|error| {
            span.record("result", "failed");
            if let OriginRefreshError::Failed(error) = error {
                self.origin_runtime.set_error(origin_id, error.to_string());
            }
        })
    }
}

pub(crate) async fn refresh_origin_snapshot<S>(
    origin_id: &str,
    syncer: &MirrorSyncService,
    storage: &S,
    cdn_refresh: Option<&OriginCdnRefresh>,
    edge_configs: &EdgeConfigStore,
    template_store: &TemplateStore,
    freshness_indexes: &OriginFreshnessIndexes,
    origin_runtime: &OriginRuntimeStore,
    activation_barrier_path: Option<&str>,
) -> Result<OriginSyncResponse>
where
    S: RemoteStorage,
{
    let previous_index = freshness_indexes
        .read()
        .expect("freshness index lock")
        .get(origin_id)
        .cloned();
    let staged = syncer
        .stage_origin_sync(origin_id, storage, previous_index.as_ref())
        .await?;
    let preparation = async {
        validate_activation_barrier(
            origin_id,
            previous_index.as_ref(),
            &staged.index,
            &staged.diff,
            activation_barrier_path,
        )?;
        let (stage_mirror, stage_origin_id) = staged_origin_mirror(&staged.staging_dir)?;
        let edge_config = load_origin_edge_config(&stage_origin_id, &stage_mirror).await?;
        let template_registry = template_store
            .compile_template_update_from_mirror(
                origin_id,
                &stage_origin_id,
                &stage_mirror,
                &staged.diff,
            )
            .await?;
        Ok::<_, anyhow::Error>((edge_config, template_registry))
    }
    .await;
    let (edge_config, template_registry) = match preparation {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Err(cleanup_error) = syncer.discard_staged_origin(staged).await {
                return Err(error.context(format!(
                    "failed to discard rejected staged origin: {cleanup_error}"
                )));
            }
            return Err(error);
        }
    };
    let next_index = staged.index.clone();
    let diff = staged.diff.clone();
    let report = staged.report.clone();
    let next_generation = origin_runtime
        .get(origin_id)
        .map(|snapshot| snapshot.generation + 1)
        .unwrap_or(1);
    let activated_at = chrono::Utc::now().to_rfc3339();
    let captured_at = next_index.captured_at.to_rfc3339();
    let snapshot = OriginSnapshotDebug {
        origin_id: origin_id.to_string(),
        generation: next_generation,
        activated_at: activated_at.clone(),
        captured_at: captured_at.clone(),
        known_files: next_index.files.len(),
        added_files: staged.diff.added.len(),
        modified_files: staged.diff.modified.len(),
        removed_files: staged.diff.removed.len(),
        unchanged_files: staged.diff.unchanged.len(),
        downloaded_files: report.downloaded,
        last_error: None,
        last_cdn_provider: None,
        last_cdn_status: None,
        last_cdn_request_id: None,
        last_cdn_refreshed_at: None,
        last_cdn_submitted_items: None,
        last_cdn_error: None,
        last_cdn_domain_provider: None,
        last_cdn_domain_status: None,
        last_cdn_domain_reconciled_at: None,
        last_cdn_domain_added: None,
        last_cdn_domain_updated: None,
        last_cdn_domain_removed: None,
        last_cdn_domain_unchanged: None,
        last_cdn_domain_error: None,
    };
    tracing::info!(
        origin = %origin_id,
        generation = next_generation,
        listed_files = staged.index.files.len(),
        added_files = staged.diff.added.len(),
        modified_files = staged.diff.modified.len(),
        removed_files = staged.diff.removed.len(),
        unchanged_files = staged.diff.unchanged.len(),
        downloaded = report.downloaded,
        "origin freshness refresh staged"
    );

    syncer.activate_staged_origin(staged).await?;
    edge_configs.set_valid(origin_id, edge_config);
    template_store.set_origin_registry(origin_id, template_registry);
    freshness_indexes
        .write()
        .expect("freshness index lock")
        .insert(origin_id.to_string(), next_index);
    origin_runtime.set_snapshot(snapshot.clone());
    let cdn = if let Some(cdn_refresh) = cdn_refresh {
        match cdn_refresh
            .refresh_after_activation(origin_id, next_generation, &diff)
            .await
        {
            Ok(Some(outcome)) => {
                tracing::info!(
                    origin = %origin_id,
                    generation = next_generation,
                    provider = %outcome.provider,
                    status = %outcome.status,
                    submitted_items = outcome.submitted_items,
                    changed_count = outcome.changed_count,
                    "cdn refresh submitted"
                );
                let cdn = OriginSyncCdnResponse {
                    provider: outcome.provider.clone(),
                    status: outcome.status.clone(),
                    request_id: outcome.request_id.clone(),
                    submitted_items: outcome.submitted_items,
                };
                origin_runtime.set_cdn_result(
                    origin_id,
                    outcome.provider,
                    outcome.status,
                    outcome.request_id,
                    outcome.submitted_items,
                );
                Some(cdn)
            }
            Ok(None) => {
                tracing::debug!(
                    origin = %origin_id,
                    generation = next_generation,
                    "cdn refresh skipped because origin has no changes"
                );
                None
            }
            Err(error) => {
                origin_runtime.set_cdn_error(origin_id, error.to_string());
                tracing::error!(
                    origin = %origin_id,
                    generation = next_generation,
                    "cdn refresh failed after origin activation: {error}"
                );
                None
            }
        }
    } else {
        None
    };
    tracing::info!(origin = %origin_id, generation = next_generation, "origin freshness refresh activated");

    Ok(OriginSyncResponse {
        origin_id: snapshot.origin_id,
        generation: snapshot.generation,
        activated_at,
        captured_at,
        known_files: snapshot.known_files,
        added_files: snapshot.added_files,
        modified_files: snapshot.modified_files,
        removed_files: snapshot.removed_files,
        unchanged_files: snapshot.unchanged_files,
        downloaded_files: snapshot.downloaded_files,
        cdn,
    })
}

fn validate_activation_barrier(
    origin_id: &str,
    previous_index: Option<&OriginFreshnessIndex>,
    next_index: &OriginFreshnessIndex,
    diff: &crate::services::freshness::OriginFreshnessDiff,
    activation_barrier_path: Option<&str>,
) -> Result<()> {
    let Some(path) = activation_barrier_path else {
        return Ok(());
    };
    let has_changes =
        !diff.added.is_empty() || !diff.modified.is_empty() || !diff.removed.is_empty();
    if !has_changes {
        return Ok(());
    }

    let previous_has_barrier = previous_index.is_some_and(|index| index.files.contains_key(path));
    let next_has_barrier = next_index.files.contains_key(path);
    let barrier_changed = diff.added.contains(path) || diff.modified.contains(path);

    if !next_has_barrier {
        return Err(anyhow::anyhow!(
            "origin {origin_id} activation barrier {path} is missing"
        ));
    }
    if previous_has_barrier && !barrier_changed {
        return Err(anyhow::anyhow!(
            "origin {origin_id} activation barrier {path} did not change with the deployment"
        ));
    }

    Ok(())
}

#[cfg(test)]
mod activation_barrier_tests {
    use chrono::{TimeZone, Utc};

    use super::validate_activation_barrier;
    use crate::{
        repositories::sync::RemoteObjectSummary,
        services::freshness::{build_origin_index, diff_origin_indexes},
    };

    const BARRIER: &str = ".rendermesh/edge.yaml";

    #[test]
    fn rejects_content_changes_when_existing_barrier_did_not_change() {
        let previous = index("edge-v1", "index-v1");
        let next = index("edge-v1", "index-v2");
        let diff = diff_origin_indexes(Some(&previous), &next);

        let error =
            validate_activation_barrier("web", Some(&previous), &next, &diff, Some(BARRIER))
                .expect_err("unchanged barrier must block activation");

        assert!(error.to_string().contains("did not change"));
    }

    #[test]
    fn accepts_content_changes_with_a_new_valid_barrier_generation() {
        let previous = index("edge-v1", "index-v1");
        let next = index("edge-v2", "index-v2");
        let diff = diff_origin_indexes(Some(&previous), &next);

        validate_activation_barrier("web", Some(&previous), &next, &diff, Some(BARRIER))
            .expect("changed barrier allows activation");
    }

    #[test]
    fn accepts_the_first_generation_when_it_contains_the_barrier() {
        let next = index("edge-v1", "index-v1");
        let diff = diff_origin_indexes(None, &next);

        validate_activation_barrier("web", None, &next, &diff, Some(BARRIER))
            .expect("first complete generation allows activation");
    }

    fn index(
        edge_etag: &str,
        index_etag: &str,
    ) -> crate::services::freshness::OriginFreshnessIndex {
        build_origin_index(
            "web",
            vec![
                summary(BARRIER, edge_etag),
                summary("index.html", index_etag),
            ],
            Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap(),
        )
        .expect("index")
    }

    fn summary(key: &str, etag: &str) -> RemoteObjectSummary {
        RemoteObjectSummary {
            key: key.to_string(),
            created_at: None,
            etag: Some(etag.to_string()),
            last_modified: None,
            size: 1,
            content_type: None,
            cache_control: None,
        }
    }
}

fn staged_origin_mirror(staging_dir: &Path) -> Result<(LocalMirrorRepository, String)> {
    let root = staging_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("staging dir has no parent"))?;
    let origin_id = staging_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow::anyhow!("staging dir has invalid origin id"))?;
    Ok((LocalMirrorRepository::new(root), origin_id.to_string()))
}

pub(crate) async fn load_origin_edge_config(
    origin_id: &str,
    mirror: &LocalMirrorRepository,
) -> Result<crate::dto::edge::EdgeConfig> {
    for candidate in EDGE_CONFIG_PATHS {
        if let Some(object) = mirror.read_object(origin_id, candidate.path).await? {
            if candidate.deprecated {
                tracing::warn!(
                    origin = %origin_id,
                    path = %candidate.path,
                    "legacy /_rendermesh edge config path is deprecated; migrate origin config to /.rendermesh"
                );
            }
            let content = String::from_utf8(object.body.to_vec())?;
            return Ok(parse_edge_config(&content)?);
        }
    }

    tracing::warn!(
        origin = %origin_id,
        "origin has no edge config file; using default edge config"
    );
    Ok(default_edge_config())
}

#[cfg(test)]
pub(crate) async fn load_edge_configs<I>(
    origin_ids: I,
    mirror: &LocalMirrorRepository,
) -> EdgeConfigStore
where
    I: IntoIterator<Item = String>,
{
    let store = EdgeConfigStore::from_configs(BTreeMap::new());
    for origin_id in origin_ids {
        match load_origin_edge_config(&origin_id, mirror).await {
            Ok(config) => store.set_valid(&origin_id, config),
            Err(error) => {
                tracing::error!(origin = %origin_id, "failed to load edge config: {error}");
                store.set_invalid(&origin_id, error.to_string());
            }
        }
    }
    store
}

#[cfg(test)]
pub(crate) async fn sync_origin_and_refresh_edge_config<S>(
    origin_id: &str,
    syncer: &MirrorSyncService,
    storage: &S,
    cdn_refresh: Option<&OriginCdnRefresh>,
    _mirror: &LocalMirrorRepository,
    edge_configs: &EdgeConfigStore,
    template_store: &TemplateStore,
) -> Result<()>
where
    S: RemoteStorage,
{
    let freshness_indexes = OriginFreshnessIndexes::default();
    let origin_runtime = OriginRuntimeStore::default();
    refresh_origin_snapshot(
        origin_id,
        syncer,
        storage,
        cdn_refresh,
        edge_configs,
        template_store,
        &freshness_indexes,
        &origin_runtime,
        None,
    )
    .await?;
    Ok(())
}
