use mahayana_product::MahayanaProductClient;
use mahayana_product::redact_secrets;
use mahayana_runtime::mahayana_runtime_close;
use mahayana_runtime::mahayana_runtime_create;
use mahayana_runtime::mahayana_runtime_execute;
use mahayana_runtime::mahayana_runtime_free_string;
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
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

fn main() {
    let arg0_guard = codex_arg0::arg0_dispatch();
    let codex_executable_path = arg0_guard
        .as_ref()
        .and_then(|guard| guard.paths().codex_self_exe.as_deref());
    if let Err(error) = run(codex_executable_path) {
        eprintln!("错误：{error}");
        std::process::exit(1);
    }
}

fn run(codex_executable_path: Option<&Path>) -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("login") => login(args.collect()),
        Some("register") => register(args.collect()),
        Some("send-code") => send_verification_code(args.collect()),
        Some("logout") => product_command("mahayana.auth.logout", json!({})),
        Some("auth") => product_command("mahayana.auth.status", json!({})),
        Some("status") => with_runtime(codex_executable_path, |runtime| {
            print_json(&runtime.execute(json!({"@type": "mahayana.runtime.status"}))?)
        }),
        Some("contacts") | Some("list") => with_runtime(codex_executable_path, |runtime| {
            let response = runtime.execute(json!({"@type": "mahayana.conversation.list"}))?;
            print_conversations(&response)
        }),
        Some("history") => {
            let conversation_id = args
                .next()
                .ok_or_else(|| "用法：mahayana history <conversation-id>".to_string())?;
            with_runtime(codex_executable_path, |runtime| {
                let response = runtime.execute(json!({
                    "@type": "mahayana.conversation.history",
                    "conversationId": conversation_id,
                    "limit": 100,
                }))?;
                print_history(&response)
            })
        }
        Some("send") => {
            let conversation_id = args
                .next()
                .ok_or_else(|| "用法：mahayana send <conversation-id> <message>".to_string())?;
            let text = args.collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return Err("消息不能为空".into());
            }
            with_runtime(codex_executable_path, |runtime| {
                send_and_stream(runtime, &conversation_id, &text)
            })
        }
        Some("chat") => {
            let conversation_id = args.next();
            with_runtime(codex_executable_path, |runtime| {
                interactive_chat(runtime, conversation_id)
            })
        }
        Some("miniapp") => miniapp_command(codex_executable_path, args.collect()),
        Some("help") | Some("--help") | Some("-h") => {
            usage();
            Ok(())
        }
        Some(other) => Err(format!("未知命令：{other}。运行 mahayana help 查看帮助。")),
        None => with_runtime(codex_executable_path, |runtime| {
            interactive_chat(runtime, None)
        }),
    }
}

fn usage() {
    println!(
        "大乘 CLI\n\n\
         mahayana login [alipay]                支付宝登录\n\
         mahayana login password <用户名>       官方账号登录（安全读取密码）\n\
         mahayana send-code <邮箱>              发送注册验证码\n\
         mahayana register <用户名> <邮箱> <码>  注册官方账号（安全读取密码）\n\
         mahayana contacts                      查看 AI、Telegram、好友和小程序\n\
         mahayana chat [conversation-id]        进入联系人式会话\n\
         mahayana send <conversation-id> <文本> 发送并流式等待回复\n\
         mahayana history <conversation-id>     查看历史\n\
         mahayana miniapp generate <目录> <需求> 机器人之父在本地生成小程序\n\
         mahayana miniapp inspect <目录>         检查小程序与权限\n\
         mahayana miniapp publish <目录>         发布到个人沙箱（无需登录）\n\
         mahayana miniapp registry              查看可用小程序\n\
         mahayana miniapp execute '<JSON>'      调用内置 Rust 小程序 Runtime\n\
         mahayana status                        查看本地 Runtime 状态\n\
         mahayana logout                        退出软件账号"
    );
}

