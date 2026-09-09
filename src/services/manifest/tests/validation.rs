use super::*;
    #[test]
    fn rejects_empty_cloudfront_saas_distribution_id_env() {
        let error = parse_manifest_yaml(
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
      distribution_id_env: " "
hosts:
  app.test:
    origin: app
"#,
        )
        .expect_err("empty distribution id env is rejected");

        assert!(error
            .to_string()
            .contains("cdn.distribution_id_env is required"));
    }


    #[test]
    fn rejects_empty_cloudfront_saas_parameter_env_value() {
        let error = parse_manifest_yaml(
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
      parameters_env:
        origin-domain: " "
hosts:
  app.test:
    origin: app
"#,
        )
        .expect_err("empty parameter env value is rejected");

        assert!(error
            .to_string()
            .contains("cdn.parameters_env.origin-domain is required"));
    }


    #[test]
    fn rejects_blank_cloudfront_saas_parameter_name() {
        let error = parse_manifest_yaml(
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
      parameters_env:
        " ": APP_CLOUDFRONT_ORIGIN_DOMAIN
hosts:
  app.test:
    origin: app
"#,
        )
        .expect_err("blank parameter name is rejected");

        assert!(error
            .to_string()
            .contains("origin app cdn.parameters_env contains an empty parameter name"));
    }

    #[test]
    fn rejects_unknown_cloudfront_saas_validation_token_host() {
        let error = serde_norway::from_str::<crate::dto::manifest::RenderMeshManifest>(
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
        validation_token_host: self_hosted
hosts:
  app.test:
    origin: app
"#,
        )
        .expect_err("self_hosted validation token host is rejected");

        assert!(error.to_string().contains("unknown variant"));
        assert!(error.to_string().contains("self_hosted"));
    }


    #[test]
    fn rejects_empty_local_origin_path() {
        let yaml = r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  docs:
    type: local
    path: " "
hosts:
  docs.test:
    origin: docs
"#;

        let manifest = serde_norway::from_str::<crate::dto::manifest::RenderMeshManifest>(yaml)
            .expect("yaml parses");
        let error = validate_manifest(&manifest).expect_err("validation fails");

        assert!(error.to_string().contains("path is required"));
    }

    #[test]
    fn rejects_local_origin_with_s3_fields() {
        let error = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  docs:
    type: local
    path: ./docs
    bucket: docs-bucket
hosts:
  docs.test:
    origin: docs
"#,
        )
        .expect_err("local origin rejects s3 field");

        assert!(error.to_string().contains("bucket"));
    }

    #[test]
    fn rejects_s3_origin_with_local_path_field() {
        let error = parse_manifest_yaml(
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
    path: ./web
hosts:
  web.test:
    origin: web
"#,
        )
        .expect_err("s3 origin rejects local path");

        assert!(error.to_string().contains("path"));
    }


    #[test]
    fn rejects_host_that_references_missing_origin() {
        let yaml = r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins: {}
hosts:
  myapp.com:
    origin: missing
"#;

        let manifest = serde_norway::from_str::<crate::dto::manifest::RenderMeshManifest>(yaml)
            .expect("yaml parses");
        let error = validate_manifest(&manifest).expect_err("validation fails");

        assert!(error.to_string().contains("unknown origin missing"));
    }

    #[test]
    fn rejects_non_positive_sync_intervals() {
        let yaml = r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 0
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
"#;

        let manifest = serde_norway::from_str::<crate::dto::manifest::RenderMeshManifest>(yaml)
            .expect("yaml parses");
        let error = validate_manifest(&manifest).expect_err("validation fails");

        assert!(error.to_string().contains("sync_interval_seconds"));
    }

