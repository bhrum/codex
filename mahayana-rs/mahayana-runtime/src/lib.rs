//! Long-lived local conversation runtime used by all Mahayana frontends.

use crossbeam_channel::Receiver;
use crossbeam_channel::RecvTimeoutError;
use crossbeam_channel::Sender;
use fabushi_official_miniapps::OfficialMiniAppEngine;
use fabushi_official_miniapps::app_definition;
use fabushi_official_miniapps::home_html;
use mahayana_agent::AgentBackend;
use mahayana_agent::AgentError;
use mahayana_agent::AgentEvent;
use mahayana_agent::AgentEventSink;
use mahayana_agent::AgentMessageRequest;
use mahayana_agent::ApprovalResolution;
use mahayana_agent::SharedAgentEventSink;
use mahayana_agent::StartThreadRequest;
use mahayana_conversation::ConversationError;
use mahayana_conversation::ConversationEventSink;
use mahayana_conversation::ConversationProvider;
use mahayana_conversation::ProviderRegistry;
use mahayana_conversation::ResolveApprovalRequest;
use mahayana_conversation::SendMessageRequest;
use mahayana_conversation::SharedConversationEventSink;
use mahayana_core::AgentThreadId;
use mahayana_core::ApprovalId;
use mahayana_core::CONVERSATION_SCHEMA_VERSION;
use mahayana_core::Conversation;
use mahayana_core::ConversationId;
use mahayana_core::MODEL_RUNTIME_VERSION;
use mahayana_core::Message;
use mahayana_core::MessageId;
use mahayana_core::MessageRole;
use mahayana_core::OperationId;
use mahayana_core::PluginCommandDescriptor;
use mahayana_core::RUNTIME_ABI_VERSION;
use mahayana_core::RuntimeCommand;
use mahayana_core::RuntimeConfig;
use mahayana_core::RuntimeEvent;
use mahayana_core::RuntimeResponse;
use mahayana_core::RuntimeStatus;
use mahayana_core::capability::CapabilityRegistry;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::Mutex as AsyncMutex;

pub struct RuntimeBuilder {
    config: RuntimeConfig,
    providers: ProviderRegistry,
}

impl RuntimeBuilder {
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            config,
            providers: ProviderRegistry::default(),
        }
    }

    pub fn with_provider(
        mut self,
        provider: Arc<dyn ConversationProvider>,
    ) -> Result<Self, RuntimeError> {
        self.providers.register(provider)?;
        Ok(self)
    }

    pub fn with_agent_backend(self, backend: Arc<dyn AgentBackend>) -> Result<Self, RuntimeError> {
        self.with_provider(Arc::new(AgentConversationProvider::new(backend)))
    }

    pub fn build(self) -> Result<MahayanaRuntime, RuntimeError> {
        MahayanaRuntime::new(self.config, self.providers)
    }

    /// Starts an Agent backend on the same Tokio runtime that the long-lived
    /// Mahayana runtime will own. This is required by in-process Codex because
    /// its app-server worker tasks must outlive synchronous FFI construction.
    pub fn build_with_agent_backend<F, Fut>(
        self,
        create_backend: F,
    ) -> Result<MahayanaRuntime, RuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Arc<dyn AgentBackend>, AgentError>>,
    {
        self.build_with_agent_backend_and(create_backend, |builder, _backend| Ok(builder))
    }

    /// Variant of [`Self::build_with_agent_backend`] that lets callers add
    /// additional conversation providers backed by the same in-process Agent
    /// before the runtime starts (for example, mini-app peers).
    pub fn build_with_agent_backend_and<F, Fut, C>(
        self,
        create_backend: F,
        configure: C,
    ) -> Result<MahayanaRuntime, RuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Arc<dyn AgentBackend>, AgentError>>,
        C: FnOnce(Self, Arc<dyn AgentBackend>) -> Result<Self, RuntimeError>,
    {
        let async_runtime = create_async_runtime()?;
        let backend = async_runtime
            .block_on(create_backend())
            .map_err(|error| RuntimeError::AgentInitialization(error.to_string()))?;
        let builder = self.with_provider(Arc::new(AgentConversationProvider::new(Arc::clone(
            &backend,
        ))))?;
        let builder = configure(builder, backend)?;
        MahayanaRuntime::new_with_async_runtime(builder.config, builder.providers, async_runtime)
    }
}

