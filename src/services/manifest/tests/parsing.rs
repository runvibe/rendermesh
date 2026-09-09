use super::*;
#[test]
fn parses_manifest_runtime_origins_and_hosts() {
    let manifest = parse_manifest_yaml(sample_manifest()).expect("manifest parses");

    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.runtime.local_store_dir, "./var/rendermesh/origins");
    assert_eq!(manifest.runtime.sync_interval_seconds, 60);
    match &manifest.origins["my_app"] {
        OriginConfig::S3(origin) => {
            assert_eq!(origin.bucket, "bucket_my_app_123");
            assert_eq!(origin.sync_interval_seconds, Some(30));
            assert_eq!(
                origin.activation_barrier_path.as_deref(),
                Some(".rendermesh/edge.yaml")
            );
            assert_eq!(
                origin.edge_context.as_ref(),
                Some(&serde_json::json!({
                    "tenant_id": "loja-123",
                    "feature_flags": {
                        "checkout_v2": true
                    }
                }))
            );
        }
        other => panic!("expected s3 origin, got {other:?}"),
    }
    assert_eq!(manifest.hosts["myapp.com"].origin, "my_app");
}

#[test]
fn parses_manifest_json() {
    let manifest = parse_manifest_config(
        r#"
{
  "version": 1,
  "runtime": {
    "local_store_dir": "./var/rendermesh/origins",
    "sync_interval_seconds": 60
  },
  "origins": {
    "my_app": {
      "type": "s3",
      "bucket": "bucket_my_app_123",
      "endpoint_env": "MY_APP_STORAGE_ENDPOINT",
      "region_env": "MY_APP_STORAGE_REGION",
      "access_key_id_env": "MY_APP_ACCESS_KEY_ID",
      "secret_access_key_env": "MY_APP_SECRET_ACCESS_KEY",
      "force_path_style_env": "MY_APP_FORCE_PATH_STYLE",
      "sync_interval_seconds": 30,
      "edge_context": {
        "tenant_id": "loja-123",
        "theme": "dark",
        "feature_flags": {
          "checkout_v2": true
        }
      }
    }
  },
  "hosts": {
    "myapp.com": {
      "origin": "my_app"
    },
    "*.myapp.com": {
      "origin": "my_app"
    }
  }
}
"#,
    )
    .expect("json manifest parses");

    assert_eq!(manifest.version, 1);
    match &manifest.origins["my_app"] {
        OriginConfig::S3(origin) => {
            assert_eq!(origin.bucket, "bucket_my_app_123");
            assert_eq!(
                origin.edge_context.as_ref(),
                Some(&serde_json::json!({
                    "tenant_id": "loja-123",
                    "theme": "dark",
                    "feature_flags": {
                        "checkout_v2": true
                    }
                }))
            );
        }
        other => panic!("expected s3 origin, got {other:?}"),
    }
    assert_eq!(manifest.hosts["*.myapp.com"].origin, "my_app");
}

#[test]
fn parses_local_origin_from_yaml() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  docs:
    type: local
    path: ./examples/local/bucket
    sync_interval_seconds: 5
    edge_context:
      app_name: docs
      audiences:
        - public
        - developers
hosts:
  docs.test:
    origin: docs
"#,
    )
    .expect("local manifest parses");

    match &manifest.origins["docs"] {
        crate::dto::manifest::OriginConfig::Local(origin) => {
            assert_eq!(origin.path, "./examples/local/bucket");
            assert_eq!(origin.sync_interval_seconds, Some(5));
            assert_eq!(
                origin.edge_context.as_ref(),
                Some(&serde_json::json!({
                    "app_name": "docs",
                    "audiences": ["public", "developers"]
                }))
            );
        }
        other => panic!("expected local origin, got {other:?}"),
    }
}

#[test]
fn parses_local_origin_from_json() {
    let manifest = parse_manifest_config(
        r#"
{
  "version": 1,
  "runtime": {
    "local_store_dir": "./var/rendermesh/origins",
    "sync_interval_seconds": 60
  },
  "origins": {
    "docs": {
      "type": "local",
      "path": "./examples/local/bucket",
      "sync_interval_seconds": 5
    }
  },
  "hosts": {
    "docs.test": {
      "origin": "docs"
    }
  }
}
"#,
    )
    .expect("local json manifest parses");

    match &manifest.origins["docs"] {
        crate::dto::manifest::OriginConfig::Local(origin) => {
            assert_eq!(origin.path, "./examples/local/bucket");
            assert_eq!(origin.sync_interval_seconds, Some(5));
        }
        other => panic!("expected local origin, got {other:?}"),
    }
}

