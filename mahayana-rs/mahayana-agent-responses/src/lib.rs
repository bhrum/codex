//! Sandboxed Agent loop for platforms that cannot embed the full Codex core.
//!
//! This backend runs thread state and event generation in the application
//! process. It calls only the configured model Responses endpoint and never a
//! remote Agent gateway. It intentionally exposes no native shell, process,
//! Git, or unrestricted filesystem tools.

use async_trait::async_trait;
use mahayana_agent::AgentBackend;
use mahayana_agent::AgentError;
use mahayana_agent::AgentEvent;
use mahayana_agent::AgentMessageRequest;
use mahayana_agent::ApprovalResolution;
use mahayana_agent::SharedAgentEventSink;
use mahayana_agent::StartThreadRequest;
use mahayana_core::AgentThreadId;
use mahayana_core::Message;
use mahayana_core::MessageId;
use mahayana_core::MessageRole;
use mahayana_core::ModelTokenUsage;
use mahayana_core::ModelTokenUsageSnapshot;
use mahayana_core::OperationId;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone)]
pub struct ResponsesAgentConfig {
    pub model: String,
    pub responses_base_url: String,
    /// Optional product account session. The first-party endpoint provides a
    /// bounded anonymous allowance when this is absent.
    pub product_session_token: Option<String>,
}

impl ResponsesAgentConfig {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.model.trim().is_empty() {
            return Err(AgentError::Unavailable(
                "Dacheng Responses model must not be empty".into(),
            ));
        }
        if !self.responses_base_url.starts_with("https://") {
            return Err(AgentError::Unavailable(
                "Dacheng Responses endpoint must use HTTPS".into(),
            ));
        }
        if self
            .product_session_token
            .as_deref()
            .is_some_and(|token| token.trim().is_empty() || token.contains(['\r', '\n']))
        {
            return Err(AgentError::Unavailable(
                "Mahayana product session is invalid".into(),
            ));
        }
        Ok(())
    }
}

/// Device-local Agent state paired with first-party model inference.
pub struct ResponsesAgentBackend {
    config: ResponsesAgentConfig,
    histories: Mutex<HashMap<AgentThreadId, Vec<Value>>>,
    active_operations: Mutex<HashSet<OperationId>>,
    interrupted_operations: Mutex<HashSet<OperationId>>,
}

impl ResponsesAgentBackend {
    pub fn new(config: ResponsesAgentConfig) -> Result<Self, AgentError> {
        config.validate()?;
        Ok(Self {
            config,
            histories: Mutex::new(HashMap::new()),
            active_operations: Mutex::new(HashSet::new()),
            interrupted_operations: Mutex::new(HashSet::new()),
        })
    }

    fn request_input(&self, request: &AgentMessageRequest) -> Result<Vec<Value>, AgentError> {
        let mut histories = self
            .histories
            .lock()
            .map_err(|_| AgentError::Backend("Responses history mutex poisoned".into()))?;
        let history = histories
            .get_mut(&request.thread_id)
            .ok_or_else(|| AgentError::ThreadNotFound(request.thread_id.clone()))?;
        history.push(json!({"role": "user", "content": request.text}));
        Ok(history.clone())
    }

    fn finish_operation(&self, operation_id: &OperationId) -> Result<bool, AgentError> {
        self.active_operations
            .lock()
            .map_err(|_| AgentError::Backend("Responses operation mutex poisoned".into()))?
            .remove(operation_id);
        Ok(self
            .interrupted_operations
            .lock()
            .map_err(|_| AgentError::Backend("Responses interrupt mutex poisoned".into()))?
            .remove(operation_id))
    }
}

#[async_trait]
impl AgentBackend for ResponsesAgentBackend {
    async fn start_thread(
        &self,
        _request: StartThreadRequest,
    ) -> Result<AgentThreadId, AgentError> {
        let thread_id = AgentThreadId::generated("responses-thread");
        self.histories
            .lock()
            .map_err(|_| AgentError::Backend("Responses history mutex poisoned".into()))?
            .insert(thread_id.clone(), Vec::new());
        Ok(thread_id)
    }

