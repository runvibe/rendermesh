use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

pub fn parse_config<T>(name: &str, input: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    match serde_norway::from_str::<T>(input) {
        Ok(value) => Ok(value),
        Err(yaml_error) => serde_json::from_str::<T>(input).with_context(|| {
            format!("failed to parse {name} as YAML or JSON; YAML error: {yaml_error}")
        }),
    }
}