pub struct MahayanaRuntime {
    config: RuntimeConfig,
    providers: Arc<ProviderRegistry>,
    async_runtime: tokio::runtime::Runtime,
    event_tx: Sender<RuntimeEvent>,
    event_rx: Receiver<RuntimeEvent>,
    operations: Arc<Mutex<HashMap<OperationId, String>>>,
    approvals: Arc<Mutex<HashMap<ApprovalId, String>>>,
    official_miniapps: Mutex<OfficialMiniAppEngine>,
    approved_local_plugin_tools: Mutex<HashSet<(String, String)>>,
}

impl MahayanaRuntime {
    fn new(config: RuntimeConfig, providers: ProviderRegistry) -> Result<Self, RuntimeError> {
        let async_runtime = create_async_runtime()?;
        Self::new_with_async_runtime(config, providers, async_runtime)
    }

    fn new_with_async_runtime(
        config: RuntimeConfig,
        providers: ProviderRegistry,
        async_runtime: tokio::runtime::Runtime,
    ) -> Result<Self, RuntimeError> {
        if config.remote_agent_enabled {
            return Err(RuntimeError::RemoteAgentForbidden);
        }
        if config.telemetry_enabled && !cfg!(feature = "telemetry") {
            return Err(RuntimeError::TelemetryNotCompiled);
        }
        if matches!(
            config.model.provider,
            mahayana_core::ModelProviderMode::UserConfiguredRemote
        ) && !cfg!(feature = "remote-model-provider")
        {
            return Err(RuntimeError::RemoteModelNotCompiled);
        }

        let (event_tx, event_rx) = crossbeam_channel::bounded(1024);
        let runtime = Self {
            config,
            providers: Arc::new(providers),
            async_runtime,
            event_tx,
            event_rx,
            operations: Arc::new(Mutex::new(HashMap::new())),
            approvals: Arc::new(Mutex::new(HashMap::new())),
            official_miniapps: Mutex::new(OfficialMiniAppEngine::default()),
            approved_local_plugin_tools: Mutex::new(HashSet::new()),
        };
        runtime
            .event_tx
            .send(RuntimeEvent::Ready {
                status: runtime.status(),
            })
            .map_err(|_| RuntimeError::EventConsumerClosed)?;
        Ok(runtime)
    }

