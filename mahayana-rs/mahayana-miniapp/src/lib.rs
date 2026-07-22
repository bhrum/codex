//! Mini-app peers backed by the same embedded Agent as the Codex contact.

use async_trait::async_trait;
use mahayana_agent::AgentBackend;
use mahayana_agent::AgentError;
use mahayana_agent::AgentEvent;
use mahayana_agent::AgentEventSink;
use mahayana_agent::AgentMessageRequest;
use mahayana_agent::ApprovalResolution;
use mahayana_agent::McpAppSession;
use mahayana_agent::OpenMcpAppRequest;
use mahayana_agent::SharedAgentEventSink;
use mahayana_conversation::ConversationError;
use mahayana_conversation::ConversationProvider;
use mahayana_conversation::ResolveApprovalRequest;
use mahayana_conversation::SendMessageRequest;
use mahayana_conversation::SharedConversationEventSink;
use mahayana_core::ApprovalDecision;
use mahayana_core::ApprovalId;
use mahayana_core::Conversation;
use mahayana_core::ConversationId;
use mahayana_core::Message;
use mahayana_core::MessageId;
use mahayana_core::MessageRole;
use mahayana_core::OperationId;
use mahayana_core::PeerKind;
use mahayana_core::PluginCommandDescriptor;
use mahayana_core::RuntimeEvent;
use mahayana_miniapp_protocol::ChatDisposition;
use mahayana_miniapp_protocol::FeedItemKind;
use mahayana_miniapp_protocol::HomeDocument;
use mahayana_miniapp_protocol::QuickAction;
use mahayana_platform_core::HostPlatform;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::oneshot;

#[async_trait]
pub trait EntitlementChecker: Send + Sync {
    async fn has_entitlement(&self, plugin_id: &str, capability: &str) -> Result<bool, String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniAppDefinition {
    #[serde(alias = "appId")]
    pub plugin_id: String,
    pub title: String,
    #[serde(default)]
    pub pinned: bool,
}

pub struct MiniAppConversationProvider {
    backend: Arc<dyn AgentBackend>,
    platform: HostPlatform,
    definitions: HashMap<ConversationId, MiniAppDefinition>,
    sessions: AsyncMutex<HashMap<ConversationId, McpAppSession>>,
    approvals: AsyncMutex<HashMap<ApprovalId, oneshot::Sender<ApprovalDecision>>>,
    session_approvals: AsyncMutex<HashSet<(ConversationId, String)>>,
    entitlement_checker: Option<Arc<dyn EntitlementChecker>>,
    history: Arc<Mutex<Vec<Message>>>,
    bootstrapped: AsyncMutex<HashSet<ConversationId>>,
}

struct ToolApprovalRequest<'a> {
    request: &'a SendMessageRequest,
    definition: &'a MiniAppDefinition,
    server: &'a str,
    descriptor: &'a Value,
    tool: &'a str,
    arguments: &'a Value,
    events: SharedAgentEventSink,
}

impl MiniAppConversationProvider {
    pub fn new(
        backend: Arc<dyn AgentBackend>,
        definitions: Vec<MiniAppDefinition>,
    ) -> Result<Self, ConversationError> {
        Self::new_for_platform(backend, definitions, HostPlatform::Desktop)
    }

    pub fn new_for_platform(
        backend: Arc<dyn AgentBackend>,
        definitions: Vec<MiniAppDefinition>,
        platform: HostPlatform,
    ) -> Result<Self, ConversationError> {
        Self::new_for_platform_with_entitlements(backend, definitions, platform, None)
    }

    pub fn new_for_platform_with_entitlements(
        backend: Arc<dyn AgentBackend>,
        definitions: Vec<MiniAppDefinition>,
        platform: HostPlatform,
        entitlement_checker: Option<Arc<dyn EntitlementChecker>>,
    ) -> Result<Self, ConversationError> {
        let mut by_conversation = HashMap::new();
        for definition in definitions {
            if definition.plugin_id.trim().is_empty() || definition.title.trim().is_empty() {
                return Err(ConversationError::Provider(
                    "mini-app id and title must not be empty".into(),
                ));
            }
            let conversation_id = ConversationId(format!("miniapp:{}", definition.plugin_id));
            if by_conversation
                .insert(conversation_id, definition)
                .is_some()
            {
                return Err(ConversationError::Provider(
                    "duplicate mini-app conversation".into(),
                ));
            }
        }
        Ok(Self {
            backend,
            platform,
            definitions: by_conversation,
            sessions: AsyncMutex::new(HashMap::new()),
            approvals: AsyncMutex::new(HashMap::new()),
            session_approvals: AsyncMutex::new(HashSet::new()),
            entitlement_checker,
            history: Arc::new(Mutex::new(Vec::new())),
            bootstrapped: AsyncMutex::new(HashSet::new()),
        })
    }

    async fn session(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<McpAppSession, ConversationError> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(conversation_id) {
            return Ok(session.clone());
        }
        let definition = self
            .definitions
            .get(conversation_id)
            .ok_or_else(|| ConversationError::ConversationNotFound(conversation_id.clone()))?;
        let session = self
            .backend
            .open_mcp_app(OpenMcpAppRequest {
                conversation_id: conversation_id.clone(),
                plugin_id: definition.plugin_id.clone(),
                platform: self.platform,
            })
            .await
            .map_err(agent_error)?;
        sessions.insert(conversation_id.clone(), session.clone());
        Ok(session)
    }

