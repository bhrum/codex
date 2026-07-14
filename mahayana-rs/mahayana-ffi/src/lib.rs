//! C/JSON ABI for native Mahayana hosts.

use mahayana_agent::UnavailableAgentBackend;
#[cfg(any(feature = "desktop-full", feature = "mobile-embedded"))]
use mahayana_agent_codex::CodexAgentBackend;
#[cfg(any(feature = "desktop-full", feature = "mobile-embedded"))]
use mahayana_agent_codex::CodexAgentConfig;
#[cfg(any(feature = "desktop-full", feature = "mobile-embedded"))]
use mahayana_conversation::ConversationProvider;
use mahayana_core::ApprovalDecision;
use mahayana_core::ApprovalId;
use mahayana_core::BuildProfile;
use mahayana_core::OperationId;
use mahayana_core::RuntimeCommand;
use mahayana_core::RuntimeConfig;
use mahayana_miniapp::MiniAppConversationProvider;
use mahayana_miniapp::MiniAppDefinition;
use mahayana_product::MahayanaProductClient;
#[cfg(feature = "desktop-full")]
use mahayana_product::default_mahayana_home;
use mahayana_runtime_core::MahayanaRuntime;
use mahayana_runtime_core::RuntimeBuilder;
use mahayana_social::MahayanaSocialConversationProvider;
use mahayana_telegram::TelegramConversationProvider;
use once_cell::sync::Lazy;
use serde_json::Value;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::ffi::CString;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);
static RUNTIMES: Lazy<Mutex<HashMap<u64, Arc<MahayanaRuntime>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RuntimeCreateConfig {
    #[serde(flatten)]
    runtime: RuntimeConfig,
    /// Optional in-memory override supplied by a platform secure-storage
    /// bridge. This value is consumed during creation and never retained in
    /// the serializable runtime status/config contract.
    product_session_token: Option<String>,
    product_session_path: Option<PathBuf>,
    codex_home: Option<PathBuf>,
    /// Optional Mahayana CLI executable that supports desktop argv helper
    /// dispatch. SDK hosts omit it and use in-process Mahayana workspace tools.
    codex_executable_path: Option<PathBuf>,
    cwd: Option<PathBuf>,
    /// Existing embedded Telegram client created by the platform login flow.
    telegram_client_id: Option<u64>,
    telegram_self_user_id: Option<i64>,
    mini_apps: Vec<MiniAppDefinition>,
}