    async fn send_message(
        &self,
        request: AgentMessageRequest,
        events: SharedAgentEventSink,
    ) -> Result<(), AgentError> {
        let input = self.request_input(&request)?;
        self.active_operations
            .lock()
            .map_err(|_| AgentError::Backend("Responses operation mutex poisoned".into()))?
            .insert(request.operation_id.clone());

        let config = self.config.clone();
        let inference = tokio::task::spawn_blocking(move || request_response(&config, input)).await;
        let interrupted = self.finish_operation(&request.operation_id)?;
        if interrupted {
            return Err(AgentError::Backend("operation interrupted".into()));
        }
        let inference = inference
            .map_err(|error| AgentError::Backend(format!("Responses task failed: {error}")))?;
        let inference = inference?;
        self.histories
            .lock()
            .map_err(|_| AgentError::Backend("Responses history mutex poisoned".into()))?
            .get_mut(&request.thread_id)
            .ok_or_else(|| AgentError::ThreadNotFound(request.thread_id.clone()))?
            .push(json!({"role": "assistant", "content": inference.text.clone()}));

        events.emit(AgentEvent::MessageDelta {
            delta: inference.text.clone(),
        })?;
        if let Some(usage) = inference.usage {
            events.emit(AgentEvent::TokenUsageUpdated { usage })?;
        }
        events.emit(AgentEvent::MessageCompleted {
            message: Message {
                id: MessageId::generated("responses-message"),
                conversation_id: request.conversation_id,
                role: MessageRole::Assistant,
                text: inference.text,
                created_at_ms: now_ms(),
                metadata: json!({
                    "agentBackend": self.name(),
                    "sandbox": "mobile-embedded",
                    "nativeProcess": false,
                    "nativeGit": false,
                }),
            },
        })
    }

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), AgentError> {
        if !self
            .active_operations
            .lock()
            .map_err(|_| AgentError::Backend("Responses operation mutex poisoned".into()))?
            .contains(operation_id)
        {
            return Err(AgentError::OperationNotFound(operation_id.clone()));
        }
        self.interrupted_operations
            .lock()
            .map_err(|_| AgentError::Backend("Responses interrupt mutex poisoned".into()))?
            .insert(operation_id.clone());
        Ok(())
    }

    async fn resolve_approval(&self, resolution: ApprovalResolution) -> Result<(), AgentError> {
        Err(AgentError::ApprovalNotFound(resolution.approval_id))
    }

    fn name(&self) -> &'static str {
        "dacheng-responses-device-agent"
    }
}

fn request_response(
    config: &ResponsesAgentConfig,
    input: Vec<Value>,
) -> Result<InferenceResult, AgentError> {
    let endpoint = if config.responses_base_url.ends_with("/responses") {
        config.responses_base_url.clone()
    } else {
        format!(
            "{}/responses",
            config.responses_base_url.trim_end_matches('/')
        )
    };
    let mut request = ureq::post(&endpoint).set("Accept", "application/json");
    if let Some(token) = config.product_session_token.as_deref() {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let response = request
        .send_json(json!({
            "model": config.model,
            "input": input,
            "stream": false,
        }))
        .map_err(redacted_http_error)?;
    let payload: Value = response
        .into_json()
        .map_err(|_| AgentError::Backend("Responses endpoint returned invalid JSON".into()))?;
    if let Some(error) = payload.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Responses endpoint returned an error");
        return Err(AgentError::Backend(message.to_string()));
    }
    let text = extract_output_text(&payload).ok_or_else(|| {
        AgentError::Backend("Responses endpoint returned no assistant output text".into())
    })?;
    Ok(InferenceResult {
        text,
        usage: extract_token_usage(&payload),
    })
}

struct InferenceResult {
    text: String,
    usage: Option<ModelTokenUsageSnapshot>,
}

fn redacted_http_error(error: ureq::Error) -> AgentError {
    match error {
        ureq::Error::Status(429, response) => {
            let payload = response.into_json::<Value>().unwrap_or(Value::Null);
            let message = payload
                .pointer("/error/message")
                .or_else(|| payload.get("message"))
                .and_then(Value::as_str)
                .filter(|message| !message.trim().is_empty())
                .unwrap_or("本月模型 token 额度已用完")
                .to_string();
            AgentError::UsageLimitExceeded(message)
        }
        ureq::Error::Status(status, _) => {
            AgentError::Backend(format!("Responses endpoint returned HTTP {status}"))
        }
        ureq::Error::Transport(error) => {
            AgentError::Backend(format!("Responses transport failed: {error}"))
        }
    }
}