    pub fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            runtime_abi_version: RUNTIME_ABI_VERSION,
            conversation_schema_version: CONVERSATION_SCHEMA_VERSION,
            model_runtime_version: MODEL_RUNTIME_VERSION,
            build_profile: self.config.build_profile,
            model_provider: self.config.model.provider,
            model: self.config.model.model.clone(),
            remote_agent_enabled: self.config.remote_agent_enabled,
            telemetry_enabled: self.config.telemetry_enabled,
            providers: self.providers.keys(),
        }
    }

    pub fn execute(&self, command: RuntimeCommand) -> Result<RuntimeResponse, RuntimeError> {
        match command {
            RuntimeCommand::Status => Ok(RuntimeResponse::Status(self.status())),
            RuntimeCommand::ListConversations => Ok(RuntimeResponse::Conversations {
                data: self.list_conversations()?,
            }),
            RuntimeCommand::ListCapabilities { query } => {
                let registry = CapabilityRegistry::from_conversations(
                    self.list_conversations()?,
                    self.config.build_profile,
                );
                Ok(RuntimeResponse::Capabilities {
                    data: registry.list(query.as_deref()),
                })
            }
            RuntimeCommand::InvokeCapability {
                capability_id,
                text,
                client_message_id,
            } => {
                let registry = CapabilityRegistry::from_conversations(
                    self.list_conversations()?,
                    self.config.build_profile,
                );
                let capability = registry
                    .resolve(&capability_id)
                    .cloned()
                    .ok_or_else(|| RuntimeError::CapabilityNotFound(capability_id.clone()))?;
                if !capability.is_invokable() {
                    return Err(RuntimeError::CapabilityUnavailable {
                        capability_id: capability.id,
                        reason: capability
                            .unavailable_reason
                            .unwrap_or_else(|| "当前平台不可用".to_string()),
                    });
                }
                let conversation_id = capability.conversation_id;
                let operation_id =
                    self.start_message(conversation_id.clone(), text, client_message_id)?;
                Ok(RuntimeResponse::CapabilityAccepted {
                    capability_id: capability.id,
                    conversation_id,
                    operation_id,
                })
            }
            RuntimeCommand::ListPluginCommands { plugin_id } => {
                if plugin_id
                    .as_deref()
                    .is_some_and(|plugin_id| app_definition(plugin_id).is_some())
                    && matches!(
                        self.config.build_profile,
                        mahayana_core::BuildProfile::MobileEmbedded
                    )
                {
                    return Ok(RuntimeResponse::PluginCommands {
                        data: local_plugin_commands(plugin_id.as_deref()),
                    });
                }
                let Some(provider) = self.providers.get("miniapp") else {
                    return Ok(RuntimeResponse::PluginCommands {
                        data: local_plugin_commands(plugin_id.as_deref()),
                    });
                };
                let data: Vec<PluginCommandDescriptor> = self
                    .async_runtime
                    .block_on(provider.list_plugin_commands(plugin_id.as_deref()))?;
                Ok(RuntimeResponse::PluginCommands { data })
            }
            RuntimeCommand::PluginUi { plugin_id } => Ok(RuntimeResponse::PluginUi {
                html: home_html(&plugin_id).map_err(RuntimeError::LocalPlugin)?,
                plugin_id,
            }),
            RuntimeCommand::ApproveLocalPluginTool { plugin_id, tool } => {
                let definition = app_definition(&plugin_id).ok_or_else(|| {
                    RuntimeError::LocalPlugin(format!("unknown official plugin: {plugin_id}"))
                })?;
                if !definition.tools.iter().any(|descriptor| {
                    descriptor.get("name").and_then(Value::as_str) == Some(tool.as_str())
                }) {
                    return Err(RuntimeError::LocalPlugin(format!(
                        "{plugin_id} has no MCP Tool {tool}"
                    )));
                }
                lock(&self.approved_local_plugin_tools)?.insert((plugin_id.clone(), tool.clone()));
                Ok(RuntimeResponse::LocalPluginToolApproved { plugin_id, tool })
            }
            RuntimeCommand::CallLocalPluginTool {
                plugin_id,
                tool,
                arguments,
            } => {
                let definition = app_definition(&plugin_id).ok_or_else(|| {
                    RuntimeError::LocalPlugin(format!("unknown official plugin: {plugin_id}"))
                })?;
                let descriptor = definition
                    .tools
                    .iter()
                    .find(|descriptor| {
                        descriptor.get("name").and_then(Value::as_str) == Some(tool.as_str())
                    })
                    .ok_or_else(|| {
                        RuntimeError::LocalPlugin(format!("{plugin_id} has no MCP Tool {tool}"))
                    })?;
                let read_only = descriptor
                    .pointer("/annotations/readOnlyHint")
                    .and_then(Value::as_bool)
                    == Some(true);
                if !read_only
                    && !lock(&self.approved_local_plugin_tools)?
                        .contains(&(plugin_id.clone(), tool.clone()))
                {
                    return Err(RuntimeError::LocalPlugin(format!(
                        "host approval is required for {plugin_id}/{tool}"
                    )));
                }
                let outcome = lock(&self.official_miniapps)?
                    .call_tool(&plugin_id, &tool, arguments)
                    .map_err(RuntimeError::LocalPlugin)?;
                let progress = outcome
                    .progress
                    .into_iter()
                    .map(|update| serde_json::to_value(update).unwrap_or(Value::Null))
                    .collect();
                Ok(RuntimeResponse::LocalPluginToolResult {
                    plugin_id,
                    tool,
                    result: outcome.result,
                    progress,
                })
            }
            RuntimeCommand::ConversationHistory {
                conversation_id,
                limit,
            } => {
                let provider = self.providers.for_conversation(&conversation_id)?;
                let data = self.async_runtime.block_on(
                    provider.history(&conversation_id, limit.unwrap_or(50).clamp(1, 500)),
                )?;
                Ok(RuntimeResponse::History { data })
            }
            RuntimeCommand::SendMessage {
                conversation_id,
                text,
                client_message_id,
            } => Ok(RuntimeResponse::Accepted {
                operation_id: self.start_message(conversation_id, text, client_message_id)?,
            }),
            RuntimeCommand::Interrupt { operation_id } => {
                let provider_key = lock(&self.operations)?
                    .get(&operation_id)
                    .cloned()
                    .ok_or_else(|| ConversationError::OperationNotFound(operation_id.clone()))?;
                let provider = self
                    .providers
                    .get(&provider_key)
                    .ok_or_else(|| ConversationError::ProviderUnavailable(provider_key.clone()))?;
                self.async_runtime
                    .block_on(provider.interrupt(&operation_id))?;
                Ok(RuntimeResponse::Interrupted { operation_id })
            }
            RuntimeCommand::ResolveApproval {
                approval_id,
                decision,
                payload,
            } => {
                let provider_key = lock(&self.approvals)?
                    .remove(&approval_id)
                    .ok_or_else(|| ConversationError::ApprovalNotFound(approval_id.clone()))?;
                let provider = self
                    .providers
                    .get(&provider_key)
                    .ok_or_else(|| ConversationError::ProviderUnavailable(provider_key.clone()))?;
                self.async_runtime
                    .block_on(provider.resolve_approval(ResolveApprovalRequest {
                        approval_id: approval_id.clone(),
                        decision,
                        payload,
                    }))?;
                Ok(RuntimeResponse::ApprovalResolved { approval_id })
            }
        }
    }

    fn list_conversations(&self) -> Result<Vec<Conversation>, RuntimeError> {
        let providers = self.providers.providers();
        let (mut conversations, degraded) = self.async_runtime.block_on(async move {
            let mut conversations = Vec::new();
            let mut degraded = Vec::new();
            for provider in providers {
                match provider.list_conversations().await {
                    Ok(mut provider_conversations) => {
                        conversations.append(&mut provider_conversations)
                    }
                    Err(error) => degraded.push((provider.key().to_string(), error.to_string())),
                }
            }
            (conversations, degraded)
        });
        conversations.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
                .then_with(|| left.id.cmp(&right.id))
        });
        for (provider, message) in degraded {
            let _ = self
                .event_tx
                .send(RuntimeEvent::ProviderDegraded { provider, message });
        }
        Ok(conversations)
    }

    fn start_message(
        &self,
        conversation_id: ConversationId,
        text: String,
        client_message_id: Option<String>,
    ) -> Result<OperationId, RuntimeError> {
        if text.trim().is_empty() {
            return Err(RuntimeError::EmptyMessage);
        }
        let provider = self.providers.for_conversation(&conversation_id)?;
        let provider_key = provider.key().to_string();
        let operation_id = OperationId::generated("operation");
        lock(&self.operations)?.insert(operation_id.clone(), provider_key.clone());
        let request = SendMessageRequest {
            conversation_id,
            operation_id: operation_id.clone(),
            text,
            client_message_id,
        };
        let sink: SharedConversationEventSink = Arc::new(RuntimeEventSink {
            provider_key,
            event_tx: self.event_tx.clone(),
            approvals: Arc::clone(&self.approvals),
        });
        let event_tx = self.event_tx.clone();
        let operations = Arc::clone(&self.operations);
        let task_operation_id = operation_id.clone();
        self.async_runtime.spawn(async move {
            let result = provider.send_message(request, sink).await;
            let event = match result {
                Ok(()) => RuntimeEvent::OperationCompleted {
                    operation_id: task_operation_id.clone(),
                },
                Err(error) => RuntimeEvent::OperationFailed {
                    operation_id: task_operation_id.clone(),
                    code: if matches!(error, ConversationError::UsageLimitExceeded(_)) {
                        "usage_limit_exceeded"
                    } else {
                        "provider_error"
                    }
                    .to_string(),
                    message: error.to_string(),
                },
            };
            let _ = event_tx.send(event);
            if let Ok(mut operations) = operations.lock() {
                operations.remove(&task_operation_id);
            }
        });
        Ok(operation_id)
    }

    pub fn receive(&self, timeout: Duration) -> Result<Option<RuntimeEvent>, RuntimeError> {
        match self.event_rx.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(RuntimeError::EventConsumerClosed),
        }
    }
}

