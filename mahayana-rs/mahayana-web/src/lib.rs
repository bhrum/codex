//! Browser-native Mahayana Runtime.
//!
//! Conversation state, operation routing, and event generation remain inside
//! WebAssembly. Model inference uses the browser Fetch API directly against
//! the configured Responses provider; there is no remote Agent gateway and no
//! desktop shell, process, or Git capability.

use mahayana_core::{
    ApprovalId, BuildProfile, CONVERSATION_SCHEMA_VERSION, Conversation, ConversationId,
    DEFAULT_DACHENG_RESPONSES_BASE_URL, DEFAULT_DEEPSEEK_MODEL, MODEL_RUNTIME_VERSION, Message,
    MessageId, MessageRole, ModelProviderMode, OperationId, PeerKind, RUNTIME_ABI_VERSION,
    RuntimeCommand, RuntimeEvent, RuntimeResponse, RuntimeStatus,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Request, RequestInit, RequestMode, Response};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct WebConfig {
    model: String,
    responses_base_url: String,
    product_session_token: Option<String>,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_DEEPSEEK_MODEL.to_string(),
            responses_base_url: DEFAULT_DACHENG_RESPONSES_BASE_URL.to_string(),
            product_session_token: None,
        }
    }
}

struct WebState {
    config: WebConfig,
    histories: HashMap<ConversationId, Vec<Message>>,
    model_inputs: HashMap<ConversationId, Vec<Value>>,
    events: VecDeque<RuntimeEvent>,
    active_operations: HashSet<OperationId>,
    next_id: u64,
}

impl WebState {
    fn next_id(&mut self, prefix: &str) -> String {
        self.next_id = self.next_id.saturating_add(1);
        format!("{prefix}:web:{}", self.next_id)
    }

    fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            runtime_abi_version: RUNTIME_ABI_VERSION,
            conversation_schema_version: CONVERSATION_SCHEMA_VERSION,
            model_runtime_version: MODEL_RUNTIME_VERSION,
            build_profile: BuildProfile::WebWasm,
            model_provider: ModelProviderMode::FirstPartyDacheng,
            model: self.config.model.clone(),
            remote_agent_enabled: false,
            telemetry_enabled: false,
            providers: vec!["codex".into(), "miniapp".into()],
        }
    }
}

/// Long-lived browser runtime with the same JSON commands/events as native.
#[wasm_bindgen]
pub struct MahayanaWebRuntime {
    state: Rc<RefCell<WebState>>,
}

