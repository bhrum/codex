//! In-process Codex app-server adapter for the Mahayana Agent contract.

use async_trait::async_trait;
use codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
use codex_app_server_client::InProcessAppServerClient;
use codex_app_server_client::InProcessAppServerRequestHandle;
use codex_app_server_client::InProcessClientStartArgs;
use codex_app_server_client::InProcessServerEvent;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallParams;
use codex_app_server_protocol::DynamicToolCallResponse;
use codex_app_server_protocol::DynamicToolFunctionSpec;
use codex_app_server_protocol::DynamicToolNamespaceSpec;
use codex_app_server_protocol::DynamicToolNamespaceTool;
use codex_app_server_protocol::DynamicToolSpec;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnInterruptResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use codex_arg0::Arg0DispatchPaths;
use codex_config::CloudConfigBundleLoader;
use codex_config::LoaderOverrides;
use codex_core::config::ConfigBuilder;
use codex_core::config::ConfigOverrides;
use codex_exec_server::EnvironmentManager;
use codex_exec_server::ExecServerRuntimePaths;
use codex_feedback::CodexFeedback;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SessionSource;
use mahayana_agent::AgentBackend;
use mahayana_agent::AgentError;
use mahayana_agent::AgentEvent;
use mahayana_agent::AgentMessageRequest;
use mahayana_agent::ApprovalResolution;
use mahayana_agent::SharedAgentEventSink;
use mahayana_agent::StartThreadRequest;
use mahayana_conversation::ConversationProvider;
use mahayana_conversation::provider_key_for_conversation_id;
use mahayana_core::AgentThreadId;
use mahayana_core::ApprovalDecision;
use mahayana_core::ApprovalId;
use mahayana_core::ConversationId;
use mahayana_core::Message;
use mahayana_core::MessageId;
use mahayana_core::MessageRole;
use mahayana_core::OperationId;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::oneshot;

const PROVIDER_ID: &str = "dacheng-deepseek";

#[derive(Clone)]
pub struct CodexAgentConfig {
    pub codex_home: PathBuf,
    pub cwd: PathBuf,
    pub workspace_roots: Vec<PathBuf>,
    pub model: String,
    pub responses_base_url: String,
    /// Optional Fabushi product session token. Logged-out users use the
    /// first-party anonymous allowance; logged-in users get their account
    /// quota. The token is injected only into the in-memory provider and is
    /// never written to Codex auth files.
    pub product_session_token: Option<String>,
    pub sandbox_mode: SandboxMode,
    pub approval_policy: AskForApproval,
    /// Mahayana CLI executable used for Codex's hidden filesystem/process
    /// helper modes. Embedded SDK hosts omit this because their application
    /// executable does not implement Codex argv dispatch.
    pub codex_executable_path: Option<PathBuf>,
    /// Non-Agent providers owned by this same Mahayana Runtime. Codex receives
    /// bounded read-only tools for inspecting their conversations.
    pub conversation_providers: Vec<Arc<dyn ConversationProvider>>,
}

impl CodexAgentConfig {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self
            .product_session_token
            .as_deref()
            .is_some_and(|token| token.trim().is_empty() || token.contains(['\r', '\n']))
        {
            return Err(AgentError::Unavailable(
                "Mahayana product session is invalid".into(),
            ));
        }
        if self.model.trim().is_empty() {
            return Err(AgentError::Backend(
                "Dacheng DeepSeek model configuration is incomplete".into(),
            ));
        }
        if !self.responses_base_url.starts_with("https://") {
            return Err(AgentError::Backend(
                "Dacheng Responses endpoint must use HTTPS".into(),
            ));
        }
        Ok(())
    }
}

struct ActiveOperation {
    thread_id: String,
    turn_id: String,
    conversation_id: mahayana_core::ConversationId,
    events: SharedAgentEventSink,
    assistant_text: String,
    completion: oneshot::Sender<Result<(), String>>,
}

struct PendingApproval {
    request_id: RequestId,
    response_kind: ApprovalResponseKind,
}

enum ApprovalResponseKind {
    CommandExecution,
    FileChange,
    Permissions { requested: Value },
    LegacyExec,
    LegacyPatch,
}

