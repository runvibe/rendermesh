use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::repositories::sync::{normalize_remote_key, RemoteObjectSummary};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginFreshnessIndex {
    pub origin_id: String,
    pub captured_at: DateTime<Utc>,
    pub files: BTreeMap<String, OriginFileState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginFileState {
    pub path: String,
    pub created_at: Option<String>,
    pub last_modified: Option<String>,
    pub captured_at: DateTime<Utc>,
    pub size: u64,
    pub etag: Option<String>,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OriginFreshnessDiff {
    pub added: BTreeSet<String>,
    pub modified: BTreeSet<String>,
    pub removed: BTreeSet<String>,
    pub unchanged: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreshnessChange {
    Added,
    Modified,
    Removed,
    Unchanged,
}

impl OriginFreshnessDiff {
    pub fn changed_paths(&self) -> impl Iterator<Item = &String> {
        self.added.iter().chain(self.modified.iter())
    }

    pub fn change_for(&self, path: &str) -> Option<FreshnessChange> {
        if self.added.contains(path) {
            Some(FreshnessChange::Added)
        } else if self.modified.contains(path) {
            Some(FreshnessChange::Modified)
        } else if self.removed.contains(path) {
            Some(FreshnessChange::Removed)
        } else if self.unchanged.contains(path) {
            Some(FreshnessChange::Unchanged)
        } else {
            None
        }
    }
}

pub fn build_origin_index(
    origin_id: &str,
    summaries: Vec<RemoteObjectSummary>,
    captured_at: DateTime<Utc>,
) -> Result<OriginFreshnessIndex> {
    let mut files = BTreeMap::new();

    for summary in summaries {
        let path = normalize_remote_key(&summary.key)?;
        files.insert(
            path.clone(),
            OriginFileState {
                path,
                created_at: summary.created_at,
                last_modified: summary.last_modified,
                captured_at,
                size: summary.size,
                etag: summary.etag,
                content_type: summary.content_type,
                cache_control: summary.cache_control,
            },
        );
    }

    Ok(OriginFreshnessIndex {
        origin_id: origin_id.to_string(),
        captured_at,
        files,
    })
}

pub fn diff_origin_indexes(
    previous: Option<&OriginFreshnessIndex>,
    next: &OriginFreshnessIndex,
) -> OriginFreshnessDiff {
    let Some(previous) = previous else {
        return OriginFreshnessDiff {
            added: next.files.keys().cloned().collect(),
            modified: BTreeSet::new(),
            removed: BTreeSet::new(),
            unchanged: BTreeSet::new(),
        };
    };

    let mut diff = OriginFreshnessDiff::default();

    for (path, next_file) in &next.files {
        match previous.files.get(path) {
            Some(previous_file) if same_source_state(previous_file, next_file) => {
                diff.unchanged.insert(path.clone());
            }
            Some(_) => {
                diff.modified.insert(path.clone());
            }
            None => {
                diff.added.insert(path.clone());
            }
        }
    }

    for path in previous.files.keys() {
        if !next.files.contains_key(path) {
            diff.removed.insert(path.clone());
        }
    }

    diff
}

fn same_source_state(left: &OriginFileState, right: &OriginFileState) -> bool {
    left.path == right.path
        && left.created_at == right.created_at
        && left.last_modified == right.last_modified
        && left.size == right.size
        && left.etag == right.etag
        && left.content_type == right.content_type
        && left.cache_control == right.cache_control
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::{
        repositories::sync::RemoteObjectSummary,
        services::freshness::{build_origin_index, diff_origin_indexes, FreshnessChange},
    };

    #[test]
    fn diff_detects_added_modified_removed_and_unchanged_files() {
        let first_capture = Utc.with_ymd_and_hms(2026, 5, 22, 10, 0, 0).unwrap();
        let second_capture = Utc.with_ymd_and_hms(2026, 5, 22, 10, 1, 0).unwrap();
        let previous = build_origin_index(
            "web",
            vec![
                summary("index.html", 10, Some("index-v1"), Some("text/html")),
                summary("old.html", 20, Some("old-v1"), Some("text/html")),
                summary("same.css", 30, Some("same-v1"), Some("text/css")),
            ],
            first_capture,
        )
        .expect("previous index builds");
        let next = build_origin_index(
            "web",
            vec![
                summary("index.html", 11, Some("index-v2"), Some("text/html")),
                summary("same.css", 30, Some("same-v1"), Some("text/css")),
                summary("new.html", 40, Some("new-v1"), Some("text/html")),
            ],
            second_capture,
        )
        .expect("next index builds");

        let diff = diff_origin_indexes(Some(&previous), &next);

        assert_eq!(diff.change_for("new.html"), Some(FreshnessChange::Added));
        assert_eq!(
            diff.change_for("index.html"),
            Some(FreshnessChange::Modified)
        );
        assert_eq!(diff.change_for("old.html"), Some(FreshnessChange::Removed));
        assert_eq!(
            diff.change_for("same.css"),
            Some(FreshnessChange::Unchanged)
        );
    }

    #[test]
    fn captured_at_alone_does_not_make_file_modified() {
        let first_capture = Utc.with_ymd_and_hms(2026, 5, 22, 10, 0, 0).unwrap();
        let second_capture = Utc.with_ymd_and_hms(2026, 5, 22, 10, 1, 0).unwrap();
        let previous = build_origin_index(
            "web",
            vec![summary(
                "index.html",
                10,
                Some("index-v1"),
                Some("text/html"),
            )],
            first_capture,
        )
        .expect("previous index builds");
        let next = build_origin_index(
            "web",
            vec![summary(
                "index.html",
                10,
                Some("index-v1"),
                Some("text/html"),
            )],
            second_capture,
        )
        .expect("next index builds");

        let diff = diff_origin_indexes(Some(&previous), &next);

        assert_eq!(
            diff.change_for("index.html"),
            Some(FreshnessChange::Unchanged)
        );
    }

    #[test]
    fn rejects_invalid_source_paths() {
        let captured_at = Utc.with_ymd_and_hms(2026, 5, 22, 10, 0, 0).unwrap();

        let error = build_origin_index(
            "web",
            vec![summary(
                "../secret.html",
                10,
                Some("secret"),
                Some("text/html"),
            )],
            captured_at,
        )
        .expect_err("invalid source path is rejected");

        assert!(error.to_string().contains("invalid object path"));
    }

    fn summary(
        key: &str,
        size: u64,
        etag: Option<&str>,
        content_type: Option<&str>,
    ) -> RemoteObjectSummary {
        RemoteObjectSummary {
            key: key.to_string(),
            created_at: None,
            last_modified: Some("2026-05-22T10:00:00Z".to_string()),
            size,
            etag: etag.map(str::to_string),
            content_type: content_type.map(str::to_string),
            cache_control: None,
        }
    }
}
