//! Telegram conversation adapter for the shared Mahayana runtime.

use async_trait::async_trait;
use fabushi_telegram_core::MessageContent;
use fabushi_telegram_core::TelegramState;
use fabushi_telegram_runtime::close_client;
use fabushi_telegram_runtime::create_client;
use fabushi_telegram_runtime::create_persistent_client;
use fabushi_telegram_runtime::execute_json;
use mahayana_conversation::ConversationError;
use mahayana_conversation::ConversationProvider;
use mahayana_conversation::ResolveApprovalRequest;
use mahayana_conversation::SendMessageRequest;
use mahayana_conversation::SharedConversationEventSink;
use mahayana_core::Conversation;
use mahayana_core::ConversationId;
use mahayana_core::Message;
use mahayana_core::MessageId;
use mahayana_core::MessageRole;
use mahayana_core::OperationId;
use mahayana_core::PeerKind;
use mahayana_core::RuntimeEvent;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

const CONVERSATION_PREFIX: &str = "telegram:chat:";

/// Adapts the embedded Telegram command/event core to the product-level
/// conversation contract. The client remains alive for the lifetime of this
/// provider; no helper subprocess or cloud Agent gateway is involved.
pub struct TelegramConversationProvider {
    client_id: u64,
    self_user_id: i64,
    next_local_message_id: AtomicI64,
    owns_client: bool,
}

impl TelegramConversationProvider {
    pub fn new_ephemeral(self_user_id: i64) -> Self {
        Self {
            client_id: create_client(),
            self_user_id,
            next_local_message_id: AtomicI64::new(-1),
            owns_client: true,
        }
    }

    pub fn open_persistent(
        database_path: &Path,
        storage_key: &[u8],
        self_user_id: i64,
    ) -> Result<Self, ConversationError> {
        let database_path = database_path.to_str().ok_or_else(|| {
            ConversationError::Provider("Telegram database path is not UTF-8".into())
        })?;
        let client_id = create_persistent_client(database_path, storage_key)
            .map_err(|error| ConversationError::Provider(error.to_string()))?;
        Ok(Self {
            client_id,
            self_user_id,
            next_local_message_id: AtomicI64::new(-1),
            owns_client: true,
        })
    }

    /// Wraps an existing Telegram client owned by another platform component.
    pub fn from_client_id(client_id: u64, self_user_id: i64) -> Self {
        Self {
            client_id,
            self_user_id,
            next_local_message_id: AtomicI64::new(-1),
            owns_client: false,
        }
    }

    pub fn client_id(&self) -> u64 {
        self.client_id
    }

    fn state(&self) -> Result<TelegramState, ConversationError> {
        let response = self.execute(json!({"@type": "telegram.getState"}))?;
        serde_json::from_value(
            response
                .get("state")
                .cloned()
                .ok_or_else(|| ConversationError::Provider("Telegram state is missing".into()))?,
        )
        .map_err(|error| ConversationError::Provider(error.to_string()))
    }

    fn execute(&self, request: Value) -> Result<Value, ConversationError> {
        let response: Value =
            serde_json::from_str(&execute_json(self.client_id, &request.to_string()))
                .map_err(|error| ConversationError::Provider(error.to_string()))?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            let code = response
                .get("errorCode")
                .and_then(Value::as_str)
                .unwrap_or("telegram_error");
            let message = response
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Telegram command failed");
            return Err(ConversationError::Provider(format!("{code}: {message}")));
        }
        response
            .get("data")
            .cloned()
            .ok_or_else(|| ConversationError::Provider("Telegram response data is missing".into()))
    }
}

impl Drop for TelegramConversationProvider {
    fn drop(&mut self) {
        if self.owns_client {
            let _ = close_client(self.client_id);
        }
    }
}