/// Creates a long-lived runtime. A null config pointer uses safe defaults.
/// Returns zero on error; call [`mahayana_runtime_last_error`] for details.
///
/// # Safety
/// A non-null `config_json` must point to valid NUL-terminated UTF-8 for the
/// duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_runtime_create(config_json: *const c_char) -> u64 {
    clear_last_error();
    let result = (|| {
        let create_config: RuntimeCreateConfig = if config_json.is_null() {
            RuntimeCreateConfig::default()
        } else {
            let source = unsafe { CStr::from_ptr(config_json) }
                .to_str()
                .map_err(|error| format!("runtime config must be UTF-8: {error}"))?;
            serde_json::from_str(source)
                .map_err(|error| format!("runtime config must be valid JSON: {error}"))?
        };
        let runtime = build_runtime(create_config)?;
        let runtime_id = NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed);
        RUNTIMES
            .lock()
            .map_err(|_| "runtime registry mutex poisoned".to_string())?
            .insert(runtime_id, Arc::new(runtime));
        Ok::<_, String>(runtime_id)
    })();
    match result {
        Ok(runtime_id) => runtime_id,
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

fn build_runtime(create: RuntimeCreateConfig) -> Result<MahayanaRuntime, String> {
    let runtime_config = create.runtime.clone();
    #[cfg(all(feature = "mobile-embedded", not(feature = "desktop-full")))]
    let runtime_config = RuntimeConfig {
        build_profile: BuildProfile::MobileEmbedded,
        ..runtime_config
    };
    let mini_apps = if create.mini_apps.is_empty() {
        default_mini_apps()
    } else {
        create.mini_apps.clone()
    };
    let product_client = create
        .product_session_path
        .map(|path| MahayanaProductClient::new("https://api.ombhrum.com", path))
        .unwrap_or_default();
    let session_token = create
        .product_session_token
        .filter(|value| !value.trim().is_empty())
        .or_else(|| product_client.session_token().ok());
    let mut builder = RuntimeBuilder::new(runtime_config.clone());
    #[cfg(any(feature = "desktop-full", feature = "mobile-embedded"))]
    let mut codex_conversation_providers: Vec<Arc<dyn ConversationProvider>> = Vec::new();
    if let Some(token) = session_token.as_ref() {
        let provider = Arc::new(MahayanaSocialConversationProvider::new(
            product_client,
            Some(token.clone()),
        ));
        #[cfg(any(feature = "desktop-full", feature = "mobile-embedded"))]
        codex_conversation_providers.push(Arc::clone(&provider) as Arc<dyn ConversationProvider>);
        builder = builder
            .with_provider(provider)
            .map_err(|error| error.to_string())?;
    }
    if let Some(telegram_client_id) = create.telegram_client_id {
        let provider = Arc::new(TelegramConversationProvider::from_client_id(
            telegram_client_id,
            create.telegram_self_user_id.unwrap_or_default(),
        ));
        #[cfg(any(feature = "desktop-full", feature = "mobile-embedded"))]
        codex_conversation_providers.push(Arc::clone(&provider) as Arc<dyn ConversationProvider>);
        builder = builder
            .with_provider(provider)
            .map_err(|error| error.to_string())?;
    }

    #[cfg(any(feature = "desktop-full", feature = "mobile-embedded"))]
    if matches!(
        runtime_config.build_profile,
        BuildProfile::DesktopFull | BuildProfile::MobileEmbedded
    ) {
        let data_dir = runtime_config.data_dir.clone();
        let cwd = create
            .cwd
            .or_else(|| runtime_config.workspace_roots.first().cloned())
            .or_else(|| data_dir.as_ref().map(|path| path.join("workspace")))
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| "current working directory is unavailable".to_string())?;
        let workspace_roots = if runtime_config.workspace_roots.is_empty() {
            vec![cwd.clone()]
        } else {
            runtime_config.workspace_roots.clone()
        };
        let codex_home = create
            .codex_home
            .or_else(|| data_dir.map(|path| path.join("codex")))
            .or_else(default_codex_home_if_available)
            .ok_or_else(|| {
                "embedded Mahayana requires an application data directory".to_string()
            })?;
        let responses_base_url = create
            .runtime
            .model
            .base_url
            .clone()
            .ok_or_else(|| "Dacheng Responses base URL is required".to_string())?;
        let settings = CodexAgentConfig {
            codex_home,
            cwd,
            workspace_roots,
            model: runtime_config.model.model.clone(),
            responses_base_url,
            product_session_token: session_token.clone(),
            sandbox_mode: codex_protocol::config_types::SandboxMode::WorkspaceWrite,
            approval_policy: codex_protocol::protocol::AskForApproval::OnRequest,
            codex_executable_path: create.codex_executable_path,
            conversation_providers: codex_conversation_providers,
        };
        return builder
            .build_with_agent_backend_and(
                || async move {
                    let backend = CodexAgentBackend::start(settings).await?;
                    Ok(Arc::new(backend) as Arc<dyn mahayana_agent::AgentBackend>)
                },
                move |builder, backend| {
                    let provider = MiniAppConversationProvider::new(backend, mini_apps)?;
                    builder.with_provider(Arc::new(provider))
                },
            )
            .map_err(|error| error.to_string());
    }

    let unavailable_reason = "this platform build has no embedded Codex backend";
    let backend: Arc<dyn mahayana_agent::AgentBackend> =
        Arc::new(UnavailableAgentBackend::new(unavailable_reason));
    let miniapp = MiniAppConversationProvider::new(Arc::clone(&backend), mini_apps)
        .map_err(|error| error.to_string())?;
    builder
        .with_agent_backend(backend)
        .map_err(|error| error.to_string())?
        .with_provider(Arc::new(miniapp))
        .map_err(|error| error.to_string())?
        .build()
        .map_err(|error| error.to_string())
}

