//! First-party Mahayana platform client.

use codex_login::token_data::parse_jwt_expiration;
use codex_secrets::LocalSecretsNamespace;
use codex_secrets::SecretName;
use codex_secrets::SecretScope;
use codex_secrets::SecretsBackendKind;
use codex_secrets::SecretsManager;
use mahayana_platform_core::AccountUsageStatus;
use mahayana_platform_core::Currency;
use mahayana_platform_core::DelegatedTokenRequest;
use mahayana_platform_core::Entitlement;
use mahayana_platform_core::PurchaseRequest;
use mahayana_platform_core::Quote;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_API_BASE_URL: &str = "https://api.ombhrum.com";
const MAHAYANA_ACCOUNT_SESSION_SECRET: &str = "MAHAYANA_ACCOUNT_SESSION";
const ACCESS_TOKEN_REFRESH_SKEW_SECONDS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletBalance {
    pub currency: Currency,
    pub available: i64,
    pub reserved: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletHistoryPage {
    pub entries: Vec<Value>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseReceipt {
    pub order_id: String,
    pub status: String,
    pub entitlement: Option<Entitlement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchasePage {
    pub purchases: Vec<Value>,
    pub next_cursor: Option<String>,
}

/// First-party product API client shared by the CLI and native application
/// shells. Authentication is stored once by Rust so every surface observes the
/// same Mahayana account session.
#[derive(Clone)]
pub struct MahayanaProductClient {
    api_base_url: String,
    /// Stable Mahayana home anchor used to locate Codex encrypted secrets.
    session_path: PathBuf,
    secrets_manager: SecretsManager,
}

impl std::fmt::Debug for MahayanaProductClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MahayanaProductClient")
            .field("api_base_url", &self.api_base_url)
            .field("legacy_session_path", &self.session_path)
            .finish_non_exhaustive()
    }
}

impl Default for MahayanaProductClient {
    fn default() -> Self {
        let api_base_url = env::var("MAHAYANA_API_BASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_API_BASE_URL.to_string());
        let home = default_mahayana_home();
        Self::new(api_base_url, home.join("session.json"))
    }
}

/// Shared Mahayana data directory used by the signed application and CLI.
/// Platform runtimes should derive their own subdirectories from this path so
/// account state and Codex conversations never split across host surfaces.
pub fn default_mahayana_home() -> PathBuf {
    if let Some(path) = env::var_os("MAHAYANA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return path;
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        // The signed App and its bundled command-line tool use the existing
        // Fabushi application group. This avoids a release-sandbox copy of
        // the account session diverging from the user's terminal CLI.
        return PathBuf::from(home)
            .join("Library")
            .join("Group Containers")
            .join("group.com.ombhrum.fabushi.share")
            .join("mahayana");
    }
    #[cfg(target_os = "windows")]
    if let Some(app_data) = env::var_os("APPDATA") {
        return PathBuf::from(app_data).join("Fabushi").join("Mahayana");
    }
    env::var_os("HOME")
        .map(|value| PathBuf::from(value).join(".mahayana"))
        .unwrap_or_else(|| PathBuf::from(".mahayana"))
}

impl MahayanaProductClient {
    pub fn new(api_base_url: impl Into<String>, session_path: impl Into<PathBuf>) -> Self {
        let session_path = session_path.into();
        let mahayana_home = session_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self {
            api_base_url: api_base_url.into().trim_end_matches('/').to_string(),
            session_path,
            secrets_manager: SecretsManager::new_with_namespace(
                mahayana_home,
                SecretsBackendKind::Local,
                LocalSecretsNamespace::MahayanaAuth,
            ),
        }
    }

    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    pub fn session_path(&self) -> &Path {
        &self.session_path
    }

    /// Stores the environment-provisioned smoke-test credential in the same
    /// encrypted Mahayana session backend used by normal account logins. The
    /// credential is never accepted as a CLI argument or returned to callers.
    pub fn store_test_account_session(&self, token: &str) -> Result<(), ProductError> {
        let token = safe_test_account_token(token)?;
        self.save_session(&json!({
            "accessToken": token,
            "provider": "test",
            "username": "TestAccount",
            "user": {
                "id": "user:test_account",
                "userId": "user:test_account",
                "username": "TestAccount",
                "membership": {"type": "lifetime", "active": true},
                "isTestAccount": true,
            },
        }))
    }

    pub fn marketplace_browse(
        &self,
        query: Option<&str>,
        platform: Option<&str>,
    ) -> Result<Value, ProductError> {
        let query = query.map(str::trim).filter(|query| !query.is_empty());
        let platform = platform.map(safe_marketplace_platform).transpose()?;
        let mut parameters = Vec::new();
        if let Some(query) = query {
            parameters.push(("q", query));
        }
        if let Some(platform) = platform {
            parameters.push(("platform", platform));
        }
        let token = self.optional_authorization_token(&Value::Null)?;
        self.get_json("/v1/marketplace/plugins", &parameters, token.as_deref())
    }

    pub fn download_marketplace_plugin(
        &self,
        plugin_id: &str,
        version: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ProductError> {
        let plugin_id = safe_path_identifier(plugin_id, "pluginId")?;
        let version = safe_path_identifier(version, "version")?;
        let token = self.optional_authorization_token(&Value::Null)?;
        let client = http_client()?;
        let mut request = client
            .get(format!(
                "{}/v1/marketplace/plugins/{plugin_id}/releases/{version}/download",
                self.api_base_url
            ))
            .header("Accept", "application/gzip, application/octet-stream");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|error| ProductError::Transport(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .unwrap_or_else(|_| "marketplace download failed".to_string());
            return Err(ProductError::HttpStatus { status, message });
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(ProductError::Response(
                "marketplace plugin package exceeds the local size limit".into(),
            ));
        }
        let bytes = response
            .bytes()
            .map_err(|error| ProductError::Transport(error.to_string()))?
            .to_vec();
        if bytes.len() > max_bytes {
            return Err(ProductError::Response(
                "marketplace plugin package exceeds the local size limit".into(),
            ));
        }
        Ok(bytes)
    }

    pub fn publish_plugin(
        &self,
        plugin_id: &str,
        version: &str,
        deployment_url: &str,
        package_sha256: &str,
        package_size: u64,
        platforms: &[String],
    ) -> Result<Value, ProductError> {
        let plugin_id = safe_path_identifier(plugin_id, "pluginId")?;
        let version = non_empty(version, "version")?;
        let deployment_url = https_deployment_url(deployment_url)?;
        let package_sha256 = safe_sha256(package_sha256)?;
        let platforms = safe_marketplace_platforms(platforms)?;
        let token = self.authorization_token(&Value::Null)?;
        let form = reqwest::blocking::multipart::Form::new()
            .text("pluginId", plugin_id.to_string())
            .text("version", version.to_string())
            .text("deploymentUrl", deployment_url)
            .text("packageSha256", package_sha256.to_string())
            .text("packageSize", package_size.to_string())
            .text(
                "platforms",
                serde_json::to_string(&platforms)
                    .map_err(|error| ProductError::Configuration(error.to_string()))?,
            );
        let client = http_client()?;
        decode_response(
            client
                .post(format!("{}/v1/marketplace/releases", self.api_base_url))
                .header("Accept", "application/json")
                .bearer_auth(token)
                .multipart(form)
                .send(),
        )
    }

    pub fn wallet_balance(&self) -> Result<WalletBalance, ProductError> {
        let token = self.authorization_token(&Value::Null)?;
        decode_value(self.get_json("/v1/wallet/balance", &[], Some(&token))?)
    }

    /// Returns the server-authoritative model-token budget for the signed-in
    /// Mahayana account. Client-observed usage events are intentionally not
    /// accepted here: only the trusted model gateway may reserve and capture
    /// billable usage.
    pub fn model_usage(&self) -> Result<AccountUsageStatus, ProductError> {
        let token = self.authorization_token(&Value::Null)?;
        decode_value(self.get_json("/v1/ai/usage", &[], Some(&token))?)
    }

    pub fn wallet_history(&self, cursor: Option<&str>) -> Result<WalletHistoryPage, ProductError> {
        let query = cursor
            .map(str::trim)
            .filter(|cursor| !cursor.is_empty())
            .map(|cursor| vec![("cursor", cursor)])
            .unwrap_or_default();
        let token = self.authorization_token(&Value::Null)?;
        decode_value(self.get_json("/v1/wallet/history", &query, Some(&token))?)
    }

    pub fn wallet_top_up(&self, sku: &str, idempotency_key: &str) -> Result<Value, ProductError> {
        let token = self.authorization_token(&Value::Null)?;
        self.post_json(
            "/v1/wallet/top-up",
            json!({
                "sku": non_empty(sku, "sku")?,
                "idempotencyKey": non_empty(idempotency_key, "idempotencyKey")?,
            }),
            Some(&token),
        )
    }

    pub fn purchases(&self, cursor: Option<&str>) -> Result<PurchasePage, ProductError> {
        let query = cursor
            .map(str::trim)
            .filter(|cursor| !cursor.is_empty())
            .map(|cursor| vec![("cursor", cursor)])
            .unwrap_or_default();
        let token = self.authorization_token(&Value::Null)?;
        decode_value(self.get_json("/v1/purchases", &query, Some(&token))?)
    }

    pub fn restore_purchases(&self) -> Result<PurchasePage, ProductError> {
        let token = self.authorization_token(&Value::Null)?;
        decode_value(self.post_json("/v1/purchases/restore", json!({}), Some(&token))?)
    }

    pub fn quote(&self, plugin_id: &str, sku: &str) -> Result<Quote, ProductError> {
        let plugin_id = safe_path_identifier(plugin_id, "pluginId")?;
        let token = self.authorization_token(&Value::Null)?;
        decode_value(self.post_json(
            &format!("/v1/plugins/{plugin_id}/commerce/quote"),
            json!({"sku": non_empty(sku, "sku")?}),
            Some(&token),
        )?)
    }

    pub fn purchase(
        &self,
        plugin_id: &str,
        request: &PurchaseRequest,
    ) -> Result<PurchaseReceipt, ProductError> {
        let plugin_id = safe_path_identifier(plugin_id, "pluginId")?;
        let token = self.authorization_token(&Value::Null)?;
        let body = serde_json::to_value(request)
            .map_err(|error| ProductError::Response(error.to_string()))?;
        decode_value(self.post_json(
            &format!("/v1/plugins/{plugin_id}/commerce/purchase"),
            body,
            Some(&token),
        )?)
    }

    pub fn entitlement(
        &self,
        plugin_id: &str,
        capability: &str,
    ) -> Result<Option<Entitlement>, ProductError> {
        let plugin_id = safe_path_identifier(plugin_id, "pluginId")?;
        let capability = safe_path_identifier(capability, "capability")?;
        let token = self.authorization_token(&Value::Null)?;
        let response = self.get_json(
            &format!("/v1/plugins/{plugin_id}/entitlements/{capability}"),
            &[],
            Some(&token),
        )?;
        response
            .get("entitlement")
            .cloned()
            .filter(|value| !value.is_null())
            .map(decode_value)
            .transpose()
    }

    pub fn delegated_plugin_token(
        &self,
        request: &DelegatedTokenRequest,
    ) -> Result<Value, ProductError> {
        let token = self.authorization_token(&Value::Null)?;
        let body = serde_json::to_value(request)
            .map_err(|error| ProductError::Response(error.to_string()))?;
        self.post_json("/v1/auth/plugin-token", body, Some(&token))
    }

    /// Returns the locally stored Fabushi/Alipay session token used by the
    /// first-party Responses provider. The value must stay in memory and must
    /// not be copied into Codex `auth.json` or logs.
    pub fn session_token(&self) -> Result<String, ProductError> {
        self.authorization_token(&Value::Null)
    }

    pub fn execute(&self, request_type: &str, request: &Value) -> Result<Value, ProductError> {
        match request_type {
            "mahayana.auth.status" => self.auth_status(request),
            "mahayana.auth.session.restore" => self.restore_session(),
            "mahayana.auth.password.login" => self.password_login(request),
            "mahayana.auth.register" => self.register(request),
            "mahayana.auth.verification.send" => self.verification_send(request),
            "mahayana.auth.password.forgot" => self.password_forgot(request),
            "mahayana.auth.password.reset" => self.password_reset(request),
            "mahayana.auth.alipay.start" => self.alipay_start(request),
            "mahayana.auth.alipay.complete" => self.alipay_complete(request),
            "mahayana.auth.alipay.poll" => self.alipay_poll(request),
            "mahayana.auth.alipay.sdk.start" => {
                self.get_json("/api/auth/alipay/auth-string", &[], None)
            }
            "mahayana.auth.alipay.sdk.complete" => self.alipay_sdk_complete(request),
            "mahayana.auth.alipay.register" => self.alipay_register(request),
            "mahayana.auth.apple.complete" => self.apple_complete(request),
            "mahayana.auth.firebase.phone.complete" => self.firebase_phone_complete(request),
            "mahayana.auth.logout" => self.logout(),
            "mahayana.platform.request" => self.platform_request(request),
            "mahayana.contacts.list" => self.authorized_get(request, "/api/social/friends", &[]),
            "mahayana.contacts.search" => {
                let query = required_string(request, "query")?;
                self.authorized_get(request, "/api/social/users/search", &[("q", query)])
            }
            "mahayana.contacts.add" => {
                let contact = required_string(request, "contact")?;
                let mut body = json!({"targetUserId": contact});
                if let Some(message) = optional_string(request, "message") {
                    body["message"] = Value::String(message.to_string());
                }
                self.authorized_post(request, "/api/social/friend-requests", body)
            }
            "mahayana.contacts.requests" => {
                self.authorized_get(request, "/api/social/friend-requests/incoming", &[])
            }
            "mahayana.contacts.accept" => {
                let request_id = required_identifier(request, "requestId")?;
                self.authorized_post(
                    request,
                    &format!("/api/social/friend-requests/{request_id}/accept"),
                    json!({}),
                )
            }
            "mahayana.messages.list" => {
                let contact = required_string(request, "contact")?;
                let limit = request
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(50)
                    .clamp(1, 200)
                    .to_string();
                self.authorized_get(
                    request,
                    "/api/social/messages",
                    &[("contactId", contact), ("limit", &limit)],
                )
            }
            "mahayana.messages.send" => {
                let contact = required_string(request, "contact")?;
                let text = required_string(request, "text")?;
                let mut body = json!({"contactId": contact, "text": text});
                if let Some(client_request_id) = optional_string(request, "clientRequestId") {
                    body["clientRequestId"] = Value::String(client_request_id.to_string());
                }
                self.authorized_post(request, "/api/social/messages", body)
            }
            "mahayana.miniapps.registry" => self.miniapp_registry(request),
            other => Err(ProductError::UnsupportedRequest(other.to_string())),
        }
    }

    fn auth_status(&self, request: &Value) -> Result<Value, ProductError> {
        let command_token = access_token(request);
        let session = self.load_session()?;
        let Some(token) = (match command_token {
            Some(token) => Some(token.to_string()),
            None => session
                .clone()
                .map(|session| self.active_session_token(session))
                .transpose()?,
        }) else {
            return Ok(json!({
                "@type": "mahayana.auth.status",
                "loggedIn": false,
                "provider": "alipay",
            }));
        };
        let provider = session
            .as_ref()
            .and_then(|value| value.get("provider"))
            .and_then(Value::as_str)
            .unwrap_or("official");
        match self.get_json("/api/auth/user-info", &[], Some(&token)) {
            Ok(user) => Ok(json!({
                "@type": "mahayana.auth.status",
                "loggedIn": true,
                "provider": provider,
                "user": user,
            })),
            Err(ProductError::HttpStatus { status: 401, .. }) => {
                if command_token.is_none() {
                    self.remove_session()?;
                }
                Ok(json!({
                    "@type": "mahayana.auth.status",
                    "loggedIn": false,
                    "provider": "alipay",
                    "expired": true,
                }))
            }
            Err(error) => Err(error),
        }
    }

    fn password_login(&self, request: &Value) -> Result<Value, ProductError> {
        let mut body = json!({
            "username": required_string(request, "username")?,
            "password": required_string(request, "password")?,
        });
        if let Some(device_id) = optional_string(request, "deviceId") {
            body["deviceId"] = Value::String(device_id.to_string());
        }
        let response = self.post_json("/api/auth/login", body, None)?;
        self.store_login_response(&response, "password")?;
        typed_session(response, "password", true)
    }

    fn register(&self, request: &Value) -> Result<Value, ProductError> {
        self.post_json(
            "/api/auth/register",
            json!({
                "username": required_string(request, "username")?,
                "email": required_string(request, "email")?,
                "password": required_string(request, "password")?,
                "verificationCode": required_string(request, "verificationCode")?,
            }),
            None,
        )
    }

    fn verification_send(&self, request: &Value) -> Result<Value, ProductError> {
        self.post_json(
            "/api/auth/send-verification-code",
            json!({
                "email": required_string(request, "email")?,
                "type": required_string(request, "type")?,
            }),
            None,
        )
    }

    fn password_forgot(&self, request: &Value) -> Result<Value, ProductError> {
        self.post_json(
            "/api/auth/forgot-password",
            json!({"email": required_string(request, "email")?}),
            None,
        )
    }

    fn password_reset(&self, request: &Value) -> Result<Value, ProductError> {
        self.post_json(
            "/api/auth/reset-password",
            json!({
                "email": required_string(request, "email")?,
                "token": required_string(request, "resetToken")?,
                "newPassword": required_string(request, "newPassword")?,
            }),
            None,
        )
    }

    fn alipay_start(&self, request: &Value) -> Result<Value, ProductError> {
        let platform = optional_string(request, "platform").unwrap_or("cli");
        let response = self.get_json(
            "/api/auth/alipay/login-url",
            &[("platform", platform)],
            None,
        )?;
        Ok(json!({
            "@type": "mahayana.auth.alipay.authorization",
            "provider": "alipay",
            "loginUrl": response.get("authUrl").or_else(|| response.get("loginUrl")),
            "state": response.get("state"),
            "appId": response.get("appId"),
            "platform": response.get("platform").cloned().unwrap_or_else(|| Value::String(platform.to_string())),
        }))
    }

    /// Restores UI-safe account state. Access and refresh credentials never
    /// cross the Rust ABI into Flutter or another host shell.
    fn restore_session(&self) -> Result<Value, ProductError> {
        let session = self.required_session()?;
        self.active_session_token(session)?;
        let session = self.required_session()?;
        let mut output = session.as_object().cloned().unwrap_or_default();
        strip_credentials(&mut output);
        output.insert(
            "@type".to_string(),
            Value::String("mahayana.auth.session".to_string()),
        );
        output.insert("loggedIn".to_string(), Value::Bool(true));
        output.insert("sessionStored".to_string(), Value::Bool(true));
        Ok(Value::Object(output))
    }

    fn alipay_complete(&self, request: &Value) -> Result<Value, ProductError> {
        let auth_code = required_string(request, "authCode")?;
        let mut body = json!({"auth_code": auth_code});
        if let Some(state) = optional_string(request, "state") {
            body["state"] = Value::String(state.to_string());
        }
        let response = self.post_json("/api/auth/alipay/login", body, None)?;
        self.store_login_response(&response, "alipay")?;
        typed_session(response, "alipay", false)
    }

    fn alipay_poll(&self, request: &Value) -> Result<Value, ProductError> {
        let state = required_string(request, "state")?;
        let response = self.get_json("/api/auth/alipay/cli-session", &[("state", state)], None)?;
        let response = normalize_alipay_cli_response(response);
        if response.get("status").and_then(Value::as_str) == Some("complete") {
            self.store_login_response(&response, "alipay")?;
        }
        Ok(response)
    }

    fn alipay_sdk_complete(&self, request: &Value) -> Result<Value, ProductError> {
        let auth_code = required_string(request, "authCode")?;
        let mut body = json!({"auth_code": auth_code});
        if let Some(target_id) = optional_string(request, "targetId") {
            body["target_id"] = Value::String(target_id.to_string());
        }
        let response = self.post_json("/api/auth/alipay/sdk-login", body, None)?;
        self.store_login_response(&response, "alipay")?;
        typed_session(response, "alipay", false)
    }

    fn alipay_register(&self, request: &Value) -> Result<Value, ProductError> {
        let mut body = json!({
            "alipayProviderSubject": required_string(request, "alipayProviderSubject")?,
        });
        copy_optional_fields(
            request,
            &mut body,
            &[
                "alipaySubjectType",
                "username",
                "password",
                "nickname",
                "avatar",
                "email",
                "alipayNickname",
                "alipayAvatar",
            ],
        );
        if request.get("oneClick").and_then(Value::as_bool) == Some(true) {
            body["oneClick"] = Value::Bool(true);
        }
        let response = self.post_json("/api/auth/alipay/register", body, None)?;
        self.store_login_response(&response, "alipay")?;
        typed_session(response, "alipay", false)
    }

    fn apple_complete(&self, request: &Value) -> Result<Value, ProductError> {
        let mut body = json!({
            "identityToken": required_string(request, "identityToken")?,
            "authorizationCode": required_string(request, "authorizationCode")?,
        });
        copy_optional_fields(request, &mut body, &["email", "givenName", "familyName"]);
        let response = self.post_json("/api/auth/apple-login", body, None)?;
        self.store_login_response(&response, "apple")?;
        typed_session(response, "apple", false)
    }

    fn firebase_phone_complete(&self, request: &Value) -> Result<Value, ProductError> {
        let response = self.post_json(
            "/api/auth/firebase-phone-login",
            json!({
                "idToken": required_string(request, "idToken")?,
                "phoneNumber": required_string(request, "phoneNumber")?,
                "firebaseUid": required_string(request, "firebaseUid")?,
                "isNewUser": request.get("isNewUser").and_then(Value::as_bool).unwrap_or(false),
            }),
            None,
        )?;
        self.store_login_response(&response, "firebase-phone")?;
        typed_session(response, "firebase-phone", false)
    }

    fn store_login_response(&self, response: &Value, provider: &str) -> Result<(), ProductError> {
        if let Some(token) = access_token(response) {
            let session = json!({
                "token": token,
                "accessToken": token,
                "refreshToken": response.get("refreshToken"),
                "accessTokenExpiresAt": response.get("accessTokenExpiresAt"),
                "refreshTokenExpiresAt": response.get("refreshTokenExpiresAt"),
                "sessionId": response.get("sessionId"),
                "deviceId": response.get("deviceId"),
                "tokenType": response.get("tokenType"),
                "provider": provider,
                "user": response.get("user"),
                "username": response.get("username"),
                "email": response.get("email"),
            });
            self.save_session(&session)?;
        }
        Ok(())
    }

    fn logout(&self) -> Result<Value, ProductError> {
        let server_session_revoked = match self.load_session()? {
            Some(session) => self
                .active_session_token(session)
                .and_then(|token| self.post_json("/api/auth/logout", json!({}), Some(&token)))
                .is_ok(),
            None => true,
        };
        self.remove_session()?;
        Ok(json!({
            "@type": "mahayana.auth.loggedOut",
            "loggedIn": false,
            "provider": "official",
            "serverSessionRevoked": server_session_revoked,
        }))
    }

    fn miniapp_registry(&self, request: &Value) -> Result<Value, ProductError> {
        let token = self.optional_authorization_token(request)?;
        self.get_json("/api/plugins/registry", &[], token.as_deref())
    }

    /// Executes one same-origin platform request with the active Rust-owned
    /// session. Flutter supplies business data but never receives bearer or
    /// refresh credentials.
    fn platform_request(&self, request: &Value) -> Result<Value, ProductError> {
        let method = required_string(request, "method")?.to_ascii_uppercase();
        if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
            return Err(ProductError::InvalidParameter("method"));
        }
        let path = safe_platform_path(required_string(request, "path")?)?;
        let mut url = url::Url::parse(&format!("{}{}", self.api_base_url, path))
            .map_err(|error| ProductError::Configuration(error.to_string()))?;
        if let Some(query) = request.get("query").and_then(Value::as_object) {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in query {
                let value = value
                    .as_str()
                    .ok_or(ProductError::InvalidParameter("query"))?;
                pairs.append_pair(name, value);
            }
        }
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| ProductError::InvalidParameter("method"))?;
        let client = http_client()?;
        let mut builder = client
            .request(method, url)
            .header("Accept", "application/json");
        if request
            .get("authenticated")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            builder = builder.bearer_auth(self.authorization_token(&Value::Null)?);
        }
        if let Some(body) = request.get("body").filter(|body| !body.is_null()) {
            builder = builder.json(body);
        }
        let response = builder
            .send()
            .map_err(|error| ProductError::Transport(error.to_string()))?;
        let status_code = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw_body_text = response
            .text()
            .map_err(|error| ProductError::Transport(error.to_string()))?;
        let decoded = serde_json::from_str::<Value>(&raw_body_text)
            .unwrap_or_else(|_| Value::String(raw_body_text.clone()));
        let data = redact_secrets(&decoded);
        let body_text = if decoded.is_object() || decoded.is_array() {
            serde_json::to_string(&data)
                .map_err(|error| ProductError::Response(error.to_string()))?
        } else {
            raw_body_text
        };
        Ok(json!({
            "@type": "mahayana.platform.response",
            "ok": (200..300).contains(&status_code),
            "statusCode": status_code,
            "contentType": content_type,
            "bodyText": body_text,
            "data": data,
        }))
    }

    /// Publishes a locally generated, scan-ready mini-app to the user's
    /// personal sandbox. The backend intentionally accepts anonymous users,
    /// so login enriches ownership but is never a prerequisite for creation.
    fn authorized_get(
        &self,
        command: &Value,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Value, ProductError> {
        let token = self.authorization_token(command)?;
        self.get_json(path, query, Some(&token))
    }

    fn authorized_post(
        &self,
        command: &Value,
        path: &str,
        body: Value,
    ) -> Result<Value, ProductError> {
        let token = self.authorization_token(command)?;
        self.post_json(path, body, Some(&token))
    }

    fn authorization_token(&self, command: &Value) -> Result<String, ProductError> {
        if let Some(token) = access_token(command) {
            return Ok(token.to_string());
        }
        let session = self.required_session()?;
        self.active_session_token(session)
    }

    fn optional_authorization_token(
        &self,
        command: &Value,
    ) -> Result<Option<String>, ProductError> {
        if let Some(token) = access_token(command) {
            return Ok(Some(token.to_string()));
        }
        self.load_session()?
            .map(|session| self.active_session_token(session))
            .transpose()
    }

    fn get_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
        token: Option<&str>,
    ) -> Result<Value, ProductError> {
        let mut url = url::Url::parse(&format!("{}{}", self.api_base_url, path))
            .map_err(|error| ProductError::Configuration(error.to_string()))?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let client = http_client()?;
        let mut request = client.get(url).header("Accept", "application/json");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        decode_response(request.send())
    }

    fn post_json(
        &self,
        path: &str,
        body: Value,
        token: Option<&str>,
    ) -> Result<Value, ProductError> {
        let url = format!("{}{}", self.api_base_url, path);
        let client = http_client()?;
        let mut request = client
            .post(&url)
            .header("Accept", "application/json")
            .json(&body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        decode_response(request.send())
    }

    fn required_session(&self) -> Result<Value, ProductError> {
        self.load_session()?.ok_or(ProductError::NotLoggedIn)
    }

    fn active_session_token(&self, session: Value) -> Result<String, ProductError> {
        let token = access_token(&session)
            .map(str::to_string)
            .ok_or(ProductError::NotLoggedIn)?;
        if !session_needs_refresh(&session, &token) {
            return Ok(token);
        }

        let refresh_token =
            optional_string(&session, "refreshToken").ok_or(ProductError::SessionExpired)?;
        let mut body = json!({"refreshToken": refresh_token});
        if let Some(device_id) = optional_string(&session, "deviceId") {
            body["deviceId"] = Value::String(device_id.to_string());
        }
        let response = self.post_json("/api/auth/refresh", body, None)?;
        let refreshed_token = access_token(&response).map(str::to_string).ok_or_else(|| {
            ProductError::Response("refresh response did not include an access token".to_string())
        })?;
        let updated = merge_refreshed_session(session, response, &refreshed_token);
        self.save_session(&updated)?;
        Ok(refreshed_token)
    }

    fn load_session(&self) -> Result<Option<Value>, ProductError> {
        let name = account_session_secret_name()?;
        let stored = self
            .secrets_manager
            .get(&SecretScope::Global, &name)
            .map_err(secrets_error)?;
        if let Some(raw) = stored {
            return serde_json::from_str(&raw)
                .map(Some)
                .map_err(|error| ProductError::Session(error.to_string()));
        }
        Ok(None)
    }

    fn save_session(&self, session: &Value) -> Result<(), ProductError> {
        let name = account_session_secret_name()?;
        let contents = serde_json::to_string(session)
            .map_err(|error| ProductError::Session(error.to_string()))?;
        self.secrets_manager
            .set(&SecretScope::Global, &name, &contents)
            .map_err(secrets_error)
    }

    fn remove_session(&self) -> Result<(), ProductError> {
        let name = account_session_secret_name()?;
        self.secrets_manager
            .delete(&SecretScope::Global, &name)
            .map(|_| ())
            .map_err(secrets_error)
    }
}

fn https_deployment_url(value: &str) -> Result<String, ProductError> {
    let mut url = url::Url::parse(value.trim())
        .map_err(|_| ProductError::InvalidParameter("deploymentUrl"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProductError::InvalidParameter("deploymentUrl"));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "::1" {
        return Err(ProductError::InvalidParameter("deploymentUrl"));
    }
    let normalized = url.path().trim_end_matches('/').to_string();
    url.set_path(&normalized);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn safe_sha256(value: &str) -> Result<&str, ProductError> {
    let value = value.trim();
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err(ProductError::InvalidParameter("packageSha256"))
    }
}

fn safe_marketplace_platform(value: &str) -> Result<&str, ProductError> {
    match value.trim() {
        "cli" => Ok("cli"),
        "desktop" => Ok("desktop"),
        "mobile" => Ok("mobile"),
        "web" => Ok("web"),
        _ => Err(ProductError::InvalidParameter("platform")),
    }
}

fn safe_marketplace_platforms(platforms: &[String]) -> Result<Vec<&str>, ProductError> {
    let mut normalized = Vec::new();
    for platform in platforms {
        let platform = safe_marketplace_platform(platform)?;
        if !normalized.contains(&platform) {
            normalized.push(platform);
        }
    }
    if normalized.is_empty() {
        return Err(ProductError::InvalidParameter("platforms"));
    }
    Ok(normalized)
}

fn safe_test_account_token(value: &str) -> Result<&str, ProductError> {
    let value = value.trim();
    if (32..=512).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic()) {
        Ok(value)
    } else {
        Err(ProductError::InvalidParameter("testAccountToken"))
    }
}

fn account_session_secret_name() -> Result<SecretName, ProductError> {
    SecretName::new(MAHAYANA_ACCOUNT_SESSION_SECRET).map_err(secrets_error)
}

fn merge_refreshed_session(session: Value, response: Value, access_token: &str) -> Value {
    let mut updated = session.as_object().cloned().unwrap_or_default();
    if let Some(response) = response.as_object() {
        for (key, value) in response {
            if !value.is_null() {
                updated.insert(key.clone(), value.clone());
            }
        }
    }
    updated.insert(
        "accessToken".to_string(),
        Value::String(access_token.to_string()),
    );
    if !updated.contains_key("accessTokenExpiresAt")
        && let Some(expires_at) = jwt_expiration_seconds(access_token)
    {
        updated.insert("accessTokenExpiresAt".to_string(), expires_at.into());
    }
    Value::Object(updated)
}

fn session_needs_refresh(session: &Value, token: &str) -> bool {
    let expires_at = explicit_expiration_seconds(session).or_else(|| jwt_expiration_seconds(token));
    expires_at.is_some_and(|expires_at| {
        expires_at <= now_seconds().saturating_add(ACCESS_TOKEN_REFRESH_SKEW_SECONDS)
    })
}

fn explicit_expiration_seconds(session: &Value) -> Option<i64> {
    let value = session.get("accessTokenExpiresAt")?;
    let raw = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))?;
    // Accept the millisecond timestamp emitted by older application shells.
    Some(if raw > 10_000_000_000 {
        raw / 1_000
    } else {
        raw
    })
}

