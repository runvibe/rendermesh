use rendermesh::repositories::{
    local_directory_storage::LocalDirectoryStorageRepository, sync::RemoteStorage,
};

#[tokio::test]
async fn local_directory_storage_lists_and_reads_regular_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("source");
    tokio::fs::create_dir_all(root.join("docs"))
        .await
        .expect("create docs dir");
    tokio::fs::write(root.join("index.html"), "<h1>Hello</h1>")
        .await
        .expect("write index");
    tokio::fs::write(root.join("docs").join("index.html"), "<h1>Docs</h1>")
        .await
        .expect("write docs");

    let storage = LocalDirectoryStorageRepository::new(&root).expect("storage builds");
    let mut summaries = storage.list_objects().await.expect("objects list");
    summaries.sort_by(|left, right| left.key.cmp(&right.key));

    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.key.as_str())
            .collect::<Vec<_>>(),
        vec!["docs/index.html", "index.html"]
    );
    assert_eq!(summaries[1].content_type.as_deref(), Some("text/html"));
    assert!(summaries[1]
        .etag
        .as_deref()
        .is_some_and(|etag| etag.starts_with("sha256:")));
    assert!(summaries[1].last_modified.is_some());

    let object = storage
        .get_object("docs/index.html")
        .await
        .expect("object reads");
    assert_eq!(object.body.as_ref(), b"<h1>Docs</h1>");
    assert_eq!(object.content_type.as_deref(), Some("text/html"));
}

#[tokio::test]
async fn local_directory_storage_uses_content_hash_as_etag() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("source");
    tokio::fs::create_dir_all(&root)
        .await
        .expect("create source dir");
    tokio::fs::write(root.join("index.html"), "first")
        .await
        .expect("write first version");

    let storage = LocalDirectoryStorageRepository::new(&root).expect("storage builds");
    let first = storage.list_objects().await.expect("first list");

    tokio::fs::write(root.join("index.html"), "other")
        .await
        .expect("write same-size second version");
    let second = storage.list_objects().await.expect("second list");

    assert_ne!(first[0].etag, second[0].etag);
}

#[tokio::test]
async fn local_directory_storage_rejects_path_traversal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("source");
    tokio::fs::create_dir_all(&root)
        .await
        .expect("create source dir");

    let storage = LocalDirectoryStorageRepository::new(&root).expect("storage builds");
    let error = storage
        .get_object("../secret.txt")
        .await
        .expect_err("traversal is rejected");

    assert!(error.to_string().contains("invalid object path"));
}