#[wasm_bindgen]
impl MahayanaWebRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new(config_json: &str) -> Result<MahayanaWebRuntime, JsValue> {
        let config: WebConfig = if config_json.trim().is_empty() {
            WebConfig::default()
        } else {
            serde_json::from_str(config_json).map_err(js_error)?
        };
        if !config.responses_base_url.starts_with("https://") {
            return Err(JsValue::from_str("Responses endpoint must use HTTPS"));
        }
        if config
            .product_session_token
            .as_deref()
            .is_some_and(|token| token.contains(['\r', '\n']))
        {
            return Err(JsValue::from_str("Mahayana product session is invalid"));
        }
        let mut state = WebState {
            config,
            histories: HashMap::new(),
            model_inputs: HashMap::new(),
            events: VecDeque::new(),
            active_operations: HashSet::new(),
            next_id: 0,
        };
        state.events.push_back(RuntimeEvent::Ready {
            status: state.status(),
        });
        Ok(Self {
            state: Rc::new(RefCell::new(state)),
        })
    }

    pub fn execute(&self, command_json: &str) -> Result<String, JsValue> {
        let command: RuntimeCommand = serde_json::from_str(command_json).map_err(js_error)?;
        let response = match command {
            RuntimeCommand::Status => RuntimeResponse::Status(self.state.borrow().status()),
            RuntimeCommand::ListConversations => RuntimeResponse::Conversations {
                data: browser_conversations(),
            },
            RuntimeCommand::ConversationHistory {
                conversation_id,
                limit,
            } => {
                ensure_browser_conversation(&conversation_id)?;
                let state = self.state.borrow();
                let history = state
                    .histories
                    .get(&conversation_id)
                    .cloned()
                    .unwrap_or_default();
                let limit = limit.unwrap_or(50).clamp(1, 500) as usize;
                let start = history.len().saturating_sub(limit);
                RuntimeResponse::History {
                    data: history[start..].to_vec(),
                }
            }
            RuntimeCommand::SendMessage {
                conversation_id,
                text,
                client_message_id,
            } => {
                ensure_browser_conversation(&conversation_id)?;
                if text.trim().is_empty() {
                    return Err(JsValue::from_str("message text must not be empty"));
                }
                let (operation_id, input, config) = {
                    let mut state = self.state.borrow_mut();
                    let token_available = state
                        .config
                        .product_session_token
                        .as_deref()
                        .is_some_and(|token| !token.trim().is_empty());
                    if !token_available {
                        return Err(JsValue::from_str(
                            "请先使用支付宝登录；浏览器不会回退到云端 Agent",
                        ));
                    }
                    let operation_id = OperationId(state.next_id("operation"));
                    let message_id = client_message_id
                        .and_then(|id| MessageId::new(id).ok())
                        .unwrap_or_else(|| MessageId(state.next_id("message")));
                    state
                        .histories
                        .entry(conversation_id.clone())
                        .or_default()
                        .push(Message {
                            id: message_id,
                            conversation_id: conversation_id.clone(),
                            role: MessageRole::User,
                            text: text.clone(),
                            created_at_ms: now_ms(),
                            metadata: json!({"sandbox": "web-wasm"}),
                        });
                    let prompt = miniapp_prompt(&conversation_id, &text);
                    let input = state
                        .model_inputs
                        .entry(conversation_id.clone())
                        .or_default();
                    input.push(json!({"role": "user", "content": prompt}));
                    let input = input.clone();
                    state.active_operations.insert(operation_id.clone());
                    (operation_id, input, state.config.clone())
                };
                spawn_inference(
                    Rc::clone(&self.state),
                    operation_id.clone(),
                    conversation_id,
                    input,
                    config,
                );
                RuntimeResponse::Accepted { operation_id }
            }
            RuntimeCommand::Interrupt { operation_id } => {
                if !self
                    .state
                    .borrow_mut()
                    .active_operations
                    .remove(&operation_id)
                {
                    return Err(JsValue::from_str("operation was not found"));
                }
                RuntimeResponse::Interrupted { operation_id }
            }
            RuntimeCommand::ResolveApproval { approval_id, .. } => {
                return Err(approval_not_found(approval_id));
            }
        };
        serde_json::to_string(&json!({"ok": true, "data": response})).map_err(js_error)
    }

    /// Returns one queued event, or null when the browser queue is empty.
    pub fn receive(&self) -> Result<Option<String>, JsValue> {
        self.state
            .borrow_mut()
            .events
            .pop_front()
            .map(|event| {
                serde_json::to_string(&json!({"ok": true, "data": event})).map_err(js_error)
            })
            .transpose()
    }
}

fn spawn_inference(
    state: Rc<RefCell<WebState>>,
    operation_id: OperationId,
    conversation_id: ConversationId,
    input: Vec<Value>,
    config: WebConfig,
) {
    spawn_local(async move {
        let result = fetch_response(&config, input).await;
        let mut state = state.borrow_mut();
        if !state.active_operations.remove(&operation_id) {
            return;
        }
        match result {
            Ok(text) => {
                state
                    .model_inputs
                    .entry(conversation_id.clone())
                    .or_default()
                    .push(json!({"role": "assistant", "content": text}));
                let role = if conversation_id.as_str().starts_with("miniapp:") {
                    MessageRole::MiniApp
                } else {
                    MessageRole::Assistant
                };
                let message = Message {
                    id: MessageId(state.next_id("message")),
                    conversation_id: conversation_id.clone(),
                    role,
                    text: text.clone(),
                    created_at_ms: now_ms(),
                    metadata: json!({
                        "agentBackend": "dacheng-responses-web-wasm",
                        "nativeProcess": false,
                        "nativeGit": false,
                    }),
                };
                state
                    .histories
                    .entry(conversation_id.clone())
                    .or_default()
                    .push(message.clone());
                state.events.push_back(RuntimeEvent::MessageDelta {
                    operation_id: operation_id.clone(),
                    conversation_id,
                    delta: text,
                });
                state.events.push_back(RuntimeEvent::MessageCompleted {
                    operation_id: operation_id.clone(),
                    message,
                });
                state
                    .events
                    .push_back(RuntimeEvent::OperationCompleted { operation_id });
            }
            Err(message) => state.events.push_back(RuntimeEvent::OperationFailed {
                operation_id,
                code: "model_inference_failed".into(),
                message,
            }),
        }
    });
}