fn jwt_expiration_seconds(token: &str) -> Option<i64> {
    parse_jwt_expiration(token)
        .ok()
        .flatten()
        .map(|expires_at| expires_at.timestamp())
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn secrets_error(error: anyhow::Error) -> ProductError {
    ProductError::Session(error.to_string())
}

fn typed_session(
    response: Value,
    provider: &str,
    require_token: bool,
) -> Result<Value, ProductError> {
    let mut output = response.as_object().cloned().unwrap_or_default();
    let session_stored = access_token(&Value::Object(output.clone())).is_some();
    if require_token && !session_stored {
        return Err(ProductError::Response(
            "login response did not include a session token".to_string(),
        ));
    }
    output.insert(
        "@type".to_string(),
        Value::String("mahayana.auth.session".to_string()),
    );
    output.insert("provider".to_string(), Value::String(provider.to_string()));
    output.insert("sessionStored".to_string(), Value::Bool(session_stored));
    strip_credentials(&mut output);
    Ok(Value::Object(output))
}

fn strip_credentials(output: &mut Map<String, Value>) {
    for key in [
        "token",
        "accessToken",
        "refreshToken",
        "accessTokenExpiresAt",
        "refreshTokenExpiresAt",
        "tokenType",
    ] {
        output.remove(key);
    }
}

fn copy_optional_fields(request: &Value, body: &mut Value, fields: &[&str]) {
    for field in fields {
        if let Some(value) = request
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            body[*field] = Value::String(value.to_string());
        }
    }
}

