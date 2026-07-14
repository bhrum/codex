#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod cell_actor;
#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod remote_session;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod runtime;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod service;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod session_runtime;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod v8_init;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) type TaskFailureHandler = std::sync::Arc<dyn Fn(String) + Send + Sync>;

pub use codex_code_mode_protocol::*;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::InProcessCodeModeSession;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::InProcessCodeModeSessionProvider;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::NoopCodeModeSessionDelegate;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::ProcessOwnedCodeModeSession;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::ProcessOwnedCodeModeSessionProvider;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::V8JitMode;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::initialize_v8;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use remote_session::ProcessOwnedCodeModeSession;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use remote_session::ProcessOwnedCodeModeSessionProvider;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use service::InProcessCodeModeSession;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use service::InProcessCodeModeSessionProvider;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use service::NoopCodeModeSessionDelegate;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use v8_init::V8JitMode;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use v8_init::initialize_v8;
