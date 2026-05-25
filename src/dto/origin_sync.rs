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