fn default_mini_apps() -> Vec<MiniAppDefinition> {
    [
        (
            "official.global-dharma",
            "全球法布施",
            "协助准备、检查和发送全球法布施内容。",
        ),
        (
            "official.flashcards",
            "法流背诵卡",
            "帮助用户复习和制作佛经背诵卡。",
        ),
        (
            "official.platform-publish",
            "平台发布",
            "协助整理并发布自媒体内容，敏感操作必须请求确认。",
        ),
        (
            "official.bot-father",
            "机器人之父",
            "根据用户描述开发、修改和调试大乘个人沙箱小程序。优先使用内置文件工具直接在当前工作区创建或更新 index.html；生成物必须是完整、自包含、少于 1800 个字符的单文件 HTML，HTML 从 <!DOCTYPE html> 开始并以 </html> 结束，CSS 和 JavaScript 内联。完成文件修改后简要说明结果。只有文件工具确实不可用时，才直接回复 HTML 源码且不使用 Markdown 围栏。",
        ),
        (
            "official.assistant",
            "大乘助手",
            "提供大乘软件功能引导和日常协助。",
        ),
    ]
    .into_iter()
    .map(|(app_id, title, instructions)| MiniAppDefinition {
        app_id: app_id.into(),
        title: title.into(),
        instructions: instructions.into(),
        pinned: false,
    })
    .collect()
}

#[cfg(feature = "desktop-full")]
fn default_codex_home() -> PathBuf {
    default_mahayana_home().join("codex")
}

#[cfg(any(feature = "desktop-full", feature = "mobile-embedded"))]
fn default_codex_home_if_available() -> Option<PathBuf> {
    #[cfg(feature = "desktop-full")]
    {
        #[allow(clippy::needless_return)]
        Some(default_codex_home())
    }
    #[cfg(not(feature = "desktop-full"))]
    {
        None
    }
}

/// Executes one command against a runtime and returns an owned JSON string.
///
/// # Safety
/// `command_json` must be a valid NUL-terminated UTF-8 string. Release the
/// returned pointer with [`mahayana_runtime_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_runtime_execute(
    runtime_id: u64,
    command_json: *const c_char,
) -> *mut c_char {
    ffi_response(|| {
        let runtime = runtime(runtime_id)?;
        let command: RuntimeCommand = unsafe { parse_json(command_json, "command") }?;
        runtime.execute(command).map_err(|error| error.to_string())
    })
}

/// Receives the next runtime event, waiting for at most `timeout_ms`.
///
/// # Safety
/// Release the returned pointer with [`mahayana_runtime_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_runtime_receive(runtime_id: u64, timeout_ms: u64) -> *mut c_char {
    ffi_response(|| {
        let runtime = runtime(runtime_id)?;
        runtime
            .receive(Duration::from_millis(timeout_ms))
            .map_err(|error| error.to_string())
    })
}

/// Interrupts an operation using `{ "operationId": "..." }`.
///
/// # Safety
/// `operation_json` must be a valid NUL-terminated UTF-8 string. Release the
/// returned pointer with [`mahayana_runtime_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_runtime_interrupt(
    runtime_id: u64,
    operation_json: *const c_char,
) -> *mut c_char {
    ffi_response(|| {
        let payload: Value = unsafe { parse_json(operation_json, "operation") }?;
        let operation_id = required_string(&payload, "operationId")?;
        runtime(runtime_id)?
            .execute(RuntimeCommand::Interrupt {
                operation_id: OperationId::new(operation_id).map_err(|error| error.to_string())?,
            })
            .map_err(|error| error.to_string())
    })
}

