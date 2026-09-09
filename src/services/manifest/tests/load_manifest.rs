use super::*;

#[tokio::test]
async fn load_manifest_reads_json_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("rendermesh.json");
    tokio::fs::write(
        &path,
        r#"
{
  "version": 1,
  "runtime": {
    "local_store_dir": "./var/rendermesh/origins",
    "sync_interval_seconds": 45
  },
  "origins": {
    "web": {
      "type": "s3",
      "bucket": "web-bucket",
      "endpoint_env": "WEB_ENDPOINT",
      "region_env": "WEB_REGION",
      "access_key_id_env": "WEB_ACCESS_KEY_ID",
      "secret_access_key_env": "WEB_SECRET_ACCESS_KEY"
    }
  },
  "hosts": {
    "app.test": {
      "origin": "web"
    }
  }
}
"#,
    )
    .await
    .expect("write manifest");

    let manifest = load_manifest(&ManifestRepository::new(), &path)
        .await
        .expect("json manifest loads");

    assert_eq!(manifest.runtime.sync_interval_seconds, 45);
    match &manifest.origins["web"] {
        OriginConfig::S3(origin) => assert_eq!(origin.bucket, "web-bucket"),
        other => panic!("expected s3 origin, got {other:?}"),
    }
    assert_eq!(manifest.hosts["app.test"].origin, "web");
}
