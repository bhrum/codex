//! Model inference boundary for native, mobile, and Web runtimes.

use async_trait::async_trait;
use mahayana_core::ModelProviderMode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequest {
    pub model: String,
    pub input: Value,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    OutputTextDelta(String),
    Completed { output: Value },
    Failed { code: String, message: String },
}

/// Receives streaming model events. Mobile and Web implementations should
/// forward from their native token callbacks without blocking the inference
/// thread.
pub trait ModelEventSink: Send + Sync {
    fn emit(&self, event: ModelEvent) -> Result<(), ModelError>;
}

pub type SharedModelEventSink = Arc<dyn ModelEventSink>;

/// Model inference implementation. The Agent loop is independent from whether
/// inference is a local library, loopback service, or explicitly enabled
/// first-party/user endpoint.
#[async_trait]
pub trait ModelRuntime: Send + Sync {
    async fn infer(
        &self,
        request: ModelRequest,
        events: SharedModelEventSink,
    ) -> Result<(), ModelError>;

    fn provider_mode(&self) -> ModelProviderMode;

    fn is_local(&self) -> bool {
        matches!(
            self.provider_mode(),
            ModelProviderMode::LocalModel | ModelProviderMode::LocalLoopback
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("model runtime is unavailable: {0}")]
    Unavailable(String),
    #[error("model request is invalid: {0}")]
    InvalidRequest(String),
    #[error("model inference failed: {0}")]
    Inference(String),
    #[error("model event consumer is closed")]
    EventConsumerClosed,
}
