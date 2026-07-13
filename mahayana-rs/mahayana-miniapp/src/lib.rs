//! Mini-app peers backed by the same embedded Agent as the Codex contact.

use async_trait::async_trait;
use mahayana_agent::{
    AgentBackend, AgentError, AgentEvent, AgentEventSink, AgentMessageRequest, ApprovalResolution,
    SharedAgentEventSink, StartThreadRequest,
};
use mahayana_conversation::{
    ConversationError, ConversationProvider, ResolveApprovalRequest, SendMessageRequest,
    SharedConversationEventSink,
};
use mahayana_core::{
    AgentThreadId, Conversation, ConversationId, Message, MessageId, MessageRole, OperationId,
    PeerKind, RuntimeEvent,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniAppDefinition {
    pub app_id: String,
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub pinned: bool,
}

pub struct MiniAppConversationProvider {
    backend: Arc<dyn AgentBackend>,
    definitions: HashMap<ConversationId, MiniAppDefinition>,
    threads: AsyncMutex<HashMap<ConversationId, AgentThreadId>>,
    history: Arc<Mutex<Vec<Message>>>,
}

impl MiniAppConversationProvider {
    pub fn new(
        backend: Arc<dyn AgentBackend>,
        definitions: Vec<MiniAppDefinition>,
    ) -> Result<Self, ConversationError> {
        let mut by_conversation = HashMap::new();
        for definition in definitions {
            if definition.app_id.trim().is_empty() || definition.title.trim().is_empty() {
                return Err(ConversationError::Provider(
                    "mini-app id and title must not be empty".into(),
                ));
            }
            let conversation_id = ConversationId(format!("miniapp:{}", definition.app_id));
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
            definitions: by_conversation,
            threads: AsyncMutex::new(HashMap::new()),
            history: Arc::new(Mutex::new(Vec::new())),
        })
    }

    async fn thread_id(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<AgentThreadId, ConversationError> {
        let mut threads = self.threads.lock().await;
        if let Some(thread_id) = threads.get(conversation_id) {
            return Ok(thread_id.clone());
        }
        let thread_id = self
            .backend
            .start_thread(StartThreadRequest {
                conversation_id: conversation_id.clone(),
            })
            .await
            .map_err(agent_error)?;
        threads.insert(conversation_id.clone(), thread_id.clone());
        Ok(thread_id)
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
                    app_id: definition.app_id.clone(),
                },
                pinned: definition.pinned,
                unread_count: 0,
                updated_at_ms: 0,
            })
            .collect())
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
        let thread_id = self.thread_id(&request.conversation_id).await?;
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
            metadata: json!({"miniAppId": definition.app_id}),
        };
        self.history
            .lock()
            .map_err(|_| ConversationError::Provider("mini-app history mutex poisoned".into()))?
            .push(user_message);
        let prompt = format!(
            "你正在通过大乘与小程序“{}”（{}）对话。请严格依据该小程序的职责和能力回答，不要冒充其他联系人。\n小程序说明：{}\n\n用户消息：\n{}",
            definition.title,
            definition.app_id,
            if definition.instructions.trim().is_empty() {
                "按小程序名称和当前会话上下文提供服务。"
            } else {
                definition.instructions.as_str()
            },
            request.text,
        );
        let sink: SharedAgentEventSink = Arc::new(MiniAppEventBridge {
            conversation_id: request.conversation_id.clone(),
            operation_id: request.operation_id.clone(),
            events,
            history: Arc::clone(&self.history),
            app_id: definition.app_id.clone(),
        });
        self.backend
            .send_message(
                AgentMessageRequest {
                    thread_id,
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
    ConversationError::Provider(error.to_string())
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
    use mahayana_conversation::ConversationEventSink;
    use mahayana_core::{ApprovalDecision, ApprovalId};

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

        fn name(&self) -> &'static str {
            "echo"
        }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<RuntimeEvent>>);

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
                app_id: "official.flashcards".into(),
                title: "法流背诵卡".into(),
                instructions: "帮助复习佛经背诵卡。".into(),
                pinned: true,
            }],
        )
        .expect("provider");
        let conversations = provider.list_conversations().await.expect("list");
        assert_eq!(conversations[0].id.as_str(), "miniapp:official.flashcards");

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

    #[allow(dead_code)]
    fn approval_types_are_linked() {
        let _ = ApprovalDecision::Accept;
        let _ = ApprovalId("approval:test".into());
    }
}
