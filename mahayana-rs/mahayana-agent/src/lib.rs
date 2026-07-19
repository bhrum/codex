//! Agent abstraction used by the conversation runtime.

use async_trait::async_trait;
use mahayana_core::AgentThreadId;
use mahayana_core::ApprovalDecision;
use mahayana_core::ApprovalId;
use mahayana_core::ConversationId;
use mahayana_core::Message;
use mahayana_core::ModelTokenUsageSnapshot;
use mahayana_core::OperationId;
use mahayana_platform_core::HostPlatform;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct StartThreadRequest {
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone)]
pub struct OpenMcpAppRequest {
    pub conversation_id: ConversationId,
    pub plugin_id: String,
    pub platform: HostPlatform,
}

#[derive(Debug, Clone)]
pub struct McpAppSession {
    pub thread_id: AgentThreadId,
    pub plugin_id: String,
    pub server: String,
    pub command_tools: HashMap<String, String>,
    /// MCP tool name to server-authoritative entitlement capability. The host
    /// checks this immediately before the tool call; plugins never decide
    /// whether their own paid capability is unlocked.
    pub tool_gates: HashMap<String, String>,
    pub tools: Vec<Value>,
    pub home_result: Value,
    pub ui_resources: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct AgentMessageRequest {
    pub thread_id: AgentThreadId,
    pub conversation_id: ConversationId,
    pub operation_id: OperationId,
    pub text: String,
    pub client_message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApprovalResolution {
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecision,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    MessageDelta {
        delta: String,
    },
    MessageCompleted {
        message: Message,
    },
    /// Provider-reported usage projected from Codex's existing token usage event.
    TokenUsageUpdated {
        usage: ModelTokenUsageSnapshot,
    },
    /// Progress reported by an MCP Tool while the containing Codex turn is
    /// still running. The MCP protocol owns numeric tokens; the Agent bridge
    /// preserves the user-facing message for every host surface.
    ToolProgress {
        message: String,
    },
    ApprovalRequested {
        approval_id: ApprovalId,
        title: String,
        details: Value,
    },
}

/// Receives streaming output from an [`AgentBackend`]. Implementations must be
/// inexpensive and thread-safe because model runtimes may call them for every
/// token delta.
pub trait AgentEventSink: Send + Sync {
    fn emit(&self, event: AgentEvent) -> Result<(), AgentError>;
}

pub type SharedAgentEventSink = Arc<dyn AgentEventSink>;

/// In-process AI engine boundary. Platform adapters implement this trait with
/// Codex core plus their platform-specific model and tool hosts.
#[async_trait]
pub trait AgentBackend: Send + Sync {
    async fn start_thread(&self, request: StartThreadRequest) -> Result<AgentThreadId, AgentError>;

    async fn send_message(
        &self,
        request: AgentMessageRequest,
        events: SharedAgentEventSink,
    ) -> Result<(), AgentError>;

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), AgentError>;

    async fn resolve_approval(&self, resolution: ApprovalResolution) -> Result<(), AgentError>;

    /// Opens a tool-isolated MCP App thread. Backends that support Codex must
    /// expose only this server's MCP tools and no general shell/filesystem
    /// tools for the returned thread.
    async fn open_mcp_app(&self, _request: OpenMcpAppRequest) -> Result<McpAppSession, AgentError> {
        Err(AgentError::Unavailable(
            "this agent backend does not host MCP Apps".into(),
        ))
    }

    async fn list_mcp_app_tools(
        &self,
        _thread_id: &AgentThreadId,
        _server: &str,
    ) -> Result<Vec<Value>, AgentError> {
        Err(AgentError::Unavailable(
            "this agent backend does not host MCP Apps".into(),
        ))
    }

    async fn call_mcp_app_tool(
        &self,
        _thread_id: &AgentThreadId,
        _server: &str,
        _tool: &str,
        _arguments: Value,
    ) -> Result<Value, AgentError> {
        Err(AgentError::Unavailable(
            "this agent backend does not host MCP Apps".into(),
        ))
    }

    /// Reads a display resource from the MCP server that owns this mini-app
    /// session. Hosts must not inject returned article text into model context
    /// unless the user explicitly asks for it.
    async fn read_mcp_app_resource(
        &self,
        _thread_id: &AgentThreadId,
        _server: &str,
        _uri: &str,
    ) -> Result<Vec<Value>, AgentError> {
        Err(AgentError::Unavailable(
            "this agent backend does not host MCP App resources".into(),
        ))
    }

    fn name(&self) -> &'static str;
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("agent backend is unavailable: {0}")]
    Unavailable(String),
    #[error("agent thread was not found: {0}")]
    ThreadNotFound(AgentThreadId),
    #[error("agent operation was not found: {0}")]
    OperationNotFound(OperationId),
    #[error("agent approval was not found: {0}")]
    ApprovalNotFound(ApprovalId),
    #[error("model usage limit exceeded: {0}")]
    UsageLimitExceeded(String),
    #[error("agent backend failed: {0}")]
    Backend(String),
    #[error("agent event consumer is closed")]
    EventConsumerClosed,
}

/// Explicit non-agent used only for capability reporting on unsupported build
/// profiles. It returns errors and never falls back to a remote Agent gateway.
pub struct UnavailableAgentBackend {
    reason: String,
}

impl UnavailableAgentBackend {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    fn error(&self) -> AgentError {
        AgentError::Unavailable(self.reason.clone())
    }
}

#[async_trait]
impl AgentBackend for UnavailableAgentBackend {
    async fn start_thread(
        &self,
        _request: StartThreadRequest,
    ) -> Result<AgentThreadId, AgentError> {
        Err(self.error())
    }

    async fn send_message(
        &self,
        _request: AgentMessageRequest,
        _events: SharedAgentEventSink,
    ) -> Result<(), AgentError> {
        Err(self.error())
    }

    async fn interrupt(&self, _operation_id: &OperationId) -> Result<(), AgentError> {
        Err(self.error())
    }

    async fn resolve_approval(&self, _resolution: ApprovalResolution) -> Result<(), AgentError> {
        Err(self.error())
    }

    fn name(&self) -> &'static str {
        "unavailable"
    }
}
