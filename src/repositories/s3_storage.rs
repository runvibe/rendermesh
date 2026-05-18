use anyhow::Result;
use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::{config::Region, Client};
use bytes::Bytes;

use crate::{
    dto::manifest::OriginConfig,
    repositories::sync::{RemoteObject, RemoteObjectSummary, RemoteStorage},
};

#[derive(Clone)]
pub struct S3StorageRepository {
    client: Client,
    bucket: String,
}

impl S3StorageRepository {
    pub async fn from_origin_config(origin: &OriginConfig) -> Result<Self> {
        let endpoint = std::env::var(&origin.endpoint_env)?;
        let region = std::env::var(&origin.region_env)?;
        let access_key_id = std::env::var(&origin.access_key_id_env)?;
        let secret_access_key = std::env::var(&origin.secret_access_key_env)?;
        let force_path_style = origin
            .force_path_style_env
            .as_ref()
            .and_then(|key| std::env::var(key).ok())
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false);

        let credentials =
            Credentials::new(access_key_id, secret_access_key, None, None, "rendermesh");
        let config = aws_sdk_s3::config::Builder::new()
            .endpoint_url(endpoint)
            .region(Region::new(region))
            .credentials_provider(credentials)
            .force_path_style(force_path_style)
            .build();

        Ok(Self {
            client: Client::from_conf(config),
            bucket: origin.bucket.clone(),
        })
    }
}

#[async_trait]
impl RemoteStorage for S3StorageRepository {
    async fn list_objects(&self) -> Result<Vec<RemoteObjectSummary>> {
        let mut output = Vec::new();
        let mut continuation_token = None;

        loop {
            let response = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .set_continuation_token(continuation_token)
                .send()
                .await?;

            for object in response.contents() {
                if let Some(key) = object.key() {
                    output.push(RemoteObjectSummary {
                        key: key.to_string(),
                        etag: object.e_tag().map(ToString::to_string),
                        last_modified: object.last_modified().map(ToString::to_string),
                        size: object.size().unwrap_or_default() as u64,
                        content_type: None,
                        cache_control: None,
                    });
                }
            }

            if response.is_truncated().unwrap_or(false) {
                continuation_token = response.next_continuation_token().map(ToString::to_string);
            } else {
                break;
            }
        }

        Ok(output)
    }

    async fn get_object(&self, key: &str) -> Result<RemoteObject> {
        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await?;
        let etag = response.e_tag().map(ToString::to_string);
        let last_modified = response.last_modified().map(ToString::to_string);
        let content_type = response.content_type().map(ToString::to_string);
        let cache_control = response.cache_control().map(ToString::to_string);
        let body = response.body.collect().await?.into_bytes();

        Ok(RemoteObject {
            key: key.to_string(),
            body: Bytes::from(body.to_vec()),
            etag,
            last_modified,
            content_type,
            cache_control,
        })
    }
}
