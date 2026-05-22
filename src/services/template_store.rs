use std::{
    collections::BTreeMap,
    path::{Component, Path},
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result};
use handlebars::Handlebars;
use serde_json::Value;
use thiserror::Error;

use crate::{
    repositories::local_mirror::{normalize_object_path, LocalMirrorRepository, METADATA_DIR_NAME},
    services::{edge_hooks::is_html, freshness::OriginFreshnessDiff},
};

#[derive(Clone, Debug, Default)]
pub struct TemplateStore {
    origins: Arc<RwLock<BTreeMap<String, Arc<Handlebars<'static>>>>>,
}

#[derive(Debug, Error)]
pub enum TemplateStoreError {
    #[error("template is not available for html rendering")]
    NotHtml,
    #[error(transparent)]
    Render(#[from] handlebars::RenderError),
}

impl TemplateStore {
    pub async fn load_origin_templates(
        &self,
        origin_id: &str,
        mirror: &LocalMirrorRepository,
    ) -> Result<()> {
        let registry = Arc::new(load_origin_registry(origin_id, mirror).await?);
        self.origins
            .write()
            .expect("template store lock")
            .insert(origin_id.to_string(), registry);
        Ok(())
    }

    pub async fn compile_template_update(
        &self,
        origin_id: &str,
        mirror: &LocalMirrorRepository,
        diff: &OriginFreshnessDiff,
    ) -> Result<Arc<Handlebars<'static>>> {
        self.compile_template_update_from_mirror(origin_id, origin_id, mirror, diff)
            .await
    }

    pub async fn compile_template_update_from_mirror(
        &self,
        origin_id: &str,
        mirror_origin_id: &str,
        mirror: &LocalMirrorRepository,
        diff: &OriginFreshnessDiff,
    ) -> Result<Arc<Handlebars<'static>>> {
        let mut registry = self
            .origins
            .read()
            .expect("template store lock")
            .get(origin_id)
            .map(|registry| (**registry).clone())
            .unwrap_or_else(Handlebars::new);

        for key in &diff.removed {
            registry.unregister_template(key);
        }

        for key in diff.changed_paths() {
            let Some(object) = mirror.read_object(mirror_origin_id, key).await? else {
                registry.unregister_template(key);
                continue;
            };

            if !is_html(key, object.metadata.content_type.as_deref()) {
                registry.unregister_template(key);
                continue;
            }

            let body = String::from_utf8(object.body.to_vec())
                .with_context(|| format!("template {origin_id}/{key} is not valid utf-8"))?;
            registry
                .register_template_string(key, body)
                .with_context(|| format!("compile template {origin_id}/{key}"))?;
        }

        Ok(Arc::new(registry))
    }

    pub fn set_origin_registry(&self, origin_id: &str, registry: Arc<Handlebars<'static>>) {
        self.origins
            .write()
            .expect("template store lock")
            .insert(origin_id.to_string(), registry);
    }

    pub fn render(
        &self,
        origin_id: &str,
        path: &str,
        params: &Value,
    ) -> std::result::Result<String, TemplateStoreError> {
        let key = normalize_object_path(path).map_err(|_| TemplateStoreError::NotHtml)?;
        let origins = self.origins.read().expect("template store lock");
        let Some(registry) = origins.get(origin_id) else {
            return Err(TemplateStoreError::NotHtml);
        };
        if !registry.has_template(&key) {
            return Err(TemplateStoreError::NotHtml);
        }
        registry.render(&key, params).map_err(Into::into)
    }
}

async fn load_origin_registry(
    origin_id: &str,
    mirror: &LocalMirrorRepository,
) -> Result<Handlebars<'static>> {
    let origin_dir = mirror.origin_dir(origin_id)?;
    let mut registry = Handlebars::new();
    let files = mirror_files(&origin_dir).await?;

    for key in files {
        let Some(object) = mirror.read_object(origin_id, &key).await? else {
            continue;
        };
        if !is_html(&key, object.metadata.content_type.as_deref()) {
            continue;
        }

        let body = String::from_utf8(object.body.to_vec())
            .with_context(|| format!("template {origin_id}/{key} is not valid utf-8"))?;
        registry
            .register_template_string(&key, body)
            .with_context(|| format!("compile template {origin_id}/{key}"))?;
    }

    Ok(registry)
}

async fn mirror_files(origin_dir: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut stack = vec![origin_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("read mirror dir {}", dir.display()))
            }
        };

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;

            if file_type.is_dir() {
                if path == origin_dir.join(METADATA_DIR_NAME) {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if file_type.is_file() {
                files.push(relative_key(origin_dir, &path)?);
            }
        }
    }

    Ok(files)
}

