use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostPlatform {
    Cli,
    Desktop,
    Mobile,
    Web,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginContext {
    pub plugin_id: String,
    pub instance_id: String,
    pub platform: HostPlatform,
    pub locale: String,
    pub theme: String,
    pub bridge_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Claims required on newly issued Mahayana account access tokens.
///
/// Refresh tokens are intentionally not represented here: they are opaque,
/// rotating credentials stored only in the encrypted host session and the
/// server-side session table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountAccessTokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub scope: Vec<String>,
    pub device_id: String,
    pub sid: String,
    pub jti: String,
    pub iat: usize,
    pub exp: usize,
    pub token_use: String,
}

/// Five-minute, plugin-audience credential returned by the Mini App bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAccessTokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub scope: Vec<String>,
    pub device_id: String,
    pub jti: String,
    pub iat: usize,
    pub exp: usize,
    pub token_use: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegatedTokenRequest {
    pub plugin_id: String,
    pub device_id: String,
    pub scopes: Vec<String>,
}