#[async_trait]
impl ConversationProvider for TelegramConversationProvider {
    fn key(&self) -> &'static str {
        "telegram"
    }

    async fn list_conversations(&self) -> Result<Vec<Conversation>, ConversationError> {
        let state = self.state()?;
        Ok(state
            .chats
            .values()
            .map(|chat| {
                let updated_at_ms = chat
                    .last_message_id
                    .and_then(|message_id| state.messages.get(&(chat.id, message_id)))
                    .map(|message| message.date_unix_ms)
                    .unwrap_or_default();
                Conversation {
                    id: conversation_id(chat.id.0),
                    title: chat.title.clone(),
                    peer: PeerKind::TelegramContact { user_id: chat.id.0 },
                    pinned: chat.pinned_message_id.is_some(),
                    unread_count: chat.unread_count,
                    updated_at_ms,
                }
            })
            .collect())
    }

    async fn history(
        &self,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<Message>, ConversationError> {
        let chat_id = parse_chat_id(conversation_id)?;
        let state = self.state()?;
        if !state.chats.keys().any(|candidate| candidate.0 == chat_id) {
            return Err(ConversationError::ConversationNotFound(
                conversation_id.clone(),
            ));
        }
        let mut messages: Vec<_> = state
            .messages
            .values()
            .filter(|message| message.chat_id.0 == chat_id && !message.is_deleted)
            .map(|message| Message {
                id: telegram_message_id(chat_id, message.id.0),
                conversation_id: conversation_id.clone(),
                role: if message.sender_user_id.0 == self.self_user_id || message.is_outgoing {
                    MessageRole::User
                } else {
                    MessageRole::Contact
                },
                text: message_content_text(&message.content),
                created_at_ms: message.date_unix_ms,
                metadata: json!({
                    "telegramMessageId": message.id.0,
                    "senderUserId": message.sender_user_id.0,
                    "deliveryState": message.delivery_state,
                }),
            })
            .collect();
        messages.sort_by_key(|message| message.created_at_ms);
        let start = messages.len().saturating_sub(limit as usize);
        Ok(messages[start..].to_vec())
    }

    async fn send_message(
        &self,
        request: SendMessageRequest,
        events: SharedConversationEventSink,
    ) -> Result<(), ConversationError> {
        let chat_id = parse_chat_id(&request.conversation_id)?;
        let local_message_id = self.next_local_message_id.fetch_sub(1, Ordering::Relaxed);
        let now_ms = now_ms();
        let client_request_id = request
            .client_message_id
            .as_deref()
            .filter(|value| !value.trim().is_empty() && value.len() <= 128)
            .unwrap_or_else(|| request.operation_id.as_str());
        self.execute(json!({
            "@type": "telegram.executeCoreCommand",
            "command": {
                "type": "queueMessage",
                "chatId": chat_id,
                "localMessageId": local_message_id,
                "senderUserId": self.self_user_id,
                "clientRequestId": client_request_id,
                "dateUnixMs": now_ms,
                "content": {
                    "type": "text",
                    "data": {"text": request.text, "entities": []}
                },
                "replyToMessageId": null,
                "messageThreadId": null
            }
        }))?;
        events.emit(RuntimeEvent::MessageCompleted {
            operation_id: request.operation_id,
            message: Message {
                id: telegram_message_id(chat_id, local_message_id),
                conversation_id: request.conversation_id,
                role: MessageRole::User,
                text: request.text,
                created_at_ms: now_ms,
                metadata: json!({"telegramMessageId": local_message_id, "queued": true}),
            },
        })
    }

    async fn interrupt(&self, _operation_id: &OperationId) -> Result<(), ConversationError> {
        Ok(())
    }

    async fn resolve_approval(
        &self,
        request: ResolveApprovalRequest,
    ) -> Result<(), ConversationError> {
        Err(ConversationError::ApprovalNotFound(request.approval_id))
    }
}

fn conversation_id(chat_id: i64) -> ConversationId {
    ConversationId(format!("{CONVERSATION_PREFIX}{chat_id}"))
}

fn parse_chat_id(conversation_id: &ConversationId) -> Result<i64, ConversationError> {
    conversation_id
        .as_str()
        .strip_prefix(CONVERSATION_PREFIX)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| ConversationError::UnsupportedConversation(conversation_id.to_string()))
}

fn telegram_message_id(chat_id: i64, message_id: i64) -> MessageId {
    MessageId(format!("telegram:chat:{chat_id}:message:{message_id}"))
}

