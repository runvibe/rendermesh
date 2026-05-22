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
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OriginListDebug {
    pub origins: Vec<OriginSnapshotDebug>,
}

impl OriginRuntimeStore {
    pub fn set_snapshot(&self, snapshot: OriginSnapshotDebug) {
        self.snapshots
            .write()
            .expect("origin runtime lock")
            .insert(snapshot.origin_id.clone(), snapshot);
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
                    },
                );
            }
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