    async fn bootstrap_home(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<(), ConversationError> {
        let mut bootstrapped = self.bootstrapped.lock().await;
        if bootstrapped.contains(conversation_id) {
            return Ok(());
        }
        let session = self.session(conversation_id).await?;
        let Some(home) = HomeDocument::from_tool_result(&session.home_result)
            .map_err(|error| ConversationError::Provider(error.to_string()))?
        else {
            bootstrapped.insert(conversation_id.clone());
            return Ok(());
        };
        let now = now_ms();
        let mut messages = Vec::new();
        if let Some(welcome) = &home.welcome {
            let mut text = welcome.markdown.clone();
            if let Some(source) = &home.app.source {
                text.push_str("\n\n来源：");
                text.push_str(source);
            }
            for tip in &home.tips {
                text.push_str("\n\n> Tip: ");
                text.push_str(&tip.markdown);
            }
            if !home.quick_replies.is_empty() {
                text.push_str("\n\n");
                text.push_str(
                    &home
                        .quick_replies
                        .iter()
                        .map(|reply| reply.label.as_str())
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
            }
            messages.push(home_message(
                conversation_id,
                "welcome",
                text,
                json!({"home":home,"welcomeId":welcome.id}),
                now,
            ));
        }
        for item in home
            .feed
            .items
            .iter()
            .filter(|item| item.kind == FeedItemKind::Announcement)
            .take(3)
        {
            let text = item.summary.as_deref().map_or_else(
                || item.title.clone(),
                |summary| format!("{}\n{}", item.title, summary),
            );
            messages.push(home_message(
                conversation_id,
                "announcement",
                text,
                json!({"feedItem":item}),
                now,
            ));
        }
        let articles = home
            .feed
            .items
            .iter()
            .filter(|item| item.kind == FeedItemKind::Article)
            .collect::<Vec<_>>();
        if !articles.is_empty() {
            let text = articles
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let summary = item.summary.as_deref().unwrap_or("");
                    format!("A{} {}\n{}", index + 1, item.title, summary)
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            messages.push(home_message(
                conversation_id,
                "feed",
                format!("最新文章（回复 A1/A2 阅读）\n\n{text}"),
                json!({"feed":home.feed}),
                now,
            ));
        }
        self.history
            .lock()
            .map_err(|_| ConversationError::Provider("mini-app history mutex poisoned".into()))?
            .extend(messages);
        bootstrapped.insert(conversation_id.clone());
        Ok(())
    }
}

#[async_trait]
impl ConversationProvider for MiniAppConversationProvider {
    fn key(&self) -> &'static str {
        "miniapp"
    }

    async fn list_conversations(&self) -> Result<Vec<Conversation>, ConversationError> {
        Ok(self
            .definitions
            .iter()
            .map(|(id, definition)| Conversation {
                id: id.clone(),
                title: definition.title.clone(),
                peer: PeerKind::MiniApp {
                    app_id: definition.plugin_id.clone(),
                },
                pinned: definition.pinned,
                unread_count: 0,
                updated_at_ms: 0,
            })
            .collect())
    }

    async fn list_plugin_commands(
        &self,
        plugin_id: Option<&str>,
    ) -> Result<Vec<PluginCommandDescriptor>, ConversationError> {
        let targets = self
            .definitions
            .iter()
            .filter(|(_, definition)| {
                plugin_id.is_none_or(|plugin_id| plugin_id == definition.plugin_id)
            })
            .map(|(conversation_id, definition)| (conversation_id.clone(), definition.clone()))
            .collect::<Vec<_>>();
        if plugin_id.is_some() && targets.is_empty() {
            return Ok(Vec::new());
        }
        let mut descriptors = Vec::new();
        for (conversation_id, definition) in targets {
            let session = self.session(&conversation_id).await?;
            let tools = self
                .backend
                .list_mcp_app_tools(&session.thread_id, &session.server)
                .await
                .map_err(agent_error)?;
            for (command, tool_name) in &session.command_tools {
                let Some(tool) = tools.iter().find(|tool| {
                    tool.get("name").and_then(Value::as_str) == Some(tool_name.as_str())
                }) else {
                    continue;
                };
                descriptors.push(PluginCommandDescriptor {
                    plugin_id: definition.plugin_id.clone(),
                    command: command.clone(),
                    tool: tool_name.clone(),
                    input_schema: tool
                        .get("inputSchema")
                        .or_else(|| tool.get("input_schema"))
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    annotations: tool
                        .get("annotations")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                });
            }
        }
        descriptors.sort_by(|left, right| {
            left.plugin_id
                .cmp(&right.plugin_id)
                .then_with(|| left.command.cmp(&right.command))
        });
        Ok(descriptors)
    }

    async fn history(
        &self,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<Message>, ConversationError> {
        if !self.definitions.contains_key(conversation_id) {
            return Err(ConversationError::ConversationNotFound(
                conversation_id.clone(),
            ));
        }
        self.bootstrap_home(conversation_id).await?;
        let history = self
            .history
            .lock()
            .map_err(|_| ConversationError::Provider("mini-app history mutex poisoned".into()))?;
        let messages: Vec<_> = history
            .iter()
            .filter(|message| &message.conversation_id == conversation_id)
            .cloned()
            .collect();
        let start = messages.len().saturating_sub(limit as usize);
        Ok(messages[start..].to_vec())
    }

    async fn send_message(
        &self,
        request: SendMessageRequest,
        events: SharedConversationEventSink,
    ) -> Result<(), ConversationError> {
        let definition = self
            .definitions
            .get(&request.conversation_id)
            .ok_or_else(|| {
                ConversationError::ConversationNotFound(request.conversation_id.clone())
            })?;
        let session = self.session(&request.conversation_id).await?;
        self.bootstrap_home(&request.conversation_id).await?;
        let user_message = Message {
            id: request
                .client_message_id
                .as_deref()
                .and_then(|id| MessageId::new(id).ok())
                .unwrap_or_else(|| MessageId::generated("miniapp-message")),
            conversation_id: request.conversation_id.clone(),
            role: MessageRole::User,
            text: request.text.clone(),
            created_at_ms: now_ms(),
            metadata: json!({"pluginId": definition.plugin_id, "mcpServer": session.server}),
        };
        self.history
            .lock()
            .map_err(|_| ConversationError::Provider("mini-app history mutex poisoned".into()))?
            .push(user_message);
        let sink: SharedAgentEventSink = Arc::new(MiniAppEventBridge {
            conversation_id: request.conversation_id.clone(),
            operation_id: request.operation_id.clone(),
            events,
            history: Arc::clone(&self.history),
            app_id: definition.plugin_id.clone(),
        });

        let home = HomeDocument::from_tool_result(&session.home_result)
            .map_err(|error| ConversationError::Provider(error.to_string()))?;
        if is_codex_repair_command(&request.text) {
            let source = home
                .as_ref()
                .and_then(|home| home.app.source.as_deref())
                .unwrap_or("未声明仓库来源");
            sink.emit(AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("miniapp-repair-preview"),
                    conversation_id: request.conversation_id,
                    role: MessageRole::Assistant,
                    text: format!(
                        "准备把当前小程序问题交给 Codex 修复。\n\n来源：{source}\n\n确认后，宿主将展示脱敏日志、目标仓库和本地 checkout；缺少 checkout 时会单独请求克隆权限。"
                    ),
                    created_at_ms: now_ms(),
                    metadata: json!({
                        "pluginId":definition.plugin_id,
                        "repairHandoff":{
                            "schema":"mahayana.miniapp.repair.v1",
                            "status":"awaitingConfirmation",
                            "source":source,
                            "maxBytes":32 * 1024,
                            "redact":["token","cookie","environment","authorization"],
                        }
                    }),
                },
            })
            .map_err(agent_error)?;
            return Ok(());
        }

        if let (Some(home), Some(index)) = (home.as_ref(), article_selection(&request.text)) {
            let articles = home
                .feed
                .items
                .iter()
                .filter(|item| item.kind == FeedItemKind::Article)
                .collect::<Vec<_>>();
            let item = articles
                .get(index)
                .ok_or_else(|| ConversationError::Provider(format!("没有文章 A{}", index + 1)))?;
            let contents = self
                .backend
                .read_mcp_app_resource(&session.thread_id, &session.server, &item.resource_uri)
                .await
                .map_err(agent_error)?;
            let text = resource_text(&contents).ok_or_else(|| {
                ConversationError::Provider(format!(
                    "文章资源 `{}` 没有 text/markdown 正文",
                    item.resource_uri
                ))
            })?;
            sink.emit(AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("miniapp-article"),
                    conversation_id: request.conversation_id,
                    role: MessageRole::Assistant,
                    text,
                    created_at_ms: now_ms(),
                    metadata: json!({
                        "pluginId":definition.plugin_id,
                        "article":item,
                        "displayOnly":true,
                        "contentReceipt":{"itemId":item.id,"revision":item.revision},
                    }),
                },
            })
            .map_err(agent_error)?;
            return Ok(());
        }

        if request.text.starts_with('/') {
            let tools = self
                .backend
                .list_mcp_app_tools(&session.thread_id, &session.server)
                .await
                .map_err(agent_error)?;
            let (tool, arguments) = parse_tool_command(
                &request.text,
                &definition.plugin_id,
                &tools,
                &session.command_tools,
            )?;
            if let Some(capability) = session.tool_gates.get(&tool) {
                let allowed = match &self.entitlement_checker {
                    Some(checker) => checker
                        .has_entitlement(&definition.plugin_id, capability)
                        .await
                        .map_err(|error| {
                            ConversationError::Provider(format!(
                                "无法检查付费权益 `{capability}`：{error}"
                            ))
                        })?,
                    None => false,
                };
                if !allowed {
                    return Err(ConversationError::Provider(format!(
                        "插件能力 `{capability}` 尚未解锁；请先通过大乘宿主购买或恢复购买"
                    )));
                }
            }
            let descriptor = tools
                .iter()
                .find(|candidate| candidate.get("name").and_then(Value::as_str) == Some(&tool))
                .ok_or_else(|| {
                    ConversationError::Provider(format!(
                        "MCP Tool /{tool} disappeared while preparing the call"
                    ))
                })?;
            self.require_tool_approval(ToolApprovalRequest {
                request: &request,
                definition,
                server: &session.server,
                descriptor,
                tool: &tool,
                arguments: &arguments,
                events: Arc::clone(&sink),
            })
            .await?;
            let result = self
                .backend
                .call_mcp_app_tool(&session.thread_id, &session.server, &tool, arguments)
                .await;
            emit_tool_call_result(
                &sink,
                &request.conversation_id,
                &definition.plugin_id,
                &session.server,
                &tool,
                result,
            )?;
            return Ok(());
        }
        let mut chat_message = request.text.clone();
        let mut action_id = None;
        if let Some(reply) = home.as_ref().and_then(|home| {
            home.quick_replies.iter().find(|reply| {
                reply.id == request.text
                    || reply.label == request.text
                    || reply.aliases.iter().any(|alias| alias == &request.text)
            })
        }) {
            action_id = Some(reply.id.clone());
            match &reply.action {
                QuickAction::Message { value } => chat_message = value.clone(),
                QuickAction::Tool { name, arguments } => {
                    let descriptor = session
                        .tools
                        .iter()
                        .find(|candidate| {
                            candidate.get("name").and_then(Value::as_str) == Some(name.as_str())
                        })
                        .ok_or_else(|| {
                            ConversationError::Provider(format!(
                                "快捷操作引用了不存在的 MCP Tool `{name}`"
                            ))
                        })?;
                    self.require_tool_approval(ToolApprovalRequest {
                        request: &request,
                        definition,
                        server: &session.server,
                        descriptor,
                        tool: name,
                        arguments,
                        events: Arc::clone(&sink),
                    })
                    .await?;
                    let result = self
                        .backend
                        .call_mcp_app_tool(
                            &session.thread_id,
                            &session.server,
                            name,
                            arguments.clone(),
                        )
                        .await;
                    emit_tool_call_result(
                        &sink,
                        &request.conversation_id,
                        &definition.plugin_id,
                        &session.server,
                        name,
                        result,
                    )?;
                    return Ok(());
                }
                QuickAction::Resource { uri } => {
                    let contents = self
                        .backend
                        .read_mcp_app_resource(&session.thread_id, &session.server, uri)
                        .await
                        .map_err(agent_error)?;
                    let text = resource_text(&contents).ok_or_else(|| {
                        ConversationError::Provider(format!("资源 `{uri}` 没有可显示的文本"))
                    })?;
                    sink.emit(AgentEvent::MessageCompleted {
                        message: Message {
                            id: MessageId::generated("miniapp-resource"),
                            conversation_id: request.conversation_id,
                            role: MessageRole::Assistant,
                            text,
                            created_at_ms: now_ms(),
                            metadata: json!({"pluginId":definition.plugin_id,"resourceUri":uri,"displayOnly":true}),
                        },
                    })
                    .map_err(agent_error)?;
                    return Ok(());
                }
                QuickAction::Url { url } => {
                    sink.emit(AgentEvent::MessageCompleted {
                        message: Message {
                            id: MessageId::generated("miniapp-url-approval"),
                            conversation_id: request.conversation_id,
                            role: MessageRole::Assistant,
                            text: format!("小程序请求打开外部链接：{url}"),
                            created_at_ms: now_ms(),
                            metadata: json!({"pluginId":definition.plugin_id,"externalUrl":url,"approvalRequired":true}),
                        },
                    })
                    .map_err(agent_error)?;
                    return Ok(());
                }
            }
        }

        if session
            .tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some("chat"))
        {
            let result = self
                .backend
                .call_mcp_app_tool(
                    &session.thread_id,
                    &session.server,
                    "chat",
                    json!({
                        "message":chat_message,
                        "surface":surface_name(self.platform),
                        "locale":"zh-CN",
                        "actionId":action_id,
                    }),
                )
                .await;
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    emit_mcp_error(
                        &sink,
                        &request.conversation_id,
                        &definition.plugin_id,
                        &session.server,
                        "chat",
                        &error.to_string(),
                    )?;
                    return Ok(());
                }
            };
            if result.get("isError").and_then(Value::as_bool) == Some(true) {
                emit_mcp_error(
                    &sink,
                    &request.conversation_id,
                    &definition.plugin_id,
                    &session.server,
                    "chat",
                    &mcp_result_text(&result),
                )?;
                return Ok(());
            }
            let disposition = match ChatDisposition::from_tool_result(&result) {
                Ok(disposition) => disposition,
                Err(error) => {
                    emit_mcp_error(
                        &sink,
                        &request.conversation_id,
                        &definition.plugin_id,
                        &session.server,
                        "chat",
                        &error.to_string(),
                    )?;
                    return Ok(());
                }
            };
            if disposition.handled {
                sink.emit(AgentEvent::MessageCompleted {
                    message: Message {
                        id: MessageId::generated("mcp-chat-result"),
                        conversation_id: request.conversation_id,
                        role: MessageRole::Assistant,
                        text: mcp_result_text(&result),
                        created_at_ms: now_ms(),
                        metadata: json!({
                            "pluginId":definition.plugin_id,
                            "mcpServer":session.server,
                            "tool":"chat",
                            "mcpResult":result,
                            "chatDisposition":disposition,
                        }),
                    },
                })
                .map_err(agent_error)?;
                return Ok(());
            }
        }
        let prompt = mcp_app_agent_prompt(
            &definition.plugin_id,
            &definition.title,
            &session.server,
            &chat_message,
        );
        self.backend
            .send_message(
                AgentMessageRequest {
                    thread_id: session.thread_id,
                    conversation_id: request.conversation_id,
                    operation_id: request.operation_id,
                    text: prompt,
                    client_message_id: request.client_message_id,
                },
                sink,
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
        if let Some(sender) = self.approvals.lock().await.remove(&request.approval_id) {
            sender
                .send(request.decision)
                .map_err(|_| ConversationError::Provider("MCP Tool 审批已失效".into()))?;
            return Ok(());
        }
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

fn mcp_app_agent_prompt(plugin_id: &str, title: &str, server: &str, message: &str) -> String {
    if plugin_id == "bot-father" {
        return format!(
            "你正在“{title}”Codex 插件工作台中。结合 Server `{server}` 的 MCP Tools 与当前 Codex 工作区工具，实际生成、诊断、修复、测试、打包和发布插件。不要只返回示例或状态。用户消息：\n{message}"
        );
    }
    format!(
        "你正在“{title}”MCP 插件的隔离会话中。只能调用 Server `{server}` 的 MCP Tools。用户消息：\n{message}"
    )
}

impl MiniAppConversationProvider {
    async fn require_tool_approval(
        &self,
        approval: ToolApprovalRequest<'_>,
    ) -> Result<(), ConversationError> {
        let ToolApprovalRequest {
            request,
            definition,
            server,
            descriptor,
            tool,
            arguments,
            events,
        } = approval;
        if tool_is_read_only(descriptor)
            || self
                .session_approvals
                .lock()
                .await
                .contains(&(request.conversation_id.clone(), tool.to_string()))
        {
            return Ok(());
        }
        let approval_id = ApprovalId::generated("mcp-tool-approval");
        let (sender, receiver) = oneshot::channel();
        self.approvals
            .lock()
            .await
            .insert(approval_id.clone(), sender);
        events
            .emit(AgentEvent::ApprovalRequested {
                approval_id: approval_id.clone(),
                title: format!("允许 {} 调用 /{tool}？", definition.title),
                details: json!({
                    "pluginId": definition.plugin_id,
                    "mcpServer": server,
                    "tool": tool,
                    "description": descriptor.get("description"),
                    "annotations": descriptor.get("annotations"),
                    "arguments": arguments,
                }),
            })
            .map_err(agent_error)?;
        let decision = receiver
            .await
            .map_err(|_| ConversationError::Provider("MCP Tool 审批已取消".into()))?;
        self.approvals.lock().await.remove(&approval_id);
        match decision {
            ApprovalDecision::Accept => Ok(()),
            ApprovalDecision::AcceptForSession => {
                self.session_approvals
                    .lock()
                    .await
                    .insert((request.conversation_id.clone(), tool.to_string()));
                Ok(())
            }
            ApprovalDecision::Decline | ApprovalDecision::Cancel => Err(
                ConversationError::Provider("用户拒绝了 MCP Tool 调用".into()),
            ),
        }
    }
}

fn tool_is_read_only(tool: &Value) -> bool {
    tool.pointer("/annotations/readOnlyHint")
        .and_then(Value::as_bool)
        == Some(true)
}

fn parse_tool_command(
    source: &str,
    plugin_id: &str,
    tools: &[Value],
    command_tools: &HashMap<String, String>,
) -> Result<(String, Value), ConversationError> {
    let (command, remainder) = source
        .split_once(char::is_whitespace)
        .map_or((source, ""), |(command, rest)| (command, rest));
    let qualified = command.trim_start_matches('/');
    let command_name = match qualified.split_once(':') {
        Some((requested_plugin_id, command_name)) if requested_plugin_id == plugin_id => {
            command_name
        }
        Some((requested_plugin_id, _)) => {
            return Err(ConversationError::Provider(format!(
                "命令属于插件 `{requested_plugin_id}`，当前小程序是 `{plugin_id}`"
            )));
        }
        None => qualified,
    };
    let name = command_tools
        .get(command_name)
        .map(String::as_str)
        .unwrap_or(command_name);
    let tool = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| ConversationError::Provider(format!("当前 MCP Server 没有 /{name} Tool")))?;
    let schema = tool.get("inputSchema").or_else(|| tool.get("input_schema"));
    let properties = schema
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
            json!({ (field): remainder })
        }
        Some(_) => {
            let parsed: Value = serde_json::from_str(remainder).map_err(|_| {
                ConversationError::Provider(format!(
                    "/{name} 需要多个字段；CLI 请在命令后传入符合 inputSchema 的 JSON 对象"
                ))
            })?;
            if !parsed.is_object() {
                return Err(ConversationError::Provider(format!(
                    "/{name} 参数必须是 JSON 对象"
                )));
            }
            parsed
        }
    };
    Ok((name.to_string(), arguments))
}

