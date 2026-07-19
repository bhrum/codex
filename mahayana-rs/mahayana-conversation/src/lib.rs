//! Conversation provider routing shared by CLI, Flutter, and Web surfaces.

use async_trait::async_trait;
use mahayana_core::ApprovalDecision;
use mahayana_core::ApprovalId;
use mahayana_core::Conversation;
use mahayana_core::ConversationId;
use mahayana_core::Message;
use mahayana_core::OperationId;
use mahayana_core::PluginCommandDescriptor;
use mahayana_core::RuntimeEvent;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct SendMessageRequest {
    pub conversation_id: ConversationId,
    pub operation_id: OperationId,
    pub text: String,
    pub client_message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolveApprovalRequest {
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecision,
    pub payload: Value,
}

/// Event sink used by provider implementations. Implementations must preserve
/// ordering for events belonging to the same operation.
pub trait ConversationEventSink: Send + Sync {
    fn emit(&self, event: RuntimeEvent) -> Result<(), ConversationError>;
}

pub type SharedConversationEventSink = Arc<dyn ConversationEventSink>;

/// One source of conversations. Implementations own provider-specific network,
/// persistence, and approval behavior while exposing one product contract.
#[async_trait]
pub trait ConversationProvider: Send + Sync {
    fn key(&self) -> &'static str;

    async fn list_conversations(&self) -> Result<Vec<Conversation>, ConversationError>;

    async fn list_plugin_commands(
        &self,
        _plugin_id: Option<&str>,
    ) -> Result<Vec<PluginCommandDescriptor>, ConversationError> {
        Ok(Vec::new())
    }

    async fn history(
        &self,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<Message>, ConversationError>;

    async fn send_message(
        &self,
        request: SendMessageRequest,
        events: SharedConversationEventSink,
    ) -> Result<(), ConversationError>;

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), ConversationError>;

    async fn resolve_approval(
        &self,
        request: ResolveApprovalRequest,
    ) -> Result<(), ConversationError>;
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, Arc<dyn ConversationProvider>>,
}

impl ProviderRegistry {
    pub fn register(
        &mut self,
        provider: Arc<dyn ConversationProvider>,
    ) -> Result<(), ConversationError> {
        let key = provider.key().trim();
        if key.is_empty() {
            return Err(ConversationError::InvalidProviderKey);
        }
        if self.providers.insert(key.to_string(), provider).is_some() {
            return Err(ConversationError::DuplicateProvider(key.to_string()));
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<Arc<dyn ConversationProvider>> {
        self.providers.get(key).cloned()
    }

    pub fn for_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Arc<dyn ConversationProvider>, ConversationError> {
        let key = provider_key_for_conversation_id(conversation_id)?;
        self.get(key)
            .ok_or_else(|| ConversationError::ProviderUnavailable(key.to_string()))
    }

    pub fn keys(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    pub fn providers(&self) -> Vec<Arc<dyn ConversationProvider>> {
        self.providers.values().cloned().collect()
    }
}

pub fn provider_key_for_conversation_id(
    conversation_id: &ConversationId,
) -> Result<&'static str, ConversationError> {
    let value = conversation_id.as_str();
    if value.starts_with("codex:") {
        Ok("codex")
    } else if value.starts_with("telegram:") {
        Ok("telegram")
    } else if value.starts_with("mahayana:") {
        Ok("mahayana-social")
    } else if value.starts_with("miniapp:") {
        Ok("miniapp")
    } else {
        Err(ConversationError::UnsupportedConversation(
            value.to_string(),
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConversationError {
    #[error("provider key must not be empty")]
    InvalidProviderKey,
    #[error("provider is already registered: {0}")]
    DuplicateProvider(String),
    #[error("provider is unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("unsupported conversation id: {0}")]
    UnsupportedConversation(String),
    #[error("conversation was not found: {0}")]
    ConversationNotFound(ConversationId),
    #[error("operation was not found: {0}")]
    OperationNotFound(OperationId),
    #[error("approval was not found: {0}")]
    ApprovalNotFound(ApprovalId),
    #[error("model usage limit exceeded: {0}")]
    UsageLimitExceeded(String),
    #[error("provider failed: {0}")]
    Provider(String),
    #[error("event consumer is closed")]
    EventConsumerClosed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_all_supported_peer_prefixes() {
        let cases = [
            ("codex:agent:assistant", "codex"),
            ("telegram:user:42", "telegram"),
            ("mahayana:contact:abc", "mahayana-social"),
            ("miniapp:official.flashcards", "miniapp"),
        ];
        for (id, expected) in cases {
            let actual = provider_key_for_conversation_id(&ConversationId(id.to_string()))
                .expect("known conversation prefix");
            assert_eq!(actual, expected);
        }
    }
}
