    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;
    use std::{collections::BTreeMap, sync::Arc};
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct FakeStorage {
        objects: Arc<Mutex<BTreeMap<String, RemoteObject>>>,
        requested_keys: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl RemoteStorage for FakeStorage {
        async fn list_objects(&self) -> anyhow::Result<Vec<RemoteObjectSummary>> {
            let objects = self.objects.lock().await;
            Ok(objects
                .values()
                .map(|object| RemoteObjectSummary {
                    key: object.key.clone(),
                    created_at: None,
                    etag: object.etag.clone(),
                    last_modified: object.last_modified.clone(),
                    size: object.body.len() as u64,
                    content_type: object.content_type.clone(),
                    cache_control: object.cache_control.clone(),
                })
                .collect())
        }

        async fn get_object(&self, key: &str) -> anyhow::Result<RemoteObject> {
            self.requested_keys.lock().await.push(key.to_string());
            self.objects
                .lock()
                .await
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing"))
        }
    }

    fn remote_object(
        key: &str,
        body: &str,
        etag: Option<&str>,
        content_type: Option<&str>,
    ) -> RemoteObject {
        RemoteObject {
            key: key.to_string(),
            body: Bytes::from(body.to_string()),
            etag: etag.map(str::to_string),
            last_modified: None,
            content_type: content_type.map(str::to_string),
            cache_control: None,
        }
    }

    async fn requested_keys(storage: &FakeStorage) -> Vec<String> {
        let mut keys = storage.requested_keys.lock().await.clone();
        keys.sort();
        keys
    }

    #[test]
    fn rejects_an_object_that_changed_after_the_remote_listing() {
        let summary = crate::services::freshness::OriginFileState {
            path: ".rendermesh/edge.yaml".to_string(),
            created_at: None,
            last_modified: None,
            captured_at: chrono::Utc::now(),
            size: 10,
            etag: Some("edge-invalid".to_string()),
            content_type: Some("application/yaml".to_string()),
            cache_control: None,
        };
        let downloaded = RemoteObject {
            key: ".rendermesh/edge.yaml".to_string(),
            body: Bytes::from_static(b"version: 1"),
            etag: Some("edge-ready".to_string()),
            last_modified: None,
            content_type: Some("application/yaml".to_string()),
            cache_control: None,
        };

        let error = validate_downloaded_object(".rendermesh/edge.yaml", &summary, &downloaded)
            .expect_err("listing/download race must be rejected");

        assert!(error.to_string().contains("changed etag"));
    }

    #[tokio::test]
    async fn initial_sync_downloads_objects_and_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            RemoteObject {
                key: "index.html".to_string(),
                body: Bytes::from_static(b"<h1>Hello</h1>"),
                etag: Some("abc".to_string()),
                last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
                content_type: Some("text/html".to_string()),
                cache_control: Some("max-age=60".to_string()),
            },
        );
        let syncer = MirrorSyncService::new(temp.path().join("origins"));

        let report = syncer
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert_eq!(report, SyncReport { downloaded: 1 });
        assert_eq!(
            tokio::fs::read_to_string(temp.path().join("origins/web/index.html"))
                .await
                .expect("read"),
            "<h1>Hello</h1>"
        );
        let metadata_path = metadata_sidecar_path(&temp.path().join("origins/web"), "index.html")
            .expect("metadata path");
        assert!(metadata_path.exists());
    }

    #[tokio::test]
    async fn staged_sync_fetches_only_changed_files_and_waits_for_activation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let syncer = MirrorSyncService::new(temp.path().join("origins"));
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            remote_object(
                "index.html",
                "old index",
                Some("index-v1"),
                Some("text/html"),
            ),
        );
        storage.objects.lock().await.insert(
            "same.css".to_string(),
            remote_object("same.css", "body{}", Some("same-v1"), Some("text/css")),
        );
        storage.objects.lock().await.insert(
            "removed.html".to_string(),
            remote_object(
                "removed.html",
                "removed",
                Some("removed-v1"),
                Some("text/html"),
            ),
        );
        syncer
            .sync_origin("web", &storage)
            .await
            .expect("initial sync");
        let previous_index = crate::services::freshness::build_origin_index(
            "web",
            storage.list_objects().await.expect("list previous"),
            chrono::Utc::now(),
        )
        .expect("previous index");
        storage.requested_keys.lock().await.clear();

        storage.objects.lock().await.insert(
            "index.html".to_string(),
            remote_object(
                "index.html",
                "new index",
                Some("index-v2"),
                Some("text/html"),
            ),
        );
        storage.objects.lock().await.remove("removed.html");
        storage.objects.lock().await.insert(
            "new.html".to_string(),
            remote_object("new.html", "new", Some("new-v1"), Some("text/html")),
        );

        let staged = syncer
            .stage_origin_sync("web", &storage, Some(&previous_index))
            .await
            .expect("stage sync");

        assert_eq!(staged.report.downloaded, 2);
        assert!(staged.diff.modified.contains("index.html"));
        assert!(staged.diff.added.contains("new.html"));
        assert!(staged.diff.removed.contains("removed.html"));
        assert!(staged.diff.unchanged.contains("same.css"));
        assert_eq!(
            tokio::fs::read_to_string(temp.path().join("origins/web/index.html"))
                .await
                .expect("read active index"),
            "old index"
        );
        assert_eq!(
            requested_keys(&storage).await,
            vec!["index.html".to_string(), "new.html".to_string()]
        );

        syncer
            .activate_staged_origin(staged)
            .await
            .expect("activate");

        assert_eq!(
            tokio::fs::read_to_string(temp.path().join("origins/web/index.html"))
                .await
                .expect("read activated index"),
            "new index"
        );
        assert!(temp.path().join("origins/web/new.html").exists());
        assert!(!temp.path().join("origins/web/removed.html").exists());
    }

    #[tokio::test]
    async fn removes_objects_missing_from_remote_listing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(origin_dir.join("assets"))
            .await
            .expect("mkdir");
        tokio::fs::write(origin_dir.join("stale.html"), "old")
            .await
            .expect("write stale");
        let stale_metadata_path =
            metadata_sidecar_path(&origin_dir, "stale.html").expect("stale metadata path");
        tokio::fs::create_dir_all(stale_metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(&stale_metadata_path, "{}")
            .await
            .expect("write stale metadata");
        tokio::fs::write(origin_dir.join("assets/keep.css"), "body{}")
            .await
            .expect("write kept object");

        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "assets/keep.css".to_string(),
            RemoteObject {
                key: "assets/keep.css".to_string(),
                body: Bytes::from_static(b"body{}"),
                etag: Some("keep".to_string()),
                last_modified: None,
                content_type: Some("text/css".to_string()),
                cache_control: None,
            },
        );

        MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert!(!origin_dir.join("stale.html").exists());
        assert!(!stale_metadata_path.exists());
        assert!(origin_dir.join("assets/keep.css").exists());
    }

    #[tokio::test]
    async fn removes_stale_real_object_ending_meta_json_and_its_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("foo.meta.json"), "real object")
            .await
            .expect("write stale object");
        let metadata_path =
            metadata_sidecar_path(&origin_dir, "foo.meta.json").expect("metadata path");
        tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(&metadata_path, r#"{"etag":"stale"}"#)
            .await
            .expect("write stale metadata");

        let storage = FakeStorage::default();

        MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert!(!origin_dir.join("foo.meta.json").exists());
        assert!(!metadata_path.exists());
    }

    #[tokio::test]
    async fn removes_orphan_metadata_sidecars() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        let orphan_path = origin_dir
            .join(METADATA_DIR_NAME)
            .join("aa")
            .join("orphan.json");
        tokio::fs::create_dir_all(orphan_path.parent().expect("orphan parent"))
            .await
            .expect("mkdir orphan metadata");
        tokio::fs::write(&orphan_path, "{}")
            .await
            .expect("write orphan metadata");

        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            RemoteObject {
                key: "index.html".to_string(),
                body: Bytes::from_static(b"<h1>Hello</h1>"),
                etag: Some("abc".to_string()),
                last_modified: None,
                content_type: Some("text/html".to_string()),
                cache_control: None,
            },
        );

        MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert!(!orphan_path.exists());
        assert!(metadata_sidecar_path(&origin_dir, "index.html")
            .expect("metadata path")
            .exists());
    }

    #[tokio::test]
    async fn syncs_and_reads_metadata_for_long_key_with_bounded_sidecar_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        let long_key = (0..8)
            .map(|index| format!("segment-{index:02}-{}", "a".repeat(40)))
            .collect::<Vec<_>>()
            .join("/");
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            long_key.clone(),
            RemoteObject {
                key: long_key.clone(),
                body: Bytes::from_static(b"long key body"),
                etag: Some("long-etag".to_string()),
                last_modified: None,
                content_type: Some("text/plain".to_string()),
                cache_control: None,
            },
        );

        MirrorSyncService::new(root.clone())
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        let metadata_path = metadata_sidecar_path(&origin_dir, &long_key).expect("metadata path");
        for component in metadata_path.components() {
            if let Component::Normal(part) = component {
                assert!(
                    part.to_string_lossy().len() <= 80,
                    "component is too long: {}",
                    part.to_string_lossy().len()
                );
            }
        }

        let object = LocalMirrorRepository::new(root)
            .read_object("web", &long_key)
            .await
            .expect("read")
            .expect("object");

        assert_eq!(object.body, Bytes::from_static(b"long key body"));
        assert_eq!(object.metadata.etag.as_deref(), Some("long-etag"));
    }

    #[tokio::test]
    async fn skips_download_when_local_object_metadata_matches_summary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("index.html"), "<h1>Hello</h1>")
            .await
            .expect("write object");
        let metadata_path =
            metadata_sidecar_path(&origin_dir, "index.html").expect("metadata path");
        tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(
            metadata_path,
            r#"{"content_type":"text/html","etag":"abc","last_modified":"Mon, 01 Jan 2024 00:00:00 GMT","cache_control":"max-age=60","size":14}"#,
        )
        .await
        .expect("write metadata");

        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            RemoteObject {
                key: "index.html".to_string(),
                body: Bytes::from_static(b"<h1>Hello</h1>"),
                etag: Some("abc".to_string()),
                last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
                content_type: Some("text/html".to_string()),
                cache_control: Some("max-age=60".to_string()),
            },
        );

        let report = MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert_eq!(report, SyncReport { downloaded: 0 });
        assert!(storage.requested_keys.lock().await.is_empty());
    }

    #[tokio::test]
    async fn rejects_remote_keys_that_escape_origin_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "../secret.txt".to_string(),
            RemoteObject {
                key: "../secret.txt".to_string(),
                body: Bytes::from_static(b"secret"),
                etag: None,
                last_modified: None,
                content_type: None,
                cache_control: None,
            },
        );

        let error = MirrorSyncService::new(temp.path().join("origins"))
            .sync_origin("web", &storage)
            .await
            .expect_err("invalid key is rejected");

        assert!(error.to_string().contains("invalid object path"));
        assert!(!temp.path().join("origins/secret.txt").exists());
    }

    #[tokio::test]
    async fn failed_sync_keeps_previous_mirror_untouched() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("index.html"), "old")
            .await
            .expect("write old index");
        tokio::fs::write(origin_dir.join("keep.html"), "keep")
            .await
            .expect("write old keep");

        let storage = FailingStorage {
            fail_key: "keep.html".to_string(),
            objects: BTreeMap::from([
                (
                    "index.html".to_string(),
                    RemoteObject {
                        key: "index.html".to_string(),
                        body: Bytes::from_static(b"new"),
                        etag: Some("new-index".to_string()),
                        last_modified: None,
                        content_type: Some("text/html".to_string()),
                        cache_control: None,
                    },
                ),
                (
                    "keep.html".to_string(),
                    RemoteObject {
                        key: "keep.html".to_string(),
                        body: Bytes::from_static(b"new keep"),
                        etag: Some("new-keep".to_string()),
                        last_modified: None,
                        content_type: Some("text/html".to_string()),
                        cache_control: None,
                    },
                ),
            ]),
        };

        let error = MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect_err("sync fails");

        assert!(error.to_string().contains("forced failure"));
        assert_eq!(
            tokio::fs::read_to_string(origin_dir.join("index.html"))
                .await
                .expect("read old index"),
            "old"
        );
        assert_eq!(
            tokio::fs::read_to_string(origin_dir.join("keep.html"))
                .await
                .expect("read old keep"),
            "keep"
        );
    }

    struct FailingStorage {
        fail_key: String,
        objects: BTreeMap<String, RemoteObject>,
    }

    #[async_trait]
    impl RemoteStorage for FailingStorage {
        async fn list_objects(&self) -> anyhow::Result<Vec<RemoteObjectSummary>> {
            Ok(self
                .objects
                .values()
                .map(|object| RemoteObjectSummary {
                    key: object.key.clone(),
                    created_at: None,
                    etag: object.etag.clone(),
                    last_modified: object.last_modified.clone(),
                    size: object.body.len() as u64,
                    content_type: object.content_type.clone(),
                    cache_control: object.cache_control.clone(),
                })
                .collect())
        }

        async fn get_object(&self, key: &str) -> anyhow::Result<RemoteObject> {
            if key == self.fail_key {
                anyhow::bail!("forced failure for {key}");
            }

            self.objects
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing {key}"))
        }
    }