fn miniapp_command(codex_executable_path: Option<&Path>, args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("generate") => {
            let workspace = args
                .get(1)
                .map(PathBuf::from)
                .ok_or_else(|| "用法：mahayana miniapp generate <目录> <需求>".to_string())?;
            let prompt = args.get(2..).unwrap_or_default().join(" ");
            if prompt.trim().is_empty() {
                return Err("小程序需求不能为空".into());
            }
            generate_miniapp(codex_executable_path, &workspace, &prompt)
        }
        Some("inspect") => {
            let workspace = args
                .get(1)
                .map(PathBuf::from)
                .ok_or_else(|| "用法：mahayana miniapp inspect <目录>".to_string())?;
            print_json(&inspect_miniapp(&workspace)?)
        }
        Some("publish") => {
            let workspace = args.get(1).map(PathBuf::from).ok_or_else(|| {
                "用法：mahayana miniapp publish <目录> [--submit-review]".to_string()
            })?;
            publish_miniapp(&workspace, args.iter().any(|arg| arg == "--submit-review"))
        }
        Some("registry") => product_command("mahayana.miniapps.registry", json!({})),
        Some("execute") => {
            let source = args
                .get(1)
                .ok_or_else(|| "用法：mahayana miniapp execute '<JSON>'".to_string())?;
            let request: Value = serde_json::from_str(source)
                .map_err(|error| format!("小程序 Runtime JSON 无效：{error}"))?;
            if !request.is_object() {
                return Err("小程序 Runtime 请求必须是 JSON 对象".into());
            }
            let response = fabushi_miniapp_runtime::execute_json(&request.to_string())?;
            let response: Value =
                serde_json::from_str(&response).map_err(|error| error.to_string())?;
            print_json(&response)
        }
        Some("spec") => {
            let spec: Value = serde_json::from_str(&fabushi_miniapp_core::host_api_spec_json())
                .map_err(|error| error.to_string())?;
            print_json(&spec)
        }
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
        _ => {
            Err("用法：mahayana miniapp generate|inspect|publish|registry|execute|spec|chat".into())
        }
    }
}

fn generate_miniapp(
    codex_executable_path: Option<&Path>,
    workspace: &Path,
    user_prompt: &str,
) -> Result<(), String> {
    fs::create_dir_all(workspace).map_err(|error| error.to_string())?;
    let workspace = fs::canonicalize(workspace).map_err(|error| error.to_string())?;
    let instructions_path = workspace.join("AGENTS.md");
    if !instructions_path.exists() {
        fs::write(
            &instructions_path,
            concat!(
                "# 大乘小程序工作区\n\n",
                "机器人之父在此目录开发一个可直接运行的小程序。\n",
                "入口必须是完整、自包含的 index.html，CSS 与 JavaScript 必须内联。\n",
                "禁止外链脚本、eval、Authorization token、桌面桥 token 和 OpenClaw token。\n",
                "宿主能力只能通过 window.FabushiMiniApp.invoke(method, params) 调用。\n",
            ),
        )
        .map_err(|error| error.to_string())?;
    }

    let runtime = RuntimeHandle::create_in_workspace(codex_executable_path, &workspace)?;
    let agent_prompt = format!(
        "请作为机器人之父完成以下小程序需求：\n{user_prompt}\n\n\
         必须直接使用 Codex 文件工具在当前工作区创建或修改 index.html；\
         它必须是完整、自包含、无需构建的 HTML，CSS 和 JavaScript 全部内联。\
         禁止外链脚本、eval 和任何 token。需要宿主能力时只调用 \
         window.FabushiMiniApp.invoke(method, params)。完成后检查文件，最终只简要报告。"
    );
    let mut assistant = send_and_collect(
        &runtime,
        "miniapp:official.bot-father",
        &agent_prompt,
        false,
    )?;

    let entry_path = workspace.join("index.html");
    if !entry_path.exists() {
        if extract_html(&assistant).is_none() {
            assistant = send_and_collect(
                &runtime,
                "miniapp:official.bot-father",
                &format!(
                    "刚才的源码被截断。请重新实现需求“{user_prompt}”，只返回一行极简 HTML，\
                     总长度必须少于 1800 个字符。必须从 <!DOCTYPE html> 开始并以 </html> 结束；\
                     只保留完成功能必需的 HTML、少量内联 CSS 和 JavaScript，不要注释、\
                     不要 Markdown 围栏、不要解释、不要调用任何非必需宿主能力。"
                ),
                false,
            )?;
        }
        if let Some(html) = extract_html(&assistant) {
            fs::write(&entry_path, html).map_err(|error| error.to_string())?;
        }
    }
    if !entry_path.exists() {
        return Err("机器人之父没有在工作区生成 index.html".into());
    }

    let manifest_path = workspace.join("manifest.json");
    if !manifest_path.exists() {
        let title = title_from_prompt(user_prompt);
        write_json_file(
            &manifest_path,
            &json!({
                "schemaVersion": 1,
                "miniAppId": format!("local.{}", now_millis()),
                "title": title,
                "subtitle": "机器人之父生成的个人沙箱小程序",
                "version": "0.0.1",
                "permissions": ["app.context", "bot.chat"],
                "entry": "index.html",
                "prompt": user_prompt,
            }),
        )?;
    }
    let inspection = inspect_miniapp(&workspace)?;
    if inspection.get("passed").and_then(Value::as_bool) != Some(true) {
        print_json(&inspection)?;
        return Err("小程序生成完成，但本地安全检查未通过".into());
    }
    print_json(&json!({
        "@type": "mahayana.miniapp.generated",
        "workspace": workspace,
        "entry": entry_path,
        "manifest": manifest_path,
        "inspection": inspection,
    }))
}