async fn fetch_response(config: &WebConfig, input: Vec<Value>) -> Result<String, String> {
    let token = config
        .product_session_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| "Mahayana product session is required".to_string())?;
    let endpoint = if config.responses_base_url.ends_with("/responses") {
        config.responses_base_url.clone()
    } else {
        format!(
            "{}/responses",
            config.responses_base_url.trim_end_matches('/')
        )
    };
    let body = serde_json::to_string(&json!({
        "model": config.model,
        "input": input,
        "stream": false,
    }))
    .map_err(|_| "could not encode Responses request".to_string())?;
    let options = RequestInit::new();
    options.set_method("POST");
    options.set_mode(RequestMode::Cors);
    options.set_body(&JsValue::from_str(&body));
    let request = Request::new_with_str_and_init(&endpoint, &options)
        .map_err(|_| "could not create Responses request".to_string())?;
    request
        .headers()
        .set("Authorization", &format!("Bearer {token}"))
        .map_err(|_| "could not set Responses authorization".to_string())?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|_| "could not set Responses content type".to_string())?;
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_string())?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| "Responses fetch failed".to_string())?
        .dyn_into::<Response>()
        .map_err(|_| "Responses fetch returned an invalid response".to_string())?;
    if !response.ok() {
        return Err(format!(
            "Responses endpoint returned HTTP {}",
            response.status()
        ));
    }
    let payload = JsFuture::from(
        response
            .json()
            .map_err(|_| "Responses body is not JSON".to_string())?,
    )
    .await
    .map_err(|_| "Responses body is not JSON".to_string())?;
    let payload = js_sys::JSON::stringify(&payload)
        .map_err(|_| "Responses body could not be decoded".to_string())?
        .as_string()
        .ok_or_else(|| "Responses body could not be decoded".to_string())?;
    let payload: Value = serde_json::from_str(&payload)
        .map_err(|_| "Responses endpoint returned invalid JSON".to_string())?;
    extract_output_text(&payload)
        .ok_or_else(|| "Responses endpoint returned no assistant output text".to_string())
}

fn browser_conversations() -> Vec<Conversation> {
    let mut conversations = vec![Conversation::codex_assistant()];
    conversations.extend(
        miniapps()
            .into_iter()
            .map(|(app_id, title, _)| Conversation {
                id: ConversationId(format!("miniapp:{app_id}")),
                title: title.into(),
                peer: PeerKind::MiniApp {
                    app_id: app_id.into(),
                },
                pinned: false,
                unread_count: 0,
                updated_at_ms: 0,
            }),
    );
    conversations
}

fn ensure_browser_conversation(conversation_id: &ConversationId) -> Result<(), JsValue> {
    if conversation_id.as_str() == "codex:agent:assistant"
        || miniapps()
            .iter()
            .any(|(id, _, _)| conversation_id.as_str() == format!("miniapp:{id}"))
    {
        Ok(())
    } else {
        Err(JsValue::from_str(
            "conversation is not available in WebAssembly",
        ))
    }
}

fn miniapp_prompt(conversation_id: &ConversationId, text: &str) -> String {
    let Some(app_id) = conversation_id.as_str().strip_prefix("miniapp:") else {
        return text.to_string();
    };
    let definition = miniapps().into_iter().find(|item| item.0 == app_id);
    match definition {
        Some((_, title, instructions)) => format!(
            "你正在通过大乘与小程序“{title}”（{app_id}）对话。请严格依据该小程序职责回答。\n小程序说明：{instructions}\n\n用户消息：\n{text}"
        ),
        None => text.to_string(),
    }
}

fn miniapps() -> [(&'static str, &'static str, &'static str); 4] {
    [
        (
            "official.global-dharma",
            "全球法布施",
            "协助准备、检查和发送全球法布施内容。",
        ),
        (
            "official.flashcards",
            "法流背诵卡",
            "帮助用户复习和制作佛经背诵卡。",
        ),
        (
            "official.platform-publish",
            "平台发布",
            "协助整理并发布自媒体内容。",
        ),
        (
            "official.assistant",
            "大乘助手",
            "提供大乘软件功能引导和日常协助。",
        ),
    ]
}

fn extract_output_text(payload: &Value) -> Option<String> {
    if let Some(text) = payload.get("output_text").and_then(Value::as_str) {
        return (!text.is_empty()).then(|| text.to_string());
    }
    if let Some(output) = payload.get("output").and_then(Value::as_array) {
        let text = output
            .iter()
            .filter_map(|item| item.get("content").and_then(Value::as_array))
            .flatten()
            .filter_map(|content| content.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        if !text.is_empty() {
            return Some(text);
        }
    }
    payload
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn approval_not_found(approval_id: ApprovalId) -> JsValue {
    JsValue::from_str(&format!("approval was not found: {approval_id}"))
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn now_ms() -> i64 {
    js_sys::Date::now().min(i64::MAX as f64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_contact_list_has_codex_and_official_miniapps() {
        let conversations = browser_conversations();
        assert_eq!(conversations.len(), 5);
        assert_eq!(conversations[0].id.as_str(), "codex:agent:assistant");
        assert!(
            conversations
                .iter()
                .any(|item| item.id.as_str() == "miniapp:official.flashcards")
        );
    }
}