struct CodexAgentInner {
    config: Arc<codex_core::config::Config>,
    requests: InProcessAppServerRequestHandle,
    next_request_id: AtomicI64,
    operations: Mutex<HashMap<OperationId, ActiveOperation>>,
    approvals: Mutex<HashMap<ApprovalId, PendingApproval>>,
    conversation_providers: Vec<Arc<dyn ConversationProvider>>,
}

impl CodexAgentInner {
    fn request_id(&self) -> RequestId {
        RequestId::Integer(self.next_request_id.fetch_add(1, Ordering::Relaxed))
    }

    async fn reject_server_request(
        &self,
        request_id: RequestId,
        message: impl Into<String>,
    ) -> Result<(), AgentError> {
        self.requests
            .reject_server_request(
                request_id,
                JSONRPCErrorError {
                    code: -32601,
                    message: message.into(),
                    data: None,
                },
            )
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))
    }

    fn operation_sink(
        &self,
        thread_id: &str,
        turn_id: Option<&str>,
    ) -> Result<Option<SharedAgentEventSink>, AgentError> {
        let operations = self
            .operations
            .lock()
            .map_err(|_| AgentError::Backend("operation mutex poisoned".into()))?;
        Ok(operations
            .values()
            .find(|operation| {
                operation.thread_id == thread_id
                    && turn_id.is_none_or(|turn_id| operation.turn_id == turn_id)
            })
            .map(|operation| Arc::clone(&operation.events)))
    }

    fn remember_approval(
        &self,
        request_id: RequestId,
        response_kind: ApprovalResponseKind,
        thread_id: &str,
        turn_id: Option<&str>,
        title: &str,
        details: Value,
    ) -> Result<(), AgentError> {
        let events = self
            .operation_sink(thread_id, turn_id)?
            .ok_or_else(|| AgentError::Backend("approval has no active Codex turn".into()))?;
        let approval_id = ApprovalId::generated("codex-approval");
        self.approvals
            .lock()
            .map_err(|_| AgentError::Backend("approval mutex poisoned".into()))?
            .insert(
                approval_id.clone(),
                PendingApproval {
                    request_id,
                    response_kind,
                },
            );
        events.emit(AgentEvent::ApprovalRequested {
            approval_id,
            title: title.into(),
            details,
        })
    }

    async fn remember_or_reject(
        &self,
        request_id: RequestId,
        response_kind: ApprovalResponseKind,
        thread_id: &str,
        turn_id: Option<&str>,
        title: &str,
        details: Value,
    ) -> Result<(), AgentError> {
        match self.remember_approval(
            request_id.clone(),
            response_kind,
            thread_id,
            turn_id,
            title,
            details,
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.reject_server_request(request_id, error.to_string())
                    .await
            }
        }
    }

    async fn handle_server_request(&self, request: ServerRequest) -> Result<(), AgentError> {
        match request {
            ServerRequest::DynamicToolCall { request_id, params } => {
                let response = self.execute_dynamic_tool(params).await;
                let response = serde_json::to_value(response)
                    .map_err(|error| AgentError::Backend(error.to_string()))?;
                self.requests
                    .resolve_server_request(request_id, response)
                    .await
                    .map_err(|error| AgentError::Backend(error.to_string()))
            }
            ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
                let details = serde_json::to_value(&params)
                    .map_err(|error| AgentError::Backend(error.to_string()))?;
                self.remember_or_reject(
                    request_id,
                    ApprovalResponseKind::CommandExecution,
                    &params.thread_id,
                    Some(&params.turn_id),
                    "Codex 请求执行命令",
                    details,
                )
                .await
            }
            ServerRequest::FileChangeRequestApproval { request_id, params } => {
                let details = serde_json::to_value(&params)
                    .map_err(|error| AgentError::Backend(error.to_string()))?;
                self.remember_or_reject(
                    request_id,
                    ApprovalResponseKind::FileChange,
                    &params.thread_id,
                    Some(&params.turn_id),
                    "Codex 请求修改文件",
                    details,
                )
                .await
            }
            ServerRequest::PermissionsRequestApproval { request_id, params } => {
                let requested = serde_json::to_value(&params.permissions)
                    .map_err(|error| AgentError::Backend(error.to_string()))?;
                let details = serde_json::to_value(&params)
                    .map_err(|error| AgentError::Backend(error.to_string()))?;
                self.remember_or_reject(
                    request_id,
                    ApprovalResponseKind::Permissions { requested },
                    &params.thread_id,
                    Some(&params.turn_id),
                    "Codex 请求扩展权限",
                    details,
                )
                .await
            }
            ServerRequest::ExecCommandApproval { request_id, params } => {
                let thread_id = params.conversation_id.to_string();
                let details = serde_json::to_value(&params)
                    .map_err(|error| AgentError::Backend(error.to_string()))?;
                self.remember_or_reject(
                    request_id,
                    ApprovalResponseKind::LegacyExec,
                    &thread_id,
                    None,
                    "Codex 请求执行命令",
                    details,
                )
                .await
            }
            ServerRequest::ApplyPatchApproval { request_id, params } => {
                let thread_id = params.conversation_id.to_string();
                let details = serde_json::to_value(&params)
                    .map_err(|error| AgentError::Backend(error.to_string()))?;
                self.remember_or_reject(
                    request_id,
                    ApprovalResponseKind::LegacyPatch,
                    &thread_id,
                    None,
                    "Codex 请求应用补丁",
                    details,
                )
                .await
            }
            request => {
                self.reject_server_request(
                    request.id().clone(),
                    "this embedded Mahayana surface does not support the requested interaction",
                )
                .await
            }
        }
    }

    async fn execute_dynamic_tool(&self, params: DynamicToolCallParams) -> DynamicToolCallResponse {
        if params.namespace.as_deref() != Some("mahayana") {
            return dynamic_tool_error("不支持的工具命名空间");
        }
        let result = match params.tool.as_str() {
            "list_conversations" => {
                let mut conversations = Vec::new();
                for provider in &self.conversation_providers {
                    match provider.list_conversations().await {
                        Ok(items) => conversations.extend(items),
                        Err(error) => {
                            return dynamic_tool_error(&format!(
                                "{} 联系人读取失败：{error}",
                                provider.key()
                            ));
                        }
                    }
                }
                conversations.sort_by(|left, right| {
                    right
                        .updated_at_ms
                        .cmp(&left.updated_at_ms)
                        .then_with(|| left.id.cmp(&right.id))
                });
                serde_json::to_value(conversations)
            }
            "conversation_history" => {
                let Some(conversation_id) = params
                    .arguments
                    .get("conversationId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                else {
                    return dynamic_tool_error("conversationId 不能为空");
                };
                let conversation_id = ConversationId(conversation_id.to_string());
                let provider_key = match provider_key_for_conversation_id(&conversation_id) {
                    Ok(key) => key,
                    Err(error) => return dynamic_tool_error(&error.to_string()),
                };
                let Some(provider) = self
                    .conversation_providers
                    .iter()
                    .find(|provider| provider.key() == provider_key)
                else {
                    return dynamic_tool_error("该会话不属于当前大乘 Runtime");
                };
                let limit = params
                    .arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(50)
                    .clamp(1, 100) as u32;
                match provider.history(&conversation_id, limit).await {
                    Ok(messages) => serde_json::to_value(messages),
                    Err(error) => return dynamic_tool_error(&error.to_string()),
                }
            }
            _ => return dynamic_tool_error("不支持的大乘会话工具"),
        };
        match result {
            Ok(value) => dynamic_tool_success(value),
            Err(error) => dynamic_tool_error(&error.to_string()),
        }
    }

    fn update_delta(
        &self,
        thread_id: &str,
        turn_id: &str,
        delta: String,
    ) -> Result<(), AgentError> {
        let events = {
            let mut operations = self
                .operations
                .lock()
                .map_err(|_| AgentError::Backend("operation mutex poisoned".into()))?;
            let Some(operation) = operations
                .values_mut()
                .find(|operation| operation.thread_id == thread_id && operation.turn_id == turn_id)
            else {
                return Ok(());
            };
            operation.assistant_text.push_str(&delta);
            Arc::clone(&operation.events)
        };
        events.emit(AgentEvent::MessageDelta { delta })
    }

    fn replace_completed_text(
        &self,
        thread_id: &str,
        turn_id: &str,
        text: String,
    ) -> Result<(), AgentError> {
        if let Some(operation) = self
            .operations
            .lock()
            .map_err(|_| AgentError::Backend("operation mutex poisoned".into()))?
            .values_mut()
            .find(|operation| operation.thread_id == thread_id && operation.turn_id == turn_id)
        {
            operation.assistant_text = text;
        }
        Ok(())
    }

    fn take_operation(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<Option<ActiveOperation>, AgentError> {
        let mut operations = self
            .operations
            .lock()
            .map_err(|_| AgentError::Backend("operation mutex poisoned".into()))?;
        let operation_id = operations.iter().find_map(|(operation_id, operation)| {
            (operation.thread_id == thread_id && operation.turn_id == turn_id)
                .then(|| operation_id.clone())
        });
        Ok(operation_id.and_then(|operation_id| operations.remove(&operation_id)))
    }

    fn complete_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        status: TurnStatus,
        error_message: Option<String>,
    ) -> Result<(), AgentError> {
        if status == TurnStatus::InProgress {
            return Ok(());
        }
        let Some(operation) = self.take_operation(thread_id, turn_id)? else {
            return Ok(());
        };
        let result = match status {
            TurnStatus::Completed => operation
                .events
                .emit(AgentEvent::MessageCompleted {
                    message: Message {
                        id: MessageId::generated("message"),
                        conversation_id: operation.conversation_id,
                        role: MessageRole::Assistant,
                        text: operation.assistant_text,
                        created_at_ms: now_ms(),
                        metadata: json!({
                            "codexThreadId": thread_id,
                            "codexTurnId": turn_id,
                            "embedded": true,
                        }),
                    },
                })
                .map_err(|error| error.to_string()),
            TurnStatus::Interrupted => Err("Codex turn was interrupted".into()),
            TurnStatus::Failed => Err(error_message.unwrap_or_else(|| "Codex turn failed".into())),
            TurnStatus::InProgress => unreachable!(),
        };
        let _ = operation.completion.send(result);
        Ok(())
    }

    fn fail_turn(&self, thread_id: &str, turn_id: &str, message: String) -> Result<(), AgentError> {
        if let Some(operation) = self.take_operation(thread_id, turn_id)? {
            let _ = operation.completion.send(Err(message));
        }
        Ok(())
    }

    fn fail_all(&self, message: &str) {
        if let Ok(mut operations) = self.operations.lock() {
            for (_, operation) in operations.drain() {
                let _ = operation.completion.send(Err(message.to_string()));
            }
        }
    }

    async fn handle_event(&self, event: InProcessServerEvent) -> Result<(), AgentError> {
        match event {
            InProcessServerEvent::Lagged { skipped } => {
                self.fail_all(&format!(
                    "in-process Codex event stream lagged by {skipped} events"
                ));
                Ok(())
            }
            InProcessServerEvent::ServerRequest(request) => {
                self.handle_server_request(request).await
            }
            InProcessServerEvent::ServerNotification(notification) => match notification {
                ServerNotification::AgentMessageDelta(delta) => {
                    self.update_delta(&delta.thread_id, &delta.turn_id, delta.delta)
                }
                ServerNotification::ItemCompleted(completed) => {
                    if let ThreadItem::AgentMessage { text, .. } = completed.item {
                        self.replace_completed_text(
                            &completed.thread_id,
                            &completed.turn_id,
                            text,
                        )?;
                    }
                    Ok(())
                }
                ServerNotification::TurnCompleted(completed) => self.complete_turn(
                    &completed.thread_id,
                    &completed.turn.id,
                    completed.turn.status,
                    completed.turn.error.map(|error| error.message),
                ),
                ServerNotification::Error(error) if !error.will_retry => {
                    self.fail_turn(&error.thread_id, &error.turn_id, error.error.message)
                }
                _ => Ok(()),
            },
        }
    }
}