fn message_content_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(text) => text.text.clone(),
        MessageContent::Photo { caption, .. } => media_text("[图片]", &caption.text),
        MessageContent::Video { caption, .. } => media_text("[视频]", &caption.text),
        MessageContent::Animation { caption, .. } => media_text("[动图]", &caption.text),
        MessageContent::Audio { caption, .. } => media_text("[音频]", &caption.text),
        MessageContent::VoiceNote { caption, .. } => media_text("[语音]", &caption.text),
        MessageContent::VideoNote { .. } => "[视频消息]".into(),
        MessageContent::Document { caption, .. } => media_text("[文件]", &caption.text),
        MessageContent::Sticker { emoji, .. } => format!("[贴纸] {emoji}"),
        MessageContent::Poll { question, .. } => format!("[投票] {}", question.text),
        MessageContent::Location { .. } => "[位置]".into(),
        MessageContent::Venue { title, address, .. } => format!("[地点] {title} {address}"),
        MessageContent::Dice { emoji, value } => format!("{emoji} {value}"),
        MessageContent::Story { story_id, .. } => format!("[动态] {}", story_id.0),
        MessageContent::Invoice {
            title,
            currency,
            total_amount_minor,
            ..
        } => format!("[账单] {title} {currency} {total_amount_minor}"),
        MessageContent::Contact {
            first_name,
            last_name,
            ..
        } => format!("[联系人] {first_name} {last_name}")
            .trim_end()
            .to_string(),
        MessageContent::Service { action } => format!("[系统消息] {action:?}"),
        MessageContent::Unsupported { constructor } => format!("[不支持的消息] {constructor}"),
    }
}

fn media_text(label: &str, caption: &str) -> String {
    if caption.is_empty() {
        label.to_string()
    } else {
        format!("{label} {caption}")
    }
}

fn now_ms() -> i64 {
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;
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
    use fabushi_telegram_core::Chat;
    use fabushi_telegram_core::ChatId;
    use fabushi_telegram_core::ChatKind;
    use fabushi_telegram_core::DeliveryState;
    use fabushi_telegram_core::FormattedText;
    use fabushi_telegram_core::Message as TelegramMessage;
    use fabushi_telegram_core::MessageId as TelegramMessageId;
    use fabushi_telegram_core::UserId;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Default)]
    struct EventCollector(Mutex<Vec<RuntimeEvent>>);

    impl mahayana_conversation::ConversationEventSink for EventCollector {
        fn emit(&self, event: RuntimeEvent) -> Result<(), ConversationError> {
            self.0.lock().expect("event mutex").push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn exposes_telegram_chats_history_and_queued_messages() {
        let provider = TelegramConversationProvider::new_ephemeral(7);
        provider
            .execute(json!({
                "@type": "telegram.executeCoreCommand",
                "command": {
                    "type": "upsertChat",
                    "chat": Chat::new(ChatId(42), ChatKind::Private, "善友")
                }
            }))
            .expect("upsert chat");
        provider
            .execute(json!({
                "@type": "telegram.executeCoreCommand",
                "command": {
                    "type": "upsertRemoteMessage",
                    "message": TelegramMessage {
                        id: TelegramMessageId(9),
                        chat_id: ChatId(42),
                        sender_user_id: UserId(42),
                        date_unix_ms: 100,
                        edit_date_unix_ms: None,
                        content: MessageContent::Text(FormattedText::plain("南无阿弥陀佛")),
                        reply_to_message_id: None,
                        message_thread_id: None,
                        delivery_state: DeliveryState::Sent,
                        reactions: vec![],
                        is_outgoing: false,
                        is_pinned: false,
                        is_deleted: false,
                    }
                }
            }))
            .expect("upsert message");

        let conversations = provider.list_conversations().await.expect("list chats");
        assert_eq!(conversations[0].id.as_str(), "telegram:chat:42");
        let history = provider
            .history(&conversation_id(42), 50)
            .await
            .expect("history");
        assert_eq!(history[0].text, "南无阿弥陀佛");
        assert_eq!(history[0].role, MessageRole::Contact);

        let events = Arc::new(EventCollector::default());
        provider
            .send_message(
                SendMessageRequest {
                    conversation_id: conversation_id(42),
                    operation_id: OperationId("operation:test".into()),
                    text: "收到".into(),
                    client_message_id: Some("client:test".into()),
                },
                events.clone(),
            )
            .await
            .expect("queue message");
        assert!(matches!(
            events.0.lock().expect("events").as_slice(),
            [RuntimeEvent::MessageCompleted { .. }]
        ));
        assert_eq!(
            provider
                .history(&conversation_id(42), 50)
                .await
                .expect("history")
                .last()
                .expect("queued message")
                .text,
            "收到"
        );
    }
}
