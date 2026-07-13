//! Sandboxed Agent loop for platforms that cannot embed the full Codex core.
//!
//! This backend runs thread state and event generation in the application
//! process. It calls only the configured model Responses endpoint and never a
//! remote Agent gateway. It intentionally exposes no native shell, process,
//! Git, or unrestricted filesystem tools.

use async_trait::async_trait;
use mahayana_agent::{
    AgentBackend, AgentError, AgentEvent, AgentMessageRequest, ApprovalResolution,
    SharedAgentEventSink, StartThreadRequest,
};
use mahayana_core::{AgentThreadId, Message, MessageId, MessageRole, OperationId};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ResponsesAgentConfig {
    pub model: String,
    pub responses_base_url: String,
    pub product_session_token: String,
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
        if self.product_session_token.trim().is_empty()
            || self.product_session_token.contains(['\r', '\n'])
        {
            return Err(AgentError::Unavailable(
                "a valid Mahayana product session is required".into(),
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
        let text = inference?;
        self.histories
            .lock()
            .map_err(|_| AgentError::Backend("Responses history mutex poisoned".into()))?
            .get_mut(&request.thread_id)
            .ok_or_else(|| AgentError::ThreadNotFound(request.thread_id.clone()))?
            .push(json!({"role": "assistant", "content": text}));

        events.emit(AgentEvent::MessageDelta {
            delta: text.clone(),
        })?;
        events.emit(AgentEvent::MessageCompleted {
            message: Message {
                id: MessageId::generated("responses-message"),
                conversation_id: request.conversation_id,
                role: MessageRole::Assistant,
                text,
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
) -> Result<String, AgentError> {
    let endpoint = if config.responses_base_url.ends_with("/responses") {
        config.responses_base_url.clone()
    } else {
        format!(
            "{}/responses",
            config.responses_base_url.trim_end_matches('/')
        )
    };
    let response = ureq::post(&endpoint)
        .set(
            "Authorization",
            &format!("Bearer {}", config.product_session_token),
        )
        .set("Accept", "application/json")
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
    extract_output_text(&payload).ok_or_else(|| {
        AgentError::Backend("Responses endpoint returned no assistant output text".into())
    })
}

fn redacted_http_error(error: ureq::Error) -> AgentError {
    match error {
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
    fn config_rejects_non_https_and_header_injection() {
        let mut config = ResponsesAgentConfig {
            model: "deepseek-chat".into(),
            responses_base_url: "http://example.test/v1".into(),
            product_session_token: "secret".into(),
        };
        assert!(config.validate().is_err());
        config.responses_base_url = "https://example.test/v1".into();
        config.product_session_token = "secret\nInjected: yes".into();
        assert!(config.validate().is_err());
    }
}