fn inspect_miniapp(workspace: &Path) -> Result<Value, String> {
    let workspace = fs::canonicalize(workspace).map_err(|error| error.to_string())?;
    let entry_path = workspace.join("index.html");
    let manifest_path = workspace.join("manifest.json");
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let html = match fs::read_to_string(&entry_path) {
        Ok(html) if html.len() <= 2 * 1024 * 1024 => html,
        Ok(_) => {
            errors.push("index.html 超过 2 MiB".to_string());
            String::new()
        }
        Err(error) => {
            errors.push(format!("无法读取 index.html：{error}"));
            String::new()
        }
    };
    let lower = html.to_ascii_lowercase();
    if !lower.contains("<html") || !lower.contains("<body") || !lower.contains("</html>") {
        errors.push("index.html 不是完整 HTML 文档".to_string());
    }
    for (needle, message) in [
        ("<script src", "禁止加载外部脚本"),
        ("eval(", "禁止使用 eval"),
        ("authorization:", "禁止在小程序中内嵌 Authorization"),
        ("bearer ", "禁止在小程序中内嵌 Bearer token"),
        ("desktop bridge token", "禁止在小程序中内嵌桌面桥 token"),
        ("openclaw token", "禁止在小程序中内嵌 OpenClaw token"),
    ] {
        if lower.contains(needle) {
            errors.push(message.to_string());
        }
    }

    let manifest = match fs::read_to_string(&manifest_path) {
        Ok(source) => serde_json::from_str::<Value>(&source)
            .map_err(|error| format!("manifest.json 无效：{error}"))?,
        Err(error) => {
            errors.push(format!("无法读取 manifest.json：{error}"));
            json!({})
        }
    };
    let permissions = manifest
        .get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let known_permissions = fabushi_miniapp_core::capabilities()
        .into_iter()
        .map(|capability| capability.id)
        .collect::<Vec<_>>();
    for permission in &permissions {
        let Some(permission) = permission.as_str() else {
            errors.push("manifest.permissions 必须都是字符串".to_string());
            continue;
        };
        if !known_permissions.contains(&permission) {
            warnings.push(format!("宿主没有名为 {permission} 的能力"));
        }
    }
    Ok(json!({
        "@type": "mahayana.miniapp.inspection",
        "passed": errors.is_empty(),
        "workspace": workspace,
        "entry": entry_path,
        "manifest": manifest,
        "hostApiVersion": fabushi_miniapp_core::HOST_API_VERSION,
        "hostSdkVersion": fabushi_miniapp_core::HOST_SDK_VERSION,
        "errors": errors,
        "warnings": warnings,
    }))
}