#[test]
fn parses_cloudfront_cdn_config() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web-bucket
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    cdn:
      provider: cloudfront
      distribution_id_env: WEB_DISTRIBUTION_ID
      strategy: changed_paths
hosts:
  web.test:
    origin: web
"#,
    )
    .expect("manifest parses");

    match &manifest.origins["web"] {
        OriginConfig::S3(origin) => {
            assert!(matches!(
                origin.cdn,
                Some(crate::dto::manifest::CdnConfig::CloudFront(_))
            ));
        }
        other => panic!("expected s3 origin, got {other:?}"),
    }
}

#[test]
fn parses_cloudfront_saas_cdn_config() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  app:
    type: s3
    bucket: app-assets
    endpoint_env: APP_STORAGE_ENDPOINT
    region_env: APP_STORAGE_REGION
    cdn:
      provider: cloudfront_saas
      distribution_id_env: APP_CLOUDFRONT_DISTRIBUTION_ID
      connection_group_id_env: APP_CLOUDFRONT_CONNECTION_GROUP_ID
      strategy: changed_paths
      parameters_env:
        origin-domain: APP_CLOUDFRONT_ORIGIN_DOMAIN
      certificate:
        mode: managed
        validation_token_host: cloudfront
hosts:
  app.test:
    origin: app
"#,
    )
    .expect("manifest parses");

    match &manifest.origins["app"] {
        OriginConfig::S3(origin) => {
            match origin.cdn.as_ref().expect("cloudfront saas cdn config") {
                crate::dto::manifest::CdnConfig::CloudFrontSaas(config) => {
                    assert_eq!(config.distribution_id_env, "APP_CLOUDFRONT_DISTRIBUTION_ID");
                    assert_eq!(
                        config.connection_group_id_env.as_deref(),
                        Some("APP_CLOUDFRONT_CONNECTION_GROUP_ID")
                    );
                    assert_eq!(
                        config.strategy,
                        crate::dto::manifest::CdnRefreshStrategy::ChangedPaths
                    );
                    assert_eq!(
                        config.parameters_env,
                        std::collections::BTreeMap::from([(
                            "origin-domain".to_string(),
                            "APP_CLOUDFRONT_ORIGIN_DOMAIN".to_string()
                        )])
                    );
                    match config.certificate.as_ref().expect("certificate config") {
                        crate::dto::manifest::CloudFrontSaasCertificateConfig::Managed {
                            validation_token_host,
                        } => {
                            assert_eq!(
                                validation_token_host,
                                &crate::dto::manifest::CloudFrontSaasValidationTokenHost::CloudFront
                            );
                        }
                    }
                }
                other => panic!("expected cloudfront saas cdn, got {other:?}"),
            }
        }
        other => panic!("expected s3 origin, got {other:?}"),
    }
}

#[test]
fn parses_cloudfront_saas_defaults_and_optional_certificate() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  app:
    type: s3
    bucket: app-assets
    endpoint_env: APP_STORAGE_ENDPOINT
    region_env: APP_STORAGE_REGION
    cdn:
      provider: cloudfront_saas
      distribution_id_env: APP_CLOUDFRONT_DISTRIBUTION_ID
      certificate:
        mode: managed
hosts:
  app.test:
    origin: app
"#,
    )
    .expect("manifest parses");

    match &manifest.origins["app"] {
        OriginConfig::S3(origin) => {
            match origin.cdn.as_ref().expect("cloudfront saas cdn config") {
                crate::dto::manifest::CdnConfig::CloudFrontSaas(config) => {
                    assert_eq!(
                        config.strategy,
                        crate::dto::manifest::CdnRefreshStrategy::ChangedPaths
                    );
                    assert!(config.parameters_env.is_empty());
                    assert!(config.connection_group_id_env.is_none());
                    match config.certificate.as_ref().expect("certificate config") {
                        crate::dto::manifest::CloudFrontSaasCertificateConfig::Managed {
                            validation_token_host,
                        } => {
                            assert_eq!(
                                validation_token_host,
                                &crate::dto::manifest::CloudFrontSaasValidationTokenHost::CloudFront
                            );
                        }
                    }
                }
                other => panic!("expected cloudfront saas cdn, got {other:?}"),
            }
        }
        other => panic!("expected s3 origin, got {other:?}"),
    }
}

