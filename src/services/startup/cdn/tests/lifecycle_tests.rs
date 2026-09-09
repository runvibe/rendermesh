use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    dto::manifest::CdnRefreshStrategy,
    repositories::cdn::{
        CdnDomainReconcileResult, CdnPurge, CdnPurgeMode, CdnPurgeRequest, CdnPurgeResult,
        CdnTenantReconcile, CdnTenantReconcileRequest,
    },
    services::{
        cdn_refresh::OriginCdnRefresh, cdn_tenants::OriginCdnTenants,
        freshness::OriginFreshnessDiff, manifest::parse_manifest_yaml,
    },
};

#[tokio::test]
async fn initial_skip_then_reconciliation_enables_later_tenant_invalidation() {
    let repository = Arc::new(LifecycleRepository::default());
    let refresh = OriginCdnRefresh::with_repository(
        repository.clone(),
        CdnRefreshStrategy::ChangedPaths,
        Vec::new(),
    );
    let tenants = OriginCdnTenants::new(repository.clone());
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  app:
    type: local
    path: ./app
hosts:
  app.example.com:
    origin: app
"#,
    )
    .expect("manifest parses");

    let initial = refresh
        .refresh_after_activation("app", 1, &diff_with_added("index.html"))
        .await
        .expect("initial refresh succeeds")
        .expect("initial refresh runs");
    assert_eq!(initial.status, "skipped_no_tenants");
    assert_eq!(initial.submitted_items, 0);

    let reconciled = tenants
        .reconcile(&manifest, "app")
        .await
        .expect("tenant reconciliation succeeds");
    assert_eq!(reconciled.added, 1);

    let later = refresh
        .refresh_after_activation("app", 2, &diff_with_added("app.js"))
        .await
        .expect("later refresh succeeds")
        .expect("later refresh runs");
    assert_eq!(later.status, "submitted");
    assert_eq!(later.request_ids, ["invalidation-2"]);
    assert_eq!(later.submitted_items, 1);
    assert_eq!(
        repository
            .state
            .lock()
            .expect("lifecycle repository lock")
            .purge_generations,
        [1, 2]
    );
}

#[derive(Default)]
struct LifecycleRepository {
    state: Mutex<LifecycleState>,
}

#[derive(Default)]
struct LifecycleState {
    tenant_ready: bool,
    purge_generations: Vec<u64>,
}

#[async_trait]
impl CdnPurge for LifecycleRepository {
    async fn purge(&self, request: CdnPurgeRequest) -> Result<CdnPurgeResult> {
        let path_count = match request.mode {
            CdnPurgeMode::Paths(paths) => paths.len(),
            mode => panic!("expected path invalidation, got {mode:?}"),
        };
        let mut state = self.state.lock().expect("lifecycle repository lock");
        state.purge_generations.push(request.generation);
        if !state.tenant_ready {
            return Ok(CdnPurgeResult {
                provider: "cloudfront_saas".to_string(),
                request_ids: Vec::new(),
                status: "skipped_no_tenants".to_string(),
                submitted_items: 0,
            });
        }

        Ok(CdnPurgeResult {
            provider: "cloudfront_saas".to_string(),
            request_ids: vec![format!("invalidation-{}", request.generation)],
            status: "submitted".to_string(),
            submitted_items: path_count,
        })
    }
}

#[async_trait]
impl CdnTenantReconcile for LifecycleRepository {
    async fn reconcile_tenants(
        &self,
        request: CdnTenantReconcileRequest,
    ) -> Result<CdnDomainReconcileResult> {
        assert_eq!(
            request.desired_domains,
            BTreeSet::from(["app.example.com".to_string()])
        );
        self.state
            .lock()
            .expect("lifecycle repository lock")
            .tenant_ready = true;
        Ok(CdnDomainReconcileResult {
            provider: "cloudfront_saas".to_string(),
            status: "submitted".to_string(),
            added: 1,
            updated: 0,
            removed: 0,
            unchanged: 0,
        })
    }
}

fn diff_with_added(path: &str) -> OriginFreshnessDiff {
    OriginFreshnessDiff {
        added: BTreeSet::from([path.to_string()]),
        modified: BTreeSet::new(),
        removed: BTreeSet::new(),
        unchanged: BTreeSet::new(),
    }
}