fn create_async_runtime() -> Result<tokio::runtime::Runtime, RuntimeError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("mahayana-runtime")
        // Codex app-server thread creation walks a large typed protocol and
        // configuration graph. The platform default (commonly 2 MiB) can
        // overflow on the first embedded thread/turn even though the same
        // code works in the standalone Codex process.
        .thread_stack_size(16 * 1024 * 1024)
        .build()
        .map_err(|error| RuntimeError::Initialization(error.to_string()))
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, RuntimeError> {
    mutex
        .lock()
        .map_err(|_| RuntimeError::Synchronization("mutex poisoned".to_string()))
}

struct RuntimeEventSink {
    provider_key: String,
    event_tx: Sender<RuntimeEvent>,
    approvals: Arc<Mutex<HashMap<ApprovalId, String>>>,
}

impl ConversationEventSink for RuntimeEventSink {
    fn emit(&self, event: RuntimeEvent) -> Result<(), ConversationError> {
        if let RuntimeEvent::ApprovalRequested { approval_id, .. } = &event {
            self.approvals
                .lock()
                .map_err(|_| ConversationError::Provider("approval map poisoned".to_string()))?
                .insert(approval_id.clone(), self.provider_key.clone());
        }
        self.event_tx
            .send(event)
            .map_err(|_| ConversationError::EventConsumerClosed)
    }
}

