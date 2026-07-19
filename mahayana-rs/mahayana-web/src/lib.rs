//! Browser-native Mahayana Runtime.
//!
//! Conversation state, operation routing, and event generation remain inside
//! WebAssembly. Model inference uses the browser Fetch API directly against
//! the configured Responses provider; there is no remote Agent gateway and no
//! desktop shell, process, or Git capability.

use mahayana_core::ApprovalId;
use mahayana_core::BuildProfile;
use mahayana_core::CONVERSATION_SCHEMA_VERSION;
use mahayana_core::Conversation;
use mahayana_core::ConversationId;
use mahayana_core::DEFAULT_DACHENG_RESPONSES_BASE_URL;
use mahayana_core::DEFAULT_DEEPSEEK_MODEL;
use mahayana_core::MODEL_RUNTIME_VERSION;
use mahayana_core::Message;
use mahayana_core::MessageId;
use mahayana_core::MessageRole;
use mahayana_core::ModelProviderMode;
use mahayana_core::OperationId;
use mahayana_core::PeerKind;
use mahayana_core::PluginCommandDescriptor;
use mahayana_core::RUNTIME_ABI_VERSION;
use mahayana_core::RuntimeCommand;
use mahayana_core::RuntimeEvent;
use mahayana_core::RuntimeResponse;
use mahayana_core::RuntimeStatus;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_futures::spawn_local;
use web_sys::Request;
use web_sys::RequestInit;
use web_sys::RequestMode;
use web_sys::Response;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct WebConfig {
    model: String,
    responses_base_url: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_DEEPSEEK_MODEL.to_string(),
            responses_base_url: DEFAULT_DACHENG_RESPONSES_BASE_URL.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct WebAccountSession {
    access_token: String,
    refresh_token: String,
    access_token_expires_at: i64,
    refresh_token_expires_at: i64,
    device_id: Option<String>,
    provider: String,
}

#[derive(Debug)]
struct ProductHttpResponse {
    status_code: u16,
    content_type: Option<String>,
    body_text: String,
    data: Value,
}

struct WebState {
    config: WebConfig,
    histories: HashMap<ConversationId, Vec<Message>>,
    model_inputs: HashMap<ConversationId, Vec<Value>>,
    events: VecDeque<RuntimeEvent>,
    active_operations: HashSet<OperationId>,
    plugins: HashMap<String, BrowserPlugin>,
    account_session: Option<WebAccountSession>,
    next_id: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserPlugin {
    plugin_id: String,
    title: String,
    tools: Vec<Value>,
    #[serde(default)]
    command_tools: HashMap<String, String>,
    #[serde(default)]
    approved_tools: HashSet<String>,
    #[serde(default)]
    ui_html: String,
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
        let mut state = WebState {
            config,
            histories: HashMap::new(),
            model_inputs: HashMap::new(),
            events: VecDeque::new(),
            active_operations: HashSet::new(),
            plugins: HashMap::new(),
            account_session: None,
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
                data: browser_conversations(&self.state.borrow().plugins),
            },
            RuntimeCommand::ListPluginCommands { plugin_id } => {
                let state = self.state.borrow();
                let mut data = Vec::new();
                for plugin in state.plugins.values().filter(|plugin| {
                    plugin_id
                        .as_deref()
                        .is_none_or(|plugin_id| plugin.plugin_id == plugin_id)
                }) {
                    for (command, tool) in &plugin.command_tools {
                        let descriptor = plugin.tools.iter().find(|descriptor| {
                            descriptor.get("name").and_then(Value::as_str) == Some(tool)
                        });
                        let input_schema = descriptor
                            .and_then(|descriptor| {
                                descriptor
                                    .get("inputSchema")
                                    .or_else(|| descriptor.get("input_schema"))
                            })
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object"}));
                        data.push(PluginCommandDescriptor {
                            plugin_id: plugin.plugin_id.clone(),
                            command: command.clone(),
                            tool: tool.clone(),
                            input_schema,
                            annotations: descriptor
                                .and_then(|descriptor| descriptor.get("annotations"))
                                .cloned()
                                .unwrap_or_else(|| json!({})),
                        });
                    }
                }
                data.sort_by(|left, right| {
                    left.plugin_id
                        .cmp(&right.plugin_id)
                        .then_with(|| left.command.cmp(&right.command))
                });
                RuntimeResponse::PluginCommands { data }
            }
            RuntimeCommand::PluginUi { plugin_id } => {
                let html = self
                    .state
                    .borrow()
                    .plugins
                    .get(&plugin_id)
                    .map(|plugin| plugin.ui_html.clone())
                    .filter(|html| !html.is_empty())
                    .ok_or_else(|| JsValue::from_str("plugin has no browser-local MCP App UI"))?;
                RuntimeResponse::PluginUi { plugin_id, html }
            }
            RuntimeCommand::ApproveLocalPluginTool { plugin_id, tool } => {
                let mut state = self.state.borrow_mut();
                let plugin = state
                    .plugins
                    .get_mut(&plugin_id)
                    .ok_or_else(|| JsValue::from_str("plugin has no browser-local Web runtime"))?;
                if !plugin.tools.iter().any(|descriptor| {
                    descriptor.get("name").and_then(Value::as_str) == Some(tool.as_str())
                }) {
                    return Err(JsValue::from_str("plugin has no matching MCP Tool"));
                }
                plugin.approved_tools.insert(tool.clone());
                RuntimeResponse::LocalPluginToolApproved { plugin_id, tool }
            }
            RuntimeCommand::CallLocalPluginTool {
                plugin_id,
                tool,
                arguments,
            } => {
                let plugin = self
                    .state
                    .borrow()
                    .plugins
                    .get(&plugin_id)
                    .cloned()
                    .ok_or_else(|| JsValue::from_str("plugin has no browser-local Web runtime"))?;
                require_browser_tool_approval(&plugin, &tool)?;
                let outcome = call_local_plugin_sync(&plugin_id, &tool, &arguments)
                    .map_err(|message| JsValue::from_str(&message))?;
                let (result, progress) = split_local_plugin_outcome(outcome);
                RuntimeResponse::LocalPluginToolResult {
                    plugin_id,
                    tool,
                    result,
                    progress,
                }
            }
            RuntimeCommand::ConversationHistory {
                conversation_id,
                limit,
            } => {
                ensure_browser_conversation(&self.state.borrow().plugins, &conversation_id)?;
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
                ensure_browser_conversation(&self.state.borrow().plugins, &conversation_id)?;
                if text.trim().is_empty() {
                    return Err(JsValue::from_str("message text must not be empty"));
                }
                if let Some(plugin_id) = conversation_id.as_str().strip_prefix("miniapp:") {
                    let plugin = self
                        .state
                        .borrow()
                        .plugins
                        .get(plugin_id)
                        .cloned()
                        .ok_or_else(|| {
                            JsValue::from_str("plugin has no installed browser-local Web runtime")
                        })?;
                    let (tool, arguments) = parse_browser_tool_command(&plugin, &text)?;
                    let operation_id = {
                        let mut state = self.state.borrow_mut();
                        let operation_id = OperationId(state.next_id("operation"));
                        state.active_operations.insert(operation_id.clone());
                        operation_id
                    };
                    spawn_local_plugin_call(
                        Rc::clone(&self.state),
                        operation_id.clone(),
                        conversation_id,
                        plugin,
                        tool,
                        arguments,
                    );
                    return serde_json::to_string(&json!({
                        "ok": true,
                        "data": RuntimeResponse::Accepted { operation_id },
                    }))
                    .map_err(js_error);
                }
                if self.state.borrow().account_session.is_none() {
                    return Err(JsValue::from_str(
                        "Web Agent accepted an unauthenticated request.",
                    ));
                }
                let (operation_id, input, config) = {
                    let mut state = self.state.borrow_mut();
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
                    let input = state
                        .model_inputs
                        .entry(conversation_id.clone())
                        .or_default();
                    input.push(json!({"role": "user", "content": text}));
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

    /// Executes account, marketplace, payment, and social commands inside the
    /// browser-local Rust Worker. Credentials are retained in Rust state and
    /// are removed from every response before it crosses into Flutter/Dart.
    pub async fn execute_product(&self, command_json: &str) -> Result<String, JsValue> {
        let command: Value = serde_json::from_str(command_json).map_err(js_error)?;
        let response = execute_product_command(Rc::clone(&self.state), command).await?;
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

    /// Registers a browser-local plugin after the package loader has verified
    /// its Codex manifest, TUF target, runtime variant, and user permissions.
    /// Jco-generated modules expose their callable object through
    /// `globalThis.__mahayanaLocalPlugins[pluginId]`; this Rust host never
    /// substitutes a cloud MCP endpoint.
    pub fn register_local_plugin(&self, plugin_json: &str) -> Result<(), JsValue> {
        let plugin: BrowserPlugin = serde_json::from_str(plugin_json).map_err(js_error)?;
        if plugin.plugin_id.trim().is_empty() || plugin.title.trim().is_empty() {
            return Err(JsValue::from_str("plugin id and title must not be empty"));
        }
        for tool in plugin.command_tools.values() {
            if !plugin
                .tools
                .iter()
                .any(|descriptor| descriptor.get("name").and_then(Value::as_str) == Some(tool))
            {
                return Err(JsValue::from_str(&format!(
                    "plugin command maps to missing MCP Tool {tool}"
                )));
            }
        }
        self.state
            .borrow_mut()
            .plugins
            .insert(plugin.plugin_id.clone(), plugin);
        Ok(())
    }
}

fn spawn_local_plugin_call(
    state: Rc<RefCell<WebState>>,
    operation_id: OperationId,
    conversation_id: ConversationId,
    plugin: BrowserPlugin,
    tool: String,
    arguments: Value,
) {
    spawn_local(async move {
        let result = call_local_plugin(&plugin.plugin_id, &tool, &arguments).await;
        let mut state = state.borrow_mut();
        if !state.active_operations.remove(&operation_id) {
            return;
        }
        match result {
            Ok(outcome) => {
                let (result, progress) = split_local_plugin_outcome(outcome);
                for update in progress {
                    state.events.push_back(RuntimeEvent::PluginProgress {
                        operation_id: operation_id.clone(),
                        plugin_id: plugin.plugin_id.clone(),
                        tool: tool.clone(),
                        progress: update.get("progress").and_then(Value::as_u64).unwrap_or(0),
                        total: update.get("total").and_then(Value::as_u64).unwrap_or(0),
                        message: update
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
                let text = mcp_result_text(&result);
                let message = Message {
                    id: MessageId(state.next_id("message")),
                    conversation_id: conversation_id.clone(),
                    role: MessageRole::MiniApp,
                    text: text.clone(),
                    created_at_ms: now_ms(),
                    metadata: json!({
                        "pluginId": plugin.plugin_id,
                        "tool": tool,
                        "mcpResult": result,
                        "execution": "browser-local-jco",
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
                code: "browser_local_plugin_failed".into(),
                message,
            }),
        }
    });
}

async fn call_local_plugin(
    plugin_id: &str,
    tool: &str,
    arguments: &Value,
) -> Result<Value, String> {
    let global = js_sys::global();
    let registry = js_sys::Reflect::get(&global, &JsValue::from_str("__mahayanaLocalPlugins"))
        .map_err(|_| "browser-local plugin registry is unavailable".to_string())?;
    let plugin = js_sys::Reflect::get(&registry, &JsValue::from_str(plugin_id))
        .map_err(|_| "browser-local plugin runtime is unavailable".to_string())?;
    if plugin.is_undefined() || plugin.is_null() {
        return Err("browser-local plugin runtime is unavailable".into());
    }
    let call_tool = local_plugin_call_function(&plugin)?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "Jco plugin callTool export is not callable".to_string())?;
    let arguments = serde_json::to_string(arguments)
        .map_err(|_| "MCP arguments could not be encoded".to_string())?;
    let arguments = js_sys::JSON::parse(&arguments)
        .map_err(|_| "MCP arguments could not be converted to JavaScript".to_string())?;
    let output = call_tool
        .call2(&plugin, &JsValue::from_str(tool), &arguments)
        .map_err(|_| "browser-local MCP Tool call failed".to_string())?;
    let output = JsFuture::from(js_sys::Promise::resolve(&output))
        .await
        .map_err(|_| "browser-local MCP Tool promise rejected".to_string())?;
    let output = js_sys::JSON::stringify(&output)
        .map_err(|_| "browser-local MCP result could not be encoded".to_string())?
        .as_string()
        .ok_or_else(|| "browser-local MCP result is not JSON".to_string())?;
    serde_json::from_str(&output)
        .map_err(|_| "browser-local MCP result is invalid JSON".to_string())
}

fn call_local_plugin_sync(plugin_id: &str, tool: &str, arguments: &Value) -> Result<Value, String> {
    let global = js_sys::global();
    let registry = js_sys::Reflect::get(&global, &JsValue::from_str("__mahayanaLocalPlugins"))
        .map_err(|_| "browser-local plugin registry is unavailable".to_string())?;
    let plugin = js_sys::Reflect::get(&registry, &JsValue::from_str(plugin_id))
        .map_err(|_| "browser-local plugin runtime is unavailable".to_string())?;
    if plugin.is_undefined() || plugin.is_null() {
        return Err("browser-local plugin runtime is unavailable".into());
    }
    let call_tool = local_plugin_call_function(&plugin)?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "WASM plugin callTool export is not callable".to_string())?;
    let arguments = serde_json::to_string(arguments)
        .map_err(|_| "MCP arguments could not be encoded".to_string())?;
    let arguments = js_sys::JSON::parse(&arguments)
        .map_err(|_| "MCP arguments could not be converted to JavaScript".to_string())?;
    let output = call_tool
        .call2(&plugin, &JsValue::from_str(tool), &arguments)
        .map_err(|_| "browser-local MCP Tool call failed".to_string())?;
    if output.is_instance_of::<js_sys::Promise>() {
        return Err("browser-local direct Tool call must complete synchronously".into());
    }
    let output = js_sys::JSON::stringify(&output)
        .map_err(|_| "browser-local MCP result could not be encoded".to_string())?
        .as_string()
        .ok_or_else(|| "browser-local MCP result is not JSON".to_string())?;
    serde_json::from_str(&output)
        .map_err(|_| "browser-local MCP result is invalid JSON".to_string())
}

fn local_plugin_call_function(plugin: &JsValue) -> Result<JsValue, String> {
    let outcome = js_sys::Reflect::get(plugin, &JsValue::from_str("callToolOutcome"))
        .map_err(|_| "browser-local plugin exports are unavailable".to_string())?;
    if !outcome.is_undefined() && !outcome.is_null() {
        return Ok(outcome);
    }
    js_sys::Reflect::get(plugin, &JsValue::from_str("callTool"))
        .map_err(|_| "browser-local plugin does not export callTool".to_string())
}

fn split_local_plugin_outcome(output: Value) -> (Value, Vec<Value>) {
    let Some(result) = output.get("result").cloned() else {
        return (output, Vec::new());
    };
    let progress = output
        .get("progress")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (result, progress)
}

fn require_browser_tool_approval(plugin: &BrowserPlugin, tool: &str) -> Result<(), JsValue> {
    let descriptor = plugin
        .tools
        .iter()
        .find(|descriptor| descriptor.get("name").and_then(Value::as_str) == Some(tool))
        .ok_or_else(|| JsValue::from_str("current browser-local plugin has no matching Tool"))?;
    let read_only = descriptor
        .pointer("/annotations/readOnlyHint")
        .and_then(Value::as_bool)
        == Some(true);
    if !read_only && !plugin.approved_tools.contains(tool) {
        return Err(JsValue::from_str(
            "browser host approval is required for this MCP Tool",
        ));
    }
    Ok(())
}

fn parse_browser_tool_command(
    plugin: &BrowserPlugin,
    source: &str,
) -> Result<(String, Value), JsValue> {
    let (command, remainder) = source
        .split_once(char::is_whitespace)
        .map_or((source, ""), |(command, remainder)| (command, remainder));
    let command = command.trim_start_matches('/');
    let tool = plugin
        .command_tools
        .get(command)
        .map(String::as_str)
        .unwrap_or(command);
    let descriptor = plugin
        .tools
        .iter()
        .find(|descriptor| descriptor.get("name").and_then(Value::as_str) == Some(tool))
        .ok_or_else(|| JsValue::from_str("current browser-local plugin has no matching Tool"))?;
    require_browser_tool_approval(plugin, tool)?;
    let properties = descriptor
        .get("inputSchema")
        .or_else(|| descriptor.get("input_schema"))
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object);
    let arguments = match properties {
        None => json!({}),
        Some(properties) if properties.is_empty() => json!({}),
        Some(properties)
            if properties.len() == 1
                && properties
                    .values()
                    .next()
                    .and_then(|field| field.get("type"))
                    .and_then(Value::as_str)
                    == Some("string") =>
        {
            let field = properties.keys().next().expect("one property");
            json!({(field): remainder})
        }
        Some(_) => {
            let arguments: Value = serde_json::from_str(remainder)
                .map_err(|_| JsValue::from_str("MCP Tool arguments must be a JSON object"))?;
            if !arguments.is_object() {
                return Err(JsValue::from_str(
                    "MCP Tool arguments must be a JSON object",
                ));
            }
            arguments
        }
    };
    Ok((tool.to_string(), arguments))
}

fn mcp_result_text(result: &Value) -> String {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        serde_json::to_string_pretty(result).unwrap_or_else(|_| "MCP Tool completed".into())
    } else {
        text
    }
}

async fn execute_product_command(
    state: Rc<RefCell<WebState>>,
    command: Value,
) -> Result<Value, JsValue> {
    let command_type = required_product_string(&command, "@type")?;
    match command_type {
        "mahayana.auth.session.restore" => {
            let user = match authenticated_product_fetch(
                Rc::clone(&state),
                "GET",
                "/api/auth/user-info",
                &[],
                None,
            )
            .await
            {
                Ok(response) if response.status_code < 300 => response.data,
                _ => {
                    state.borrow_mut().account_session = None;
                    return Err(JsValue::from_str("no active Mahayana account session"));
                }
            };
            let provider = state
                .borrow()
                .account_session
                .as_ref()
                .map(|session| session.provider.clone())
                .unwrap_or_else(|| "password".into());
            Ok(json!({
                "@type": "mahayana.auth.session",
                "loggedIn": true,
                "sessionStored": true,
                "provider": provider,
                "user": user,
            }))
        }
        "mahayana.auth.status" => {
            if state.borrow().account_session.is_none() {
                return Ok(json!({
                    "@type": "mahayana.auth.status",
                    "loggedIn": false,
                }));
            }
            match authenticated_product_fetch(
                Rc::clone(&state),
                "GET",
                "/api/auth/user-info",
                &[],
                None,
            )
            .await
            {
                Ok(response) if response.status_code < 300 => Ok(json!({
                    "@type": "mahayana.auth.status",
                    "loggedIn": true,
                    "user": response.data,
                })),
                _ => {
                    state.borrow_mut().account_session = None;
                    Ok(json!({
                        "@type": "mahayana.auth.status",
                        "loggedIn": false,
                        "expired": true,
                    }))
                }
            }
        }
        "mahayana.auth.logout" => {
            if state.borrow().account_session.is_some() {
                let _ = authenticated_product_fetch(
                    Rc::clone(&state),
                    "POST",
                    "/api/auth/logout",
                    &[],
                    Some(json!({})),
                )
                .await;
            }
            state.borrow_mut().account_session = None;
            Ok(json!({
                "@type": "mahayana.auth.loggedOut",
                "loggedIn": false,
            }))
        }
        "mahayana.platform.request" => platform_product_request(state, &command).await,
        _ => {
            let route = product_command_route(command_type, &command)?;
            let response = if route.authenticated {
                authenticated_product_fetch(
                    Rc::clone(&state),
                    route.method,
                    &route.path,
                    &route.query,
                    route.body,
                )
                .await?
            } else {
                raw_product_fetch(
                    route.method,
                    &route.path,
                    &route.query,
                    route.body.as_ref(),
                    None,
                )
                .await?
            };
            ensure_product_success(&response)?;
            let mut data = response.data;
            if route.establishes_session
                && let Some(session) = web_session_from_response(&data, route.provider)
            {
                state.borrow_mut().account_session = Some(session);
                mark_web_session_stored(&mut data);
            }
            strip_web_credentials(&mut data);
            Ok(data)
        }
    }
}

struct ProductCommandRoute {
    method: &'static str,
    path: String,
    query: Vec<(String, String)>,
    body: Option<Value>,
    authenticated: bool,
    establishes_session: bool,
    provider: &'static str,
}

fn product_command_route(
    command_type: &str,
    command: &Value,
) -> Result<ProductCommandRoute, JsValue> {
    let route = match command_type {
        "mahayana.auth.password.login" => ProductCommandRoute {
            method: "POST",
            path: "/api/auth/login".into(),
            query: vec![],
            body: Some(json!({
                "username": required_product_string(command, "username")?,
                "password": required_product_string(command, "password")?,
            })),
            authenticated: false,
            establishes_session: true,
            provider: "password",
        },
        "mahayana.auth.register" => unauthenticated_route(
            "POST",
            "/api/auth/register",
            Some(json!({
                "username": required_product_string(command, "username")?,
                "email": required_product_string(command, "email")?,
                "password": required_product_string(command, "password")?,
                "verificationCode": required_product_string(command, "verificationCode")?,
            })),
        ),
        "mahayana.auth.verification.send" => unauthenticated_route(
            "POST",
            "/api/auth/send-verification-code",
            Some(json!({
                "email": required_product_string(command, "email")?,
                "type": required_product_string(command, "type")?,
            })),
        ),
        "mahayana.auth.password.forgot" => unauthenticated_route(
            "POST",
            "/api/auth/forgot-password",
            Some(json!({"email": required_product_string(command, "email")?})),
        ),
        "mahayana.auth.password.reset" => unauthenticated_route(
            "POST",
            "/api/auth/reset-password",
            Some(json!({
                "email": required_product_string(command, "email")?,
                "token": required_product_string(command, "resetToken")?,
                "newPassword": required_product_string(command, "newPassword")?,
            })),
        ),
        "mahayana.auth.alipay.start" => ProductCommandRoute {
            query: vec![(
                "platform".into(),
                optional_product_string(command, "platform").unwrap_or_else(|| "web".into()),
            )],
            ..unauthenticated_route("GET", "/api/auth/alipay/login-url", None)
        },
        "mahayana.auth.alipay.complete" => login_route(
            "/api/auth/alipay/login",
            "alipay",
            json!({
                "auth_code": required_product_string(command, "authCode")?,
                "state": optional_product_string(command, "state"),
            }),
        ),
        "mahayana.auth.alipay.poll" => ProductCommandRoute {
            method: "GET",
            path: "/api/auth/alipay/cli-session".into(),
            query: vec![(
                "state".into(),
                required_product_string(command, "state")?.into(),
            )],
            body: None,
            authenticated: false,
            establishes_session: true,
            provider: "alipay",
        },
        "mahayana.auth.alipay.sdk.start" => {
            unauthenticated_route("GET", "/api/auth/alipay/auth-string", None)
        }
        "mahayana.auth.alipay.sdk.complete" => login_route(
            "/api/auth/alipay/sdk-login",
            "alipay",
            json!({
                "auth_code": required_product_string(command, "authCode")?,
                "target_id": optional_product_string(command, "targetId"),
            }),
        ),
        "mahayana.auth.alipay.register" => login_route(
            "/api/auth/alipay/register",
            "alipay",
            json!({
                "alipayProviderSubject": required_product_string(command, "alipayProviderSubject")?,
                "alipaySubjectType": optional_product_string(command, "alipaySubjectType"),
                "username": optional_product_string(command, "username"),
                "password": optional_product_string(command, "password"),
                "nickname": optional_product_string(command, "nickname"),
                "avatar": optional_product_string(command, "avatar"),
                "email": optional_product_string(command, "email"),
                "alipayNickname": optional_product_string(command, "alipayNickname"),
                "alipayAvatar": optional_product_string(command, "alipayAvatar"),
                "oneClick": command.get("oneClick").and_then(Value::as_bool).unwrap_or(false),
            }),
        ),
        "mahayana.auth.apple.complete" => login_route(
            "/api/auth/apple-login",
            "apple",
            json!({
                "identityToken": required_product_string(command, "identityToken")?,
                "authorizationCode": required_product_string(command, "authorizationCode")?,
                "email": optional_product_string(command, "email"),
                "givenName": optional_product_string(command, "givenName"),
                "familyName": optional_product_string(command, "familyName"),
            }),
        ),
        "mahayana.auth.firebase.phone.complete" => login_route(
            "/api/auth/firebase-phone-login",
            "firebase-phone",
            json!({
                "idToken": required_product_string(command, "idToken")?,
                "phoneNumber": required_product_string(command, "phoneNumber")?,
                "firebaseUid": required_product_string(command, "firebaseUid")?,
                "isNewUser": command.get("isNewUser").and_then(Value::as_bool).unwrap_or(false),
            }),
        ),
        "mahayana.contacts.list" => authenticated_route("GET", "/api/social/friends", None),
        "mahayana.contacts.search" => ProductCommandRoute {
            query: vec![(
                "q".into(),
                required_product_string(command, "query")?.into(),
            )],
            ..authenticated_route("GET", "/api/social/users/search", None)
        },
        "mahayana.contacts.add" => authenticated_route(
            "POST",
            "/api/social/friend-requests",
            Some(json!({
                "targetUserId": required_product_string(command, "contact")?,
                "message": optional_product_string(command, "message"),
            })),
        ),
        "mahayana.contacts.requests" => {
            authenticated_route("GET", "/api/social/friend-requests/incoming", None)
        }
        "mahayana.contacts.accept" => authenticated_route(
            "POST",
            &format!(
                "/api/social/friend-requests/{}/accept",
                encode_uri_component(required_product_string(command, "requestId")?)
            ),
            Some(json!({})),
        ),
        "mahayana.messages.list" => ProductCommandRoute {
            query: vec![
                (
                    "contactId".into(),
                    required_product_string(command, "contact")?.into(),
                ),
                (
                    "limit".into(),
                    command
                        .get("limit")
                        .and_then(Value::as_u64)
                        .unwrap_or(50)
                        .to_string(),
                ),
            ],
            ..authenticated_route("GET", "/api/social/messages", None)
        },
        "mahayana.messages.send" => authenticated_route(
            "POST",
            "/api/social/messages",
            Some(json!({
                "contactId": required_product_string(command, "contact")?,
                "text": required_product_string(command, "text")?,
                "clientRequestId": optional_product_string(command, "clientRequestId"),
            })),
        ),
        "mahayana.miniapps.registry" => authenticated_route("GET", "/api/plugins/registry", None),
        _ => {
            return Err(JsValue::from_str(&format!(
                "unsupported Mahayana product command: {command_type}"
            )));
        }
    };
    Ok(route)
}

fn unauthenticated_route(
    method: &'static str,
    path: &str,
    body: Option<Value>,
) -> ProductCommandRoute {
    ProductCommandRoute {
        method,
        path: path.into(),
        query: vec![],
        body,
        authenticated: false,
        establishes_session: false,
        provider: "",
    }
}

fn authenticated_route(
    method: &'static str,
    path: &str,
    body: Option<Value>,
) -> ProductCommandRoute {
    ProductCommandRoute {
        authenticated: true,
        ..unauthenticated_route(method, path, body)
    }
}

fn login_route(path: &str, provider: &'static str, body: Value) -> ProductCommandRoute {
    ProductCommandRoute {
        establishes_session: true,
        provider,
        ..unauthenticated_route("POST", path, Some(body))
    }
}

async fn platform_product_request(
    state: Rc<RefCell<WebState>>,
    command: &Value,
) -> Result<Value, JsValue> {
    let method = required_product_string(command, "method")?.to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
        return Err(JsValue::from_str("unsupported platform request method"));
    }
    let path = required_product_string(command, "path")?;
    if !safe_platform_path(path) {
        return Err(JsValue::from_str(
            "platform request must be a same-origin API path",
        ));
    }
    let query = command
        .get("query")
        .and_then(Value::as_object)
        .map(|query| {
            query
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string()),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let body = command.get("body").cloned();
    let authenticated = command
        .get("authenticated")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let response = if authenticated {
        authenticated_product_fetch(state, &method, path, &query, body).await?
    } else {
        raw_product_fetch(&method, path, &query, body.as_ref(), None).await?
    };
    let mut data = response.data;
    strip_web_credentials(&mut data);
    let body_text = if data.is_object() || data.is_array() {
        serde_json::to_string(&data).map_err(js_error)?
    } else {
        response.body_text
    };
    Ok(json!({
        "@type": "mahayana.platform.response",
        "ok": (200..300).contains(&response.status_code),
        "statusCode": response.status_code,
        "contentType": response.content_type,
        "bodyText": body_text,
        "data": data,
    }))
}

async fn authenticated_product_fetch(
    state: Rc<RefCell<WebState>>,
    method: &str,
    path: &str,
    query: &[(String, String)],
    body: Option<Value>,
) -> Result<ProductHttpResponse, JsValue> {
    let token = active_web_access_token(Rc::clone(&state), false).await?;
    let mut response = raw_product_fetch(method, path, query, body.as_ref(), Some(&token)).await?;
    if response.status_code == 401 {
        let token = active_web_access_token(Rc::clone(&state), true).await?;
        response = raw_product_fetch(method, path, query, body.as_ref(), Some(&token)).await?;
    }
    Ok(response)
}

async fn active_web_access_token(
    state: Rc<RefCell<WebState>>,
    force_refresh: bool,
) -> Result<String, JsValue> {
    let session = state
        .borrow()
        .account_session
        .clone()
        .ok_or_else(|| JsValue::from_str("Mahayana account login is required"))?;
    let now = now_ms() / 1000;
    if !force_refresh && session.access_token_expires_at > now + 60 {
        return Ok(session.access_token);
    }
    if session.refresh_token_expires_at <= now {
        state.borrow_mut().account_session = None;
        return Err(JsValue::from_str("Mahayana account session has expired"));
    }
    let response = raw_product_fetch(
        "POST",
        "/api/auth/refresh",
        &[],
        Some(&json!({
            "refreshToken": session.refresh_token,
            "deviceId": session.device_id,
        })),
        None,
    )
    .await?;
    ensure_product_success(&response)?;
    let refreshed =
        web_session_from_response(&response.data, &session.provider).ok_or_else(|| {
            JsValue::from_str("refresh response did not contain a Rust account session")
        })?;
    let token = refreshed.access_token.clone();
    state.borrow_mut().account_session = Some(refreshed);
    Ok(token)
}

async fn raw_product_fetch(
    method: &str,
    path: &str,
    query: &[(String, String)],
    body: Option<&Value>,
    access_token: Option<&str>,
) -> Result<ProductHttpResponse, JsValue> {
    let mut url = path.to_string();
    if !query.is_empty() {
        url.push('?');
        url.push_str(
            &query
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}={}",
                        encode_uri_component(key),
                        encode_uri_component(value)
                    )
                })
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    let options = RequestInit::new();
    options.set_method(method);
    options.set_mode(RequestMode::SameOrigin);
    let encoded_body = body
        .map(serde_json::to_string)
        .transpose()
        .map_err(js_error)?;
    if let Some(body) = encoded_body.as_deref() {
        options.set_body(&JsValue::from_str(body));
    }
    let request = Request::new_with_str_and_init(&url, &options)?;
    request.headers().set("Accept", "application/json")?;
    if body.is_some() {
        request.headers().set("Content-Type", "application/json")?;
    }
    if let Some(token) = access_token {
        request
            .headers()
            .set("Authorization", &format!("Bearer {token}"))?;
    }
    let global: web_sys::WorkerGlobalScope = js_sys::global()
        .dyn_into()
        .map_err(|_| JsValue::from_str("Mahayana Web Runtime requires a browser Worker"))?;
    let response = JsFuture::from(global.fetch_with_request(&request))
        .await?
        .dyn_into::<Response>()?;
    let status_code = response.status();
    let content_type = response.headers().get("Content-Type")?;
    let body_text = JsFuture::from(response.text()?)
        .await?
        .as_string()
        .unwrap_or_default();
    let data = if body_text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&body_text).unwrap_or_else(|_| json!({"body": body_text}))
    };
    Ok(ProductHttpResponse {
        status_code,
        content_type,
        body_text,
        data,
    })
}

fn web_session_from_response(response: &Value, provider: &str) -> Option<WebAccountSession> {
    let access_token = response.get("accessToken")?.as_str()?.trim();
    let refresh_token = response.get("refreshToken")?.as_str()?.trim();
    if access_token.is_empty() || refresh_token.is_empty() {
        return None;
    }
    Some(WebAccountSession {
        access_token: access_token.into(),
        refresh_token: refresh_token.into(),
        access_token_expires_at: response.get("accessTokenExpiresAt")?.as_i64()?,
        refresh_token_expires_at: response.get("refreshTokenExpiresAt")?.as_i64()?,
        device_id: response
            .get("deviceId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        provider: provider.into(),
    })
}

fn mark_web_session_stored(response: &mut Value) {
    if let Some(object) = response.as_object_mut() {
        object.insert("loggedIn".into(), Value::Bool(true));
        object.insert("sessionStored".into(), Value::Bool(true));
        object.insert(
            "@type".into(),
            Value::String("mahayana.auth.session".into()),
        );
    }
}

fn strip_web_credentials(response: &mut Value) {
    match response {
        Value::Object(object) => {
            for key in [
                "token",
                "accessToken",
                "refreshToken",
                "accessTokenExpiresAt",
                "refreshTokenExpiresAt",
                "tokenType",
            ] {
                object.remove(key);
            }
            for value in object.values_mut() {
                strip_web_credentials(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                strip_web_credentials(value);
            }
        }
        _ => {}
    }
}

fn ensure_product_success(response: &ProductHttpResponse) -> Result<(), JsValue> {
    if (200..300).contains(&response.status_code)
        && response.data.get("success").and_then(Value::as_bool) != Some(false)
        && response.data.get("error").is_none()
    {
        return Ok(());
    }
    let message = response
        .data
        .get("error")
        .or_else(|| response.data.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Mahayana API returned HTTP {}", response.status_code));
    Err(JsValue::from_str(&message))
}

fn required_product_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, JsValue> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| JsValue::from_str(&format!("Mahayana product command requires {key}")))
}

fn optional_product_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn safe_platform_path(path: &str) -> bool {
    (path.starts_with("/api/") || path.starts_with("/v1/"))
        && !path.contains("..")
        && !path.contains('\\')
        && !path.contains(['\r', '\n'])
}

fn encode_uri_component(value: &str) -> String {
    js_sys::encode_uri_component(value).into()
}

fn spawn_inference(
    state: Rc<RefCell<WebState>>,
    operation_id: OperationId,
    conversation_id: ConversationId,
    input: Vec<Value>,
    config: WebConfig,
) {
    spawn_local(async move {
        let result = match active_web_access_token(Rc::clone(&state), false).await {
            Ok(access_token) => fetch_response(&config, input, &access_token).await,
            Err(_) => Err("Mahayana account login is required".to_string()),
        };
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

async fn fetch_response(
    config: &WebConfig,
    input: Vec<Value>,
    access_token: &str,
) -> Result<String, String> {
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
        .set("Authorization", &format!("Bearer {access_token}"))
        .map_err(|_| "could not set Responses authorization".to_string())?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|_| "could not set Responses content type".to_string())?;
    let global: web_sys::WorkerGlobalScope = js_sys::global()
        .dyn_into()
        .map_err(|_| "Mahayana Web Runtime requires a browser Worker".to_string())?;
    let response = JsFuture::from(global.fetch_with_request(&request))
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

fn browser_conversations(plugins: &HashMap<String, BrowserPlugin>) -> Vec<Conversation> {
    let mut conversations = vec![Conversation::codex_assistant()];
    conversations.extend(plugins.values().map(|plugin| Conversation {
        id: ConversationId(format!("miniapp:{}", plugin.plugin_id)),
        title: plugin.title.clone(),
        peer: PeerKind::MiniApp {
            app_id: plugin.plugin_id.clone(),
        },
        pinned: false,
        unread_count: 0,
        updated_at_ms: 0,
    }));
    conversations
}

fn ensure_browser_conversation(
    plugins: &HashMap<String, BrowserPlugin>,
    conversation_id: &ConversationId,
) -> Result<(), JsValue> {
    if conversation_id.as_str() == "codex:agent:assistant"
        || conversation_id
            .as_str()
            .strip_prefix("miniapp:")
            .is_some_and(|plugin_id| plugins.contains_key(plugin_id))
    {
        Ok(())
    } else {
        Err(JsValue::from_str(
            "conversation is not available in WebAssembly",
        ))
    }
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
    fn web_contact_list_only_exposes_registered_local_plugins() {
        let plugins = HashMap::from([(
            "local-example".into(),
            BrowserPlugin {
                plugin_id: "local-example".into(),
                title: "本地示例".into(),
                tools: vec![],
                command_tools: HashMap::new(),
                approved_tools: HashSet::new(),
                ui_html: String::new(),
            },
        )]);
        let conversations = browser_conversations(&plugins);
        assert_eq!(conversations.len(), 2);
        assert_eq!(conversations[0].id.as_str(), "codex:agent:assistant");
        assert!(
            conversations
                .iter()
                .any(|item| item.id.as_str() == "miniapp:local-example")
        );
    }
}