async fn dispatch_events(mut client: InProcessAppServerClient, inner: Weak<CodexAgentInner>) {
    while let Some(event) = client.next_event().await {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        if let Err(error) = inner.handle_event(event).await {
            inner.fail_all(&error.to_string());
        }
    }
    if let Some(inner) = inner.upgrade() {
        inner.fail_all("in-process Codex event stream closed");
    }
}

/// A long-lived, in-process Codex runtime. One background dispatcher owns the
/// app-server event receiver and routes concurrent turns to their own sinks.
/// Requests, interrupts, and approvals use the cloneable in-process handle;
/// no `codex` executable, socket, or cloud Agent is involved.
pub struct CodexAgentBackend {
    inner: Arc<CodexAgentInner>,
}

impl CodexAgentBackend {
    pub async fn start(settings: CodexAgentConfig) -> Result<Self, AgentError> {
        settings.validate()?;
        std::fs::create_dir_all(&settings.codex_home)
            .map_err(|error| AgentError::Backend(error.to_string()))?;

        let loader_overrides = LoaderOverrides {
            ignore_user_config: true,
            ignore_user_and_project_exec_policy_rules: true,
            ..LoaderOverrides::default()
        };
        let mut config = ConfigBuilder::default()
            .codex_home(settings.codex_home)
            .loader_overrides(loader_overrides.clone())
            .harness_overrides(ConfigOverrides {
                model: Some(settings.model.clone()),
                cwd: Some(settings.cwd),
                approval_policy: Some(settings.approval_policy),
                sandbox_mode: Some(settings.sandbox_mode),
                workspace_roots: Some(
                    settings
                        .workspace_roots
                        .iter()
                        .map(|root| {
                            codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(root)
                                .map_err(|error| AgentError::Backend(error.to_string()))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                ..ConfigOverrides::default()
            })
            .build()
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?;

        let provider = ModelProviderInfo {
            name: "大乘 DeepSeek Responses".into(),
            base_url: Some(settings.responses_base_url),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: settings.product_session_token,
            auth: None,
            aws: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(2),
            stream_max_retries: Some(2),
            stream_idle_timeout_ms: Some(300_000),
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        };
        config.model = Some(settings.model);
        config.model_provider_id = PROVIDER_ID.into();
        config.model_provider = provider.clone();
        config
            .model_providers
            .insert(PROVIDER_ID.into(), provider.clone());

        // App-server derives a fresh Config for every thread. Keep the
        // first-party provider in its in-memory CLI override layer as well as
        // the startup Config so thread/start and resume see the exact same
        // provider without writing credentials to config.toml.
        let provider_override = toml::Value::try_from(&provider)
            .map_err(|error| AgentError::Backend(error.to_string()))?;
        let cli_overrides = vec![(format!("model_providers.{PROVIDER_ID}"), provider_override)];

        let config = Arc::new(config);
        let state_db = codex_core::init_state_db(config.as_ref()).await;
        let (arg0_paths, environment_manager) =
            if let Some(codex_executable_path) = settings.codex_executable_path {
                let arg0_paths = Arg0DispatchPaths {
                    codex_self_exe: Some(codex_executable_path),
                    ..Arg0DispatchPaths::default()
                };
                let local_runtime_paths = ExecServerRuntimePaths::from_optional_paths(
                    arg0_paths.codex_self_exe.clone(),
                    arg0_paths.codex_linux_sandbox_exe.clone(),
                )
                .map_err(|error| AgentError::Backend(error.to_string()))?;
                let environment_manager = EnvironmentManager::from_env(Some(local_runtime_paths))
                    .await
                    .map_err(|error| AgentError::Backend(error.to_string()))?;
                (arg0_paths, environment_manager)
            } else {
                (
                    Arg0DispatchPaths::default(),
                    EnvironmentManager::without_environments(),
                )
            };
        let config_warnings = config
            .startup_warnings
            .iter()
            .map(
                |summary| codex_app_server_protocol::ConfigWarningNotification {
                    summary: summary.clone(),
                    details: None,
                    path: None,
                    range: None,
                },
            )
            .collect();
        let client = InProcessAppServerClient::start(InProcessClientStartArgs {
            arg0_paths,
            config: Arc::clone(&config),
            cli_overrides,
            loader_overrides,
            strict_config: false,
            cloud_config_bundle: CloudConfigBundleLoader::default(),
            feedback: CodexFeedback::new(),
            log_db: None,
            state_db,
            environment_manager: Arc::new(environment_manager),
            config_warnings,
            session_source: SessionSource::Exec,
            enable_codex_api_key_env: false,
            client_name: "mahayana-runtime".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            experimental_api: true,
            mcp_server_openai_form_elicitation: false,
            opt_out_notification_methods: Vec::new(),
            channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
        })
        .await
        .map_err(|error| AgentError::Backend(error.to_string()))?;
        let inner = Arc::new(CodexAgentInner {
            config,
            requests: client.request_handle(),
            next_request_id: AtomicI64::new(1),
            operations: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            conversation_providers: settings.conversation_providers,
        });
        tokio::spawn(dispatch_events(client, Arc::downgrade(&inner)));
        Ok(Self { inner })
    }
}

#[async_trait]
impl AgentBackend for CodexAgentBackend {
    async fn start_thread(
        &self,
        _request: StartThreadRequest,
    ) -> Result<AgentThreadId, AgentError> {
        let response: ThreadStartResponse = self
            .inner
            .requests
            .request_typed(ClientRequest::ThreadStart {
                request_id: self.inner.request_id(),
                params: ThreadStartParams {
                    model: self.inner.config.model.clone(),
                    model_provider: Some(self.inner.config.model_provider_id.clone()),
                    cwd: Some(self.inner.config.cwd.to_string_lossy().into_owned()),
                    runtime_workspace_roots: Some(self.inner.config.workspace_roots.clone()),
                    approval_policy: Some(
                        self.inner.config.permissions.approval_policy.value().into(),
                    ),
                    ephemeral: Some(false),
                    dynamic_tools: Some(mahayana_dynamic_tools()),
                    ..ThreadStartParams::default()
                },
            })
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?;
        AgentThreadId::new(response.thread.id)
            .map_err(|error| AgentError::Backend(error.to_string()))
    }

    async fn send_message(
        &self,
        request: AgentMessageRequest,
        events: SharedAgentEventSink,
    ) -> Result<(), AgentError> {
        let thread_id = request.thread_id.to_string();
        let response: TurnStartResponse = self
            .inner
            .requests
            .request_typed(ClientRequest::TurnStart {
                request_id: self.inner.request_id(),
                params: TurnStartParams {
                    thread_id: thread_id.clone(),
                    client_user_message_id: request.client_message_id,
                    input: vec![UserInput::Text {
                        text: request.text,
                        text_elements: Vec::new(),
                    }],
                    cwd: Some(self.inner.config.cwd.to_path_buf()),
                    runtime_workspace_roots: Some(self.inner.config.workspace_roots.clone()),
                    ..TurnStartParams::default()
                },
            })
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?;
        let turn_id = response.turn.id;
        let (completion, result) = oneshot::channel();
        self.inner
            .operations
            .lock()
            .map_err(|_| AgentError::Backend("operation mutex poisoned".into()))?
            .insert(
                request.operation_id,
                ActiveOperation {
                    thread_id,
                    turn_id,
                    conversation_id: request.conversation_id,
                    events,
                    assistant_text: String::new(),
                    completion,
                },
            );
        result
            .await
            .map_err(|_| AgentError::Backend("Codex turn dispatcher stopped".into()))?
            .map_err(AgentError::Backend)
    }

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), AgentError> {
        let operation = self
            .inner
            .operations
            .lock()
            .map_err(|_| AgentError::Backend("operation mutex poisoned".into()))?
            .get(operation_id)
            .map(|operation| (operation.thread_id.clone(), operation.turn_id.clone()))
            .ok_or_else(|| AgentError::OperationNotFound(operation_id.clone()))?;
        let _: TurnInterruptResponse = self
            .inner
            .requests
            .request_typed(ClientRequest::TurnInterrupt {
                request_id: self.inner.request_id(),
                params: TurnInterruptParams {
                    thread_id: operation.0,
                    turn_id: operation.1,
                },
            })
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?;
        Ok(())
    }

    async fn resolve_approval(&self, resolution: ApprovalResolution) -> Result<(), AgentError> {
        let pending = self
            .inner
            .approvals
            .lock()
            .map_err(|_| AgentError::Backend("approval mutex poisoned".into()))?
            .remove(&resolution.approval_id)
            .ok_or_else(|| AgentError::ApprovalNotFound(resolution.approval_id.clone()))?;
        let response = approval_response(pending.response_kind, resolution.decision);
        self.inner
            .requests
            .resolve_server_request(pending.request_id, response)
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))
    }

