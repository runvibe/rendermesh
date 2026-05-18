use std::{collections::BTreeMap, sync::Arc};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use bytes::Bytes;

use crate::{
    dto::{
        edge::EdgeConfig,
        render::{RenderRequest, RenderResponse},
    },
    repositories::local_mirror::LocalMirrorRepository,
    services::{cors::CorsPolicy, manifest::HostResolver, static_rules::resolve_root_object},
};

#[derive(Clone)]
pub struct RenderGatewayService {
    resolver: Arc<HostResolver>,
    cors: Arc<CorsPolicy>,
    mirror: LocalMirrorRepository,
    edge_configs: Arc<BTreeMap<String, EdgeConfig>>,
}

impl RenderGatewayService {
    pub fn new(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: BTreeMap<String, EdgeConfig>,
    ) -> Self {
        Self {
            resolver: Arc::new(resolver),
            cors: Arc::new(cors),
            mirror,
            edge_configs: Arc::new(edge_configs),
        }
    }

    pub fn new_for_tests(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: BTreeMap<String, EdgeConfig>,
    ) -> Self {
        Self::new(resolver, cors, mirror, edge_configs)
    }

    pub async fn handle(&self, request: RenderRequest) -> Result<RenderResponse> {
        let Some(resolved) = self.resolver.resolve(&request.host) else {
            return Ok(RenderResponse::empty(StatusCode::MISDIRECTED_REQUEST));
        };

        if request.method == Method::OPTIONS {
            return Ok(RenderResponse::empty(StatusCode::NO_CONTENT));
        }

        if request.method != Method::GET && request.method != Method::HEAD {
            return Ok(RenderResponse::empty(StatusCode::METHOD_NOT_ALLOWED));
        }

        let _cors = &self.cors;
        let config = self
            .edge_configs
            .get(&resolved.origin_id)
            .expect("edge config exists for resolved origin");
        let path = resolve_root_object(config, &request.path);

        if let Some(object) = self.mirror.read_object(&resolved.origin_id, &path).await? {
            let mut response = RenderResponse {
                status: StatusCode::OK,
                headers: BTreeMap::new(),
                body: if request.method == Method::HEAD {
                    Bytes::new()
                } else {
                    object.body
                },
            };

            if let Some(content_type) = object.metadata.content_type {
                response
                    .headers
                    .insert("content-type".to_string(), content_type);
            }

            return Ok(response);
        }

        Ok(RenderResponse {
            status: StatusCode::NOT_FOUND,
            headers: BTreeMap::new(),
            body: if request.method == Method::HEAD {
                Bytes::new()
            } else {
                Bytes::from_static(b"not found")
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        repositories::local_mirror::{metadata_sidecar_path, LocalMirrorRepository},
        services::{
            cors::CorsPolicy,
            edge_config::default_edge_config,
            manifest::{parse_manifest_yaml, HostResolver},
        },
    };
    use axum::http::{Method, StatusCode};

    #[tokio::test]
    async fn serves_get_from_local_mirror() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("index.html"), "<h1>Hello</h1>")
            .await
            .expect("write");
        let metadata_path =
            metadata_sidecar_path(&origin_dir, "index.html").expect("metadata path");
        tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(metadata_path, r#"{"content_type":"text/html"}"#)
            .await
            .expect("meta");
        let service = test_gateway(root);

        let response = service
            .handle(RenderRequest {
                method: Method::GET,
                host: "web.test".to_string(),
                path: "/".to_string(),
                query: None,
                scheme: "https".to_string(),
                headers: Default::default(),
                body: bytes::Bytes::new(),
            })
            .await
            .expect("response");

        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>Hello</h1>"));
        assert_eq!(
            response.headers.get("content-type").map(String::as_str),
            Some("text/html")
        );
    }

    #[tokio::test]
    async fn unknown_host_returns_421() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = test_gateway(temp.path().join("origins"));

        let response = service
            .handle(RenderRequest {
                method: Method::GET,
                host: "unknown.test".to_string(),
                path: "/".to_string(),
                query: None,
                scheme: "https".to_string(),
                headers: Default::default(),
                body: bytes::Bytes::new(),
            })
            .await
            .expect("response");

        assert_eq!(response.status, StatusCode::MISDIRECTED_REQUEST);
    }

    fn test_gateway(root: std::path::PathBuf) -> RenderGatewayService {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  web.test:
    origin: web
"#,
        )
        .expect("manifest");

        RenderGatewayService::new_for_tests(
            HostResolver::new(&manifest).expect("resolver"),
            CorsPolicy::from_manifest(&manifest),
            LocalMirrorRepository::new(root),
            [("web".to_string(), default_edge_config())].into(),
        )
    }
}