fn http_client() -> Result<reqwest::blocking::Client, ProductError> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| ProductError::Configuration(error.to_string()))
}

fn decode_response(
    response: Result<reqwest::blocking::Response, reqwest::Error>,
) -> Result<Value, ProductError> {
    let response = response.map_err(|error| ProductError::Transport(error.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return response
            .json::<Value>()
            .map_err(|error| ProductError::Response(error.to_string()));
    }
    let body = response
        .text()
        .unwrap_or_else(|_| "request failed".to_string());
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or(body);
    Err(ProductError::HttpStatus {
        status: status.as_u16(),
        message,
    })
}

fn required_string<'a>(request: &'a Value, name: &'static str) -> Result<&'a str, ProductError> {
    request
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ProductError::InvalidParameter(name))
}

fn non_empty<'a>(value: &'a str, name: &'static str) -> Result<&'a str, ProductError> {
    let value = value.trim();
    (!value.is_empty())
        .then_some(value)
        .ok_or(ProductError::InvalidParameter(name))
}

fn safe_path_identifier<'a>(value: &'a str, name: &'static str) -> Result<&'a str, ProductError> {
    let value = non_empty(value, name)?;
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        .then_some(value)
        .ok_or(ProductError::InvalidParameter(name))
}

fn safe_platform_path(value: &str) -> Result<&str, ProductError> {
    let value = non_empty(value, "path")?;
    let allowed_prefix = value.starts_with("/api/") || value.starts_with("/v1/");
    let safe = allowed_prefix
        && !value.contains(['\r', '\n', '\\'])
        && !value.split('/').any(|segment| segment == "..")
        && !value.starts_with("//")
        && !value.contains('?')
        && !value.contains('#');
    safe.then_some(value)
        .ok_or(ProductError::InvalidParameter("path"))
}