    fn name(&self) -> &'static str {
        "codex-app-server-in-process"
    }
}

fn mahayana_dynamic_tools() -> Vec<DynamicToolSpec> {
    vec![DynamicToolSpec::Namespace(DynamicToolNamespaceSpec {
        name: "mahayana".into(),
        description: "读取当前大乘 Runtime 中的 Telegram 与法布施联系人会话。".into(),
        tools: vec![
            DynamicToolNamespaceTool::Function(DynamicToolFunctionSpec {
                name: "list_conversations".into(),
                description: "列出当前大乘 Runtime 中可供 Codex 接入的联系人会话。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                defer_loading: false,
            }),
            DynamicToolNamespaceTool::Function(DynamicToolFunctionSpec {
                name: "conversation_history".into(),
                description: "读取一个 Telegram 或法布施联系人会话的最近历史。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "conversationId": {"type": "string"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 100}
                    },
                    "required": ["conversationId"],
                    "additionalProperties": false
                }),
                defer_loading: false,
            }),
        ],
    })]
}

fn dynamic_tool_success(value: Value) -> DynamicToolCallResponse {
    match serde_json::to_string(&value) {
        Ok(text) if text.len() <= 32_000 => DynamicToolCallResponse {
            content_items: vec![DynamicToolCallOutputContentItem::InputText { text }],
            success: true,
        },
        _ => dynamic_tool_error("会话数据超过单次 Codex 上下文上限，请缩小查询范围"),
    }
}

