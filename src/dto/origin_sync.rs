use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OriginSyncResponse {
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
    pub cdn: Option<OriginSyncCdnResponse>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OriginSyncCdnResponse {
    pub provider: String,
    pub status: String,
    pub request_id: Option<String>,
    pub request_ids: Vec<String>,
    pub submitted_items: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OriginSyncErrorResponse {
    pub error: OriginSyncErrorBody,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OriginSyncErrorBody {
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use serde_json::{json, to_value};

    use super::OriginSyncCdnResponse;

    #[test]
    fn serializes_legacy_request_id_and_full_request_ids() {
        let value = to_value(OriginSyncCdnResponse {
            provider: "cloudfront_saas".to_string(),
            status: "submitted".to_string(),
            request_id: Some("INV-1".to_string()),
            request_ids: vec!["INV-1".to_string(), "INV-2".to_string()],
            submitted_items: 4,
        })
        .expect("serialize response");

        assert_eq!(value["request_id"], "INV-1");
        assert_eq!(value["request_ids"], json!(["INV-1", "INV-2"]));
    }
}
