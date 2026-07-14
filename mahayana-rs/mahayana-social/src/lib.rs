//! Fabushi software-contact conversation provider.

use async_trait::async_trait;
use chrono::DateTime;
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
use mahayana_product::MahayanaProductClient;
use serde_json::Value;
use serde_json::json;

const CONVERSATION_PREFIX: &str = "mahayana:contact:";

#[derive(Clone)]
pub struct MahayanaSocialConversationProvider {
    client: MahayanaProductClient,
    session_token: Option<String>,
}

impl MahayanaSocialConversationProvider {
    pub fn new(client: MahayanaProductClient, session_token: Option<String>) -> Self {
        Self {
            client,
            session_token,
        }
    }

    async fn execute(
        &self,
        request_type: &'static str,
        mut request: Value,
    ) -> Result<Value, ConversationError> {
        if let Some(token) = self.session_token.as_ref()
            && let Some(object) = request.as_object_mut()
        {
            object.insert("token".into(), Value::String(token.clone()));
        }
        let client = self.client.clone();
        tokio::task::spawn_blocking(move || client.execute(request_type, &request))
            .await
            .map_err(|error| ConversationError::Provider(error.to_string()))?
            .map_err(|error| ConversationError::Provider(error.to_string()))
    }
}

#[async_trait]
impl ConversationProvider for MahayanaSocialConversationProvider {
    fn key(&self) -> &'static str {
        "mahayana-social"
    }

    async fn list_conversations(&self) -> Result<Vec<Conversation>, ConversationError> {
        let response = self.execute("mahayana.contacts.list", json!({})).await?;
        conversations_from_response(&response)
    }

    async fn history(
        &self,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<Message>, ConversationError> {
        let contact_id = parse_contact_id(conversation_id)?;
        let response = self
            .execute(
                "mahayana.messages.list",
                json!({"contact": contact_id, "limit": limit}),
            )
            .await?;
        messages_from_response(conversation_id, &response)
    }

    async fn send_message(
        &self,
        request: SendMessageRequest,
        events: SharedConversationEventSink,
    ) -> Result<(), ConversationError> {
        let contact_id = parse_contact_id(&request.conversation_id)?;
        let response = self
            .execute(
                "mahayana.messages.send",
                json!({
                    "contact": contact_id,
                    "text": request.text,
                    "clientRequestId": request
                        .client_message_id
                        .as_deref()
                        .unwrap_or_else(|| request.operation_id.as_str()),
                }),
            )
            .await?;
        let persisted = response.get("message").cloned().unwrap_or(Value::Null);
        let message_id = value_identifier(persisted.get("id"))
            .map(|id| MessageId(format!("mahayana:message:{id}")))
            .unwrap_or_else(|| MessageId::generated("mahayana-message"));
        events.emit(RuntimeEvent::MessageCompleted {
            operation_id: request.operation_id,
            message: Message {
                id: message_id,
                conversation_id: request.conversation_id,
                role: MessageRole::User,
                text: request.text,
                created_at_ms: timestamp_ms(persisted.get("createdAt")),
                metadata: persisted,
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

fn conversations_from_response(response: &Value) -> Result<Vec<Conversation>, ConversationError> {
    let friends = response
        .pointer("/data/friends")
        .and_then(Value::as_array)
        .ok_or_else(|| ConversationError::Provider("contacts response has no friends".into()))?;
    friends
        .iter()
        .map(|friend| {
            let contact_id = contact_identifier(friend).ok_or_else(|| {
                ConversationError::Provider("contact has no stable identifier".into())
            })?;
            let title = ["displayName", "nickname", "username", "userNo"]
                .iter()
                .find_map(|key| friend.get(key).and_then(Value::as_str))
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&contact_id)
                .to_string();
            Ok(Conversation {
                id: ConversationId(format!("{CONVERSATION_PREFIX}{contact_id}")),
                title,
                peer: PeerKind::MahayanaContact {
                    contact_id: contact_id.clone(),
                },
                pinned: false,
                unread_count: friend
                    .get("unreadCount")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
                    .try_into()
                    .unwrap_or(u32::MAX),
                updated_at_ms: timestamp_ms(
                    friend
                        .get("updatedAt")
                        .or_else(|| friend.get("friendshipUpdatedAt")),
                ),
            })
        })
        .collect()
}

fn messages_from_response(
    conversation_id: &ConversationId,
    response: &Value,
) -> Result<Vec<Message>, ConversationError> {
    let messages = response
        .pointer("/data/messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ConversationError::Provider("messages response has no messages".into()))?;
    Ok(messages
        .iter()
        .map(|message| {
            let id = value_identifier(message.get("id"))
                .map(|id| MessageId(format!("mahayana:message:{id}")))
                .unwrap_or_else(|| MessageId::generated("mahayana-message"));
            Message {
                id,
                conversation_id: conversation_id.clone(),
                role: if message
                    .get("isOutgoing")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    MessageRole::User
                } else {
                    MessageRole::Contact
                },
                text: message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                created_at_ms: timestamp_ms(message.get("createdAt")),
                metadata: message.clone(),
            }
        })
        .collect())
}

fn parse_contact_id(conversation_id: &ConversationId) -> Result<&str, ConversationError> {
    conversation_id
        .as_str()
        .strip_prefix(CONVERSATION_PREFIX)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ConversationError::UnsupportedConversation(conversation_id.to_string()))
}

fn contact_identifier(contact: &Value) -> Option<String> {
    ["id", "userId", "userNo", "username"]
        .iter()
        .find_map(|key| value_identifier(contact.get(key)))
}

fn value_identifier(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn timestamp_ms(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(value)) => value.as_i64().unwrap_or_default(),
        Some(Value::String(value)) => DateTime::parse_from_rfc3339(value)
            .map(|value| value.timestamp_millis())
            .unwrap_or_default(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_contacts_and_messages_to_common_contract() {
        let conversations = conversations_from_response(&json!({
            "data": {"friends": [{
                "id": 42,
                "username": "shanyou",
                "displayName": "善友",
                "friendshipUpdatedAt": "2026-07-13T10:00:00Z"
            }]}
        }))
        .expect("contacts");
        assert_eq!(conversations[0].id.as_str(), "mahayana:contact:42");
        assert_eq!(conversations[0].title, "善友");

        let messages = messages_from_response(
            &conversations[0].id,
            &json!({"data": {"messages": [
                {"id": 1, "text": "阿弥陀佛", "isOutgoing": false, "createdAt": "2026-07-13T10:01:00Z"},
                {"id": 2, "text": "善哉", "isOutgoing": true, "createdAt": "2026-07-13T10:02:00Z"}
            ]}}),
        )
        .expect("messages");
        assert_eq!(messages[0].role, MessageRole::Contact);
        assert_eq!(messages[1].role, MessageRole::User);
    }
}