fn decode_value<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ProductError> {
    serde_json::from_value(value).map_err(|error| ProductError::Response(error.to_string()))
}

fn optional_string<'a>(request: &'a Value, name: &str) -> Option<&'a str> {
    request
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn access_token(value: &Value) -> Option<&str> {
    optional_string(value, "accessToken")
}

fn normalize_alipay_cli_response(mut response: Value) -> Value {
    // Older account workers returned the one-time CLI credential as `token`,
    // while shared sessions deliberately accept only `accessToken`. Normalize
    // only this trusted completed response instead of weakening the general
    // authorization parser.
    if response.get("status").and_then(Value::as_str) == Some("complete")
        && response.get("accessToken").is_none()
        && let Some(token) = optional_string(&response, "token").map(str::to_string)
    {
        response["accessToken"] = Value::String(token);
        if let Some(object) = response.as_object_mut() {
            object.remove("token");
        }
    }
    response
}

fn required_identifier(request: &Value, name: &'static str) -> Result<String, ProductError> {
    match request.get(name) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Some(Value::Number(value)) => Ok(value.to_string()),
        _ => Err(ProductError::InvalidParameter(name)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductError {
    InvalidParameter(&'static str),
    UnsupportedRequest(String),
    Configuration(String),
    NotLoggedIn,
    SessionExpired,
    Session(String),
    Transport(String),
    HttpStatus { status: u16, message: String },
    Response(String),
}

impl std::fmt::Display for ProductError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidParameter(name) => {
                write!(formatter, "request parameter {name} is invalid")
            }
            Self::UnsupportedRequest(name) => {
                write!(formatter, "unsupported product request: {name}")
            }
            Self::Configuration(error) => {
                write!(formatter, "product API configuration failed: {error}")
            }
            Self::NotLoggedIn => write!(
                formatter,
                "请先登录大乘软件账号：mahayana login（支持支付宝或官方账号）"
            ),
            Self::SessionExpired => write!(
                formatter,
                "大乘登录已过期且没有可轮换的 refresh token，请重新运行 mahayana login"
            ),
            Self::Session(error) => write!(formatter, "Mahayana account session failed: {error}"),
            Self::Transport(error) => {
                write!(formatter, "Mahayana product API transport failed: {error}")
            }
            Self::HttpStatus { status, message } => write!(
                formatter,
                "Mahayana product API returned HTTP {status}: {message}"
            ),
            Self::Response(error) => {
                write!(formatter, "Mahayana product API response failed: {error}")
            }
        }
    }
}

