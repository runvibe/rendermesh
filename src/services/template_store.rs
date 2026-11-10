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
    services::{
        date_helpers::register_date_helpers, edge_hooks::is_html, freshness::OriginFreshnessDiff,
    },
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
            .unwrap_or_else(new_template_registry);

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
    let mut registry = new_template_registry();
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

fn new_template_registry() -> Handlebars<'static> {
    let mut registry = Handlebars::new();
    register_date_helpers(&mut registry);
    registry
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

    #[tokio::test]
    async fn date_format_normalizes_offsets_and_unix_seconds_to_utc() {
        let temp = tempfile::tempdir().expect("tempdir");
        let template = concat!(
            "{{date_format east \"%Y-%m-%d %H:%M:%S\"}}|",
            "{{date_format west \"%Y-%m-%d %H:%M:%S\"}}|",
            "{{date_format epoch \"%Y-%m-%d %H:%M:%S\"}}|",
            "{{date_format before_epoch \"%Y-%m-%d %H:%M:%S\"}}|",
            "{{date_format fractional \"%Y-%m-%d %H:%M:%S%.3f\"}}"
        );
        write_object(temp.path(), "index.html", template, None).await;
        let store = TemplateStore::default();
        let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("templates load");

        let rendered = store
            .render(
                "web",
                "/index.html",
                &json!({
                    "east": "2024-01-01T00:30:45+02:00",
                    "west": "2019-10-31T23:59:59-05:00",
                    "epoch": 0,
                    "before_epoch": -1,
                    "fractional": "2024-01-02T03:04:05.678+01:00"
                }),
            )
            .expect("date helpers render");

        assert_eq!(
            rendered,
            "2023-12-31 22:30:45|2019-11-01 04:59:59|1970-01-01 00:00:00|1969-12-31 23:59:59|2024-01-02 02:04:05.678"
        );
    }

    #[tokio::test]
    async fn date_format_respects_html_escaping_and_explicit_field_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "index.html",
            concat!(
                "{{this.date_format}}|",
                "{{date_format instant \"<%Y>\"}}|",
                "{{{date_format instant \"<%Y>\"}}}"
            ),
            None,
        )
        .await;
        let store = TemplateStore::default();
        let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("templates load");

        let rendered = store
            .render(
                "web",
                "/index.html",
                &json!({"date_format": "from params", "instant": 0}),
            )
            .expect("template renders");

        assert_eq!(rendered, "from params|&lt;1970&gt;|<1970>");
    }

    #[tokio::test]
    async fn date_format_rejects_invalid_arguments_and_patterns() {
        let cases = [
            ("missing arguments", "{{date_format}}", json!({})),
            (
                "missing format",
                "{{date_format input}}",
                json!({"input": 0}),
            ),
            (
                "extra argument",
                "{{date_format input \"%F\" \"extra\"}}",
                json!({"input": 0}),
            ),
            (
                "named argument",
                "{{date_format input \"%F\" extra=true}}",
                json!({"input": 0}),
            ),
            (
                "invalid date",
                "{{date_format input \"%F\"}}",
                json!({"input": "not a date"}),
            ),
            (
                "date without timezone",
                "{{date_format input \"%F\"}}",
                json!({"input": "2026-11-10T12:00:00"}),
            ),
            (
                "nonexistent date",
                "{{date_format input \"%F\"}}",
                json!({"input": "2024-02-30T00:00:00Z"}),
            ),
            (
                "timestamp as string",
                "{{date_format input \"%F\"}}",
                json!({"input": "0"}),
            ),
            (
                "out-of-range timestamp",
                "{{date_format input \"%F\"}}",
                json!({"input": i64::MAX}),
            ),
            (
                "fractional timestamp",
                "{{date_format input \"%F\"}}",
                json!({"input": 1.5}),
            ),
            (
                "null timestamp",
                "{{date_format input \"%F\"}}",
                json!({"input": null}),
            ),
            (
                "non-string format",
                "{{date_format input format}}",
                json!({"input": 0, "format": 12}),
            ),
            (
                "unknown format specifier",
                "{{date_format input \"%Q\"}}",
                json!({"input": 0}),
            ),
            (
                "incomplete format specifier",
                "{{date_format input \"%\"}}",
                json!({"input": 0}),
            ),
            (
                "parsing-only format specifier",
                "{{date_format input \"%#z\"}}",
                json!({"input": 0}),
            ),
        ];
        let temp = tempfile::tempdir().expect("tempdir");
        for (index, (_, template, _)) in cases.iter().enumerate() {
            write_object(temp.path(), &format!("case-{index}.html"), template, None).await;
        }
        let store = TemplateStore::default();
        let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("templates load");

        for (index, (case, _, params)) in cases.iter().enumerate() {
            let error = store
                .render("web", &format!("/case-{index}.html"), params)
                .expect_err(case);
            match error {
                TemplateStoreError::Render(error) => {
                    assert!(
                        !matches!(
                            error.reason(),
                            handlebars::RenderErrorReason::HelperNotFound(_)
                        ),
                        "{case}: helper must reject the input rather than be missing"
                    );
                }
                other => panic!("{case}: expected a render error, got {other}"),
            }
        }
    }

    #[tokio::test]
    async fn date_format_survives_incremental_updates_and_origin_reload() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "index.html",
            "<p>{{date_format instant \"%F\"}}</p>",
            None,
        )
        .await;
        let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
        let store = TemplateStore::default();
        let added = OriginFreshnessDiff {
            added: ["index.html".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let registry = store
            .compile_template_update("web", &mirror, &added)
            .await
            .expect("compile first update");
        store.set_origin_registry("web", registry);
        assert_eq!(
            store
                .render("web", "/index.html", &json!({"instant": 0}))
                .expect("initial update renders"),
            "<p>1970-01-01</p>"
        );

        write_object(
            temp.path(),
            "index.html",
            "<strong>{{date_format instant \"%F\"}}</strong>",
            None,
        )
        .await;
        let modified = OriginFreshnessDiff {
            modified: ["index.html".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let registry = store
            .compile_template_update("web", &mirror, &modified)
            .await
            .expect("compile subsequent update");
        assert_eq!(
            store
                .render("web", "/index.html", &json!({"instant": 0}))
                .expect("old registry remains active"),
            "<p>1970-01-01</p>"
        );
        store.set_origin_registry("web", registry);
        assert_eq!(
            store
                .render("web", "/index.html", &json!({"instant": 0}))
                .expect("updated registry renders"),
            "<strong>1970-01-01</strong>"
        );

        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("reload origin");
        assert_eq!(
            store
                .render("web", "/index.html", &json!({"instant": 0}))
                .expect("reloaded registry renders"),
            "<strong>1970-01-01</strong>"
        );
    }

    #[tokio::test]
    async fn date_current_helpers_render_current_utc_values() {
        use chrono::{DateTime, NaiveDate, NaiveTime, Utc};

        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "index.html",
            "{{date_now}}|{{date_today}}|{{date_time}}|{{date_timestamp}}",
            None,
        )
        .await;
        let store = TemplateStore::default();
        let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("templates load");

        let before = Utc::now();
        let rendered = store
            .render("web", "/index.html", &json!({}))
            .expect("current helpers render");
        let after = Utc::now();
        let parts = rendered.split('|').collect::<Vec<_>>();
        assert_eq!(parts.len(), 4);

        let now = DateTime::parse_from_rfc3339(parts[0]).expect("RFC 3339 date");
        assert_eq!(parts[0].len(), 20);
        assert!(parts[0].ends_with('Z'));
        assert_eq!(now.offset().local_minus_utc(), 0);
        assert!((before.timestamp()..=after.timestamp()).contains(&now.timestamp()));

        let today = NaiveDate::parse_from_str(parts[1], "%Y-%m-%d").expect("date");
        assert!(today >= before.date_naive() && today <= after.date_naive());
        let time = NaiveTime::parse_from_str(parts[2], "%H:%M:%S").expect("time");
        assert_eq!(parts[2].len(), 8);
        let start_day_time = before.date_naive().and_time(time).and_utc().timestamp();
        let end_day_time = after.date_naive().and_time(time).and_utc().timestamp();
        let window = before.timestamp()..=after.timestamp();
        assert!(
            window.contains(&start_day_time)
                || window.contains(&end_day_time)
                || after.timestamp() - before.timestamp() >= 86_400
        );

        let timestamp = parts[3].parse::<i64>().expect("Unix timestamp");
        assert!(window.contains(&timestamp));
    }

    #[tokio::test]
    async fn date_current_helpers_work_as_date_format_subexpressions() {
        use chrono::{NaiveDate, Utc};

        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "index.html",
            concat!(
                "{{date_format (date_now) \"%F\"}}|",
                "{{date_format (date_timestamp) \"%F\"}}"
            ),
            None,
        )
        .await;
        let store = TemplateStore::default();
        let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("templates load");

        let before = Utc::now().date_naive();
        let rendered = store
            .render("web", "/index.html", &json!({}))
            .expect("subexpressions render");
        let after = Utc::now().date_naive();
        let dates = rendered.split('|').collect::<Vec<_>>();
        assert_eq!(dates.len(), 2);
        for date in dates {
            let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").expect("formatted date");
            assert!(date >= before && date <= after);
        }
    }

    #[tokio::test]
    async fn date_current_helpers_reject_arguments_and_blocks() {
        let cases = [
            "{{date_now 1}}",
            "{{date_today 1}}",
            "{{date_time 1}}",
            "{{date_timestamp 1}}",
            "{{date_now extra=true}}",
            "{{date_today extra=true}}",
            "{{date_time extra=true}}",
            "{{date_timestamp extra=true}}",
            "{{#date_now}}ignored{{/date_now}}",
        ];
        let temp = tempfile::tempdir().expect("tempdir");
        for (index, template) in cases.iter().enumerate() {
            write_object(temp.path(), &format!("case-{index}.html"), template, None).await;
        }
        let store = TemplateStore::default();
        let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("templates load");

        for (index, template) in cases.iter().enumerate() {
            let error = store
                .render("web", &format!("/case-{index}.html"), &json!({}))
                .expect_err(template);
            match error {
                TemplateStoreError::Render(error) => {
                    assert!(
                        !matches!(
                            error.reason(),
                            handlebars::RenderErrorReason::HelperNotFound(_)
                        ),
                        "{template}: helper must reject the input rather than be missing"
                    );
                }
                other => panic!("{template}: expected a render error, got {other}"),
            }
        }
    }

    #[tokio::test]
    async fn date_current_helper_allows_explicit_access_to_same_named_param() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "index.html",
            "{{this.date_now}}|{{date_now}}",
            None,
        )
        .await;
        let store = TemplateStore::default();
        let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
        store
            .load_origin_templates("web", &mirror)
            .await
            .expect("templates load");

        let rendered = store
            .render("web", "/index.html", &json!({"date_now": "from params"}))
            .expect("template renders");
        let (param, now) = rendered.split_once('|').expect("two values");
        assert_eq!(param, "from params");
        assert!(chrono::DateTime::parse_from_rfc3339(now).is_ok());
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
