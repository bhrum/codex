use std::path::PathBuf;
use std::sync::Arc;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeSessionProviderFuture;
use codex_code_mode_protocol::CodeModeSessionResultFuture;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::NotificationFuture;
use codex_code_mode_protocol::StartedCell;
use codex_code_mode_protocol::ToolInvocationFuture;
use codex_code_mode_protocol::WaitOutcome;
use codex_code_mode_protocol::WaitRequest;
use tokio_util::sync::CancellationToken;

const UNAVAILABLE_MESSAGE: &str =
    "the V8 code-mode tool is unavailable in the embedded mobile runtime";

/// Mobile keeps the full Codex agent and native tool stack in process, but does
/// not link V8 because rusty_v8 does not publish Android or iOS artifacts.
pub struct NoopCodeModeSessionDelegate;

impl CodeModeSessionDelegate for NoopCodeModeSessionDelegate {
    fn invoke_tool<'a>(
        &'a self,
        _invocation: CodeModeNestedToolCall,
        _cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async { Err(UNAVAILABLE_MESSAGE.to_string()) })
    }

    fn notify<'a>(
        &'a self,
        _call_id: String,
        _cell_id: CellId,
        _text: String,
        _cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell_id: &CellId) {}
}

#[derive(Default)]
pub struct InProcessCodeModeSessionProvider;

impl CodeModeSessionProvider for InProcessCodeModeSessionProvider {
    fn create_session<'a>(
        &'a self,
        _delegate: Arc<dyn CodeModeSessionDelegate>,
    ) -> CodeModeSessionProviderFuture<'a> {
        Box::pin(async { Ok(Arc::new(InProcessCodeModeSession) as Arc<dyn CodeModeSession>) })
    }
}

pub struct InProcessCodeModeSession;

impl InProcessCodeModeSession {
    pub fn new() -> Self {
        Self
    }

    pub fn with_delegate(_delegate: Arc<dyn CodeModeSessionDelegate>) -> Self {
        Self
    }

    pub fn with_delegate_and_task_failure_handler(
        _delegate: Arc<dyn CodeModeSessionDelegate>,
        _task_failure_handler: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        Self
    }

    pub async fn execute(&self, _request: ExecuteRequest) -> Result<StartedCell, String> {
        Err(UNAVAILABLE_MESSAGE.to_string())
    }

    pub async fn wait(&self, _request: WaitRequest) -> Result<WaitOutcome, String> {
        Err(UNAVAILABLE_MESSAGE.to_string())
    }

    pub async fn terminate(&self, _cell_id: CellId) -> Result<WaitOutcome, String> {
        Err(UNAVAILABLE_MESSAGE.to_string())
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Default for InProcessCodeModeSession {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeSession for InProcessCodeModeSession {
    fn execute<'a>(
        &'a self,
        request: ExecuteRequest,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(InProcessCodeModeSession::execute(self, request))
    }

    fn wait<'a>(&'a self, request: WaitRequest) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(InProcessCodeModeSession::wait(self, request))
    }

    fn terminate<'a>(&'a self, cell_id: CellId) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(InProcessCodeModeSession::terminate(self, cell_id))
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(InProcessCodeModeSession::shutdown(self))
    }
}

#[derive(Default)]
pub struct ProcessOwnedCodeModeSessionProvider;

impl ProcessOwnedCodeModeSessionProvider {
    pub fn with_host_program(_host_program: PathBuf) -> Self {
        Self
    }
}

impl CodeModeSessionProvider for ProcessOwnedCodeModeSessionProvider {
    fn create_session<'a>(
        &'a self,
        delegate: Arc<dyn CodeModeSessionDelegate>,
    ) -> CodeModeSessionProviderFuture<'a> {
        InProcessCodeModeSessionProvider.create_session(delegate)
    }
}

pub struct ProcessOwnedCodeModeSession(InProcessCodeModeSession);

impl ProcessOwnedCodeModeSession {
    pub fn new() -> Self {
        Self(InProcessCodeModeSession::new())
    }

    pub async fn execute(&self, request: ExecuteRequest) -> Result<StartedCell, String> {
        self.0.execute(request).await
    }

    pub async fn wait(&self, request: WaitRequest) -> Result<WaitOutcome, String> {
        self.0.wait(request).await
    }

    pub async fn terminate(&self, cell_id: CellId) -> Result<WaitOutcome, String> {
        self.0.terminate(cell_id).await
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.0.shutdown().await
    }
}

impl Default for ProcessOwnedCodeModeSession {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeSession for ProcessOwnedCodeModeSession {
    fn execute<'a>(
        &'a self,
        request: ExecuteRequest,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(ProcessOwnedCodeModeSession::execute(self, request))
    }

    fn wait<'a>(&'a self, request: WaitRequest) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(ProcessOwnedCodeModeSession::wait(self, request))
    }

    fn terminate<'a>(&'a self, cell_id: CellId) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(ProcessOwnedCodeModeSession::terminate(self, cell_id))
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(ProcessOwnedCodeModeSession::shutdown(self))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum V8JitMode {
    #[default]
    Enabled,
    Disabled,
}

pub fn initialize_v8(_jit_mode: V8JitMode) -> Result<(), String> {
    Err(UNAVAILABLE_MESSAGE.to_string())
}