fn mcp_result_text(result: &Value) -> String {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|content| content.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        serde_json::to_string_pretty(
            result
                .get("structuredContent")
                .or_else(|| result.get("structured_content"))
                .unwrap_or(result),
        )
        .unwrap_or_else(|_| "MCP Tool 已完成".into())
    } else {
        text
    }
}

fn home_message(
    conversation_id: &ConversationId,
    kind: &str,
    text: String,
    payload: Value,
    created_at_ms: i64,
) -> Message {
    Message {
        id: MessageId::generated("miniapp-home"),
        conversation_id: conversation_id.clone(),
        role: MessageRole::MiniApp,
        text,
        created_at_ms,
        metadata: json!({
            "miniAppHome": true,
            "kind": kind,
            "displayOnly": true,
            "payload": payload,
        }),
    }
}

fn is_codex_repair_command(text: &str) -> bool {
    text == "@codex" || text.starts_with("@codex ")
}

fn article_selection(text: &str) -> Option<usize> {
    let number = text
        .trim()
        .strip_prefix('A')
        .or_else(|| text.trim().strip_prefix('a'))?;
    number
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
}

fn resource_text(contents: &[Value]) -> Option<String> {
    let text = contents
        .iter()
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn surface_name(platform: HostPlatform) -> &'static str {
    match platform {
        HostPlatform::Cli => "cli",
        HostPlatform::Desktop => "desktop",
        HostPlatform::Mobile => "mobile",
        HostPlatform::Web => "web",
    }
}