fn extract_output_text(payload: &Value) -> Option<String> {
    if let Some(text) = payload.get("output_text").and_then(Value::as_str) {
        return non_empty(text);
    }
    if let Some(output) = payload.get("output").and_then(Value::as_array) {
        let text = output
            .iter()
            .filter_map(|item| item.get("content").and_then(Value::as_array))
            .flatten()
            .filter_map(|content| content.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        if let Some(text) = non_empty(&text) {
            return Some(text);
        }
    }
    payload
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .and_then(non_empty)
}

fn extract_token_usage(payload: &Value) -> Option<ModelTokenUsageSnapshot> {
    let usage = payload.get("usage").or_else(|| {
        payload
            .get("response")
            .and_then(|response| response.get("usage"))
    })?;
    let input_tokens = usage_i64(usage, &["input_tokens", "prompt_tokens", "inputTokens"]);
    let cached_input_tokens = usage_i64(usage, &["cached_input_tokens", "cachedInputTokens"]).max(
        usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    );
    let output_tokens = usage_i64(
        usage,
        &["output_tokens", "completion_tokens", "outputTokens"],
    );
    let reasoning_output_tokens =
        usage_i64(usage, &["reasoning_output_tokens", "reasoningOutputTokens"]).max(
            usage
                .pointer("/output_tokens_details/reasoning_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        );
    let explicit_total = usage_i64(usage, &["total_tokens", "totalTokens"]);
    let total_tokens = explicit_total.max(input_tokens.saturating_add(output_tokens));
    if total_tokens <= 0 {
        return None;
    }
    Some(ModelTokenUsageSnapshot {
        total: None,
        last: ModelTokenUsage {
            total_tokens,
            input_tokens,
            cached_input_tokens,
            output_tokens,
            reasoning_output_tokens,
        },
        model_context_window: None,
    })
}

fn usage_i64(usage: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| usage.get(*key).and_then(Value::as_i64))
        .unwrap_or(0)
        .max(0)
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_responses_and_compatible_chat_payloads() {
        let responses = json!({
            "output": [{"content": [
                {"type": "output_text", "text": "南无"},
                {"type": "output_text", "text": "阿弥陀佛"}
            ]}]
        });
        assert_eq!(
            extract_output_text(&responses).as_deref(),
            Some("南无阿弥陀佛")
        );
        let chat = json!({"choices": [{"message": {"content": "善哉"}}]});
        assert_eq!(extract_output_text(&chat).as_deref(), Some("善哉"));
    }

    #[test]
    fn projects_provider_usage_without_estimating_tokens() {
        let payload = json!({
            "usage": {
                "input_tokens": 120,
                "cached_input_tokens": 40,
                "output_tokens": 30,
                "reasoning_output_tokens": 5,
                "total_tokens": 150
            }
        });
        assert_eq!(
            extract_token_usage(&payload),
            Some(ModelTokenUsageSnapshot {
                total: None,
                last: ModelTokenUsage {
                    total_tokens: 150,
                    input_tokens: 120,
                    cached_input_tokens: 40,
                    output_tokens: 30,
                    reasoning_output_tokens: 5,
                },
                model_context_window: None,
            })
        );
    }

    #[test]
    fn config_rejects_non_https_and_header_injection() {
        let mut config = ResponsesAgentConfig {
            model: "deepseek-chat".into(),
            responses_base_url: "http://example.test/v1".into(),
            product_session_token: Some("secret".into()),
        };
        assert!(config.validate().is_err());
        config.responses_base_url = "https://example.test/v1".into();
        config.product_session_token = Some("secret\nInjected: yes".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn config_accepts_anonymous_first_party_allowance() {
        let config = ResponsesAgentConfig {
            model: "deepseek-chat".into(),
            responses_base_url: "https://example.test/v1".into(),
            product_session_token: None,
        };
        assert!(config.validate().is_ok());
    }
}
