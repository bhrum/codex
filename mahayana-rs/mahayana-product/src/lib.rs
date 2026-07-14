//! First-party Fabushi account, contacts, and messaging client.

use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_API_BASE_URL: &str = "https://api.ombhrum.com";

/// First-party product API client shared by the CLI and native application
/// shells. Authentication is stored once by Rust so every surface observes the
/// same Mahayana account session.
#[derive(Debug, Clone)]
pub struct MahayanaProductClient {
    api_base_url: String,
    session_path: PathBuf,
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
        Self {
            api_base_url: api_base_url.into().trim_end_matches('/').to_string(),
            session_path: session_path.into(),
        }
    }

    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    pub fn session_path(&self) -> &Path {
        &self.session_path
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
            "mahayana.auth.session.sync" => self.sync_session(request),
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
            "mahayana.miniapp.sandbox.publish" => self.miniapp_sandbox_publish(request),
            "mahayana.miniapp.review.submit" => self.miniapp_submit_review(request),
            other => Err(ProductError::UnsupportedRequest(other.to_string())),
        }
    }

    fn auth_status(&self, request: &Value) -> Result<Value, ProductError> {
        let command_token = optional_string(request, "token");
        let session = self.load_session()?;
        let stored_token = session
            .as_ref()
            .and_then(|value| value.get("token"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let Some(token) = command_token.or(stored_token) else {
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
        match self.get_json("/api/auth/user-info", &[], Some(token)) {
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
        let response = self.post_json(
            "/api/auth/login",
            json!({
                "username": required_string(request, "username")?,
                "password": required_string(request, "password")?,
            }),
            None,
        )?;
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

    /// Persists an already-authenticated app session so the native Runtime and
    /// the `mahayana` CLI observe the same software account. Platform shells
    /// call this after their normal login flow; the token is never echoed.
    fn sync_session(&self, request: &Value) -> Result<Value, ProductError> {
        let token = required_string(request, "token")?;
        let user = request.get("user").cloned().unwrap_or(Value::Null);
        let username = optional_string(request, "username")
            .map(str::to_string)
            .or_else(|| {
                user.get("username")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let email = optional_string(request, "email")
            .map(str::to_string)
            .or_else(|| {
                user.get("email")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        self.save_session(&json!({
            "token": token,
            "provider": optional_string(request, "provider").unwrap_or("app"),
            "user": user,
            "username": username,
            "email": email,
        }))?;
        Ok(json!({
            "@type": "mahayana.auth.session",
            "loggedIn": true,
            "sessionStored": true,
            "provider": optional_string(request, "provider").unwrap_or("app"),
        }))
    }

    /// Restores the same private local session for an embedded App shell after
    /// a CLI login. This is an in-process bridge, not a network or log output;
    /// callers must keep the returned token in platform secure storage.
    fn restore_session(&self) -> Result<Value, ProductError> {
        let session = self.required_session()?;
        let mut output = session.as_object().cloned().unwrap_or_default();
        required_string(&session, "token")?;
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
        Ok(response)
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
        if let Some(token) = response.get("token").and_then(Value::as_str) {
            let session = json!({
                "token": token,
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
        self.remove_session()?;
        Ok(json!({
            "@type": "mahayana.auth.loggedOut",
            "loggedIn": false,
            "provider": "alipay",
        }))
    }

    fn miniapp_registry(&self, request: &Value) -> Result<Value, ProductError> {
        let token = self.optional_authorization_token(request)?;
        self.get_json("/api/miniapps/registry", &[], token.as_deref())
    }

    /// Publishes a locally generated, scan-ready mini-app to the user's
    /// personal sandbox. The backend intentionally accepts anonymous users,
    /// so login enriches ownership but is never a prerequisite for creation.
    fn miniapp_sandbox_publish(&self, request: &Value) -> Result<Value, ProductError> {
        let token = self.optional_authorization_token(request)?;
        let title = required_string(request, "title")?;
        let source_html = required_string(request, "sourceHtml")?;
        let subtitle =
            optional_string(request, "subtitle").unwrap_or("大乘 CLI 生成的个人沙箱小程序");
        let prompt = optional_string(request, "prompt").unwrap_or(title);
        let permissions = request
            .get("permissions")
            .cloned()
            .filter(Value::is_array)
            .unwrap_or_else(|| json!(["app.context", "bot.chat"]));

        let created = self.post_json(
            "/api/miniapps/dev/create",
            json!({
                "title": title,
                "subtitle": subtitle,
                "prompt": prompt,
                "permissions": permissions,
            }),
            token.as_deref(),
        )?;
        let miniapp_id = created
            .get("miniApp")
            .and_then(|value| value.get("miniAppId"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProductError::Response("sandbox create response has no miniAppId".to_string())
            })?;
        let miniapp_id = safe_path_identifier(miniapp_id, "miniAppId")?;
        let version = optional_string(request, "version").unwrap_or("0.0.1");
        let updated = self.post_json(
            &format!("/api/miniapps/dev/{miniapp_id}/version"),
            json!({
                "title": title,
                "subtitle": subtitle,
                "prompt": prompt,
                "sourceHtml": source_html,
                "permissions": permissions,
                "version": version,
            }),
            token.as_deref(),
        )?;

        let review = if request.get("submitReview").and_then(Value::as_bool) == Some(true) {
            Some(self.post_json(
                &format!("/api/miniapps/{miniapp_id}/submit-review"),
                json!({}),
                token.as_deref(),
            )?)
        } else {
            None
        };
        Ok(json!({
            "@type": "mahayana.miniapp.published",
            "success": true,
            "authenticated": token.is_some(),
            "miniAppId": miniapp_id,
            "miniApp": updated.get("miniApp"),
            "bot": updated.get("bot"),
            "scan": updated.get("scan"),
            "review": review,
        }))
    }

    fn miniapp_submit_review(&self, request: &Value) -> Result<Value, ProductError> {
        let token = self.optional_authorization_token(request)?;
        let miniapp_id = safe_path_identifier(required_string(request, "miniAppId")?, "miniAppId")?;
        self.post_json(
            &format!("/api/miniapps/{miniapp_id}/submit-review"),
            json!({}),
            token.as_deref(),
        )
    }

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
        if let Some(token) = optional_string(command, "token") {
            return Ok(token.to_string());
        }
        let session = self.required_session()?;
        Ok(required_string(&session, "token")?.to_string())
    }

    fn optional_authorization_token(
        &self,
        command: &Value,
    ) -> Result<Option<String>, ProductError> {
        if let Some(token) = optional_string(command, "token") {
            return Ok(Some(token.to_string()));
        }
        Ok(self
            .load_session()?
            .and_then(|session| {
                session
                    .get("token")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .filter(|token| !token.is_empty()))
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
        let agent = http_agent();
        let mut request = agent.get(url.as_str()).set("Accept", "application/json");
        if let Some(token) = token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        decode_response(request.call())
    }

    fn post_json(
        &self,
        path: &str,
        body: Value,
        token: Option<&str>,
    ) -> Result<Value, ProductError> {
        let url = format!("{}{}", self.api_base_url, path);
        let agent = http_agent();
        let mut request = agent
            .post(&url)
            .set("Accept", "application/json")
            .set("Content-Type", "application/json");
        if let Some(token) = token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        decode_response(request.send_json(body))
    }

    fn required_session(&self) -> Result<Value, ProductError> {
        self.load_session()?.ok_or(ProductError::NotLoggedIn)
    }

    fn load_session(&self) -> Result<Option<Value>, ProductError> {
        let raw = match fs::read_to_string(&self.session_path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(ProductError::Session(error.to_string())),
        };
        serde_json::from_str(&raw)
            .map(Some)
            .map_err(|error| ProductError::Session(error.to_string()))
    }

    fn save_session(&self, session: &Value) -> Result<(), ProductError> {
        let parent = self
            .session_path
            .parent()
            .ok_or_else(|| ProductError::Session("session path has no parent".to_string()))?;
        fs::create_dir_all(parent).map_err(|error| ProductError::Session(error.to_string()))?;
        let temporary = self.session_path.with_extension("json.tmp");
        let contents = serde_json::to_vec_pretty(session)
            .map_err(|error| ProductError::Session(error.to_string()))?;
        write_private_file(&temporary, &contents)?;
        fs::rename(&temporary, &self.session_path)
            .map_err(|error| ProductError::Session(error.to_string()))?;
        Ok(())
    }

    fn remove_session(&self) -> Result<(), ProductError> {
        match fs::remove_file(&self.session_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ProductError::Session(error.to_string())),
        }
    }
}

fn typed_session(
    response: Value,
    provider: &str,
    require_token: bool,
) -> Result<Value, ProductError> {
    let mut output = response.as_object().cloned().unwrap_or_default();
    let session_stored = output.get("token").and_then(Value::as_str).is_some();
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
    Ok(Value::Object(output))
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

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), ProductError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| ProductError::Session(error.to_string()))?;
        file.write_all(contents)
            .map_err(|error| ProductError::Session(error.to_string()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents).map_err(|error| ProductError::Session(error.to_string()))
    }
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .build()
}

fn decode_response(response: Result<ureq::Response, ureq::Error>) -> Result<Value, ProductError> {
    match response {
        Ok(response) => response
            .into_json::<Value>()
            .map_err(|error| ProductError::Response(error.to_string())),
        Err(ureq::Error::Status(status, response)) => {
            let body = response
                .into_string()
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
            Err(ProductError::HttpStatus { status, message })
        }
        Err(ureq::Error::Transport(error)) => Err(ProductError::Transport(error.to_string())),
    }
}

fn required_string<'a>(request: &'a Value, name: &'static str) -> Result<&'a str, ProductError> {
    request
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ProductError::InvalidParameter(name))
}

fn optional_string<'a>(request: &'a Value, name: &str) -> Option<&'a str> {
    request
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_identifier(request: &Value, name: &'static str) -> Result<String, ProductError> {
    match request.get(name) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Some(Value::Number(value)) => Ok(value.to_string()),
        _ => Err(ProductError::InvalidParameter(name)),
    }
}

fn safe_path_identifier(value: &str, name: &'static str) -> Result<String, ProductError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || value.contains("..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(ProductError::InvalidParameter(name));
    }
    Ok(value.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductError {
    InvalidParameter(&'static str),
    UnsupportedRequest(String),
    Configuration(String),
    NotLoggedIn,
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
    }

    #[cfg(unix)]
    #[test]
    fn account_session_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::SystemTime;
        use std::time::UNIX_EPOCH;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("mahayana-private-session-{nonce}.json"));
        write_private_file(&path, b"{}").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = fs::remove_file(&path);
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn app_session_sync_is_available_to_cli_and_redacts_the_token() {
        use std::time::SystemTime;
        use std::time::UNIX_EPOCH;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("mahayana-app-session-{nonce}.json"));
        let client = MahayanaProductClient::new("https://api.example.test", &path);
        let response = client
            .execute(
                "mahayana.auth.session.sync",
                &json!({
                    "token": "app-session-token",
                    "provider": "alipay",
                    "user": {"username": "tester", "email": "tester@example.test"},
                }),
            )
            .unwrap();

        assert_eq!(
            response,
            json!({
                "@type": "mahayana.auth.session",
                "loggedIn": true,
                "sessionStored": true,
                "provider": "alipay",
            })
        );
        assert_eq!(client.session_token().unwrap(), "app-session-token");
        assert_eq!(
            client
                .execute("mahayana.auth.session.restore", &json!({}))
                .unwrap(),
            json!({
                "@type": "mahayana.auth.session",
                "loggedIn": true,
                "sessionStored": true,
                "token": "app-session-token",
                "provider": "alipay",
                "user": {"username": "tester", "email": "tester@example.test"},
                "username": "tester",
                "email": "tester@example.test",
            })
        );
        let redacted = redact_secrets(&client.load_session().unwrap().unwrap());
        assert_eq!(redacted["token"], "[stored by Mahayana]");
        let _ = fs::remove_file(path);
    }
}
