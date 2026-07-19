use clap::Parser;
use clap::Subcommand;
use codex_cli::plugin_cmd::PluginCli;
use codex_cli::plugin_cmd::PluginSubcommand;
use codex_core_plugins::plugin_bundle_archive::pack_plugin_bundle_tar_gz;
use mahayana_plugin_host::LocalPlugin;
use mahayana_product::MahayanaProductClient;
use mahayana_product::redact_secrets;
use mahayana_runtime::mahayana_runtime_close;
use mahayana_runtime::mahayana_runtime_create;
use mahayana_runtime::mahayana_runtime_execute;
use mahayana_runtime::mahayana_runtime_free_string;
use mahayana_runtime::mahayana_runtime_interrupt;
use mahayana_runtime::mahayana_runtime_last_error;
use mahayana_runtime::mahayana_runtime_receive;
use mahayana_runtime::mahayana_runtime_resolve_approval;
use serde_json::Value;
use serde_json::json;
use std::ffi::CStr;
use std::ffi::CString;
use std::fs;
use std::io::Write;
use std::io::{self};
use std::os::raw::c_char;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

mod chat_tui;
mod plugin_dev;
mod plugin_dev_template;

#[derive(Debug, Parser)]
#[command(
    name = "mahayana",
    version,
    about = "大乘 CLI：Codex、插件、MCP 与 Mini App 统一宿主"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    Login {
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    Register {
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    SendCode {
        email: String,
    },
    Logout,
    Auth,
    /// 查看服务端权威的模型 Token 用量与剩余额度。
    Usage,
    Status,
    #[command(alias = "list")]
    Contacts,
    History {
        conversation_id: String,
    },
    Send {
        conversation_id: String,
        #[arg(required = true, trailing_var_arg = true)]
        text: Vec<String>,
    },
    Chat {
        conversation_id: Option<String>,
    },
    Miniapp {
        #[arg(required = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    Marketplace {
        #[command(subcommand)]
        command: MarketplaceCommand,
    },
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    Wallet {
        #[command(subcommand)]
        command: WalletCommand,
    },
    Purchases {
        #[command(subcommand)]
        command: PurchasesCommand,
    },
}

#[derive(Debug, Subcommand)]
enum MarketplaceCommand {
    Browse,
    Search { query: String },
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    /// Non-destructively add a conversational MCP plugin under plugins/<name>.
    Init {
        name: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        title: Option<String>,
    },
    List {
        #[arg(long, short = 'm')]
        marketplace: Option<String>,
        #[arg(long)]
        available: bool,
        #[arg(long)]
        json: bool,
    },
    Info {
        plugin_id: String,
    },
    Install {
        plugin: String,
        #[arg(long, short = 'm')]
        marketplace: Option<String>,
        #[arg(long)]
        json: bool,
        /// Permit bundled stdio/local runtimes after source validation.
        #[arg(long)]
        allow_local: bool,
    },
    Update {
        plugin: String,
        #[arg(long, short = 'm')]
        marketplace: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Uninstall {
        plugin: String,
        #[arg(long, short = 'm')]
        marketplace: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Open {
        plugin_id: String,
    },
    Run {
        plugin_id: String,
        command: String,
        #[arg(long, value_name = "ARGUMENTS")]
        json: Option<String>,
    },
    Validate {
        path: PathBuf,
    },
    Pack {
        path: PathBuf,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
    Publish {
        path: PathBuf,
        #[arg(long)]
        plugin_id: String,
        #[arg(long)]
        version: String,
    },
}

#[derive(Debug, Subcommand)]
enum WalletCommand {
    Balance,
    History,
    TopUp {
        sku: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum PurchasesCommand {
    List,
    Restore,
}

fn main() {
    let arg0_guard = codex_arg0::arg0_dispatch();
    let codex_executable_path = arg0_guard
        .as_ref()
        .and_then(|guard| guard.paths().codex_self_exe.as_deref());
    if let Err(error) = run(codex_executable_path, Cli::parse()) {
        eprintln!("错误：{error}");
        std::process::exit(1);
    }
}

fn run(codex_executable_path: Option<&Path>, cli: Cli) -> Result<(), String> {
    match cli.command {
        Some(CliCommand::Login { args }) => login(args),
        Some(CliCommand::Register { args }) => register(args),
        Some(CliCommand::SendCode { email }) => send_verification_code(vec![email]),
        Some(CliCommand::Logout) => product_command("mahayana.auth.logout", json!({})),
        Some(CliCommand::Auth) => product_command("mahayana.auth.status", json!({})),
        Some(CliCommand::Usage) => model_usage_command(),
        Some(CliCommand::Status) => with_runtime(codex_executable_path, |runtime| {
            print_json(&runtime.execute(json!({"@type": "mahayana.runtime.status"}))?)
        }),
        Some(CliCommand::Contacts) => with_runtime(codex_executable_path, |runtime| {
            let response = runtime.execute(json!({"@type": "mahayana.conversation.list"}))?;
            print_conversations(&response)
        }),
        Some(CliCommand::History { conversation_id }) => {
            with_runtime(codex_executable_path, |runtime| {
                let response = runtime.execute(json!({
                    "@type": "mahayana.conversation.history",
                    "conversationId": conversation_id,
                    "limit": 100,
                }))?;
                print_history(&response)
            })
        }
        Some(CliCommand::Send {
            conversation_id,
            text,
        }) => {
            let text = text.join(" ");
            with_runtime(codex_executable_path, |runtime| {
                send_and_stream(runtime, &conversation_id, &text)
            })
        }
        Some(CliCommand::Chat { conversation_id }) => {
            with_runtime(codex_executable_path, |runtime| {
                interactive_chat(runtime, conversation_id)
            })
        }
        Some(CliCommand::Miniapp { args }) => miniapp_command(codex_executable_path, args),
        Some(CliCommand::Marketplace { command }) => marketplace_command(command),
        Some(CliCommand::Plugin { command }) => plugin_command(codex_executable_path, command),
        Some(CliCommand::Wallet { command }) => wallet_command(command),
        Some(CliCommand::Purchases { command }) => purchases_command(command),
        None => with_runtime(codex_executable_path, |runtime| {
            interactive_chat(runtime, None)
        }),
    }
}

fn marketplace_command(command: MarketplaceCommand) -> Result<(), String> {
    let client = MahayanaProductClient::default();
    let response = match command {
        MarketplaceCommand::Browse => client.marketplace_browse(None),
        MarketplaceCommand::Search { query } => client.marketplace_browse(Some(&query)),
    }
    .map_err(|error| error.to_string())?;
    print_json(&response)
}

fn model_usage_command() -> Result<(), String> {
    let usage = MahayanaProductClient::default()
        .model_usage()
        .map_err(|error| error.to_string())?;
    let response = serde_json::to_value(usage).map_err(|error| error.to_string())?;
    print_json(&response)
}

fn plugin_command(
    codex_executable_path: Option<&Path>,
    command: PluginCommand,
) -> Result<(), String> {
    match command {
        PluginCommand::Init {
            name,
            repository,
            title,
        } => print_json(&plugin_dev::init_repository(
            &repository,
            &name,
            title.as_deref(),
        )?),
        PluginCommand::List {
            marketplace,
            available,
            json,
        } => {
            let mut args = vec!["list".to_string()];
            if let Some(marketplace) = marketplace {
                args.extend(["--marketplace".into(), marketplace]);
            }
            if json || available {
                args.push("--json".into());
            }
            if available {
                args.push("--available".into());
            }
            run_codex_plugin(args)
        }
        PluginCommand::Info { plugin_id } => {
            let response = MahayanaProductClient::default()
                .marketplace_browse(Some(&plugin_id))
                .map_err(|error| error.to_string())?;
            print_json(&response)
        }
        PluginCommand::Install {
            plugin,
            marketplace,
            json,
            allow_local,
        } => {
            if marketplace.is_none() && plugin_dev::looks_like_github_source(&plugin) {
                let remote = plugin_dev::validate_github_source(&plugin)?;
                if remote.has_local_runtimes && !allow_local {
                    return Err(
                        "仓库包含本地 CLI/stdio runtime；请审查源码后使用 --allow-local 单独授权"
                            .into(),
                    );
                }
                let mut marketplace_args =
                    vec!["marketplace".to_string(), "add".to_string(), plugin.clone()];
                if json {
                    marketplace_args.push("--json".into());
                }
                run_codex_plugin(marketplace_args)?;
                for plugin_name in remote.plugins {
                    let mut args = vec![
                        "add".to_string(),
                        plugin_name,
                        "--marketplace".into(),
                        remote.marketplace.clone(),
                    ];
                    if json {
                        args.push("--json".into());
                    }
                    run_codex_plugin(args)?;
                }
                return Ok(());
            }
            let mut args = vec!["add".to_string(), plugin];
            if let Some(marketplace) = marketplace {
                args.extend(["--marketplace".into(), marketplace]);
            }
            if json {
                args.push("--json".into());
            }
            run_codex_plugin(args)
        }
        PluginCommand::Update {
            plugin,
            marketplace,
            json,
        } => {
            let mut args = vec!["add".to_string(), plugin];
            if let Some(marketplace) = marketplace {
                args.extend(["--marketplace".into(), marketplace]);
            }
            if json {
                args.push("--json".into());
            }
            run_codex_plugin(args)
        }
        PluginCommand::Uninstall {
            plugin,
            marketplace,
            json,
        } => {
            let mut args = vec!["remove".to_string(), plugin];
            if let Some(marketplace) = marketplace {
                args.extend(["--marketplace".into(), marketplace]);
            }
            if json {
                args.push("--json".into());
            }
            run_codex_plugin(args)
        }
        PluginCommand::Open { plugin_id } => with_runtime(codex_executable_path, |runtime| {
            interactive_chat(runtime, Some(format!("miniapp:{plugin_id}")))
        }),
        PluginCommand::Run {
            plugin_id,
            command,
            json,
        } => {
            let arguments = json
                .as_deref()
                .map(serde_json::from_str::<Value>)
                .transpose()
                .map_err(|error| format!("--json 必须是合法 JSON：{error}"))?
                .unwrap_or_else(|| json!({}));
            if !arguments.is_object() {
                return Err("--json 必须是 MCP Tool 参数对象".into());
            }
            let message = format!("/{command} {arguments}");
            with_runtime(codex_executable_path, |runtime| {
                send_and_stream(runtime, &format!("miniapp:{plugin_id}"), &message)
            })
        }
        PluginCommand::Validate { path } => print_json(&plugin_dev::validate_path(&path)?),
        PluginCommand::Pack { path, output } => {
            LocalPlugin::load(&path).map_err(|error| error.to_string())?;
            let archive = pack_plugin_bundle_tar_gz(&path, 50 * 1024 * 1024)
                .map_err(|error| error.to_string())?;
            let output = output.unwrap_or_else(|| path.with_extension("tar.gz"));
            fs::write(&output, archive).map_err(|error| error.to_string())?;
            println!("{}", output.display());
            Ok(())
        }
        PluginCommand::Publish {
            path,
            plugin_id,
            version,
        } => {
            LocalPlugin::load(&path).map_err(|error| error.to_string())?;
            let archive = pack_plugin_bundle_tar_gz(&path, 50 * 1024 * 1024)
                .map_err(|error| error.to_string())?;
            let response = MahayanaProductClient::default()
                .publish_plugin(&plugin_id, &version, archive)
                .map_err(|error| error.to_string())?;
            print_json(&response)
        }
    }
}

fn run_codex_plugin(args: Vec<String>) -> Result<(), String> {
    let cli = PluginCli::try_parse_from(std::iter::once("codex plugin".to_string()).chain(args))
        .map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime
        .block_on(async move {
            let overrides = cli
                .config_overrides
                .parse_overrides()
                .map_err(anyhow::Error::msg)?;
            match cli.subcommand {
                PluginSubcommand::Add(args) => {
                    codex_cli::plugin_cmd::run_plugin_add(overrides, args).await
                }
                PluginSubcommand::List(args) => {
                    codex_cli::plugin_cmd::run_plugin_list(overrides, args).await
                }
                PluginSubcommand::Marketplace(marketplace) => marketplace.run().await,
                PluginSubcommand::Remove(args) => {
                    codex_cli::plugin_cmd::run_plugin_remove(overrides, args).await
                }
            }
        })
        .map_err(|error| error.to_string())
}

fn wallet_command(command: WalletCommand) -> Result<(), String> {
    let client = MahayanaProductClient::default();
    let response = match command {
        WalletCommand::Balance => {
            serde_json::to_value(client.wallet_balance().map_err(|error| error.to_string())?)
        }
        WalletCommand::History => serde_json::to_value(
            client
                .wallet_history(None)
                .map_err(|error| error.to_string())?,
        ),
        WalletCommand::TopUp {
            sku,
            idempotency_key,
        } => {
            let idempotency_key =
                idempotency_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            Ok(client
                .wallet_top_up(&sku, &idempotency_key)
                .map_err(|error| error.to_string())?)
        }
    }
    .map_err(|error| error.to_string())?;
    print_json(&response)
}

fn purchases_command(command: PurchasesCommand) -> Result<(), String> {
    let client = MahayanaProductClient::default();
    let response = match command {
        PurchasesCommand::List => {
            serde_json::to_value(client.purchases(None).map_err(|error| error.to_string())?)
        }
        PurchasesCommand::Restore => serde_json::to_value(
            client
                .restore_purchases()
                .map_err(|error| error.to_string())?,
        ),
    }
    .map_err(|error| error.to_string())?;
    print_json(&response)
}

fn miniapp_command(codex_executable_path: Option<&Path>, args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("registry") => product_command("mahayana.miniapps.registry", json!({})),
        Some("chat") => {
            let miniapp_id = args
                .get(1)
                .ok_or_else(|| "用法：mahayana miniapp chat <miniapp-id> <消息>".to_string())?;
            let message = args.get(2..).unwrap_or_default().join(" ");
            if message.trim().is_empty() {
                return Err("小程序消息不能为空".into());
            }
            with_runtime(codex_executable_path, |runtime| {
                send_and_stream(runtime, &format!("miniapp:{miniapp_id}"), &message)
            })
        }
        _ => Err("用法：mahayana miniapp registry|chat".into()),
    }
}

fn login(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("alipay") => alipay_login(),
        Some("password") => password_login(&args[1..]),
        Some(other) => Err(format!(
            "未知登录方式：{other}。使用 mahayana login alipay 或 mahayana login password <用户名>"
        )),
    }
}

fn password_login(args: &[String]) -> Result<(), String> {
    let username = args
        .first()
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "用法：mahayana login password <用户名> [--password-stdin]".to_string())?;
    let password = read_password(args.get(1).map(String::as_str))?;
    let response = MahayanaProductClient::default()
        .execute(
            "mahayana.auth.password.login",
            &json!({"username": username, "password": password}),
        )
        .map_err(|error| error.to_string())?;
    if response.get("sessionStored").and_then(Value::as_bool) != Some(true) {
        return Err("官方登录没有返回可保存的软件会话".into());
    }
    println!("登录成功。App 与 CLI 将共用同一大乘账号会话；无需 OpenAI 登录。");
    Ok(())
}

fn send_verification_code(args: Vec<String>) -> Result<(), String> {
    let email = args
        .first()
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "用法：mahayana send-code <邮箱>".to_string())?;
    product_command(
        "mahayana.auth.verification.send",
        json!({"email": email, "type": "register"}),
    )
}

fn register(args: Vec<String>) -> Result<(), String> {
    if args.len() < 3 {
        return Err("用法：mahayana register <用户名> <邮箱> <验证码> [--password-stdin]".into());
    }
    let password = read_password(args.get(3).map(String::as_str))?;
    product_command(
        "mahayana.auth.register",
        json!({
            "username": args[0],
            "email": args[1],
            "verificationCode": args[2],
            "password": password,
        }),
    )
}

fn read_password(mode: Option<&str>) -> Result<String, String> {
    let password = if mode == Some("--password-stdin") {
        read_line()?
    } else if mode.is_none() {
        rpassword::prompt_password("密码：").map_err(|error| error.to_string())?
    } else {
        return Err("密码不能放在命令参数中；请省略密码或使用 --password-stdin".into());
    };
    let password = password.trim().to_string();
    if password.is_empty() {
        return Err("密码不能为空".into());
    }
    Ok(password)
}

fn alipay_login() -> Result<(), String> {
    let client = MahayanaProductClient::default();
    let authorization = client
        .execute("mahayana.auth.alipay.start", &json!({"platform": "cli"}))
        .map_err(|error| error.to_string())?;
    let url = authorization
        .get("loginUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| "登录服务没有返回支付宝授权地址".to_string())?;
    let state = authorization
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| "登录服务没有返回会话状态".to_string())?
        .to_string();
    println!("请在浏览器完成支付宝登录：\n{url}");
    open_browser(url);
    for _ in 0..150 {
        thread::sleep(Duration::from_secs(2));
        let response = client
            .execute("mahayana.auth.alipay.poll", &json!({"state": state}))
            .map_err(|error| error.to_string())?;
        match response.get("status").and_then(Value::as_str) {
            Some("complete") => {
                println!("登录成功。软件会话已安全保存；Codex 不需要 OpenAI 登录。");
                return Ok(());
            }
            Some("expired") | Some("failed") => {
                return Err(format!("支付宝登录未完成：{}", redact_secrets(&response)));
            }
            _ => {}
        }
    }
    Err("支付宝登录等待超时，请重试".into())
}

fn product_command(request_type: &str, request: Value) -> Result<(), String> {
    let response = MahayanaProductClient::default()
        .execute(request_type, &request)
        .map_err(|error| error.to_string())?;
    print_json(&redact_secrets(&response))
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut value = Command::new("cmd");
        value.args(["/C", "start", ""]);
        value
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return;
    let _ = command.arg(url).spawn();
}

fn with_runtime(
    codex_executable_path: Option<&Path>,
    operation: impl FnOnce(&RuntimeHandle) -> Result<(), String>,
) -> Result<(), String> {
    let runtime = RuntimeHandle::create(codex_executable_path)?;
    operation(&runtime)
}

struct RuntimeHandle(u64);

impl RuntimeHandle {
    fn create(codex_executable_path: Option<&Path>) -> Result<Self, String> {
        Self::create_with_config(json!({
            "codexExecutablePath": codex_executable_path,
            "hostPlatform": "cli",
        }))
    }

    fn create_with_config(config: Value) -> Result<Self, String> {
        let config = serde_json::to_string(&config).map_err(|error| error.to_string())?;
        let config = CString::new(config).map_err(|error| error.to_string())?;
        let id = unsafe { mahayana_runtime_create(config.as_ptr()) };
        if id == 0 {
            let error = unsafe { take_json(mahayana_runtime_last_error()) }?;
            return Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("本地 Runtime 创建失败")
                .to_string());
        }
        Ok(Self(id))
    }

    fn execute(&self, command: Value) -> Result<Value, String> {
        let command = CString::new(command.to_string()).map_err(|error| error.to_string())?;
        let response = unsafe { take_json(mahayana_runtime_execute(self.0, command.as_ptr())) }?;
        unwrap_ffi(response)
    }

    fn receive(&self, timeout_ms: u64) -> Result<Option<Value>, String> {
        let response = unsafe { take_json(mahayana_runtime_receive(self.0, timeout_ms)) }?;
        let data = unwrap_ffi(response)?;
        if data.is_null() {
            Ok(None)
        } else {
            Ok(Some(data))
        }
    }

    fn resolve_approval(&self, approval_id: &str, decision: &str) -> Result<(), String> {
        let request =
            CString::new(json!({"approvalId": approval_id, "decision": decision}).to_string())
                .map_err(|error| error.to_string())?;
        let response =
            unsafe { take_json(mahayana_runtime_resolve_approval(self.0, request.as_ptr())) }?;
        unwrap_ffi(response).map(|_| ())
    }

    fn interrupt(&self, operation_id: &str) -> Result<(), String> {
        let request = CString::new(json!({"operationId": operation_id}).to_string())
            .map_err(|error| error.to_string())?;
        let response = unsafe { take_json(mahayana_runtime_interrupt(self.0, request.as_ptr())) }?;
        unwrap_ffi(response).map(|_| ())
    }
}

impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        unsafe {
            let pointer = mahayana_runtime_close(self.0);
            mahayana_runtime_free_string(pointer);
        }
    }
}