fn dynamic_tool_error(message: &str) -> DynamicToolCallResponse {
    DynamicToolCallResponse {
        content_items: vec![DynamicToolCallOutputContentItem::InputText {
            text: json!({"error": message}).to_string(),
        }],
        success: false,
    }
}

fn approval_response(kind: ApprovalResponseKind, decision: ApprovalDecision) -> Value {
    match kind {
        ApprovalResponseKind::CommandExecution | ApprovalResponseKind::FileChange => json!({
            "decision": match decision {
                ApprovalDecision::Accept => "accept",
                ApprovalDecision::AcceptForSession => "acceptForSession",
                ApprovalDecision::Decline => "decline",
                ApprovalDecision::Cancel => "cancel",
            }
        }),
        ApprovalResponseKind::Permissions { requested } => match decision {
            ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => json!({
                "permissions": requested,
                "scope": if matches!(decision, ApprovalDecision::AcceptForSession) {
                    "session"
                } else {
                    "turn"
                }
            }),
            ApprovalDecision::Decline | ApprovalDecision::Cancel => json!({
                "permissions": {},
                "scope": "turn"
            }),
        },
        ApprovalResponseKind::LegacyExec | ApprovalResponseKind::LegacyPatch => json!({
            "decision": match decision {
                ApprovalDecision::Accept => "approved",
                ApprovalDecision::AcceptForSession => "approved_for_session",
                ApprovalDecision::Decline => "denied",
                ApprovalDecision::Cancel => "abort",
            }
        }),
    }
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
    fn maps_product_approval_decisions_to_app_server_responses() {
        assert_eq!(
            approval_response(
                ApprovalResponseKind::CommandExecution,
                ApprovalDecision::AcceptForSession,
            ),
            json!({"decision": "acceptForSession"})
        );
        assert_eq!(
            approval_response(ApprovalResponseKind::LegacyExec, ApprovalDecision::Cancel),
            json!({"decision": "abort"})
        );
    }

    #[test]
    fn accepts_anonymous_product_session_for_first_party_allowance() {
        let config = CodexAgentConfig {
            codex_home: PathBuf::from("/tmp/mahayana-test"),
            cwd: PathBuf::from("/tmp"),
            workspace_roots: Vec::new(),
            model: "deepseek-chat".into(),
            responses_base_url: "https://example.test/v1".into(),
            product_session_token: None,
            sandbox_mode: SandboxMode::ReadOnly,
            approval_policy: AskForApproval::OnRequest,
            codex_executable_path: None,
            conversation_providers: Vec::new(),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_non_https_provider_and_header_injection() {
        let mut config = CodexAgentConfig {
            codex_home: PathBuf::from("/tmp/mahayana-test"),
            cwd: PathBuf::from("/tmp"),
            workspace_roots: Vec::new(),
            model: "deepseek-chat".into(),
            responses_base_url: "http://example.test/v1".into(),
            product_session_token: Some("secret".into()),
            sandbox_mode: SandboxMode::ReadOnly,
            approval_policy: AskForApproval::OnRequest,
            codex_executable_path: None,
            conversation_providers: Vec::new(),
        };
        assert!(config.validate().is_err());
        config.responses_base_url = "https://example.test/v1".into();
        config.product_session_token = Some("secret\nInjected: yes".into());
        assert!(config.validate().is_err());
    }
}