fn publish_miniapp(workspace: &Path, submit_review: bool) -> Result<(), String> {
    let inspection = inspect_miniapp(workspace)?;
    if inspection.get("passed").and_then(Value::as_bool) != Some(true) {
        print_json(&inspection)?;
        return Err("拒绝发布：本地安全检查未通过".into());
    }
    let workspace = fs::canonicalize(workspace).map_err(|error| error.to_string())?;
    let manifest_path = workspace.join("manifest.json");
    let mut manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let source_html =
        fs::read_to_string(workspace.join("index.html")).map_err(|error| error.to_string())?;
    let request = json!({
        "title": manifest.get("title").and_then(Value::as_str).unwrap_or("个人小程序"),
        "subtitle": manifest.get("subtitle").and_then(Value::as_str),
        "prompt": manifest.get("prompt").and_then(Value::as_str),
        "version": manifest.get("version").and_then(Value::as_str).unwrap_or("0.0.1"),
        "permissions": manifest.get("permissions").cloned().unwrap_or_else(|| json!([])),
        "sourceHtml": source_html,
        "submitReview": submit_review,
    });
    let response = MahayanaProductClient::default()
        .execute("mahayana.miniapp.sandbox.publish", &request)
        .map_err(|error| error.to_string())?;
    if let Some(object) = manifest.as_object_mut() {
        if let Some(miniapp_id) = response.get("miniAppId").cloned() {
            object.insert("miniAppId".to_string(), miniapp_id);
        }
        object.insert("published".to_string(), redact_secrets(&response));
    }
    write_json_file(&manifest_path, &manifest)?;
    print_json(&redact_secrets(&response))
}

fn extract_html(source: &str) -> Option<&str> {
    let lower = source.to_ascii_lowercase();
    let start = lower
        .find("<!doctype html")
        .or_else(|| lower.find("<html"))?;
    let end = lower[start..].find("</html>")? + start + "</html>".len();
    source.get(start..end)
}

fn title_from_prompt(prompt: &str) -> String {
    let title = prompt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(18)
        .collect::<String>();
    if title.is_empty() {
        "个人小程序".to_string()
    } else {
        title
    }
}

fn write_json_file(path: &Path, value: &Value) -> Result<(), String> {
    let source = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, source).map_err(|error| error.to_string())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
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
        }))
    }

    fn create_in_workspace(
        codex_executable_path: Option<&Path>,
        workspace: &Path,
    ) -> Result<Self, String> {
        Self::create_with_config(json!({
            "codexExecutablePath": codex_executable_path,
            "cwd": workspace,
            "workspaceRoots": [workspace],
            "dataDir": mahayana_product::default_mahayana_home(),
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
    let response = runtime.execute(json!({"@type": "mahayana.conversation.list"}))?;
    let conversations = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Runtime 没有返回联系人列表".to_string())?;
    let conversation_id = if let Some(selected) = selected {
        selected
    } else {
        print_conversation_rows(conversations);
        print!("请选择联系人编号：");
        io::stdout().flush().map_err(|error| error.to_string())?;
        let selection = read_line()?;
        let index = selection
            .trim()
            .parse::<usize>()
            .map_err(|_| "联系人编号无效".to_string())?;
        conversations
            .get(index.saturating_sub(1))
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| "联系人编号不存在".to_string())?
            .to_string()
    };
    println!("已进入 {conversation_id}。输入 /contacts 切换，/history 查看历史，/quit 退出。");
    loop {
        print!("> ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        let text = read_line()?;
        let text = text.trim();
        match text {
            "" => continue,
            "/quit" | "/exit" => return Ok(()),
            "/contacts" => return interactive_chat(runtime, None),
            "/history" => {
                let history = runtime.execute(json!({
                    "@type": "mahayana.conversation.history",
                    "conversationId": conversation_id,
                    "limit": 100,
                }))?;
                print_history(&history)?;
            }
            _ => send_and_stream(runtime, &conversation_id, text)?,
        }
    }
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
            Some("mahayana.operation.completed")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
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

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