fn interactive_chat(runtime: &RuntimeHandle, selected: Option<String>) -> Result<(), String> {
    chat_tui::run(runtime, selected)
}

fn send_and_stream(
    runtime: &RuntimeHandle,
    conversation_id: &str,
    text: &str,
) -> Result<(), String> {
    send_and_collect(runtime, conversation_id, text, true).map(|_| ())
}

fn send_and_collect(
    runtime: &RuntimeHandle,
    conversation_id: &str,
    text: &str,
    print_stream: bool,
) -> Result<String, String> {
    let accepted = runtime.execute(json!({
        "@type": "mahayana.conversation.send",
        "conversationId": conversation_id,
        "text": text,
    }))?;
    let operation_id = accepted
        .get("operationId")
        .and_then(Value::as_str)
        .ok_or_else(|| "Runtime 没有返回操作编号".to_string())?;
    let mut assistant = String::new();
    let mut usage_summary = None;
    loop {
        let Some(event) = runtime.receive(30_000)? else {
            continue;
        };
        match event.get("@type").and_then(Value::as_str) {
            Some("mahayana.message.delta")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assistant.push_str(delta);
                if print_stream {
                    print!("{delta}");
                    io::stdout().flush().map_err(|error| error.to_string())?;
                }
            }
            Some("mahayana.message.completed")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
                if event.get("message").and_then(|value| value.get("role"))
                    != Some(&Value::String("user".into()))
                {
                    if let Some(text) = event
                        .get("message")
                        .and_then(|value| value.get("text"))
                        .and_then(Value::as_str)
                    {
                        assistant = text.to_string();
                    }
                    if print_stream {
                        println!();
                    }
                }
            }
            Some("mahayana.approval.requested")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
                handle_approval(runtime, &event)?;
            }
            Some("mahayana.model.usage.updated")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
                usage_summary = format_usage_summary(&event);
            }
            Some("mahayana.operation.completed")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
                if print_stream && let Some(usage) = usage_summary {
                    eprintln!("{usage}");
                }
                return Ok(assistant);
            }
            Some("mahayana.operation.failed")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
                return Err(event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("操作失败")
                    .to_string());
            }
            _ => {}
        }
    }
}