impl std::error::Error for ProductError {}

pub fn redact_secrets(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut redacted = Map::new();
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "token" | "apiKey" | "accessToken" | "refreshToken" | "productSessionToken"
                ) {
                    redacted.insert(
                        key.clone(),
                        Value::String("[stored by Mahayana]".to_string()),
                    );
                } else {
                    redacted.insert(key.clone(), redact_secrets(value));
                }
            }
            Value::Object(redacted)
        }
        Value::Array(values) => Value::Array(values.iter().map(redact_secrets).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_removes_nested_account_tokens() {
        let value = json!({
            "token": "secret",
            "nested": {"accessToken": "also-secret", "name": "kept"},
        });
        let output = redact_secrets(&value);
        assert_eq!(output["token"], "[stored by Mahayana]");
        assert_eq!(output["nested"]["accessToken"], "[stored by Mahayana]");
        assert_eq!(output["nested"]["name"], "kept");
    }

    #[test]
    fn session_refresh_uses_explicit_expiry_and_preserves_rotating_credentials() {
        let future = now_seconds() + 3_600;
        let current = json!({
            "accessToken": "old-access",
            "refreshToken": "old-refresh",
            "accessTokenExpiresAt": future,
            "provider": "password",
            "user": {"id": "user-1"}
        });
        assert!(!session_needs_refresh(&current, "old-access"));

        let expired = json!({
            "accessToken": "old-access",
            "refreshToken": "old-refresh",
            "accessTokenExpiresAt": (now_seconds() - 1) * 1_000,
        });
        assert!(session_needs_refresh(&expired, "old-access"));

        let refreshed = merge_refreshed_session(
            current,
            json!({
                "accessToken": "new-access",
                "refreshToken": "new-refresh",
                "accessTokenExpiresAt": future + 3_600,
            }),
            "new-access",
        );
        assert!(refreshed.get("token").is_none());
        assert_eq!(refreshed["accessToken"], "new-access");
        assert_eq!(refreshed["refreshToken"], "new-refresh");
        assert_eq!(refreshed["provider"], "password");
        assert_eq!(refreshed["user"]["id"], "user-1");
    }

    #[test]
    fn path_identifiers_allow_product_ids_but_reject_traversal() {
        assert_eq!(
            safe_path_identifier("sandbox.test-1", "miniAppId").as_deref(),
            Ok("sandbox.test-1")
        );
        assert_eq!(
            safe_path_identifier("../admin", "miniAppId"),
            Err(ProductError::InvalidParameter("miniAppId"))
        );
        assert_eq!(
            safe_path_identifier("app/submit", "miniAppId"),
            Err(ProductError::InvalidParameter("miniAppId"))
        );
        assert_eq!(
            safe_platform_path("/api/social/friends"),
            Ok("/api/social/friends")
        );
        assert_eq!(
            safe_platform_path("https://evil.example/api/social/friends"),
            Err(ProductError::InvalidParameter("path"))
        );
        assert_eq!(
            safe_platform_path("/api/../admin"),
            Err(ProductError::InvalidParameter("path"))
        );
    }

    #[test]
    fn marketplace_deployment_metadata_requires_public_https_and_sha256() {
        assert_eq!(
            https_deployment_url("https://plugin.example/"),
            Ok("https://plugin.example".to_string())
        );
        assert_eq!(
            https_deployment_url("http://localhost:8787"),
            Err(ProductError::InvalidParameter("deploymentUrl"))
        );
        assert!(safe_sha256(&"a".repeat(64)).is_ok());
        assert_eq!(
            safe_sha256("not-a-digest"),
            Err(ProductError::InvalidParameter("packageSha256"))
        );
        assert_eq!(safe_marketplace_platform("desktop"), Ok("desktop"));
        assert_eq!(
            safe_marketplace_platform("android"),
            Err(ProductError::InvalidParameter("platform"))
        );
        assert_eq!(
            safe_marketplace_platforms(&["desktop".into(), "desktop".into(), "cli".into()]),
            Ok(vec!["desktop", "cli"])
        );
    }

    #[test]
    fn test_account_tokens_must_be_secret_strength_and_never_contain_whitespace() {
        assert!(safe_test_account_token(&"a".repeat(64)).is_ok());
        assert_eq!(
            safe_test_account_token("short"),
            Err(ProductError::InvalidParameter("testAccountToken"))
        );
        assert_eq!(
            safe_test_account_token(&format!("{} token", "a".repeat(32))),
            Err(ProductError::InvalidParameter("testAccountToken"))
        );
    }

    #[test]
    fn ui_sessions_never_expose_account_credentials_or_legacy_token_aliases() {
        assert!(access_token(&json!({"token": "legacy"})).is_none());
        let session = typed_session(
            json!({
                "accessToken": "access",
                "refreshToken": "refresh",
                "accessTokenExpiresAt": 123,
                "user": {"username": "tester"},
            }),
            "password",
            true,
        )
        .unwrap();
        assert_eq!(session["sessionStored"], true);
        assert_eq!(session["user"]["username"], "tester");
        assert!(session.get("token").is_none());
        assert!(session.get("accessToken").is_none());
        assert!(session.get("refreshToken").is_none());
    }

    #[test]
    fn completed_alipay_cli_tokens_are_normalized_without_weakening_auth_parsing() {
        let normalized = normalize_alipay_cli_response(json!({
            "status": "complete",
            "token": "one-time-access",
        }));
        assert_eq!(access_token(&normalized), Some("one-time-access"));
        assert!(normalized.get("token").is_none());

        let pending = normalize_alipay_cli_response(json!({
            "status": "pending",
            "token": "not-yet-valid",
        }));
        assert!(access_token(&pending).is_none());
        assert_eq!(pending["token"], "not-yet-valid");
    }
}
