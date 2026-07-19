//! Versioned bridge shared by native, iframe, and WebAssembly Mini App hosts.

use mahayana_platform_core::HostPermission;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;

pub const BRIDGE_VERSION: &str = "1.0";
pub const WIT_PACKAGE: &str = "mahayana:host@1.0.0";
pub const WIT_SOURCE: &str = include_str!("../wit/host.wit");

pub mod method {
    pub const CONTEXT_GET: &str = "context.get";
    pub const PROFILE_GET_BASIC: &str = "profile.getBasic";
    pub const AUTH_GET_INIT_DATA: &str = "auth.getInitData";
    pub const AUTH_REQUEST_TOKEN: &str = "auth.requestToken";
    pub const MCP_LIST_TOOLS: &str = "mcp.listTools";
    pub const MCP_CALL_TOOL: &str = "mcp.callTool";
    pub const COMMERCE_QUOTE: &str = "commerce.quote";
    pub const COMMERCE_PURCHASE: &str = "commerce.purchase";
    pub const COMMERCE_ENTITLEMENT: &str = "commerce.entitlement";
    pub const STORAGE_GET: &str = "storage.get";
    pub const STORAGE_SET: &str = "storage.set";
    pub const STORAGE_REMOVE: &str = "storage.remove";
    pub const UI_CLOSE: &str = "ui.close";
    pub const UI_BACK: &str = "ui.back";
    pub const UI_SET_TITLE: &str = "ui.setTitle";
    pub const UI_THEME: &str = "ui.theme";
    pub const UI_VIEWPORT: &str = "ui.viewport";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl BridgeRequest {
    pub fn new(id: impl Into<Value>, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BridgeErrorObject>,
}

impl BridgeResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn failure(id: Value, error: BridgeError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeErrorObject {
    pub code: i32,
    pub message: String,
}

/// Executes approved host operations. Implementations keep login sessions,
/// payment credentials, and storage namespaces outside plugin memory.
pub trait HostBridge: Send + Sync {
    fn invoke(&self, method: &str, params: Value) -> Result<Value, BridgeError>;
}

pub struct BridgeRouter<B> {
    host: B,
    permissions: HashSet<HostPermission>,
}

impl<B> BridgeRouter<B>
where
    B: HostBridge,
{
    pub fn new(host: B, permissions: impl IntoIterator<Item = HostPermission>) -> Self {
        Self {
            host,
            permissions: permissions.into_iter().collect(),
        }
    }

    pub fn handle(&self, request: BridgeRequest) -> BridgeResponse {
        let id = request.id.clone();
        let result = self.validate(&request).and_then(|()| {
            self.host
                .invoke(&request.method, request.params)
                .map_err(|error| error.with_method(&request.method))
        });
        match result {
            Ok(value) => BridgeResponse::success(id, value),
            Err(error) => BridgeResponse::failure(id, error),
        }
    }

    fn validate(&self, request: &BridgeRequest) -> Result<(), BridgeError> {
        if request.jsonrpc != "2.0" {
            return Err(BridgeError::InvalidRequest("jsonrpc must be 2.0".into()));
        }
        let permission = required_permission(&request.method)?;
        if permission.is_some_and(|permission| !self.permissions.contains(&permission)) {
            return Err(BridgeError::PermissionDenied(request.method.clone()));
        }
        Ok(())
    }
}

fn required_permission(method: &str) -> Result<Option<HostPermission>, BridgeError> {
    let permission = match method {
        method::CONTEXT_GET | method::AUTH_GET_INIT_DATA => None,
        method::PROFILE_GET_BASIC => Some(HostPermission::ProfileBasic),
        method::AUTH_REQUEST_TOKEN => Some(HostPermission::AuthDelegatedToken),
        method::MCP_LIST_TOOLS | method::MCP_CALL_TOOL => Some(HostPermission::McpCall),
        method::COMMERCE_QUOTE | method::COMMERCE_PURCHASE | method::COMMERCE_ENTITLEMENT => {
            Some(HostPermission::CommercePurchase)
        }
        method::STORAGE_GET | method::STORAGE_SET | method::STORAGE_REMOVE => {
            Some(HostPermission::StorageLocal)
        }
        method::UI_CLOSE
        | method::UI_BACK
        | method::UI_SET_TITLE
        | method::UI_THEME
        | method::UI_VIEWPORT => Some(HostPermission::UiControl),
        _ => return Err(BridgeError::MethodNotFound(method.to_string())),
    };
    Ok(permission)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    #[error("invalid Mini App bridge request: {0}")]
    InvalidRequest(String),
    #[error("Mini App bridge method was not found: {0}")]
    MethodNotFound(String),
    #[error("Mini App does not have permission to call {0}")]
    PermissionDenied(String),
    #[error("Mini App host operation failed: {0}")]
    Host(String),
}

impl BridgeError {
    fn with_method(self, method: &str) -> Self {
        match self {
            Self::Host(message) => Self::Host(format!("{method}: {message}")),
            other => other,
        }
    }
}

impl From<BridgeError> for BridgeErrorObject {
    fn from(value: BridgeError) -> Self {
        let code = match value {
            BridgeError::InvalidRequest(_) => -32600,
            BridgeError::MethodNotFound(_) => -32601,
            BridgeError::PermissionDenied(_) => -32001,
            BridgeError::Host(_) => -32000,
        };
        Self {
            code,
            message: value.to_string(),
        }
    }
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
