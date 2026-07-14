//! Platform-specific tool execution boundary.

use async_trait::async_trait;
use mahayana_core::BuildProfile;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRequest {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Value,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilities {
    pub build_profile: BuildProfile,
    pub native_filesystem: bool,
    pub app_sandbox_filesystem: bool,
    pub native_process: bool,
    pub native_git: bool,
    pub http: bool,
    pub telegram: bool,
    pub miniapp: bool,
    pub secure_storage: bool,
}

impl ToolCapabilities {
    pub fn for_profile(profile: BuildProfile) -> Self {
        match profile {
            BuildProfile::DesktopFull => Self {
                build_profile: profile,
                native_filesystem: true,
                app_sandbox_filesystem: true,
                native_process: true,
                native_git: true,
                http: true,
                telegram: true,
                miniapp: true,
                secure_storage: true,
            },
            BuildProfile::MobileEmbedded => Self {
                build_profile: profile,
                native_filesystem: false,
                app_sandbox_filesystem: true,
                native_process: false,
                native_git: false,
                http: true,
                telegram: true,
                miniapp: true,
                secure_storage: true,
            },
            BuildProfile::WebWasm => Self {
                build_profile: profile,
                native_filesystem: false,
                app_sandbox_filesystem: true,
                native_process: false,
                native_git: false,
                http: true,
                telegram: true,
                miniapp: true,
                secure_storage: true,
            },
        }
    }
}

/// Executes Agent tools inside platform policy. Implementations must reject
/// capabilities absent from [`ToolCapabilities`] instead of proxying them to a
/// cloud Agent.
#[async_trait]
pub trait ToolHost: Send + Sync {
    async fn execute(&self, request: ToolRequest) -> Result<ToolResult, ToolError>;

    fn capabilities(&self) -> ToolCapabilities;
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool is not available on this platform: {0}")]
    Unsupported(String),
    #[error("tool request requires user approval: {0}")]
    ApprovalRequired(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_and_web_never_advertise_native_shell() {
        for profile in [BuildProfile::MobileEmbedded, BuildProfile::WebWasm] {
            let capabilities = ToolCapabilities::for_profile(profile);
            assert!(!capabilities.native_process);
            assert!(!capabilities.native_git);
            assert!(capabilities.app_sandbox_filesystem);
        }
    }
}
