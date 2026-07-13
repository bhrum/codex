use fabushi_telegram_core::{Command, TelegramEngine};
use fabushi_telegram_protocol::{AuthCommand, AuthorizationMachine};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct TelegramWasmClient {
    core: TelegramEngine,
    authorization: AuthorizationMachine,
}

impl Default for TelegramWasmClient {
    fn default() -> Self {
        Self {
            core: TelegramEngine::new(),
            authorization: AuthorizationMachine::new(),
        }
    }
}

#[wasm_bindgen]
impl TelegramWasmClient {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Executes the same JSON command envelope used by the native C ABI.
    #[wasm_bindgen(js_name = execute)]
    pub fn execute_json(&mut self, request_json: &str) -> String {
        let extra = serde_json::from_str::<Value>(request_json)
            .ok()
            .and_then(|request| request.get("@extra").cloned());
        let result = serde_json::from_str::<Value>(request_json)
            .map_err(|error| ("invalid_json", error.to_string()))
            .and_then(|request| self.execute_value(request));
        match result {
            Ok(mut data) => {
                if let (Some(extra), Some(object)) = (extra, data.as_object_mut()) {
                    object.insert("@extra".to_string(), extra);
                }
                json!({"ok": true, "data": data}).to_string()
            }
            Err((code, message)) => json!({
                "ok": false,
                "errorCode": code,
                "message": message,
                "@extra": extra,
            })
            .to_string(),
        }
    }

    /// Exports a plain state snapshot for the host adapter. The IndexedDB
    /// adapter must encrypt it before persistence; this method does not claim
    /// browser persistence is already connected.
    #[wasm_bindgen(js_name = exportState)]
    pub fn export_state(&self) -> String {
        serde_json::to_string(self.core.state()).unwrap_or_else(|error| {
            json!({
                "errorCode": "state_serialization_failed",
                "message": error.to_string(),
            })
            .to_string()
        })
    }

    #[wasm_bindgen(js_name = importState)]
    pub fn import_state(&mut self, state_json: &str) -> String {
        match serde_json::from_str(state_json) {
            Ok(state) => {
                self.core = TelegramEngine::from_state(state);
                json!({"ok": true}).to_string()
            }
            Err(error) => json!({
                "ok": false,
                "errorCode": "invalid_state_snapshot",
                "message": error.to_string(),
            })
            .to_string(),
        }
    }
}

impl TelegramWasmClient {
    fn execute_value(&mut self, request: Value) -> Result<Value, (&'static str, String)> {
        let request_type = request
            .get("@type")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or((
                "missing_request_type",
                "request must contain a non-empty @type".to_string(),
            ))?;
        match request_type {
            "telegram.getStatus" => Ok(json!({
                "@type": "telegram.status",
                "architecture": "rust-wasm-command-event-core",
                "platform": "web",
                "persistentStorage": false,
                "transportConnected": false,
            })),
            "telegram.getState" => Ok(json!({
                "@type": "telegram.state",
                "state": self.core.state(),
            })),
            "telegram.getAuthorizationState" => Ok(json!({
                "@type": "telegram.authorizationState",
                "authorizationState": self.authorization.state(),
            })),
            "telegram.executeCoreCommand" => {
                let command: Command =
                    serde_json::from_value(request.get("command").cloned().ok_or((
                        "missing_command",
                        "core request must contain command".to_string(),
                    ))?)
                    .map_err(|error| ("invalid_core_command", error.to_string()))?;
                let events = self
                    .core
                    .execute(command)
                    .map_err(|error| ("core_command_failed", error.to_string()))?;
                Ok(json!({
                    "@type": "telegram.coreResult",
                    "events": events,
                    "state": self.core.state(),
                }))
            }
            "telegram.executeAuthorizationCommand" => {
                let command: AuthCommand =
                    serde_json::from_value(request.get("command").cloned().ok_or((
                        "missing_command",
                        "authorization request must contain command".to_string(),
                    ))?)
                    .map_err(|error| ("invalid_authorization_command", error.to_string()))?;
                let events = self
                    .authorization
                    .execute(command)
                    .map_err(|error| ("authorization_command_failed", error.to_string()))?;
                Ok(json!({
                    "@type": "telegram.authorizationResult",
                    "events": events,
                    "authorizationState": self.authorization.state(),
                }))
            }
            other => Err((
                "unsupported_request_type",
                format!("request type {other} is not supported"),
            )),
        }
    }
}
