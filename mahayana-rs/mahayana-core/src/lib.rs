//! Stable product contracts shared by every Mahayana surface.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::fmt;
use std::path::PathBuf;

pub const RUNTIME_ABI_VERSION: u32 = 1;
pub const CONVERSATION_SCHEMA_VERSION: u32 = 1;
pub const MODEL_RUNTIME_VERSION: u32 = 1;
pub const CODEX_ASSISTANT_CONVERSATION_ID: &str = "codex:agent:assistant";
pub const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-chat";
pub const DEFAULT_DACHENG_RESPONSES_BASE_URL: &str = "https://api.ombhrum.com/codex-deepseek/v1";

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(ContractError::EmptyIdentifier(stringify!($name)));
                }
                Ok(Self(value))
            }

            pub fn generated(prefix: &str) -> Self {
                Self(format!("{prefix}:{}", uuid::Uuid::new_v4()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_id!(ConversationId);
string_id!(MessageId);
string_id!(OperationId);
string_id!(ApprovalId);
string_id!(AgentThreadId);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildProfile {
    #[default]
    DesktopFull,
    MobileEmbedded,
    WebWasm,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelProviderMode {
    LocalModel,
    LocalLoopback,
    #[default]
    FirstPartyDacheng,
    UserConfiguredRemote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelConfig {
    pub provider: ModelProviderMode,
    pub model: String,
    pub base_url: Option<String>,
    /// The key used to locate a secret in platform secure storage. Secret
    /// values are never part of this serializable runtime configuration.
    pub credential_key: Option<String>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: ModelProviderMode::FirstPartyDacheng,
            model: DEFAULT_DEEPSEEK_MODEL.to_string(),
            base_url: Some(DEFAULT_DACHENG_RESPONSES_BASE_URL.to_string()),
            credential_key: Some("mahayana.account.session".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RuntimeConfig {
    #[serde(default)]
    pub build_profile: BuildProfile,
    #[serde(default)]
    pub model: ModelConfig,
    pub data_dir: Option<PathBuf>,
    pub workspace_roots: Vec<PathBuf>,
    #[serde(default)]
    pub remote_agent_enabled: bool,
    #[serde(default)]
    pub telemetry_enabled: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            build_profile: BuildProfile::DesktopFull,
            model: ModelConfig::default(),
            data_dir: None,
            workspace_roots: Vec::new(),
            remote_agent_enabled: false,
            telemetry_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PeerKind {
    CodexAi,
    TelegramContact { user_id: i64 },
    MahayanaContact { contact_id: String },
    MiniApp { app_id: String },
}

impl PeerKind {
    pub fn provider_key(&self) -> &'static str {
        match self {
            Self::CodexAi => "codex",
            Self::TelegramContact { .. } => "telegram",
            Self::MahayanaContact { .. } => "mahayana-social",
            Self::MiniApp { .. } => "miniapp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: ConversationId,
    pub title: String,
    pub peer: PeerKind,
    pub pinned: bool,
    pub unread_count: u32,
    pub updated_at_ms: i64,
}

impl Conversation {
    pub fn codex_assistant() -> Self {
        Self {
            id: ConversationId(CODEX_ASSISTANT_CONVERSATION_ID.to_string()),
            title: "Codex（大乘 AI）".to_string(),
            peer: PeerKind::CodexAi,
            pinned: true,
            unread_count: 0,
            updated_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    User,
    Assistant,
    Contact,
    MiniApp,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub role: MessageRole,
    pub text: String,
    pub created_at_ms: i64,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandDescriptor {
    pub plugin_id: String,
    pub command: String,
    pub tool: String,
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "@type")]
pub enum RuntimeCommand {
    #[serde(rename = "mahayana.runtime.status")]
    Status,
    #[serde(rename = "mahayana.conversation.list")]
    ListConversations,
    #[serde(rename = "mahayana.plugin.commands")]
    ListPluginCommands {
        #[serde(rename = "pluginId")]
        plugin_id: Option<String>,
    },
    #[serde(rename = "mahayana.plugin.ui")]
    PluginUi {
        #[serde(rename = "pluginId")]
        plugin_id: String,
    },
    #[serde(rename = "mahayana.plugin.approveLocal")]
    ApproveLocalPluginTool {
        #[serde(rename = "pluginId")]
        plugin_id: String,
        tool: String,
    },
    #[serde(rename = "mahayana.plugin.callLocal")]
    CallLocalPluginTool {
        #[serde(rename = "pluginId")]
        plugin_id: String,
        tool: String,
        #[serde(default)]
        arguments: Value,
    },
    #[serde(rename = "mahayana.conversation.history")]
    ConversationHistory {
        #[serde(rename = "conversationId")]
        conversation_id: ConversationId,
        limit: Option<u32>,
    },
    #[serde(rename = "mahayana.conversation.send")]
    SendMessage {
        #[serde(rename = "conversationId")]
        conversation_id: ConversationId,
        text: String,
        #[serde(rename = "clientMessageId")]
        client_message_id: Option<String>,
    },
    #[serde(rename = "mahayana.operation.interrupt")]
    Interrupt {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
    },
    #[serde(rename = "mahayana.approval.resolve")]
    ResolveApproval {
        #[serde(rename = "approvalId")]
        approval_id: ApprovalId,
        decision: ApprovalDecision,
        #[serde(default)]
        payload: Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "@type")]
pub enum RuntimeResponse {
    #[serde(rename = "mahayana.runtime.status")]
    Status(RuntimeStatus),
    #[serde(rename = "mahayana.conversation.list")]
    Conversations { data: Vec<Conversation> },
    #[serde(rename = "mahayana.plugin.commands")]
    PluginCommands { data: Vec<PluginCommandDescriptor> },
    #[serde(rename = "mahayana.plugin.ui")]
    PluginUi {
        #[serde(rename = "pluginId")]
        plugin_id: String,
        html: String,
    },
    #[serde(rename = "mahayana.plugin.approvedLocal")]
    LocalPluginToolApproved {
        #[serde(rename = "pluginId")]
        plugin_id: String,
        tool: String,
    },
    #[serde(rename = "mahayana.plugin.localResult")]
    LocalPluginToolResult {
        #[serde(rename = "pluginId")]
        plugin_id: String,
        tool: String,
        result: Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        progress: Vec<Value>,
    },
    #[serde(rename = "mahayana.conversation.history")]
    History { data: Vec<Message> },
    #[serde(rename = "mahayana.operation.accepted")]
    Accepted {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
    },
    #[serde(rename = "mahayana.operation.interrupted")]
    Interrupted {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
    },
    #[serde(rename = "mahayana.approval.resolved")]
    ApprovalResolved {
        #[serde(rename = "approvalId")]
        approval_id: ApprovalId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub runtime_abi_version: u32,
    pub conversation_schema_version: u32,
    pub model_runtime_version: u32,
    pub build_profile: BuildProfile,
    pub model_provider: ModelProviderMode,
    pub model: String,
    pub remote_agent_enabled: bool,
    pub telemetry_enabled: bool,
    pub providers: Vec<String>,
}

/// Provider-reported model token counts. These values are projected from the
/// Codex Responses usage event and are never estimated by the Mahayana host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokenUsage {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
}

/// Latest model usage checkpoint for a running operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokenUsageSnapshot {
    /// Codex supplies the cumulative thread total. Lightweight Responses-only
    /// runtimes omit it instead of pretending a client-side estimate is authoritative.
    pub total: Option<ModelTokenUsage>,
    pub last: ModelTokenUsage,
    pub model_context_window: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "@type")]
pub enum RuntimeEvent {
    #[serde(rename = "mahayana.runtime.ready")]
    Ready { status: RuntimeStatus },
    #[serde(rename = "mahayana.message.delta")]
    MessageDelta {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
        #[serde(rename = "conversationId")]
        conversation_id: ConversationId,
        delta: String,
    },
    #[serde(rename = "mahayana.message.completed")]
    MessageCompleted {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
        message: Message,
    },
    #[serde(rename = "mahayana.model.usage.updated")]
    ModelUsageUpdated {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
        usage: ModelTokenUsageSnapshot,
    },
    #[serde(rename = "mahayana.approval.requested")]
    ApprovalRequested {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
        #[serde(rename = "approvalId")]
        approval_id: ApprovalId,
        title: String,
        details: Value,
    },
    #[serde(rename = "mahayana.plugin.progress")]
    PluginProgress {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
        #[serde(rename = "pluginId")]
        plugin_id: String,
        tool: String,
        progress: u64,
        total: u64,
        message: String,
    },
    #[serde(rename = "mahayana.operation.completed")]
    OperationCompleted {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
    },
    #[serde(rename = "mahayana.operation.failed")]
    OperationFailed {
        #[serde(rename = "operationId")]
        operation_id: OperationId,
        code: String,
        message: String,
    },
    #[serde(rename = "mahayana.runtime.lagged")]
    Lagged { skipped: usize },
    #[serde(rename = "mahayana.provider.degraded")]
    ProviderDegraded { provider: String, message: String },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("{0} must not be empty")]
    EmptyIdentifier(&'static str),
    #[error("message text must not be empty")]
    EmptyMessage,
    #[error("unsupported conversation id: {0}")]
    UnsupportedConversation(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_first_party_deepseek_without_remote_agent() {
        let config = RuntimeConfig::default();
        assert_eq!(config.model.provider, ModelProviderMode::FirstPartyDacheng);
        assert_eq!(config.model.model, "deepseek-chat");
        assert!(!config.remote_agent_enabled);
        assert!(!config.telemetry_enabled);
    }

    #[test]
    fn command_wire_contract_uses_stable_type_and_camel_case_ids() {
        let command = RuntimeCommand::SendMessage {
            conversation_id: ConversationId(CODEX_ASSISTANT_CONVERSATION_ID.to_string()),
            text: "你好".to_string(),
            client_message_id: Some("client-1".to_string()),
        };
        let json = serde_json::to_value(command).expect("serialize command");
        assert_eq!(json["@type"], "mahayana.conversation.send");
        assert_eq!(json["conversationId"], CODEX_ASSISTANT_CONVERSATION_ID);
        assert_eq!(json["clientMessageId"], "client-1");
    }

    #[test]
    fn codex_assistant_is_a_pinned_conversation() {
        let conversation = Conversation::codex_assistant();
        assert_eq!(conversation.id.as_str(), CODEX_ASSISTANT_CONVERSATION_ID);
        assert_eq!(conversation.peer, PeerKind::CodexAi);
        assert!(conversation.pinned);
    }
}
