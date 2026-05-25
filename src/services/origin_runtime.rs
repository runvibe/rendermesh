use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use serde::Serialize;

#[derive(Clone, Debug, Default)]
pub struct OriginRuntimeStore {
    snapshots: Arc<RwLock<BTreeMap<String, OriginSnapshotDebug>>>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OriginSnapshotDebug {
    pub origin_id: String,
    pub generation: u64,
    pub activated_at: String,
    pub captured_at: String,
    pub known_files: usize,
    pub added_files: usize,
    pub modified_files: usize,
    pub removed_files: usize,
    pub unchanged_files: usize,
    pub downloaded_files: usize,
    pub last_error: Option<String>,
    pub last_cdn_provider: Option<String>,
    pub last_cdn_status: Option<String>,
    pub last_cdn_request_id: Option<String>,
    pub last_cdn_refreshed_at: Option<String>,
    pub last_cdn_submitted_items: Option<usize>,
    pub last_cdn_error: Option<String>,
    pub last_cdn_domain_provider: Option<String>,
    pub last_cdn_domain_status: Option<String>,
    pub last_cdn_domain_reconciled_at: Option<String>,
    pub last_cdn_domain_added: Option<usize>,
    pub last_cdn_domain_updated: Option<usize>,
    pub last_cdn_domain_removed: Option<usize>,
    pub last_cdn_domain_unchanged: Option<usize>,
    pub last_cdn_domain_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OriginListDebug {
    pub origins: Vec<OriginSnapshotDebug>,
}

impl OriginRuntimeStore {
    pub fn set_snapshot(&self, snapshot: OriginSnapshotDebug) {
        let mut snapshots = self.snapshots.write().expect("origin runtime lock");
        let snapshot = match snapshots.get(&snapshot.origin_id) {
            Some(previous) => OriginSnapshotDebug {
                last_cdn_provider: previous.last_cdn_provider.clone(),
                last_cdn_status: previous.last_cdn_status.clone(),
                last_cdn_request_id: previous.last_cdn_request_id.clone(),
                last_cdn_refreshed_at: previous.last_cdn_refreshed_at.clone(),
                last_cdn_submitted_items: previous.last_cdn_submitted_items,
                last_cdn_error: previous.last_cdn_error.clone(),
                last_cdn_domain_provider: previous.last_cdn_domain_provider.clone(),
                last_cdn_domain_status: previous.last_cdn_domain_status.clone(),
                last_cdn_domain_reconciled_at: previous.last_cdn_domain_reconciled_at.clone(),
                last_cdn_domain_added: previous.last_cdn_domain_added,
                last_cdn_domain_updated: previous.last_cdn_domain_updated,
                last_cdn_domain_removed: previous.last_cdn_domain_removed,
                last_cdn_domain_unchanged: previous.last_cdn_domain_unchanged,
                last_cdn_domain_error: previous.last_cdn_domain_error.clone(),
                ..snapshot
            },
            None => snapshot,
        };
        snapshots.insert(snapshot.origin_id.clone(), snapshot);
    }

    pub fn set_error(&self, origin_id: impl Into<String>, error: impl Into<String>) {
        let origin_id = origin_id.into();
        let error = error.into();
        let mut snapshots = self.snapshots.write().expect("origin runtime lock");
        match snapshots.get_mut(&origin_id) {
            Some(snapshot) => snapshot.last_error = Some(error),
            None => {
                snapshots.insert(
                    origin_id.clone(),
                    OriginSnapshotDebug {
                        origin_id,
                        generation: 0,
                        activated_at: String::new(),
                        captured_at: String::new(),
                        known_files: 0,
                        added_files: 0,
                        modified_files: 0,
                        removed_files: 0,
                        unchanged_files: 0,
                        downloaded_files: 0,
                        last_error: Some(error),
                        last_cdn_provider: None,
                        last_cdn_status: None,
                        last_cdn_request_id: None,
                        last_cdn_refreshed_at: None,
                        last_cdn_submitted_items: None,
                        last_cdn_error: None,
                        last_cdn_domain_provider: None,
                        last_cdn_domain_status: None,
                        last_cdn_domain_reconciled_at: None,
                        last_cdn_domain_added: None,
                        last_cdn_domain_updated: None,
                        last_cdn_domain_removed: None,
                        last_cdn_domain_unchanged: None,
                        last_cdn_domain_error: None,
                    },
                );
            }
        }
    }

    pub fn set_cdn_result(
        &self,
        origin_id: &str,
        provider: impl Into<String>,
        status: impl Into<String>,
        request_id: Option<String>,
        submitted_items: usize,
    ) {
        if let Some(snapshot) = self
            .snapshots
            .write()
            .expect("origin runtime lock")
            .get_mut(origin_id)
        {
            snapshot.last_cdn_provider = Some(provider.into());
            snapshot.last_cdn_status = Some(status.into());
            snapshot.last_cdn_request_id = request_id;
            snapshot.last_cdn_refreshed_at = Some(chrono::Utc::now().to_rfc3339());
            snapshot.last_cdn_submitted_items = Some(submitted_items);
            snapshot.last_cdn_error = None;
        }
    }

    pub fn set_cdn_error(&self, origin_id: &str, error: impl Into<String>) {
        if let Some(snapshot) = self
            .snapshots
            .write()
            .expect("origin runtime lock")
            .get_mut(origin_id)
        {
            snapshot.last_cdn_error = Some(error.into());
        }
    }

    pub fn set_cdn_domain_result(
        &self,
        origin_id: &str,
        provider: impl Into<String>,
        status: impl Into<String>,
        added: usize,
        updated: usize,
        removed: usize,
        unchanged: usize,
    ) {
        if let Some(snapshot) = self
            .snapshots
            .write()
            .expect("origin runtime lock")
            .get_mut(origin_id)
        {
            snapshot.last_cdn_domain_provider = Some(provider.into());
            snapshot.last_cdn_domain_status = Some(status.into());
            snapshot.last_cdn_domain_reconciled_at = Some(chrono::Utc::now().to_rfc3339());
            snapshot.last_cdn_domain_added = Some(added);
            snapshot.last_cdn_domain_updated = Some(updated);
            snapshot.last_cdn_domain_removed = Some(removed);
            snapshot.last_cdn_domain_unchanged = Some(unchanged);
            snapshot.last_cdn_domain_error = None;
        }
    }

    pub fn set_cdn_domain_error(&self, origin_id: &str, error: impl Into<String>) {
        if let Some(snapshot) = self
            .snapshots
            .write()
            .expect("origin runtime lock")
            .get_mut(origin_id)
        {
            snapshot.last_cdn_domain_error = Some(error.into());
        }
    }

    pub fn list(&self) -> OriginListDebug {
        OriginListDebug {
            origins: self
                .snapshots
                .read()
                .expect("origin runtime lock")
                .values()
                .cloned()
                .collect(),
        }
    }

    pub fn get(&self, origin_id: &str) -> Option<OriginSnapshotDebug> {
        self.snapshots
            .read()
            .expect("origin runtime lock")
            .get(origin_id)
            .cloned()
    }
}
