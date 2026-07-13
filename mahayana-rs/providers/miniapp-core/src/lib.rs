use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::process::Command;
use std::time::Duration;

pub const HOST_API_VERSION: &str = "2.0";
pub const HOST_SDK_VERSION: &str = "2.0.0";
pub const SPEC_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityLayer {
    Core,
    BotMessaging,
    Identity,
    Payments,
    NativeNetwork,
    System,
    Creation,
    LocalAutomation,
    ExternalNavigation,
    NativeUi,
    Device,
    Storage,
    Share,
    Game,
    Performance,
    Window,
}

impl CapabilityLayer {
    pub fn storage_value(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::BotMessaging => "botMessaging",
            Self::Identity => "identity",
            Self::Payments => "payments",
            Self::NativeNetwork => "nativeNetwork",
            Self::System => "system",
            Self::Creation => "creation",
            Self::LocalAutomation => "localAutomation",
            Self::ExternalNavigation => "externalNavigation",
            Self::NativeUi => "nativeUi",
            Self::Device => "device",
            Self::Storage => "storage",
            Self::Share => "share",
            Self::Game => "game",
            Self::Performance => "performance",
            Self::Window => "window",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityRisk {
    Low,
    Medium,
    High,
    Critical,
}

impl CapabilityRisk {
    pub fn storage_value(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Always,
    NativeIo,
    DesktopNative,
}

impl Availability {
    pub fn storage_value(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::NativeIo => "nativeIo",
            Self::DesktopNative => "desktopNative",
        }
    }

    pub fn is_available(self, platform: HostPlatform) -> bool {
        match self {
            Self::Always => true,
            Self::NativeIo => platform.is_native_io(),
            Self::DesktopNative => platform.is_desktop_native(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustRequirement {
    Declared,
    TrustedOfficial,
}

impl TrustRequirement {
    pub fn storage_value(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::TrustedOfficial => "trustedOfficial",
        }
    }

    pub fn is_satisfied(self, trusted_official: bool) -> bool {
        match self {
            Self::Declared => true,
            Self::TrustedOfficial => trusted_official,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Web,
    Ios,
    Android,
    Macos,
    Windows,
    Linux,
    Unknown,
}

impl HostPlatform {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "web" => Self::Web,
            "ios" => Self::Ios,
            "android" => Self::Android,
            "macos" | "mac" | "darwin" => Self::Macos,
            "windows" | "win32" => Self::Windows,
            "linux" => Self::Linux,
            _ => Self::Unknown,
        }
    }

    pub fn storage_value(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Ios => "ios",
            Self::Android => "android",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_native_io(self) -> bool {
        !matches!(self, Self::Web | Self::Unknown)
    }

    pub fn is_desktop_native(self) -> bool {
        matches!(self, Self::Macos | Self::Windows | Self::Linux)
    }
}

#[derive(Debug, Clone)]
pub struct Capability {
    pub id: &'static str,
    pub layer: &'static str,
    pub native: bool,
    pub adapter: &'static str,
    pub availability: &'static str,
    pub trust: &'static str,
    pub risk: &'static str,
    pub methods: &'static [&'static str],
    pub note: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct HostMethod {
    pub method: &'static str,
    pub permission: &'static str,
    pub risk: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone)]
pub struct PolicyContext {
    pub declared_permissions: BTreeSet<String>,
    pub platform: HostPlatform,
    pub trusted_official: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyStatus {
    Granted,
    Denied,
    UnsupportedPlatform,
    UnknownMethod,
    UnknownCapability,
    TrustRequired,
}

impl PolicyStatus {
    pub fn storage_value(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::UnsupportedPlatform => "unsupportedPlatform",
            Self::UnknownMethod => "unknownMethod",
            Self::UnknownCapability => "unknownCapability",
            Self::TrustRequired => "trustRequired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub status: PolicyStatus,
    pub method: String,
    pub permission: Option<String>,
    pub capability: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiniAppRuntimeError {
    pub code: String,
    pub message: String,
}

impl MiniAppRuntimeError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for MiniAppRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MiniAppRuntimeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFetchRequest {
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: u64,
    pub max_body_bytes: usize,
}

impl HttpFetchRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: "GET".to_string(),
            headers: BTreeMap::new(),
            body: None,
            timeout_ms: 15_000,
            max_body_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFetchResponse {
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpFetchResponse {
    pub fn body_text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExecuteRequest {
    pub command: String,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl ProcessExecuteRequest {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            arguments: Vec::new(),
            working_directory: None,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExecuteOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ProcessExecuteOutput {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }

    pub fn stdout_text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }

    pub fn stderr_text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).to_string()
    }
}

pub fn capabilities() -> Vec<Capability> {
    use Availability::*;
    use CapabilityLayer::*;
    use CapabilityRisk::*;
    use TrustRequirement::*;

    vec![
        capability(
            "app.context",
            Core,
            false,
            "MiniAppHostContextAdapter",
            Always,
            Declared,
            Low,
            &[
                "app.getContext",
                "app.getCapabilities",
                "app.requestCapabilities",
                "app.getHostApiSpec",
                "app.getTheme",
            ],
            Some("能力协商基础层；小程序读取上下文、权限列表和宿主 API 规格。"),
        ),
        capability(
            "bot.chat",
            BotMessaging,
            false,
            "MiniAppBotBridgeAdapter",
            Always,
            Declared,
            Low,
            &[
                "bot.sendMessage",
                "bot.postMessage",
                "bot.reportCommandResult",
                "bot.takePendingCommands",
                "bot.openPanel",
                "bot.setPanelState",
                "bot.setCommands",
                "bot.getCommands",
                "bot.setInputPlaceholder",
                "bot.setComposerText",
                "bot.getComposerState",
                "bot.close",
            ],
            Some("宿主只做聊天命令、面板和回写媒介，不承载小程序业务逻辑。"),
        ),
        capability(
            "auth.session",
            Identity,
            false,
            "AuthSessionAdapter",
            Always,
            Declared,
            Medium,
            &["auth.getSession", "auth.requireLogin", "auth.getInitData", "auth.getScopedToken"],
            Some("返回脱敏用户与会员状态；不返回宿主访问 token。"),
        ),
        capability(
            "auth.token",
            Identity,
            false,
            "AuthTokenAdapter",
            Always,
            TrustedOfficial,
            Critical,
            &["auth.getAccessToken"],
            Some("访问 token 只给受信官方小程序；第三方使用 Telegram 风格 initData 签名。"),
        ),
        capability(
            "payments.alipay",
            Payments,
            true,
            "AlipayPaymentAdapter",
            Always,
            TrustedOfficial,
            High,
            &[
                "payments.alipay.createOrder",
                "payments.alipay.pay",
                "payments.alipay.queryOrder",
            ],
            Some("官方兼容能力；开放平台商品结算应优先走福德金 / invoice 抽象。"),
        ),
        capability(
            "payments.entitlement",
            Payments,
            false,
            "MiniAppEntitlementAdapter",
            Always,
            Declared,
            Medium,
            &[
                "payments.checkEntitlement",
                "payments.alipay.checkEntitlement",
            ],
            None,
        ),
        capability(
            "payments.invoice",
            Payments,
            false,
            "InvoicePaymentAdapter",
            Always,
            Declared,
            High,
            &["payments.createInvoice", "payments.openInvoice", "payments.queryInvoice"],
            Some("统一平台发票结算入口，取代零散的支付方法。"),
        ),
        capability(
            "payments.fudeGold",
            Payments,
            false,
            "FudeGoldPaymentAdapter",
            Always,
            Declared,
            High,
            &["payments.requestPayment"],
            Some("统一平台代币支付入口；宿主负责原生确认和权益登记。"),
        ),
        capability(
            "wallet.balance",
            Payments,
            false,
            "WalletBalanceAdapter",
            Always,
            Declared,
            Medium,
            &["wallet.getBalance"],
            None,
        ),
        capability(
            "network.udp",
            NativeNetwork,
            true,
            "UdpSocketAdapter",
            NativeIo,
            TrustedOfficial,
            High,
            &[
                "network.udp.open",
                "network.udp.send",
                "network.udp.broadcast",
                "network.udp.close",
            ],
            Some("宿主提供 UDP socket 原语，小程序自行解释业务协议。"),
        ),
        capability(
            "network.interfaces",
            NativeNetwork,
            true,
            "NetworkInterfaceAdapter",
            NativeIo,
            TrustedOfficial,
            High,
            &["network.interfaces.list"],
            Some("暴露本机网卡与地址，默认只给受信官方小程序。"),
        ),
        capability(
            "network.http",
            NativeNetwork,
            true,
            "RustHttpClientAdapter",
            NativeIo,
            Declared,
            High,
            &["network.http.fetch"],
            Some("宿主提供 HTTP(S) fetch 原语，带超时、响应大小限制和权限审计；小程序自行解释内容。"),
        ),
        capability(
            "system.keepAwake",
            System,
            true,
            "KeepAwakeAdapter",
            Always,
            Declared,
            Medium,
            &["system.keepAwake"],
            None,
        ),
        capability(
            "hotspot.settings",
            System,
            true,
            "HotspotSettingsAdapter",
            NativeIo,
            TrustedOfficial,
            High,
            &["hotspot.openSettings"],
            None,
        ),
        capability(
            "game.runtime",
            Game,
            true,
            "GameRuntimeAdapter",
            Always,
            Declared,
            Low,
            &["game.runtime.getInfo"],
            Some("小游戏运行时能力协商入口；返回平台、性能档、渲染后端、输入和资源缓存建议。"),
        ),
        capability(
            "game.performance",
            Performance,
            true,
            "GamePerformanceAdapter",
            NativeIo,
            Declared,
            Medium,
            &["game.performance.setMode"],
            Some("允许小游戏请求 battery / balanced / performance / ultra 运行档，宿主按平台映射到帧预算和资源优先级。"),
        ),
        capability(
            "game.nativeSurface",
            Game,
            true,
            "NativeGameSurfaceAdapter",
            NativeIo,
            Declared,
            High,
            &["game.surface.request", "game.surface.release"],
            Some("为端游级小游戏预留 Metal / Vulkan / Direct3D / OpenGL 原生渲染面；WebView 只能作为降级路径。"),
        ),
        capability(
            "game.input",
            Game,
            true,
            "GameInputAdapter",
            Always,
            Declared,
            Medium,
            &["game.input.setCapture", "game.input.getState"],
            Some("统一键盘、鼠标、触控、手柄和指针锁输入抽象，便于小游戏迁移桌面级交互。"),
        ),
        capability(
            "game.assets",
            Game,
            false,
            "MiniAppAssetCacheAdapter",
            Always,
            Declared,
            Medium,
            &["game.assets.prefetch"],
            Some("宿主侧资源预热缓存，用来降低小游戏首帧和切场景等待。"),
        ),
        capability(
            "game.save",
            Game,
            false,
            "MiniAppSaveDataAdapter",
            Always,
            Declared,
            Medium,
            &["game.save.read", "game.save.write", "game.save.delete"],
            Some("每个小程序隔离的存档 API，可被后续云存档同步层复用。"),
        ),
        capability(
            "window.lifecycle",
            Window,
            true,
            "HostWindowLifecycleAdapter",
            NativeIo,
            Declared,
            Medium,
            &[
                "window.fullscreen.request",
                "window.fullscreen.exit",
                "window.orientation.lock",
                "window.orientation.unlock",
            ],
            Some("对标 Telegram fullscreen/safe area，又比 WebView 更接近原生窗口与方向控制。"),
        ),
        capability(
            "ui.native",
            NativeUi,
            true,
            "NativeUiAdapter",
            Always,
            Declared,
            Medium,
            &["ui.alert", "ui.confirm", "ui.mainButton.set"],
            Some("对标 Telegram MainButton 与微信原生弹窗；具体 UI adapter 可逐平台补齐。"),
        ),
        capability(
            "haptics.feedback",
            Device,
            true,
            "HapticFeedbackAdapter",
            NativeIo,
            Declared,
            Medium,
            &[
                "haptics.impact",
                "haptics.notification",
                "haptics.selection",
            ],
            None,
        ),
        capability(
            "device.biometrics",
            Device,
            true,
            "BiometricAuthAdapter",
            NativeIo,
            TrustedOfficial,
            High,
            &["device.biometrics.authenticate"],
            Some("高危操作可拉起 Face ID / Touch ID / Android biometrics。"),
        ),
        capability(
            "device.qrScanner",
            Device,
            true,
            "QrScannerAdapter",
            NativeIo,
            Declared,
            Medium,
            &["device.qrScanner.scan"],
            None,
        ),
        capability(
            "cloud.kv",
            Storage,
            false,
            "CloudKeyValueStorageAdapter",
            Always,
            Declared,
            Medium,
            &["cloud.kv.get", "cloud.kv.set", "cloud.kv.delete"],
            Some("每个小程序隔离的轻量云端 Key-Value。"),
        ),
        capability(
            "runtime.storage",
            Storage,
            true,
            "RustLocalConsistencyStoreAdapter",
            NativeIo,
            TrustedOfficial,
            High,
            &[
                "runtime.storage.configure",
                "runtime.storage.getStatus",
                "runtime.storage.put",
                "runtime.storage.get",
                "runtime.storage.delete",
                "runtime.storage.list",
                "runtime.storage.snapshot",
            ],
            Some("TDLib 风格本地一致性存储：支持 revision 乐观并发、快照持久化和 update 流。"),
        ),
        capability(
            "runtime.file",
            LocalAutomation,
            true,
            "RustFileStateRegistryAdapter",
            NativeIo,
            TrustedOfficial,
            High,
            &[
                "runtime.file.register",
                "runtime.file.updateState",
                "runtime.file.get",
                "runtime.file.list",
            ],
            Some("将文件注册为一等对象，通过 updateFile 推送本地、上传、下载、失败等状态变化。"),
        ),
        capability(
            "globalDharma.delivery",
            NativeNetwork,
            true,
            "GlobalDharmaDeliveryKernelAdapter",
            NativeIo,
            TrustedOfficial,
            High,
            &[
                "globalDharma.delivery.enqueue",
                "globalDharma.delivery.getJob",
                "globalDharma.delivery.listJobs",
                "globalDharma.delivery.nextRetry",
                "globalDharma.delivery.markAttempt",
                "globalDharma.delivery.recordReceipt",
                "globalDharma.delivery.listReceipts",
            ],
            Some("全球法布施投递内核：统一管理 jobs、receipts、retry 队列和 delivery updates。"),
        ),
        capability(
            "share.chat",
            Share,
            false,
            "ShareToChatAdapter",
            Always,
            Declared,
            Medium,
            &["share.chat.send"],
            Some("生成富文本卡片并唤起法布施联系人分享。"),
        ),
        capability(
            "flashcards.create",
            Creation,
            false,
            "FlashcardDeckAdapter",
            Always,
            Declared,
            Medium,
            &["flashcards.createDeck", "flashcards.openDeck"],
            Some("复用宿主制卡流水线；小程序只声明权限并传入内容。"),
        ),
        capability(
            "platform.publish",
            Creation,
            false,
            "PlatformPublishAdapter",
            Always,
            Declared,
            Medium,
            &[
                "platformPublish.createDraft",
                "platformPublish.publishDraft",
            ],
            None,
        ),
        capability(
            "files.pick",
            LocalAutomation,
            true,
            "FilePickerAdapter",
            NativeIo,
            Declared,
            Medium,
            &["files.pick"],
            None,
        ),
        capability(
            "projects.read",
            LocalAutomation,
            true,
            "LocalProjectCatalogAdapter",
            DesktopNative,
            TrustedOfficial,
            High,
            &["projects.list", "projects.select"],
            None,
        ),
        capability(
            "openclaw.status",
            LocalAutomation,
            true,
            "OpenClawRuntimeStatusAdapter",
            DesktopNative,
            TrustedOfficial,
            High,
            &["openclaw.status", "openclaw.restart"],
            None,
        ),
        capability(
            "openclaw.chat",
            LocalAutomation,
            false,
            "OpenClawChatAdapter",
            DesktopNative,
            TrustedOfficial,
            High,
            &["openclaw.chat"],
            Some("桌面 AI/终端协作通道；必须由宿主预置具体 adapter。"),
        ),
        capability(
            "desktop.control",
            LocalAutomation,
            true,
            "DesktopControlAdapter",
            DesktopNative,
            TrustedOfficial,
            Critical,
            &["desktopControl.executeTool"],
            Some("桌面控制原语，只给受信官方小程序。"),
        ),
        capability(
            "local.loopback",
            LocalAutomation,
            true,
            "LocalLoopbackAdapter",
            DesktopNative,
            TrustedOfficial,
            High,
            &["localLoopback.fetch"],
            Some("仅允许访问 localhost / 127.0.0.1 / ::1。"),
        ),
        capability(
            "fs.readWrite",
            LocalAutomation,
            true,
            "MiniAppPrivateFileSystemAdapter",
            DesktopNative,
            TrustedOfficial,
            Critical,
            &["fs.writeFile", "fs.readFile"],
            Some("默认限制到小程序私有目录；绝对路径需后续接用户授权 token。"),
        ),
        capability(
            "shell.execute",
            LocalAutomation,
            true,
            "LocalShellAdapter",
            DesktopNative,
            TrustedOfficial,
            Critical,
            &["shell.execute"],
            Some("本地命令执行必须受信、可审计，并由宿主流式回传日志。"),
        ),
        capability(
            "runtime.process",
            LocalAutomation,
            true,
            "RustProcessAdapter",
            DesktopNative,
            TrustedOfficial,
            Critical,
            &["runtime.process.execute"],
            Some("宿主提供本地进程执行原语；命令、参数、工作目录与环境由小程序声明和审计。"),
        ),
        capability(
            "browser.external",
            ExternalNavigation,
            true,
            "ExternalBrowserAdapter",
            Always,
            Declared,
            Medium,
            &["browser.open"],
            None,
        ),
    ]
}

pub fn host_methods() -> Vec<HostMethod> {
    capabilities()
        .into_iter()
        .flat_map(|capability| {
            capability
                .methods
                .iter()
                .copied()
                .map(move |method| HostMethod {
                    method,
                    permission: capability.id,
                    risk: capability.risk,
                    description: method_description(method),
                })
        })
        .collect()
}

pub fn permission_groups() -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut groups: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for capability in capabilities() {
        groups
            .entry(capability.layer)
            .or_default()
            .push(capability.id);
    }
    for values in groups.values_mut() {
        values.sort_unstable();
    }
    groups
}

pub fn host_api_spec_json() -> String {
    let capabilities = capabilities();
    let native_capabilities = capabilities
        .iter()
        .filter(|capability| capability.native)
        .cloned()
        .collect::<Vec<_>>();
    format!(
        "{{\n  \"schemaVersion\": {},\n  \"hostApiVersion\": {},\n  \"hostSdkVersion\": {},\n  \"invokePattern\": {},\n  \"commandProtocol\": {},\n  \"permissionGroups\": {},\n  \"capabilityModel\": {},\n  \"capabilities\": {},\n  \"nativeCapabilities\": {},\n  \"methods\": {}\n}}",
        SPEC_SCHEMA_VERSION,
        quote(HOST_API_VERSION),
        quote(HOST_SDK_VERSION),
        quote("window.FabushiMiniApp.invoke(method, params)"),
        command_protocol_json(),
        permission_groups_json(),
        capability_model_json(),
        capabilities_json(&capabilities),
        capabilities_json(&native_capabilities),
        methods_json(&host_methods()),
    )
}

pub fn evaluate_method(method: &str, context: &PolicyContext) -> PolicyDecision {
    let method = method.trim();
    let Some(host_method) = host_methods()
        .into_iter()
        .find(|item| item.method == method)
    else {
        return PolicyDecision {
            allowed: false,
            status: PolicyStatus::UnknownMethod,
            method: method.to_string(),
            permission: None,
            capability: None,
            reason: format!("未知小程序能力：{method}"),
        };
    };

    let Some(capability) = capabilities()
        .into_iter()
        .find(|item| item.id == host_method.permission)
    else {
        return PolicyDecision {
            allowed: false,
            status: PolicyStatus::UnknownCapability,
            method: method.to_string(),
            permission: Some(host_method.permission.to_string()),
            capability: None,
            reason: format!("宿主缺少 capability 定义：{}", host_method.permission),
        };
    };

    if !context.declared_permissions.contains(capability.id) {
        return PolicyDecision {
            allowed: false,
            status: PolicyStatus::Denied,
            method: method.to_string(),
            permission: Some(capability.id.to_string()),
            capability: Some(capability.id.to_string()),
            reason: format!("小程序未声明或未获准使用 {}", capability.id),
        };
    }

    let availability = availability_from_str(capability.availability);
    if !availability.is_available(context.platform) {
        return PolicyDecision {
            allowed: false,
            status: PolicyStatus::UnsupportedPlatform,
            method: method.to_string(),
            permission: Some(capability.id.to_string()),
            capability: Some(capability.id.to_string()),
            reason: format!(
                "{} 在当前平台 {} 不可用",
                capability.id,
                context.platform.storage_value()
            ),
        };
    }

    let trust = trust_from_str(capability.trust);
    if !trust.is_satisfied(context.trusted_official) {
        return PolicyDecision {
            allowed: false,
            status: PolicyStatus::TrustRequired,
            method: method.to_string(),
            permission: Some(capability.id.to_string()),
            capability: Some(capability.id.to_string()),
            reason: format!("{} 只允许受信官方小程序调用", capability.id),
        };
    }

    PolicyDecision {
        allowed: true,
        status: PolicyStatus::Granted,
        method: method.to_string(),
        permission: Some(capability.id.to_string()),
        capability: Some(capability.id.to_string()),
        reason: "granted".to_string(),
    }
}

pub fn network_http_fetch(
    request: &HttpFetchRequest,
) -> Result<HttpFetchResponse, MiniAppRuntimeError> {
    let url = request.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(MiniAppRuntimeError::new(
            "invalid_url",
            "network.http.fetch only supports http:// and https:// URLs",
        ));
    }

    let method = request.method.trim().to_ascii_uppercase();
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
    ) {
        return Err(MiniAppRuntimeError::new(
            "invalid_method",
            format!("unsupported HTTP method: {method}"),
        ));
    }

    let timeout_ms = request.timeout_ms.clamp(1_000, 120_000);
    let max_body_bytes = request.max_body_bytes.clamp(1, 16 * 1024 * 1024);
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(timeout_ms))
        .build();
    let mut outbound = agent.request(&method, url);
    for (key, value) in &request.headers {
        outbound = outbound.set(key, value);
    }

    let response = match request.body.as_deref() {
        Some(body) => outbound.send_bytes(body),
        None => outbound.call(),
    };
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(error)) => {
            return Err(MiniAppRuntimeError::new("network_error", error.to_string()));
        }
    };

    let status_code = response.status();
    let mut headers = BTreeMap::new();
    for name in response.headers_names() {
        if let Some(value) = response.header(&name) {
            headers.insert(name, value.to_string());
        }
    }

    let mut body = Vec::new();
    let mut reader = response.into_reader().take((max_body_bytes + 1) as u64);
    reader
        .read_to_end(&mut body)
        .map_err(|error| MiniAppRuntimeError::new("read_failed", error.to_string()))?;
    if body.len() > max_body_bytes {
        return Err(MiniAppRuntimeError::new(
            "response_too_large",
            format!("response exceeded {max_body_bytes} bytes"),
        ));
    }

    Ok(HttpFetchResponse {
        status_code,
        headers,
        body,
    })
}

pub fn runtime_process_execute(
    request: &ProcessExecuteRequest,
) -> Result<ProcessExecuteOutput, MiniAppRuntimeError> {
    let command = request.command.trim();
    if command.is_empty() {
        return Err(MiniAppRuntimeError::new(
            "invalid_command",
            "process command cannot be empty",
        ));
    }

    let mut process = Command::new(command);
    process.args(&request.arguments);
    if let Some(working_directory) = request.working_directory.as_deref() {
        if !working_directory.trim().is_empty() {
            process.current_dir(working_directory);
        }
    }
    process.envs(&request.env);

    let output = process
        .output()
        .map_err(|error| MiniAppRuntimeError::new("spawn_failed", error.to_string()))?;
    Ok(ProcessExecuteOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

pub fn sign_init_data(secret: &[u8], fields: &BTreeMap<String, String>) -> String {
    hmac_sha256_hex(secret, init_data_payload(fields).as_bytes())
}

pub fn verify_init_data(
    secret: &[u8],
    fields: &BTreeMap<String, String>,
    expected_hash: &str,
) -> bool {
    let actual = sign_init_data(secret, fields);
    constant_time_eq(actual.as_bytes(), expected_hash.as_bytes())
}

pub fn build_signed_init_data(
    secret: &[u8],
    fields: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut signed = fields.clone();
    signed.insert("hash".to_string(), sign_init_data(secret, fields));
    signed
}

pub fn init_data_payload(fields: &BTreeMap<String, String>) -> String {
    fields
        .iter()
        .filter(|(key, _)| key.as_str() != "hash")
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn capability(
    id: &'static str,
    layer: CapabilityLayer,
    native: bool,
    adapter: &'static str,
    availability: Availability,
    trust: TrustRequirement,
    risk: CapabilityRisk,
    methods: &'static [&'static str],
    note: Option<&'static str>,
) -> Capability {
    Capability {
        id,
        layer: layer.storage_value(),
        native,
        adapter,
        availability: availability.storage_value(),
        trust: trust.storage_value(),
        risk: risk.storage_value(),
        methods,
        note,
    }
}

fn availability_from_str(value: &str) -> Availability {
    match value {
        "nativeIo" => Availability::NativeIo,
        "desktopNative" => Availability::DesktopNative,
        _ => Availability::Always,
    }
}

fn trust_from_str(value: &str) -> TrustRequirement {
    match value {
        "trustedOfficial" => TrustRequirement::TrustedOfficial,
        _ => TrustRequirement::Declared,
    }
}

fn method_description(method: &str) -> &'static str {
    match method {
        "app.getContext" => "读取宿主、小程序、机器人和平台上下文。",
        "app.getCapabilities" => "读取当前小程序可用能力列表。",
        "app.requestCapabilities" => "按 manifest、adapter、平台、信任等级协商能力状态。",
        "app.getHostApiSpec" => "读取宿主 API 规格。",
        "app.getTheme" => "读取宿主主题 token。",
        "auth.getSession" => "读取宿主已登录用户的简要状态（不含敏感 token）。",
        "auth.requireLogin" => "通知宿主弹出原生登录；如果已登录则立即返回 session。",
        "auth.getInitData" => "获取 Telegram 风格的安全签名 InitData，用于向第三方后端自证身份。",
        "auth.getScopedToken" => "获取带有严格范围和有效期的短时 token。",
        "auth.getAccessToken" => "【高危】读取宿主完整通行 token，仅限受信官方小程序。",
        "payments.requestPayment" => "请求宿主弹出原生确认并扣除福德金。",
        "payments.createInvoice" => "创建统一支付发票。",
        "payments.openInvoice" => "打开并支付给定的发票。",
        "payments.queryInvoice" => "查询发票支付状态。",
        "payments.checkEntitlement" | "payments.alipay.checkEntitlement" => {
            "查询宿主后端是否已解锁一次性付费商品。"
        }
        "payments.alipay.createOrder" => "创建支付宝订单；开放平台应优先使用统一 invoice。",
        "payments.alipay.pay" => "拉起支付宝 App 或网页支付。",
        "payments.alipay.queryOrder" => "查询支付宝订单状态。",
        "wallet.getBalance" => "读取当前用户福德金余额。",
        "network.udp.open" => "打开原生 UDP socket。",
        "network.udp.send" => "通过已打开的 socket 发送 base64 UDP 数据包。",
        "network.udp.broadcast" => "向广播地址发送 base64 UDP 数据包。",
        "network.udp.close" => "关闭指定 UDP socket。",
        "network.interfaces.list" => "列出宿主网络接口和 IP 地址。",
        "network.http.fetch" => {
            "通过宿主 Rust HTTP 客户端请求 HTTP(S) 资源，返回状态、响应头和受限大小的正文。"
        }
        "system.keepAwake" => "请求宿主在任务期间尽量保持唤醒。",
        "hotspot.openSettings" => "打开或引导系统热点设置。",
        "game.runtime.getInfo" => "读取小游戏运行时、渲染后端、输入、帧预算和宿主能力建议。",
        "game.performance.setMode" => "请求宿主切换小游戏性能档和帧预算。",
        "game.surface.request" => "请求宿主分配原生游戏渲染面或返回可用降级路径。",
        "game.surface.release" => "释放宿主分配的原生游戏渲染面。",
        "game.input.setCapture" => "设置键鼠、触控、手柄和指针捕获策略。",
        "game.input.getState" => "读取当前输入捕获能力和最近一次设置。",
        "game.assets.prefetch" => "请求宿主预热并缓存小游戏资源。",
        "game.save.read" => "读取小游戏隔离存档。",
        "game.save.write" => "写入小游戏隔离存档。",
        "game.save.delete" => "删除小游戏隔离存档。",
        "window.fullscreen.request" => "请求宿主进入沉浸式全屏。",
        "window.fullscreen.exit" => "请求宿主退出沉浸式全屏。",
        "window.orientation.lock" => "请求宿主锁定屏幕方向。",
        "window.orientation.unlock" => "请求宿主恢复系统方向。",
        "ui.alert" => "显示宿主原生提示弹窗。",
        "ui.confirm" => "显示宿主原生确认弹窗。",
        "ui.mainButton.set" => "设置宿主底部主按钮状态。",
        "haptics.impact" => "触发冲击触觉反馈。",
        "haptics.notification" => "触发通知触觉反馈。",
        "haptics.selection" => "触发选择触觉反馈。",
        "device.biometrics.authenticate" => "拉起系统生物识别确认。",
        "device.qrScanner.scan" => "拉起原生扫码并返回结果。",
        "cloud.kv.get" => "读取小程序隔离云端 Key-Value。",
        "cloud.kv.set" => "写入小程序隔离云端 Key-Value。",
        "cloud.kv.delete" => "删除小程序隔离云端 Key-Value。",
        "runtime.storage.configure" => "配置 Rust 本地一致性存储快照路径。",
        "runtime.storage.getStatus" => "读取本地一致性存储 generation、collection 和 record 统计。",
        "runtime.storage.put" => "写入本地一致性记录，支持 expectedRevision 乐观并发。",
        "runtime.storage.get" => "读取本地一致性记录。",
        "runtime.storage.delete" => "写入 tombstone 删除记录并推进 generation。",
        "runtime.storage.list" => "列出指定 collection 的本地一致性记录。",
        "runtime.storage.snapshot" => "导出本地一致性存储快照。",
        "runtime.file.register" => "注册 runtime 文件对象并推送 updateFile。",
        "runtime.file.updateState" => "更新 runtime 文件状态并推送 updateFile。",
        "runtime.file.get" => "读取 runtime 文件对象。",
        "runtime.file.list" => "列出 runtime 文件对象。",
        "globalDharma.delivery.enqueue" => "创建全球法布施投递 job 并进入 retry 队列。",
        "globalDharma.delivery.getJob" => "读取全球法布施投递 job。",
        "globalDharma.delivery.listJobs" => "按状态列出全球法布施投递 jobs。",
        "globalDharma.delivery.nextRetry" => "取出到期的全球法布施 retry job 并标记 in_flight。",
        "globalDharma.delivery.markAttempt" => {
            "记录一次投递尝试并决定 sent、failed 或 retry_scheduled。"
        }
        "globalDharma.delivery.recordReceipt" => "记录全球法布施投递回执并推送 receipt update。",
        "globalDharma.delivery.listReceipts" => "列出全球法布施投递回执。",
        "share.chat.send" => "生成富文本卡片并分享给联系人。",
        "bot.sendMessage" => "小程序向宿主机器人发送消息。",
        "bot.postMessage" => "小程序把后台命令进度、结果或错误写回聊天框。",
        "bot.reportCommandResult" => "按 commandId 上报后台命令完成、失败或仍在运行。",
        "bot.takePendingCommands" => "从宿主消息队列拉取聊天命令。",
        "bot.openPanel" => "请求打开小程序面板。",
        "bot.setPanelState" => "设置小程序面板状态。",
        "bot.setCommands" => "向宿主暴露可从聊天触发的命令。",
        "bot.getCommands" => "读取小程序已暴露给宿主聊天输入框的命令列表。",
        "bot.setInputPlaceholder" => "设置宿主聊天输入框占位提示。",
        "bot.setComposerText" => "请求宿主把文字同步到聊天输入框。",
        "bot.getComposerState" => "读取宿主聊天输入框当前文字、占位提示和命令状态。",
        "bot.close" => "请求关闭小程序。",
        "flashcards.createDeck" => "复用宿主背诵闪卡流水线生成卡组。",
        "flashcards.openDeck" => "打开宿主闪卡学习界面。",
        "platformPublish.createDraft" => "复用宿主发布草稿生成能力。",
        "platformPublish.publishDraft" => "请求宿主执行发布草稿流程。",
        "files.pick" => "调用宿主文件选择器。",
        "projects.list" => "列出宿主本地项目目录。",
        "projects.select" => "选择宿主本地项目。",
        "openclaw.status" => "读取本机 OpenClaw 运行状态。",
        "openclaw.restart" => "重启本机 OpenClaw runtime。",
        "openclaw.chat" => "通过宿主 OpenClaw adapter 对话。",
        "desktopControl.executeTool" => "调用宿主桌面控制工具。",
        "localLoopback.fetch" => "通过宿主访问本机回环服务。",
        "fs.writeFile" => "写入小程序私有目录或授权路径。",
        "fs.readFile" => "读取小程序私有目录或授权路径。",
        "shell.execute" => "启动本地命令并将日志流回宿主聊天。",
        "runtime.process.execute" => {
            "通过宿主 Rust runtime 启动本地进程并收集退出码、stdout 和 stderr。"
        }
        "browser.open" => "使用系统浏览器打开 URL。",
        _ => "Fabushi mini app host method.",
    }
}

fn command_protocol_json() -> String {
    r#"{
    "event": "fabushi-miniapp-command",
    "lastCommandCache": "window.__fabushiLastMiniAppCommand",
    "helpers": [
      "window.FabushiMiniApp.bot.onAnyCommand(callback)",
      "window.FabushiMiniApp.bot.onCommand(command, callback)"
    ],
    "defaultCommand": "/start",
    "detail": {
      "id": "stable command id",
      "command": "/start",
      "args": "message text without command prefix",
      "text": "raw chat text",
      "background": true,
      "source": "chat"
    },
    "resultMethods": ["bot.postMessage", "bot.reportCommandResult"]
  }"#
    .to_string()
}

fn capability_model_json() -> String {
    r#"{
    "name": "Rust-backed capability negotiation layer",
    "manifestField": "permissions",
    "requestMethod": "app.requestCapabilities",
    "preinstalledAdapterRequired": true,
    "canCreateNewSystemApi": false,
    "rule": "小程序只能使用 manifest 已声明、宿主已预置 adapter、当前平台可用、并满足信任等级的能力原语。",
    "principle": "小程序不能让宿主凭空获得新的系统 API；新增系统级能力必须先进入 Rust 契约和宿主 adapter，再由 SDK 暴露。",
    "statusMeanings": {
      "granted": "权限已声明、adapter 已预置、平台可用、信任等级满足。",
      "denied": "宿主支持该能力，但当前小程序 manifest 未声明或未获准。",
      "unsupportedPlatform": "宿主已内置该能力原语，但当前平台不可用。",
      "trustRequired": "能力属于高危原生面，只允许受信官方小程序调用。",
      "unknown": "宿主没有内置该能力原语或 adapter。"
    },
    "flow": [
      "小程序在 manifest.permissions 声明需要的能力原语。",
      "运行时调用 app.requestCapabilities 请求协商。",
      "宿主按权限声明、adapter、平台可用性、信任等级返回状态。",
      "小程序只调用 granted 能力；业务逻辑仍留在小程序或其服务侧。"
    ]
  }"#
    .to_string()
}

fn permission_groups_json() -> String {
    let entries = permission_groups()
        .into_iter()
        .map(|(key, values)| format!("    {}: {}", quote(key), string_array_json(&values)))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\n{}\n  }}", entries)
}