#[test]
fn parses_cloudfront_saas_without_certificate() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  app:
    type: s3
    bucket: app-assets
    endpoint_env: APP_STORAGE_ENDPOINT
    region_env: APP_STORAGE_REGION
    cdn:
      provider: cloudfront_saas
      distribution_id_env: APP_CLOUDFRONT_DISTRIBUTION_ID
hosts:
  app.test:
    origin: app
"#,
    )
    .expect("manifest parses");

    match &manifest.origins["app"] {
        OriginConfig::S3(origin) => {
            match origin.cdn.as_ref().expect("cloudfront saas cdn config") {
                crate::dto::manifest::CdnConfig::CloudFrontSaas(config) => {
                    assert!(config.certificate.is_none());
                }
                other => panic!("expected cloudfront saas cdn, got {other:?}"),
            }
        }
        other => panic!("expected s3 origin, got {other:?}"),
    }
}

#[test]
fn parses_cloudflare_cdn_config_from_json() {
    let manifest = parse_manifest_config(
        r#"
{
  "version": 1,
  "runtime": {
    "local_store_dir": "./var/rendermesh/origins",
    "sync_interval_seconds": 60
  },
  "origins": {
    "docs": {
      "type": "local",
      "path": "./docs",
      "cdn": {
        "provider": "cloudflare",
        "zone_id_env": "DOCS_ZONE_ID",
        "api_token_env": "DOCS_API_TOKEN",
        "strategy": "changed_paths",
        "url_prefixes": ["https://docs.test"]
      }
    }

  },
  "hosts": {
    "docs.test": {
      "origin": "docs"
    }
  }
}
"#,
    )
    .expect("manifest parses");

    match &manifest.origins["docs"] {
        OriginConfig::Local(origin) => {
            assert!(matches!(
                origin.cdn,
                Some(crate::dto::manifest::CdnConfig::Cloudflare(_))
            ));
        }
        other => panic!("expected local origin, got {other:?}"),
    }
}

#[test]
fn parses_cdn_domain_reconciliation_config() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  loja:
    type: s3
    bucket: loja-bucket
    endpoint_env: LOJA_ENDPOINT
    region_env: LOJA_REGION
    cdn:
      provider: cloudfront
      distribution_id_env: LOJA_DISTRIBUTION_ID
      domains:
        enabled: true
        origin_domain_env: RENDERMESH_PUBLIC_ORIGIN
        certificate_arn_env: LOJA_CERTIFICATE_ARN
        include_wildcards: true
        remove_extra_domains: true
hosts:
  megaloja.com.br:
    origin: loja
"#,
    )
    .expect("manifest parses");

    let cdn = manifest.origins["loja"].cdn().expect("cdn config");
    match cdn {
        crate::dto::manifest::CdnConfig::CloudFront(config) => {
            let domains = config.domains.as_ref().expect("domain config");
            assert!(domains.enabled);
            assert_eq!(domains.origin_domain_env, "RENDERMESH_PUBLIC_ORIGIN");
            assert_eq!(
                domains.certificate_arn_env.as_deref(),
                Some("LOJA_CERTIFICATE_ARN")
            );
            assert!(domains.include_wildcards);
            assert!(domains.remove_extra_domains);
        }
        other => panic!("expected cloudfront cdn, got {other:?}"),
    }
}

#[test]
fn parses_s3_origin_without_static_credential_envs() {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web-bucket
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
hosts:
  app.test:
    origin: web
"#,
    )
    .expect("manifest parses without static credential envs");

    match &manifest.origins["web"] {
        OriginConfig::S3(origin) => {
            assert_eq!(origin.bucket, "web-bucket");
            assert_eq!(origin.access_key_id_env, None);
            assert_eq!(origin.secret_access_key_env, None);
        }
        other => panic!("expected s3 origin, got {other:?}"),
    }
}