fn emit_tool_call_result(
    events: &SharedAgentEventSink,
    conversation_id: &ConversationId,
    plugin_id: &str,
    server: &str,
    tool: &str,
    result: Result<Value, AgentError>,
) -> Result<(), ConversationError> {
    match result {
        Ok(result) if result.get("isError").and_then(Value::as_bool) == Some(true) => {
            emit_mcp_error(
                events,
                conversation_id,
                plugin_id,
                server,
                tool,
                &mcp_result_text(&result),
            )
        }
        Ok(result) => events
            .emit(AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("mcp-tool-result"),
                    conversation_id: conversation_id.clone(),
                    role: MessageRole::Assistant,
                    text: mcp_result_text(&result),
                    created_at_ms: now_ms(),
                    metadata: json!({
                        "pluginId":plugin_id,
                        "mcpServer":server,
                        "tool":tool,
                        "mcpResult":result,
                    }),
                },
            })
            .map_err(agent_error),
        Err(error) => emit_mcp_error(
            events,
            conversation_id,
            plugin_id,
            server,
            tool,
            &error.to_string(),
        ),
    }
}

fn emit_mcp_error(
    events: &SharedAgentEventSink,
    conversation_id: &ConversationId,
    plugin_id: &str,
    server: &str,
    tool: &str,
    detail: &str,
) -> Result<(), ConversationError> {
    events
        .emit(AgentEvent::MessageCompleted {
            message: Message {
                id: MessageId::generated("mcp-error"),
                conversation_id: conversation_id.clone(),
                role: MessageRole::Assistant,
                text: format!(
                    "小程序 MCP 调用失败：{detail}\n\n发送 `@codex` 可查看修复交接信息。"
                ),
                created_at_ms: now_ms(),
                metadata: json!({
                    "pluginId":plugin_id,
                    "mcpServer":server,
                    "tool":tool,
                    "error":{"code":"MCP_CALL_FAILED","message":detail},
                    "codexRepairAvailable":true,
                    "fallbackSuppressed":true,
                }),
            },
        })
        .map_err(agent_error)
}

