pub fn sample_manifest() -> &'static str {
    r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  my_app:
    type: s3
    bucket: bucket_my_app_123
    endpoint_env: MY_APP_STORAGE_ENDPOINT
    region_env: MY_APP_STORAGE_REGION
    access_key_id_env: MY_APP_ACCESS_KEY_ID
    secret_access_key_env: MY_APP_SECRET_ACCESS_KEY
    force_path_style_env: MY_APP_FORCE_PATH_STYLE
    sync_interval_seconds: 30
    activation_barrier_path: .rendermesh/edge.yaml
    edge_context:
      tenant_id: loja-123
      feature_flags:
        checkout_v2: true
hosts:
  myapp.com:
    origin: my_app
  "*.myapp.com":
    origin: my_app
"#
}