fn format_usage_summary(event: &Value) -> Option<String> {
    let usage = event.get("usage")?;
    let last = usage.get("last")?;
    let total = last.get("totalTokens").and_then(Value::as_i64)?;
    let input = last
        .get("inputTokens")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let cached = last
        .get("cachedInputTokens")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let output = last
        .get("outputTokens")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let reasoning = last
        .get("reasoningOutputTokens")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    Some(format!(
        "本次模型用量：{total} tokens（输入 {input}，缓存输入 {cached}，输出 {output}，推理 {reasoning}）"
    ))
}

fn handle_approval(runtime: &RuntimeHandle, event: &Value) -> Result<(), String> {
    let approval_id = event
        .get("approvalId")
        .and_then(Value::as_str)
        .ok_or_else(|| "审批事件缺少编号".to_string())?;
    println!(
        "\n审批请求：{}\n{}",
        event
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Codex 操作"),
        event.get("details").cloned().unwrap_or(Value::Null)
    );
    print!("允许？[y] 本次 / [a] 本会话 / [n] 拒绝 / [c] 取消：");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let decision = match read_line()?.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => "accept",
        "a" | "always" => "acceptForSession",
        "c" | "cancel" => "cancel",
        _ => "decline",
    };
    runtime.resolve_approval(approval_id, decision)
}