fn relative_key(origin_dir: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(origin_dir)
        .with_context(|| format!("template path {} is outside origin dir", path.display()))?;
    let key = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    normalize_object_path(&key).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::local_mirror::metadata_sidecar_path;
    use serde_json::json;

    #[tokio::test]
    async fn loads_only_html_templates_for_origin() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "index.html", "<h1>{{title}}</h1>", None).await;
        write_object(
            temp.path(),
            "content",
            "<p>{{title}}</p>",
            Some(r#"{"content_type":"text/html; charset=utf-8"}"#),
        )
        .await;
        write_object(
            temp.path(),
            "data.json",
            r#"{"title":"{{title}}"}"#,
            Some(r#"{"content_type":"application/json"}"#),
        )
        .await;

        let store = TemplateStore::default();
        let mirror = crate::repositories::local_mirror::LocalMirrorRepository::new(
            temp.path().join("origins"),
        );

        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("templates load");

        assert_eq!(
            store
                .render("web", "/index.html", &json!({"title":"Hello"}))
                .expect("index renders"),
            "<h1>Hello</h1>"
        );
        assert_eq!(
            store
                .render("web", "/content", &json!({"title":"Typed"}))
                .expect("content-type html renders"),
            "<p>Typed</p>"
        );
        assert!(matches!(
            store.render("web", "/data.json", &json!({"title":"Nope"})),
            Err(TemplateStoreError::NotHtml)
        ));
    }

    #[tokio::test]
    async fn replacing_origin_templates_drops_removed_templates() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "index.html", "<h1>{{title}}</h1>", None).await;
        let mirror = crate::repositories::local_mirror::LocalMirrorRepository::new(
            temp.path().join("origins"),
        );
        let store = TemplateStore::default();
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("initial load");
        assert!(store
            .render("web", "/index.html", &json!({"title":"First"}))
            .is_ok());

        tokio::fs::remove_file(temp.path().join("origins/web/index.html"))
            .await
            .expect("remove html");
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("reload");

        assert!(matches!(
            store.render("web", "/index.html", &json!({"title":"Gone"})),
            Err(TemplateStoreError::NotHtml)
        ));
    }

    #[tokio::test]
    async fn compiles_template_update_without_replacing_active_registry() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "index.html", "<h1>{{title}}</h1>", None).await;
        write_object(temp.path(), "old.html", "<p>{{title}}</p>", None).await;
        let mirror = crate::repositories::local_mirror::LocalMirrorRepository::new(
            temp.path().join("origins"),
        );
        let store = TemplateStore::default();
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("initial load");

        tokio::fs::write(
            temp.path().join("origins/web/index.html"),
            "<h2>{{title}}</h2>",
        )
        .await
        .expect("write updated index");
        write_object(temp.path(), "new.html", "<strong>{{title}}</strong>", None).await;
        tokio::fs::remove_file(temp.path().join("origins/web/old.html"))
            .await
            .expect("remove old");
        let diff = crate::services::freshness::OriginFreshnessDiff {
            added: ["new.html".to_string()].into_iter().collect(),
            modified: ["index.html".to_string()].into_iter().collect(),
            removed: ["old.html".to_string()].into_iter().collect(),
            unchanged: Default::default(),
        };

        let registry = store
            .compile_template_update("web", &mirror, &diff)
            .await
            .expect("compile update");

        assert_eq!(
            store
                .render("web", "/index.html", &json!({"title":"Active"}))
                .expect("active registry still renders"),
            "<h1>Active</h1>"
        );

        store.set_origin_registry("web", registry);

        assert_eq!(
            store
                .render("web", "/index.html", &json!({"title":"Updated"}))
                .expect("updated registry renders"),
            "<h2>Updated</h2>"
        );
        assert_eq!(
            store
                .render("web", "/new.html", &json!({"title":"New"}))
                .expect("new template renders"),
            "<strong>New</strong>"
        );
        assert!(matches!(
            store.render("web", "/old.html", &json!({"title":"Gone"})),
            Err(TemplateStoreError::NotHtml)
        ));
    }

    async fn write_object(
        temp_path: &std::path::Path,
        key: &str,
        body: &str,
        metadata: Option<&str>,
    ) {
        let origin_dir = temp_path.join("origins/web");
        let object_path = origin_dir.join(key);
        tokio::fs::create_dir_all(object_path.parent().expect("object parent"))
            .await
            .expect("mkdir object parent");
        tokio::fs::write(&object_path, body)
            .await
            .expect("write object");
        if let Some(metadata) = metadata {
            let metadata_path = metadata_sidecar_path(&origin_dir, key).expect("metadata path");
            tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
                .await
                .expect("mkdir metadata parent");
            tokio::fs::write(metadata_path, metadata)
                .await
                .expect("write metadata");
        }
    }
}