/// Resolves an approval using `{ "approvalId", "decision", "payload"? }`.
///
/// # Safety
/// `approval_json` must be a valid NUL-terminated UTF-8 string. Release the
/// returned pointer with [`mahayana_runtime_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_runtime_resolve_approval(
    runtime_id: u64,
    approval_json: *const c_char,
) -> *mut c_char {
    ffi_response(|| {
        let payload: Value = unsafe { parse_json(approval_json, "approval") }?;
        let approval_id = ApprovalId::new(required_string(&payload, "approvalId")?)
            .map_err(|error| error.to_string())?;
        let decision: ApprovalDecision = serde_json::from_value(
            payload
                .get("decision")
                .cloned()
                .ok_or_else(|| "approval decision is required".to_string())?,
        )
        .map_err(|error| format!("approval decision is invalid: {error}"))?;
        runtime(runtime_id)?
            .execute(RuntimeCommand::ResolveApproval {
                approval_id,
                decision,
                payload: payload.get("payload").cloned().unwrap_or(Value::Null),
            })
            .map_err(|error| error.to_string())
    })
}

/// Closes and removes a runtime handle.
///
/// # Safety
/// Release the returned pointer with [`mahayana_runtime_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_runtime_close(runtime_id: u64) -> *mut c_char {
    ffi_response(|| {
        let removed = RUNTIMES
            .lock()
            .map_err(|_| "runtime registry mutex poisoned".to_string())?
            .remove(&runtime_id)
            .is_some();
        if !removed {
            return Err(format!("runtime was not found: {runtime_id}"));
        }
        Ok(json!({"runtimeId": runtime_id, "closed": true}))
    })
}

/// Returns the most recent creation error for the calling thread.
///
/// # Safety
/// Release the returned pointer with [`mahayana_runtime_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_runtime_last_error() -> *mut c_char {
    let error = LAST_ERROR.with(|slot| slot.borrow().clone());
    into_c_string(json!({"ok": error.is_none(), "message": error}))
}

/// Executes first-party account, contact-management, or direct-message
/// commands. Conversation reads/sends should use the long-lived runtime ABI;
/// this side API exists for login and contact-management operations that are
/// not conversations themselves.
///
/// # Safety
/// `request_json` must point to valid NUL-terminated UTF-8 JSON. Release the
/// returned pointer with [`mahayana_runtime_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_product_execute(request_json: *const c_char) -> *mut c_char {
    ffi_response(|| {
        let request: Value = unsafe { parse_json(request_json, "product request") }?;
        let request_type = required_string(&request, "@type")?;
        MahayanaProductClient::default()
            .execute(request_type, &request)
            .map_err(|error| error.to_string())
    })
}

/// Linker anchor for Flutter builds that load the shared Mahayana library and
/// resolve the legacy Telegram/mini-app host symbols from that same artifact.
#[unsafe(no_mangle)]
pub extern "C" fn mahayana_runtime_force_link() -> u32 {
    let runtime_symbols = [
        mahayana_runtime_create as *const () as usize,
        mahayana_runtime_execute as *const () as usize,
        mahayana_runtime_receive as *const () as usize,
        mahayana_runtime_interrupt as *const () as usize,
        mahayana_runtime_resolve_approval as *const () as usize,
        mahayana_runtime_close as *const () as usize,
        mahayana_product_execute as *const () as usize,
        mahayana_runtime_free_string as *const () as usize,
    ];
    let telegram_symbols = [
        fabushi_telegram_runtime::fabushi_telegram_create_client as *const () as usize,
        fabushi_telegram_runtime::fabushi_telegram_create_persistent_client as *const () as usize,
        fabushi_telegram_runtime::fabushi_telegram_execute as *const () as usize,
        fabushi_telegram_runtime::fabushi_telegram_close_client as *const () as usize,
        fabushi_telegram_runtime::fabushi_telegram_free_string as *const () as usize,
    ];
    let miniapp_symbols = [
        fabushi_miniapp_runtime::fabushi_runtime_create_client as *const () as usize,
        fabushi_miniapp_runtime::fabushi_runtime_send as *const () as usize,
        fabushi_miniapp_runtime::fabushi_runtime_receive as *const () as usize,
        fabushi_miniapp_runtime::fabushi_runtime_execute as *const () as usize,
        fabushi_miniapp_runtime::fabushi_runtime_close as *const () as usize,
        fabushi_miniapp_runtime::fabushi_runtime_free_string as *const () as usize,
    ];
    std::hint::black_box((runtime_symbols, telegram_symbols, miniapp_symbols));
    1
}