fn print_conversations(response: &Value) -> Result<(), String> {
    let conversations = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Runtime 没有返回联系人列表".to_string())?;
    print_conversation_rows(conversations);
    Ok(())
}

fn print_conversation_rows(conversations: &[Value]) {
    for (index, conversation) in conversations.iter().enumerate() {
        let marker = if conversation
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "★"
        } else {
            " "
        };
        println!(
            "{:>2}. {marker} {:<24} {}",
            index + 1,
            conversation
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("未命名"),
            conversation
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
    }
}

fn print_history(response: &Value) -> Result<(), String> {
    let messages = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Runtime 没有返回消息历史".to_string())?;
    for message in messages {
        println!(
            "[{}] {}",
            message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn read_line() -> Result<String, String> {
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|error| error.to_string())?;
    Ok(value)
}

fn print_json(value: &Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn unwrap_ffi(response: Value) -> Result<Value, String> {
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(response.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Runtime 调用失败")
            .to_string())
    }
}

unsafe fn take_json(pointer: *mut c_char) -> Result<Value, String> {
    if pointer.is_null() {
        return Err("Runtime 返回了空指针".into());
    }
    let source = unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|error| error.to_string())?
        .to_string();
    unsafe { mahayana_runtime_free_string(pointer) };
    serde_json::from_str(&source).map_err(|error| error.to_string())
}
