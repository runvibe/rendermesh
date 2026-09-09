use std::{collections::BTreeSet, sync::Arc};

use anyhow::Result;

use crate::{
    dto::manifest::RenderMeshManifest,
    repositories::cdn::{CdnTenantReconcile, CdnTenantReconcileRequest},
    services::{cdn_domains::CdnDomainReconcileOutcome, manifest::normalize_host},
};

#[derive(Clone)]
pub struct OriginCdnTenants {
    repository: Arc<dyn CdnTenantReconcile>,
}

impl OriginCdnTenants {
    pub fn new(repository: Arc<dyn CdnTenantReconcile>) -> Self {
        Self { repository }
    }

    pub async fn reconcile(
        &self,
        manifest: &RenderMeshManifest,
        origin_id: &str,
    ) -> Result<CdnDomainReconcileOutcome> {
        let result = self
            .repository
            .reconcile_tenants(CdnTenantReconcileRequest {
                origin_id: origin_id.to_string(),
                desired_domains: exact_hosts_for_origin(manifest, origin_id),
            })
            .await?;

        Ok(CdnDomainReconcileOutcome {
            provider: result.provider,
            status: result.status,
            added: result.added,
            updated: result.updated,
            removed: result.removed,
            unchanged: result.unchanged,
        })
    }
}

pub fn exact_hosts_for_origin(manifest: &RenderMeshManifest, origin_id: &str) -> BTreeSet<String> {
    manifest
        .hosts
        .iter()
        .filter(|(host, config)| config.origin == origin_id && !host.trim().starts_with('*'))
        .filter_map(|(host, _)| normalize_host(host))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };

    use anyhow::Result;
    use async_trait::async_trait;

    use crate::{
        repositories::cdn::{
            CdnDomainReconcileResult, CdnTenantReconcile, CdnTenantReconcileRequest,
        },
        services::{
            cdn_tenants::{exact_hosts_for_origin, OriginCdnTenants},
            manifest::parse_manifest_yaml,
        },
    };

    #[test]
    fn exact_hosts_for_origin_only_returns_normalized_exact_hosts_for_origin() {
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
  shared:
    type: local
    path: ./shared
hosts:
  APP.EXAMPLE.COM:
    origin: app
  "*.example.com":
    origin: app
  "*":
    origin: app
  SHARED.EXAMPLE.COM:
    origin: shared
"#,
        )
        .expect("manifest parses");

        assert_eq!(
            exact_hosts_for_origin(&manifest, "app"),
            BTreeSet::from(["app.example.com".to_string()])
        );
    }

    #[tokio::test]
    async fn reconcile_forwards_exact_hosts_and_maps_result_counts() {
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
  shared:
    type: local
    path: ./shared
hosts:
  APP.EXAMPLE.COM:
    origin: app
  "*.example.com":
    origin: app
  "*":
    origin: app
  SHARED.EXAMPLE.COM:
    origin: shared
"#,
        )
        .expect("manifest parses");

        let repository = Arc::new(RecordingTenantReconcile::new(CdnDomainReconcileResult {
            provider: "cloudfront_saas".to_string(),
            status: "submitted".to_string(),
            added: 1,
            updated: 2,
            removed: 3,
            unchanged: 4,
        }));
        let service = OriginCdnTenants::new(repository.clone());

        let outcome = service
            .reconcile(&manifest, "app")
            .await
            .expect("reconciles");

        assert_eq!(outcome.provider, "cloudfront_saas");
        assert_eq!(outcome.status, "submitted");
        assert_eq!(outcome.added, 1);
        assert_eq!(outcome.updated, 2);
        assert_eq!(outcome.removed, 3);
        assert_eq!(outcome.unchanged, 4);
        assert_eq!(
            repository.request.lock().expect("request lock").clone(),
            Some(CdnTenantReconcileRequest {
                origin_id: "app".to_string(),
                desired_domains: BTreeSet::from(["app.example.com".to_string()]),
            })
        );
    }

    #[derive(Clone)]
    struct RecordingTenantReconcile {
        request: Arc<Mutex<Option<CdnTenantReconcileRequest>>>,
        response: CdnDomainReconcileResult,
    }

    impl RecordingTenantReconcile {
        fn new(response: CdnDomainReconcileResult) -> Self {
            Self {
                request: Arc::new(Mutex::new(None)),
                response,
            }
        }
    }

    #[async_trait]
    impl CdnTenantReconcile for RecordingTenantReconcile {
        async fn reconcile_tenants(
            &self,
            request: CdnTenantReconcileRequest,
        ) -> Result<CdnDomainReconcileResult> {
            *self.request.lock().expect("request lock") = Some(request);
            Ok(self.response.clone())
        }
    }
}
