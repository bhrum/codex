use mahayana_product::{MahayanaProductClient, redact_secrets};
use mahayana_runtime::{
    mahayana_runtime_close, mahayana_runtime_create, mahayana_runtime_execute,
    mahayana_runtime_free_string, mahayana_runtime_last_error, mahayana_runtime_receive,
    mahayana_runtime_resolve_approval,
};
use serde_json::{Value, json};
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::raw::c_char;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("错误：{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("login") => login(),
        Some("logout") => product_command("mahayana.auth.logout", json!({})),
        Some("auth") => product_command("mahayana.auth.status", json!({})),
        Some("status") => with_runtime(|runtime| {
            print_json(&runtime.execute(json!({"@type": "mahayana.runtime.status"}))?)
        }),
        Some("contacts") | Some("list") => with_runtime(|runtime| {
            let response = runtime.execute(json!({"@type": "mahayana.conversation.list"}))?;
            print_conversations(&response)
        }),
        Some("history") => {
            let conversation_id = args
                .next()
                .ok_or_else(|| "用法：mahayana history <conversation-id>".to_string())?;
            with_runtime(|runtime| {
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
            with_runtime(|runtime| send_and_stream(runtime, &conversation_id, &text))
        }
        Some("chat") => {
            let conversation_id = args.next();
            with_runtime(|runtime| interactive_chat(runtime, conversation_id))
        }
        Some("help") | Some("--help") | Some("-h") => {
            usage();
            Ok(())
        }
        Some(other) => Err(format!("未知命令：{other}。运行 mahayana help 查看帮助。")),
        None => with_runtime(|runtime| interactive_chat(runtime, None)),
    }
}

fn usage() {
    println!(
        "大乘 CLI\n\n\
         mahayana login                         支付宝登录\n\
         mahayana contacts                      查看 AI、Telegram、好友和小程序\n\
         mahayana chat [conversation-id]        进入联系人式会话\n\
         mahayana send <conversation-id> <文本> 发送并流式等待回复\n\
         mahayana history <conversation-id>     查看历史\n\
         mahayana status                        查看本地 Runtime 状态\n\
         mahayana logout                        退出软件账号"
    );
}

fn login() -> Result<(), String> {
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
    operation: impl FnOnce(&RuntimeHandle) -> Result<(), String>,
) -> Result<(), String> {
    let runtime = RuntimeHandle::create()?;
    operation(&runtime)
}

struct RuntimeHandle(u64);

impl RuntimeHandle {
    fn create() -> Result<Self, String> {
        let id = unsafe { mahayana_runtime_create(std::ptr::null()) };
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
    let accepted = runtime.execute(json!({
        "@type": "mahayana.conversation.send",
        "conversationId": conversation_id,
        "text": text,
    }))?;
    let operation_id = accepted
        .get("operationId")
        .and_then(Value::as_str)
        .ok_or_else(|| "Runtime 没有返回操作编号".to_string())?;
    loop {
        let Some(event) = runtime.receive(30_000)? else {
            continue;
        };
        match event.get("@type").and_then(Value::as_str) {
            Some("mahayana.message.delta")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
                print!(
                    "{}",
                    event
                        .get("delta")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                );
                io::stdout().flush().map_err(|error| error.to_string())?;
            }
            Some("mahayana.message.completed")
                if event.get("operationId").and_then(Value::as_str) == Some(operation_id) =>
            {
                if event.get("message").and_then(|value| value.get("role"))
                    != Some(&Value::String("user".into()))
                {
                    println!();
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
                return Ok(());
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
