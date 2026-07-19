use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageReservationRequest {
    pub request_id: String,
    pub input_token_budget: i64,
    pub output_token_budget: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReservation {
    pub reservation_id: String,
    pub request_id: String,
    pub reserved_tokens: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageCaptureRequest {
    pub provider_response_id: String,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageStatus {
    pub window_start: i64,
    pub window_end: i64,
    pub token_limit: i64,
    pub used_tokens: i64,
    pub reserved_tokens: i64,
    pub remaining_tokens: i64,
}