fn capabilities_json(capabilities: &[Capability]) -> String {
    let entries = capabilities
        .iter()
        .map(capability_json)
        .collect::<Vec<_>>()
        .join(",\n");
    format!("[\n{}\n  ]", entries)
}

fn capability_json(capability: &Capability) -> String {
    let mut fields = vec![
        format!("      \"id\": {}", quote(capability.id)),
        format!("      \"layer\": {}", quote(capability.layer)),
        format!("      \"native\": {}", capability.native),
        format!("      \"adapter\": {}", quote(capability.adapter)),
        format!("      \"availability\": {}", quote(capability.availability)),
        format!("      \"trust\": {}", quote(capability.trust)),
        format!("      \"risk\": {}", quote(capability.risk)),
        format!(
            "      \"methods\": {}",
            string_array_json(capability.methods)
        ),
    ];
    if let Some(note) = capability.note {
        fields.push(format!("      \"note\": {}", quote(note)));
    }
    format!("    {{\n{}\n    }}", fields.join(",\n"))
}

fn methods_json(methods: &[HostMethod]) -> String {
    let entries = methods
        .iter()
        .map(|method| {
            format!(
                "    {{\n      \"method\": {},\n      \"permission\": {},\n      \"risk\": {},\n      \"description\": {}\n    }}",
                quote(method.method),
                quote(method.permission),
                quote(method.risk),
                quote(method.description)
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("[\n{}\n  ]", entries)
}

fn string_array_json(values: &[&str]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| quote(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn quote(value: &str) -> String {
    let mut output = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control() => output.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => output.push(ch),
        }
    }
    output.push('"');
    output
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        key_block[..32].copy_from_slice(&sha256(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut outer_key_pad = [0u8; 64];
    let mut inner_key_pad = [0u8; 64];
    for index in 0..64 {
        outer_key_pad[index] = key_block[index] ^ 0x5c;
        inner_key_pad[index] = key_block[index] ^ 0x36;
    }

    let mut inner = Vec::with_capacity(64 + data.len());
    inner.extend_from_slice(&inner_key_pad);
    inner.extend_from_slice(data);
    let inner_hash = sha256(&inner);

    let mut outer = Vec::with_capacity(64 + inner_hash.len());
    outer.extend_from_slice(&outer_key_pad);
    outer.extend_from_slice(&inner_hash);
    hex_encode(&sha256(&outer))
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (input.len() as u64) * 8;
    let mut message = input.to_vec();
    message.push(0x80);
    while (message.len() + 8) % 64 != 0 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            let start = index * 4;
            *word = u32::from_be_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut output = [0u8; 32];
    for (index, word) in h.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissions(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn shell_execute_requires_trusted_official_desktop() {
        let context = PolicyContext {
            declared_permissions: permissions(&["app.context", "bot.chat", "shell.execute"]),
            platform: HostPlatform::Macos,
            trusted_official: false,
        };
        let decision = evaluate_method("shell.execute", &context);
        assert!(!decision.allowed);
        assert_eq!(decision.status, PolicyStatus::TrustRequired);
    }

    #[test]
    fn official_desktop_can_use_shell_execute_when_declared() {
        let context = PolicyContext {
            declared_permissions: permissions(&["app.context", "bot.chat", "shell.execute"]),
            platform: HostPlatform::Macos,
            trusted_official: true,
        };
        let decision = evaluate_method("shell.execute", &context);
        assert!(decision.allowed);
        assert_eq!(decision.status, PolicyStatus::Granted);
    }

    #[test]
    fn network_http_fetch_is_declared_native_io_primitive() {
        let context = PolicyContext {
            declared_permissions: permissions(&["app.context", "bot.chat", "network.http"]),
            platform: HostPlatform::Macos,
            trusted_official: false,
        };
        let decision = evaluate_method("network.http.fetch", &context);
        assert!(decision.allowed);
        assert_eq!(decision.status, PolicyStatus::Granted);
    }

    #[test]
    fn runtime_process_execute_requires_trusted_official_desktop() {
        let context = PolicyContext {
            declared_permissions: permissions(&["app.context", "bot.chat", "runtime.process"]),
            platform: HostPlatform::Macos,
            trusted_official: false,
        };
        let decision = evaluate_method("runtime.process.execute", &context);
        assert!(!decision.allowed);
        assert_eq!(decision.status, PolicyStatus::TrustRequired);
    }

    #[test]
    fn desktop_capability_is_unsupported_on_web() {
        let context = PolicyContext {
            declared_permissions: permissions(&["app.context", "bot.chat", "fs.readWrite"]),
            platform: HostPlatform::Web,
            trusted_official: true,
        };
        let decision = evaluate_method("fs.readFile", &context);
        assert!(!decision.allowed);
        assert_eq!(decision.status, PolicyStatus::UnsupportedPlatform);
    }

    #[test]
    fn generated_spec_contains_unique_methods() {
        let mut seen = BTreeSet::new();
        for method in host_methods() {
            assert!(
                seen.insert(method.method),
                "duplicate method {}",
                method.method
            );
        }
    }

    #[test]
    fn game_native_surface_requires_native_io_platform() {
        let context = PolicyContext {
            declared_permissions: permissions(&["app.context", "bot.chat", "game.nativeSurface"]),
            platform: HostPlatform::Web,
            trusted_official: true,
        };
        let decision = evaluate_method("game.surface.request", &context);
        assert!(!decision.allowed);
        assert_eq!(decision.status, PolicyStatus::UnsupportedPlatform);
    }

    #[test]
    fn game_save_is_available_when_declared_on_web() {
        let context = PolicyContext {
            declared_permissions: permissions(&["app.context", "bot.chat", "game.save"]),
            platform: HostPlatform::Web,
            trusted_official: false,
        };
        let decision = evaluate_method("game.save.write", &context);
        assert!(decision.allowed);
        assert_eq!(decision.status, PolicyStatus::Granted);
    }

    #[test]
    fn generated_spec_contains_game_contract() {
        let spec = host_api_spec_json();
        assert!(spec.contains("\"game.nativeSurface\""));
        assert!(spec.contains("\"game.runtime.getInfo\""));
        assert!(spec.contains("\"window.fullscreen.request\""));
    }

    #[test]
    fn generated_spec_contains_runtime_primitives() {
        let spec = host_api_spec_json();
        assert!(spec.contains("\"network.http\""));
        assert!(spec.contains("\"network.http.fetch\""));
        assert!(spec.contains("\"runtime.process\""));
        assert!(spec.contains("\"runtime.process.execute\""));
    }

    #[test]
    fn rust_http_fetch_reads_local_response() {
        use std::io::{Read as _, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test http server");
        let address = listener.local_addr().expect("read listener address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept http request");
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nX-Test: rust\r\n\r\nhello runtime",
                )
                .expect("write http response");
        });

        let response =
            network_http_fetch(&HttpFetchRequest::get(format!("http://{address}/hello")))
                .expect("fetch local response");
        server.join().expect("join test http server");

        assert_eq!(response.status_code, 200);
        assert_eq!(response.body_text_lossy(), "hello runtime");
        assert!(response
            .headers
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("x-test") && value == "rust"));
    }

    #[test]
    fn rust_process_execute_collects_output() {
        #[cfg(windows)]
        let request = {
            let mut request = ProcessExecuteRequest::new("cmd");
            request.arguments = vec!["/C".to_string(), "echo".to_string(), "hello".to_string()];
            request
        };

        #[cfg(not(windows))]
        let request = {
            let mut request = ProcessExecuteRequest::new("/bin/echo");
            request.arguments = vec!["hello".to_string()];
            request
        };

        let output = runtime_process_execute(&request).expect("execute echo");
        assert!(output.ok());
        assert!(output.stdout_text_lossy().contains("hello"));
    }

    #[test]
    fn hmac_sha256_matches_standard_test_vector() {
        let actual = hmac_sha256_hex(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            actual,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn init_data_signature_is_sorted_and_verifiable() {
        let mut fields = BTreeMap::new();
        fields.insert("user".to_string(), "{\"id\":1}".to_string());
        fields.insert("auth_date".to_string(), "1767072000".to_string());
        fields.insert("query_id".to_string(), "abc".to_string());

        let hash = sign_init_data(b"bot-secret", &fields);
        assert!(verify_init_data(b"bot-secret", &fields, &hash));

        fields.insert("query_id".to_string(), "tampered".to_string());
        assert!(!verify_init_data(b"bot-secret", &fields, &hash));
    }
}