struct MiniAppEventBridge {
    conversation_id: ConversationId,
    operation_id: OperationId,
    events: SharedConversationEventSink,
    history: Arc<Mutex<Vec<Message>>>,
    app_id: String,
}

impl AgentEventSink for MiniAppEventBridge {
    fn emit(&self, event: AgentEvent) -> Result<(), AgentError> {
        let event = match event {
            AgentEvent::MessageDelta { delta } => RuntimeEvent::MessageDelta {
                operation_id: self.operation_id.clone(),
                conversation_id: self.conversation_id.clone(),
                delta,
            },
            AgentEvent::MessageCompleted { mut message } => {
                message.conversation_id = self.conversation_id.clone();
                message.role = MessageRole::MiniApp;
                message.metadata = json!({"miniAppId": self.app_id, "agent": message.metadata});
                self.history
                    .lock()
                    .map_err(|_| AgentError::Backend("mini-app history mutex poisoned".into()))?
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
                plugin_id: self.app_id.clone(),
                tool: String::new(),
                progress: 0,
                total: 0,
                message,
            },
            AgentEvent::ApprovalRequested {
                approval_id,
                title,
                mut details,
            } => {
                if let Some(object) = details.as_object_mut() {
                    object.insert("miniAppId".into(), Value::String(self.app_id.clone()));
                }
                RuntimeEvent::ApprovalRequested {
                    operation_id: self.operation_id.clone(),
                    approval_id,
                    title,
                    details,
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mahayana_agent::AgentMessageRequest;
    use mahayana_agent::StartThreadRequest;
    use mahayana_conversation::ConversationEventSink;
    use mahayana_core::AgentThreadId;
    use mahayana_core::ApprovalDecision;
    use mahayana_core::ApprovalId;

    #[test]
    fn bot_father_prompt_uses_codex_workspace_orchestration() {
        let prompt =
            mcp_app_agent_prompt("bot-father", "Bot Father", "bot-father-local", "修复插件");
        assert!(prompt.contains("Codex 插件工作台"));
        assert!(prompt.contains("实际生成、诊断、修复、测试、打包和发布"));
        assert!(!prompt.contains("只能调用"));
    }

    struct EchoAgent;

    #[async_trait]
    impl AgentBackend for EchoAgent {
        async fn start_thread(
            &self,
            request: StartThreadRequest,
        ) -> Result<AgentThreadId, AgentError> {
            Ok(AgentThreadId(format!("thread:{}", request.conversation_id)))
        }

        async fn send_message(
            &self,
            request: AgentMessageRequest,
            events: SharedAgentEventSink,
        ) -> Result<(), AgentError> {
            events.emit(AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("message"),
                    conversation_id: request.conversation_id,
                    role: MessageRole::Assistant,
                    text: "小程序答复".into(),
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

        async fn open_mcp_app(
            &self,
            request: OpenMcpAppRequest,
        ) -> Result<McpAppSession, AgentError> {
            Ok(McpAppSession {
                thread_id: AgentThreadId(format!("thread:{}", request.conversation_id)),
                plugin_id: request.plugin_id.clone(),
                server: request.plugin_id,
                command_tools: HashMap::new(),
                tool_gates: HashMap::new(),
                tools: vec![
                    json!({"name": "home", "annotations": {"readOnlyHint": true}, "inputSchema": {"type": "object", "properties": {}}}),
                    json!({"name": "write", "annotations": {"readOnlyHint": false}, "inputSchema": {"type": "object", "properties": {"content": {"type": "string"}}}}),
                ],
                home_result: json!({"content": [{"type": "text", "text": "首页"}]}),
                ui_resources: Vec::new(),
            })
        }

        async fn list_mcp_app_tools(
            &self,
            _thread_id: &AgentThreadId,
            _server: &str,
        ) -> Result<Vec<Value>, AgentError> {
            Ok(vec![
                json!({"name": "home", "annotations": {"readOnlyHint": true}, "inputSchema": {"type": "object", "properties": {}}}),
                json!({"name": "write", "annotations": {"readOnlyHint": false}, "inputSchema": {"type": "object", "properties": {"content": {"type": "string"}}}}),
            ])
        }

        async fn call_mcp_app_tool(
            &self,
            _thread_id: &AgentThreadId,
            _server: &str,
            _tool: &str,
            _arguments: Value,
        ) -> Result<Value, AgentError> {
            Ok(json!({"content": [{"type": "text", "text": "首页"}]}))
        }

        fn name(&self) -> &'static str {
            "echo"
        }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<RuntimeEvent>>);

    struct DenyEntitlement;

    #[async_trait]
    impl EntitlementChecker for DenyEntitlement {
        async fn has_entitlement(
            &self,
            _plugin_id: &str,
            _capability: &str,
        ) -> Result<bool, String> {
            Ok(false)
        }
    }

    impl ConversationEventSink for Events {
        fn emit(&self, event: RuntimeEvent) -> Result<(), ConversationError> {
            self.0.lock().expect("events").push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn miniapp_is_a_first_class_conversation() {
        let provider = MiniAppConversationProvider::new(
            Arc::new(EchoAgent),
            vec![MiniAppDefinition {
                plugin_id: "faliu-flashcards".into(),
                title: "法流背诵卡".into(),
                pinned: true,
            }],
        )
        .expect("provider");
        let conversations = provider.list_conversations().await.expect("list");
        assert_eq!(conversations[0].id.as_str(), "miniapp:faliu-flashcards");

        let events = Arc::new(Events::default());
        provider
            .send_message(
                SendMessageRequest {
                    conversation_id: conversations[0].id.clone(),
                    operation_id: OperationId("operation:test".into()),
                    text: "开始复习".into(),
                    client_message_id: None,
                },
                events.clone(),
            )
            .await
            .expect("send");
        let emitted = events.0.lock().expect("events");
        let RuntimeEvent::MessageCompleted { message, .. } = &emitted[0] else {
            panic!("expected completed message")
        };
        assert_eq!(message.role, MessageRole::MiniApp);
        assert_eq!(message.text, "小程序答复");
    }

    #[test]
    fn tool_commands_follow_mcp_input_schemas() {
        let tools = vec![
            json!({"name": "home", "inputSchema": {"type": "object", "properties": {}}}),
            json!({"name": "send", "inputSchema": {"type": "object", "properties": {"content": {"type": "string"}}}}),
            json!({"name": "publish", "inputSchema": {"type": "object", "properties": {"draft_id": {"type": "string"}, "platforms": {"type": "array"}}}}),
        ];

        assert_eq!(
            parse_tool_command("/home", "test-plugin", &tools, &HashMap::new())
                .expect("home command"),
            ("home".to_string(), json!({}))
        );
        assert_eq!(
            parse_tool_command(
                "/send 保留  后续文本",
                "test-plugin",
                &tools,
                &HashMap::new(),
            )
            .expect("send command"),
            ("send".to_string(), json!({"content": "保留  后续文本"}))
        );
        assert_eq!(
            parse_tool_command(
                r#"/publish {"draft_id":"draft-1","platforms":["wechat"]}"#,
                "test-plugin",
                &tools,
                &HashMap::new(),
            )
            .expect("publish command"),
            (
                "publish".to_string(),
                json!({"draft_id": "draft-1", "platforms": ["wechat"]}),
            )
        );
        assert!(parse_tool_command("/quit", "test-plugin", &tools, &HashMap::new()).is_err());
        assert!(parse_tool_command("/history", "test-plugin", &tools, &HashMap::new()).is_err());
        assert_eq!(
            parse_tool_command(
                "/deliver 原样内容",
                "test-plugin",
                &tools,
                &HashMap::from([("deliver".into(), "send".into())]),
            )
            .expect("command projection"),
            ("send".to_string(), json!({"content": "原样内容"}))
        );
        assert_eq!(
            parse_tool_command(
                "/test-plugin:deliver 原样内容",
                "test-plugin",
                &tools,
                &HashMap::from([("deliver".into(), "send".into())]),
            )
            .expect("qualified command projection"),
            ("send".to_string(), json!({"content": "原样内容"})),
        );
        assert!(
            parse_tool_command(
                "/another-plugin:deliver 原样内容",
                "test-plugin",
                &tools,
                &HashMap::new(),
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn write_tool_uses_the_runtime_approval_chain() {
        let provider = Arc::new(
            MiniAppConversationProvider::new(
                Arc::new(EchoAgent),
                vec![MiniAppDefinition {
                    plugin_id: "writer".into(),
                    title: "写入插件".into(),
                    pinned: false,
                }],
            )
            .expect("provider"),
        );
        let events = Arc::new(Events::default());
        let sending = {
            let provider = Arc::clone(&provider);
            let events = Arc::clone(&events);
            tokio::spawn(async move {
                provider
                    .send_message(
                        SendMessageRequest {
                            conversation_id: ConversationId("miniapp:writer".into()),
                            operation_id: OperationId("operation:approval".into()),
                            text: "/write 原样内容".into(),
                            client_message_id: None,
                        },
                        events,
                    )
                    .await
            })
        };
        let approval_id = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let found = events
                    .0
                    .lock()
                    .expect("events")
                    .iter()
                    .find_map(|event| match event {
                        RuntimeEvent::ApprovalRequested { approval_id, .. } => {
                            Some(approval_id.clone())
                        }
                        _ => None,
                    });
                if let Some(approval_id) = found {
                    break approval_id;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("approval request");
        provider
            .resolve_approval(ResolveApprovalRequest {
                approval_id,
                decision: ApprovalDecision::Accept,
                payload: Value::Null,
            })
            .await
            .expect("resolve approval");
        sending.await.expect("join send").expect("send message");

        assert!(
            events
                .0
                .lock()
                .expect("events")
                .iter()
                .any(|event| matches!(event, RuntimeEvent::MessageCompleted { .. }))
        );
    }

    #[tokio::test]
    async fn entitlement_gate_fails_closed_before_direct_tool_call() {
        struct PaidAgent;

        #[async_trait]
        impl AgentBackend for PaidAgent {
            async fn start_thread(
                &self,
                _request: mahayana_agent::StartThreadRequest,
            ) -> Result<AgentThreadId, AgentError> {
                unreachable!()
            }

            async fn send_message(
                &self,
                _request: AgentMessageRequest,
                _events: SharedAgentEventSink,
            ) -> Result<(), AgentError> {
                unreachable!()
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

            async fn open_mcp_app(
                &self,
                request: OpenMcpAppRequest,
            ) -> Result<McpAppSession, AgentError> {
                Ok(McpAppSession {
                    thread_id: AgentThreadId(format!("thread:{}", request.conversation_id)),
                    plugin_id: request.plugin_id.clone(),
                    server: request.plugin_id,
                    command_tools: HashMap::from([("forecast".into(), "get_forecast".into())]),
                    tool_gates: HashMap::from([("get_forecast".into(), "weather.pro".into())]),
                    tools: vec![json!({
                        "name": "get_forecast",
                        "annotations": {"readOnlyHint": true},
                        "inputSchema": {"type": "object", "properties": {}}
                    })],
                    home_result: Value::Null,
                    ui_resources: Vec::new(),
                })
            }

            async fn list_mcp_app_tools(
                &self,
                _thread_id: &AgentThreadId,
                _server: &str,
            ) -> Result<Vec<Value>, AgentError> {
                Ok(vec![json!({
                    "name": "get_forecast",
                    "annotations": {"readOnlyHint": true},
                    "inputSchema": {"type": "object", "properties": {}}
                })])
            }

            async fn call_mcp_app_tool(
                &self,
                _thread_id: &AgentThreadId,
                _server: &str,
                _tool: &str,
                _arguments: Value,
            ) -> Result<Value, AgentError> {
                panic!("gated tool must not be called")
            }

            fn name(&self) -> &'static str {
                "paid-test"
            }
        }

        let provider = MiniAppConversationProvider::new_for_platform_with_entitlements(
            Arc::new(PaidAgent),
            vec![MiniAppDefinition {
                plugin_id: "weather".into(),
                title: "天气".into(),
                pinned: false,
            }],
            HostPlatform::Desktop,
            Some(Arc::new(DenyEntitlement)),
        )
        .expect("provider");
        let result = provider
            .send_message(
                SendMessageRequest {
                    conversation_id: ConversationId("miniapp:weather".into()),
                    operation_id: OperationId("operation:paid".into()),
                    text: "/weather:forecast".into(),
                    client_message_id: None,
                },
                Arc::new(Events::default()),
            )
            .await;

        assert!(
            result
                .expect_err("missing entitlement")
                .to_string()
                .contains("weather.pro")
        );
    }

    struct ContractAgent {
        chat_result: Value,
        model_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl AgentBackend for ContractAgent {
        async fn start_thread(
            &self,
            request: StartThreadRequest,
        ) -> Result<AgentThreadId, AgentError> {
            Ok(AgentThreadId(format!("thread:{}", request.conversation_id)))
        }

        async fn send_message(
            &self,
            request: AgentMessageRequest,
            events: SharedAgentEventSink,
        ) -> Result<(), AgentError> {
            self.model_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            events.emit(AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("model"),
                    conversation_id: request.conversation_id,
                    role: MessageRole::Assistant,
                    text: "Codex fallback".into(),
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

        async fn open_mcp_app(
            &self,
            request: OpenMcpAppRequest,
        ) -> Result<McpAppSession, AgentError> {
            Ok(McpAppSession {
                thread_id: AgentThreadId(format!("thread:{}", request.conversation_id)),
                plugin_id: request.plugin_id.clone(),
                server: request.plugin_id,
                command_tools: HashMap::new(),
                tool_gates: HashMap::new(),
                tools: vec![
                    json!({"name":"home","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object","properties":{}}}),
                    json!({"name":"chat","inputSchema":{"type":"object","properties":{"message":{"type":"string"}}}}),
                ],
                home_result: contract_home(),
                ui_resources: Vec::new(),
            })
        }

        async fn list_mcp_app_tools(
            &self,
            _thread_id: &AgentThreadId,
            _server: &str,
        ) -> Result<Vec<Value>, AgentError> {
            Ok(vec![json!({"name":"chat"})])
        }

        async fn call_mcp_app_tool(
            &self,
            _thread_id: &AgentThreadId,
            _server: &str,
            _tool: &str,
            _arguments: Value,
        ) -> Result<Value, AgentError> {
            Ok(self.chat_result.clone())
        }

        async fn read_mcp_app_resource(
            &self,
            _thread_id: &AgentThreadId,
            _server: &str,
            uri: &str,
        ) -> Result<Vec<Value>, AgentError> {
            Ok(vec![
                json!({"uri":uri,"mimeType":"text/markdown","text":"# 正文"}),
            ])
        }

        fn name(&self) -> &'static str {
            "contract"
        }
    }

    fn contract_home() -> Value {
        json!({"structuredContent":{
            "schema":"mahayana.miniapp.home.v1","revision":"bundle-1",
            "app":{"id":"example","title":"示例","version":"1.0.0","source":"https://github.com/example/app"},
            "welcome":{"id":"welcome","markdown":"欢迎"},
            "tips":[{"id":"tip","markdown":"提示"}],
            "quickReplies":[{"id":"one","label":"1 进入","aliases":["1"],"action":{"type":"message","value":"进入"}}],
            "feed":{"items":[{"id":"guide","revision":"1","kind":"article","title":"指南","publishedAt":"2026-07-19","resourceUri":"mahayana://example/content/articles/guide"}],"nextCursor":null}
        }})
    }

    fn contract_provider(agent: Arc<ContractAgent>) -> MiniAppConversationProvider {
        MiniAppConversationProvider::new_for_platform(
            agent,
            vec![MiniAppDefinition {
                plugin_id: "example".into(),
                title: "示例".into(),
                pinned: false,
            }],
            HostPlatform::Cli,
        )
        .expect("provider")
    }

    #[tokio::test]
    async fn standard_home_bootstraps_once_and_articles_are_lazy() {
        let agent = Arc::new(ContractAgent {
            chat_result: json!({"structuredContent":{"handled":true}}),
            model_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let provider = contract_provider(agent);
        let conversation = ConversationId("miniapp:example".into());
        let first = provider.history(&conversation, 100).await.expect("history");
        let second = provider.history(&conversation, 100).await.expect("history");
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        assert!(
            first[0]
                .text
                .contains("来源：https://github.com/example/app")
        );
        assert!(first[1].text.contains("A1 指南"));

        let events = Arc::new(Events::default());
        provider
            .send_message(
                SendMessageRequest {
                    conversation_id: conversation,
                    operation_id: OperationId("operation:article".into()),
                    text: "A1".into(),
                    client_message_id: None,
                },
                events.clone(),
            )
            .await
            .expect("article");
        assert!(events.0.lock().expect("events").iter().any(|event| {
            matches!(event, RuntimeEvent::MessageCompleted { message, .. } if message.text == "# 正文")
        }));
    }

    #[tokio::test]
    async fn chat_handled_false_falls_back_but_errors_do_not() {
        let fallback_agent = Arc::new(ContractAgent {
            chat_result: json!({"content":[],"structuredContent":{"handled":false}}),
            model_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let fallback = contract_provider(Arc::clone(&fallback_agent));
        fallback
            .send_message(
                SendMessageRequest {
                    conversation_id: ConversationId("miniapp:example".into()),
                    operation_id: OperationId("operation:fallback".into()),
                    text: "普通消息".into(),
                    client_message_id: None,
                },
                Arc::new(Events::default()),
            )
            .await
            .expect("fallback");
        assert_eq!(
            fallback_agent
                .model_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );

        let error_agent = Arc::new(ContractAgent {
            chat_result: json!({"content":[{"type":"text","text":"boom"}],"isError":true}),
            model_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let errors = contract_provider(Arc::clone(&error_agent));
        let events = Arc::new(Events::default());
        errors
            .send_message(
                SendMessageRequest {
                    conversation_id: ConversationId("miniapp:example".into()),
                    operation_id: OperationId("operation:error".into()),
                    text: "普通消息".into(),
                    client_message_id: None,
                },
                events.clone(),
            )
            .await
            .expect("displayed error");
        assert_eq!(
            error_agent
                .model_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert!(events.0.lock().expect("events").iter().any(|event| {
            matches!(event, RuntimeEvent::MessageCompleted { message, .. }
                if message.metadata.pointer("/agent/fallbackSuppressed").and_then(Value::as_bool) == Some(true))
        }));
    }

    #[allow(dead_code)]
    fn approval_types_are_linked() {
        let _ = ApprovalDecision::Accept;
        let _ = ApprovalId("approval:test".into());
    }
}
