use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use crate::dto::edge::EdgeConfig;

#[derive(Clone, Debug)]
pub struct EdgeConfigStore {
    inner: Arc<RwLock<BTreeMap<String, EdgeConfigEntry>>>,
}

#[derive(Clone, Debug)]
enum EdgeConfigEntry {
    Valid(Arc<EdgeConfig>),
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeConfigStoreError {
    Missing,
    Invalid(String),
}

impl EdgeConfigStore {
    pub fn from_configs(configs: BTreeMap<String, EdgeConfig>) -> Self {
        let entries = configs
            .into_iter()
            .map(|(origin_id, config)| (origin_id, EdgeConfigEntry::Valid(Arc::new(config))))
            .collect();
        Self {
            inner: Arc::new(RwLock::new(entries)),
        }
    }

    pub fn get(&self, origin_id: &str) -> Result<Arc<EdgeConfig>, EdgeConfigStoreError> {
        let configs = self.inner.read().expect("edge config store lock");
        match configs.get(origin_id) {
            Some(EdgeConfigEntry::Valid(config)) => Ok(config.clone()),
            Some(EdgeConfigEntry::Invalid(error)) => {
                Err(EdgeConfigStoreError::Invalid(error.clone()))
            }
            None => Err(EdgeConfigStoreError::Missing),
        }
    }

    pub fn set_valid(&self, origin_id: impl Into<String>, config: EdgeConfig) {
        self.inner
            .write()
            .expect("edge config store lock")
            .insert(origin_id.into(), EdgeConfigEntry::Valid(Arc::new(config)));
    }

    pub fn set_invalid(&self, origin_id: impl Into<String>, error: impl Into<String>) {
        self.inner
            .write()
            .expect("edge config store lock")
            .insert(origin_id.into(), EdgeConfigEntry::Invalid(error.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::edge_config::default_edge_config;

    #[test]
    fn returns_invalid_state_until_replaced_by_valid_config() {
        let store = EdgeConfigStore::from_configs(BTreeMap::new());

        store.set_invalid("web", "parse error");
        assert_eq!(
            store.get("web").expect_err("invalid"),
            EdgeConfigStoreError::Invalid("parse error".to_string())
        );

        store.set_valid("web", default_edge_config());
        assert_eq!(store.get("web").expect("valid").version, 1);
    }
}