struct AgentConversationProvider {
    backend: Arc<dyn AgentBackend>,
    thread_id: AsyncMutex<Option<AgentThreadId>>,
    history: Arc<Mutex<Vec<Message>>>,
}

impl AgentConversationProvider {
    fn new(backend: Arc<dyn AgentBackend>) -> Self {
        Self {
            backend,
            thread_id: AsyncMutex::new(None),
            history: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn thread_id(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<AgentThreadId, ConversationError> {
        let mut thread_id = self.thread_id.lock().await;
        if let Some(thread_id) = thread_id.as_ref() {
            return Ok(thread_id.clone());
        }
        let created = self
            .backend
            .start_thread(StartThreadRequest {
                conversation_id: conversation_id.clone(),
            })
            .await
            .map_err(agent_error)?;
        *thread_id = Some(created.clone());
        Ok(created)
    }
}

#[async_trait::async_trait]
impl ConversationProvider for AgentConversationProvider {
    fn key(&self) -> &'static str {
        "codex"
    }

    async fn list_conversations(&self) -> Result<Vec<Conversation>, ConversationError> {
        Ok(vec![Conversation::codex_assistant()])
    }

    async fn history(
        &self,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<Message>, ConversationError> {
        let history = self
            .history
            .lock()
            .map_err(|_| ConversationError::Provider("history mutex poisoned".to_string()))?;
        let matching: Vec<_> = history
            .iter()
            .filter(|message| &message.conversation_id == conversation_id)
            .cloned()
            .collect();
        let start = matching.len().saturating_sub(limit as usize);
        Ok(matching[start..].to_vec())
    }

    async fn send_message(
        &self,
        request: SendMessageRequest,
        events: SharedConversationEventSink,
    ) -> Result<(), ConversationError> {
        let thread_id = self.thread_id(&request.conversation_id).await?;
        let user_message = Message {
            id: request
                .client_message_id
                .as_deref()
                .and_then(|id| MessageId::new(id).ok())
                .unwrap_or_else(|| MessageId::generated("message")),
            conversation_id: request.conversation_id.clone(),
            role: MessageRole::User,
            text: request.text.clone(),
            created_at_ms: now_ms(),
            metadata: Value::Null,
        };
        self.history
            .lock()
            .map_err(|_| ConversationError::Provider("history mutex poisoned".to_string()))?
            .push(user_message);
        let agent_sink: SharedAgentEventSink = Arc::new(AgentEventBridge {
            conversation_id: request.conversation_id.clone(),
            operation_id: request.operation_id.clone(),
            events,
            history: Arc::clone(&self.history),
        });
        self.backend
            .send_message(
                AgentMessageRequest {
                    thread_id,
                    conversation_id: request.conversation_id,
                    operation_id: request.operation_id,
                    text: request.text,
                    client_message_id: request.client_message_id,
                },
                agent_sink,
            )
            .await
            .map_err(agent_error)
    }

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), ConversationError> {
        self.backend
            .interrupt(operation_id)
            .await
            .map_err(agent_error)
    }

    async fn resolve_approval(
        &self,
        request: ResolveApprovalRequest,
    ) -> Result<(), ConversationError> {
        self.backend
            .resolve_approval(ApprovalResolution {
                approval_id: request.approval_id,
                decision: request.decision,
                payload: request.payload,
            })
            .await
            .map_err(agent_error)
    }
}

struct AgentEventBridge {
    conversation_id: ConversationId,
    operation_id: OperationId,
    events: SharedConversationEventSink,
    history: Arc<Mutex<Vec<Message>>>,
}

impl AgentEventSink for AgentEventBridge {
    fn emit(&self, event: AgentEvent) -> Result<(), AgentError> {
        let event = match event {
            AgentEvent::MessageDelta { delta } => RuntimeEvent::MessageDelta {
                operation_id: self.operation_id.clone(),
                conversation_id: self.conversation_id.clone(),
                delta,
            },
            AgentEvent::MessageCompleted { mut message } => {
                message.conversation_id = self.conversation_id.clone();
                self.history
                    .lock()
                    .map_err(|_| AgentError::Backend("history mutex poisoned".to_string()))?
                    .push(message.clone());
                RuntimeEvent::MessageCompleted {
                    operation_id: self.operation_id.clone(),
                    message,
                }
            }
            AgentEvent::TokenUsageUpdated { usage } => RuntimeEvent::ModelUsageUpdated {
                operation_id: self.operation_id.clone(),
                usage,
            },
            AgentEvent::ToolProgress { message } => RuntimeEvent::PluginProgress {
                operation_id: self.operation_id.clone(),
                plugin_id: "codex".into(),
                tool: String::new(),
                progress: 0,
                total: 0,
                message,
            },
            AgentEvent::ApprovalRequested {
                approval_id,
                title,
                details,
            } => RuntimeEvent::ApprovalRequested {
                operation_id: self.operation_id.clone(),
                approval_id,
                title,
                details,
            },
        };
        self.events
            .emit(event)
            .map_err(|error| AgentError::Backend(error.to_string()))
    }
}

fn agent_error(error: AgentError) -> ConversationError {
    match error {
        AgentError::UsageLimitExceeded(message) => ConversationError::UsageLimitExceeded(message),
        error => ConversationError::Provider(error.to_string()),
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

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime initialization failed: {0}")]
    Initialization(String),
    #[error("Agent backend initialization failed: {0}")]
    AgentInitialization(String),
    #[error("remote Agent gateways are forbidden in embedded runtime builds")]
    RemoteAgentForbidden,
    #[error("remote model provider support was not compiled")]
    RemoteModelNotCompiled,
    #[error("telemetry support was not compiled")]
    TelemetryNotCompiled,
    #[error("message text must not be empty")]
    EmptyMessage,
    #[error(transparent)]
    Conversation(#[from] ConversationError),
    #[error("runtime event consumer is closed")]
    EventConsumerClosed,
    #[error("runtime synchronization failed: {0}")]
    Synchronization(String),
    #[error("local Mini App runtime failed: {0}")]
    LocalPlugin(String),
    #[error("capability not found: {0}")]
    CapabilityNotFound(String),
    #[error("capability unavailable: {capability_id}: {reason}")]
    CapabilityUnavailable {
        capability_id: String,
        reason: String,
    },
}

fn local_plugin_commands(plugin_id: Option<&str>) -> Vec<PluginCommandDescriptor> {
    let mut commands = fabushi_official_miniapps::OFFICIAL_PLUGIN_IDS
        .iter()
        .filter(|candidate| plugin_id.is_none_or(|plugin_id| plugin_id == **candidate))
        .filter_map(|plugin_id| app_definition(plugin_id))
        .flat_map(|definition| {
            definition
                .commands
                .into_iter()
                .filter_map(move |(command, tool)| {
                    let descriptor = definition.tools.iter().find(|descriptor| {
                        descriptor.get("name").and_then(Value::as_str) == Some(tool.as_str())
                    })?;
                    Some(PluginCommandDescriptor {
                        plugin_id: definition.id.clone(),
                        command,
                        tool,
                        input_schema: descriptor
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({"type":"object"})),
                        annotations: descriptor
                            .get("annotations")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    })
                })
        })
        .collect::<Vec<_>>();
    commands.sort_by(|left, right| {
        left.plugin_id
            .cmp(&right.plugin_id)
            .then_with(|| left.command.cmp(&right.command))
    });
    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use mahayana_core::ApprovalDecision;
    use mahayana_core::CODEX_ASSISTANT_CONVERSATION_ID;

    struct EchoAgent;

    #[async_trait]
    impl AgentBackend for EchoAgent {
        async fn start_thread(
            &self,
            _request: StartThreadRequest,
        ) -> Result<AgentThreadId, AgentError> {
            AgentThreadId::new("thread:test")
                .map_err(|error| AgentError::Backend(error.to_string()))
        }

        async fn send_message(
            &self,
            request: AgentMessageRequest,
            events: SharedAgentEventSink,
        ) -> Result<(), AgentError> {
            events.emit(AgentEvent::MessageDelta {
                delta: "大乘：".to_string(),
            })?;
            events.emit(AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("message"),
                    conversation_id: request.conversation_id,
                    role: MessageRole::Assistant,
                    text: format!("大乘：{}", request.text),
                    created_at_ms: now_ms(),
                    metadata: Value::Null,
                },
            })
        }

        async fn interrupt(&self, _operation_id: &OperationId) -> Result<(), AgentError> {
            Ok(())
        }

        async fn resolve_approval(
            &self,
            _resolution: ApprovalResolution,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn name(&self) -> &'static str {
            "echo-test"
        }
    }

    #[test]
    fn routes_codex_contact_and_streams_events() {
        let runtime = RuntimeBuilder::new(RuntimeConfig::default())
            .with_agent_backend(Arc::new(EchoAgent))
            .expect("register agent")
            .build()
            .expect("build runtime");
        let ready = runtime
            .receive(Duration::from_millis(10))
            .expect("receive ready")
            .expect("ready event");
        assert!(matches!(ready, RuntimeEvent::Ready { .. }));

        let response = runtime
            .execute(RuntimeCommand::SendMessage {
                conversation_id: ConversationId(CODEX_ASSISTANT_CONVERSATION_ID.to_string()),
                text: "你好".to_string(),
                client_message_id: None,
            })
            .expect("send message");
        let RuntimeResponse::Accepted { operation_id } = response else {
            panic!("expected accepted response");
        };

        let mut saw_delta = false;
        let mut saw_message = false;
        let mut saw_complete = false;
        for _ in 0..5 {
            let event = runtime
                .receive(Duration::from_secs(1))
                .expect("receive event")
                .expect("event before timeout");
            match event {
                RuntimeEvent::MessageDelta {
                    operation_id: event_operation,
                    delta,
                    ..
                } => {
                    assert_eq!(event_operation, operation_id);
                    assert_eq!(delta, "大乘：");
                    saw_delta = true;
                }
                RuntimeEvent::MessageCompleted { message, .. } => {
                    assert_eq!(message.text, "大乘：你好");
                    saw_message = true;
                }
                RuntimeEvent::OperationCompleted { .. } => {
                    saw_complete = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_delta && saw_message && saw_complete);
    }

    #[test]
    fn rejects_cloud_agent_configuration_at_runtime_creation() {
        let config = RuntimeConfig {
            remote_agent_enabled: true,
            ..RuntimeConfig::default()
        };
        let result = RuntimeBuilder::new(config)
            .with_agent_backend(Arc::new(EchoAgent))
            .expect("register agent")
            .build();
        assert!(matches!(result, Err(RuntimeError::RemoteAgentForbidden)));
    }

    #[test]
    fn approval_decision_wire_values_remain_stable() {
        assert_eq!(
            serde_json::to_value(ApprovalDecision::AcceptForSession).expect("serialize decision"),
            "acceptForSession"
        );
    }
}