/// Releases strings returned by this ABI.
///
/// # Safety
/// `pointer` must be null or a pointer returned by a Mahayana string-returning
/// ABI function, and it must be released exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_runtime_free_string(pointer: *mut c_char) {
    if !pointer.is_null() {
        drop(unsafe { CString::from_raw(pointer) });
    }
}

fn runtime(runtime_id: u64) -> Result<Arc<MahayanaRuntime>, String> {
    RUNTIMES
        .lock()
        .map_err(|_| "runtime registry mutex poisoned".to_string())?
        .get(&runtime_id)
        .cloned()
        .ok_or_else(|| format!("runtime was not found: {runtime_id}"))
}

unsafe fn parse_json<T>(pointer: *const c_char, name: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    if pointer.is_null() {
        return Err(format!("{name} JSON must not be null"));
    }
    let source = unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|error| format!("{name} JSON must be UTF-8: {error}"))?;
    serde_json::from_str(source).map_err(|error| format!("{name} JSON is invalid: {error}"))
}

fn required_string<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn ffi_response<T>(operation: impl FnOnce() -> Result<T, String>) -> *mut c_char
where
    T: serde::Serialize,
{
    let response = match operation() {
        Ok(data) => json!({"ok": true, "data": data}),
        Err(message) => json!({
            "ok": false,
            "errorCode": "mahayana_runtime_error",
            "message": message,
        }),
    };
    into_c_string(response)
}

fn into_c_string(value: Value) -> *mut c_char {
    let encoded = serde_json::to_string(&value).unwrap_or_else(|_| {
        "{\"ok\":false,\"errorCode\":\"mahayana_serialization_error\"}".to_string()
    });
    CString::new(encoded)
        .expect("serialized JSON contains no NUL")
        .into_raw()
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

fn set_last_error(error: String) {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(error));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(pointer: *mut c_char) -> Value {
        assert!(!pointer.is_null());
        let text = unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .expect("UTF-8 response")
            .to_string();
        unsafe { mahayana_runtime_free_string(pointer) };
        serde_json::from_str(&text).expect("JSON response")
    }

    #[test]
    fn lifecycle_abi_creates_executes_receives_and_closes() {
        let runtime_id = unsafe { mahayana_runtime_create(std::ptr::null()) };
        assert_ne!(runtime_id, 0);
        let command = CString::new(r#"{"@type":"mahayana.runtime.status"}"#).unwrap();
        let status = take(unsafe { mahayana_runtime_execute(runtime_id, command.as_ptr()) });
        assert_eq!(status["ok"], true);
        assert_eq!(status["data"]["runtimeAbiVersion"], 1);
        assert_eq!(status["data"]["remoteAgentEnabled"], false);

        let ready = take(unsafe { mahayana_runtime_receive(runtime_id, 10) });
        assert_eq!(ready["ok"], true);
        assert_eq!(ready["data"]["@type"], "mahayana.runtime.ready");

        let closed = take(unsafe { mahayana_runtime_close(runtime_id) });
        assert_eq!(closed["data"]["closed"], true);
    }

    #[test]
    fn create_rejects_remote_agent_gateway() {
        let config = CString::new(r#"{"remoteAgentEnabled":true}"#).unwrap();
        let runtime_id = unsafe { mahayana_runtime_create(config.as_ptr()) };
        assert_eq!(runtime_id, 0);
        let error = take(unsafe { mahayana_runtime_last_error() });
        assert!(
            error["message"]
                .as_str()
                .expect("error message")
                .contains("remote Agent")
        );
    }
}
