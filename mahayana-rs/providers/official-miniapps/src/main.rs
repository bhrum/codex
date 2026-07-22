use fabushi_official_miniapps::{
    OFFICIAL_PLUGIN_IDS, OfficialMiniAppEngine, PROTOCOL_VERSION, app_definition,
    combined_manifest, content_resources, home_html,
};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if let Err(error) = run(env::args().collect()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run(mut args: Vec<String>) -> Result<(), String> {
    let executable = args.first().cloned().unwrap_or_default();
    args.drain(..1);
    let plugin_id = take_option(&mut args, "--plugin")
        .or_else(|| take_option(&mut args, "--plugin-id"))
        .or_else(|| env::var("FABUSHI_PLUGIN_ID").ok())
        .or_else(|| infer_plugin_id(&executable))
        .ok_or_else(|| {
            format!(
                "--plugin is required; expected one of {}",
                OFFICIAL_PLUGIN_IDS.join(", ")
            )
        })?;
    app_definition(&plugin_id).ok_or_else(|| format!("unknown official plugin: {plugin_id}"))?;
    let command = args.first().cloned().unwrap_or_else(|| "help".into());
    match command.as_str() {
        "--dump-manifest" | "dump-manifest" => print_json(&combined_manifest(&plugin_id)?),
        "mcp-serve" | "mcp" => serve_mcp(&plugin_id),
        "web" | "web-serve" => {
            args.remove(0);
            serve_web(&plugin_id, &mut args)
        }
        "help" | "--help" | "-h" => {
            let app = app_definition(&plugin_id).expect("validated plugin");
            println!(
                "{} local CLI/MCP\n\nUsage:\n  fabushi-plugin-cli --plugin {} --dump-manifest\n  fabushi-plugin-cli --plugin {} <command> [--json '{{...}}']\n  fabushi-plugin-cli --plugin {} mcp-serve\n  fabushi-plugin-cli --plugin {} web-serve [--port 8787]\n\nCommands: {}",
                app.title,
                plugin_id,
                plugin_id,
                plugin_id,
                plugin_id,
                app.commands.keys().cloned().collect::<Vec<_>>().join(", ")
            );
            Ok(())
        }
        direct => {
            args.remove(0);
            let definition = app_definition(&plugin_id).expect("validated plugin");
            let tool = definition
                .commands
                .get(direct)
                .map(String::as_str)
                .unwrap_or(direct);
            let arguments = take_option(&mut args, "--json")
                .map(|source| {
                    serde_json::from_str(&source)
                        .map_err(|error| format!("invalid --json: {error}"))
                })
                .transpose()?
                .unwrap_or_else(|| {
                    if args.is_empty() {
                        json!({})
                    } else {
                        json!({"input":args.join(" ")})
                    }
                });
            if let Some(native_result) = run_chatgpt_native_tool(&plugin_id, tool, &arguments)? {
                return print_json(&native_result);
            }
            let mut store = StateStore::load(&plugin_id)?;
            let output = store.engine.call_tool(&plugin_id, tool, arguments)?;
            store.save()?;
            print_json(&output.result)
        }
    }
}

fn serve_mcp(plugin_id: &str) -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut store = StateStore::load(plugin_id)?;
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_error(&mut stdout, Value::Null, -32700, &error.to_string())?;
                continue;
            }
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            write_error(
                &mut stdout,
                request.get("id").cloned().unwrap_or(Value::Null),
                -32600,
                "invalid request",
            )?;
            continue;
        };
        if request.get("id").is_none() {
            continue;
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let response = match method {
            "initialize" => Ok(
                json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{"tools":{"listChanged":false},"resources":{"subscribe":false,"listChanged":false}},"serverInfo":{"name":format!("fabushi-{plugin_id}"),"version":env!("CARGO_PKG_VERSION")}}),
            ),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(
                json!({"tools":app_definition(plugin_id).expect("validated plugin").tools.into_iter().map(|mut tool|{if tool.get("name").and_then(Value::as_str)==Some("home") { tool["_meta"] = json!({"ui/resourceUri":resource_uri(plugin_id)}); } tool}).collect::<Vec<_>>() }),
            ),
            "tools/call" => call_mcp_tool(&mut store, plugin_id, &request, &mut stdout),
            "resources/list" => {
                let mut resources = vec![
                    json!({"uri":resource_uri(plugin_id),"name":format!("{}首页",app_definition(plugin_id).unwrap().title),"mimeType":"text/html;profile=mcp-app"}),
                ];
                resources.extend(content_resources(plugin_id)?.into_iter().map(
                    |(uri, _)| json!({"uri":uri,"name":"小程序内容","mimeType":"text/markdown"}),
                ));
                Ok(json!({"resources":resources}))
            }
            "resources/read" => read_resource(plugin_id, &request),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        match response {
            Ok(result) => write_json_line(
                &mut stdout,
                &json!({"jsonrpc":"2.0","id":id,"result":result}),
            )?,
            Err((code, message)) => write_error(&mut stdout, id, code, &message)?,
        }
    }
    Ok(())
}

fn serve_web(plugin_id: &str, args: &mut Vec<String>) -> Result<(), String> {
    if plugin_id != "chatgpt-auto-confirm" || !cfg!(target_os = "macos") {
        return Err("本地 Web UI 当前只支持 macOS 的 chatgpt-auto-confirm 插件".into());
    }
    let port = take_option(args, "--port")
        .unwrap_or_else(|| "8787".into())
        .parse::<u16>()
        .map_err(|error| format!("无效端口: {error}"))?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("无法绑定本地 Web UI: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("读取 Web UI 地址失败: {error}"))?;
    println!("Mahayana ChatGPT 自动确认 Web UI: http://{address}/");
    io::stdout()
        .flush()
        .map_err(|error| format!("刷新 Web UI 地址失败: {error}"))?;
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(error) = handle_web_connection(plugin_id, &mut stream) {
                    let _ = write_web_response(
                        &mut stream,
                        500,
                        "application/json; charset=utf-8",
                        &json!({"ok":false,"errorCode":"web_request_failed","message":error}),
                    );
                }
            }
            Err(error) => eprintln!("Web UI 连接失败: {error}"),
        }
    }
    Ok(())
}

fn handle_web_connection(plugin_id: &str, stream: &mut TcpStream) -> Result<(), String> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let (method, target, body) = read_http_request(stream)?;
    let route = target.split('?').next().unwrap_or(target.as_str());
    if method == "GET" && (route == "/" || route == "/index.html") {
        return write_web_response(stream, 200, "text/html; charset=utf-8", CHATGPT_WEB_UI);
    }
    let result = match (method.as_str(), route) {
        ("GET", "/api/status") => web_native_tool(plugin_id, "status", json!({})),
        ("GET", "/api/audit") => web_native_tool(
            plugin_id,
            "audit_log",
            json!({"limit": query_limit(&target)}),
        ),
        ("POST", "/api/start") => web_native_tool(plugin_id, "start", parse_json_body(&body)?),
        ("POST", "/api/stop") => web_native_tool(plugin_id, "stop", json!({})),
        ("POST", "/api/scan") => web_native_tool(plugin_id, "scan_once", json!({})),
        _ => Err(format!("未找到路由: {method} {route}")),
    }?;
    write_web_response(stream, 200, "application/json; charset=utf-8", &result)
}

fn web_native_tool(plugin_id: &str, tool: &str, arguments: Value) -> Result<Value, String> {
    run_chatgpt_native_tool(plugin_id, tool, &arguments)?
        .ok_or_else(|| "Mahayana CLI 找不到 ChatGPT macOS 原生运行时；请确认插件包完整".to_string())
}

fn parse_json_body(body: &str) -> Result<Value, String> {
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    let value: Value =
        serde_json::from_str(body).map_err(|error| format!("请求 JSON 无效: {error}"))?;
    if !value.is_object() {
        return Err("请求体必须是 JSON 对象".into());
    }
    Ok(value)
}

fn query_limit(target: &str) -> u64 {
    target
        .split_once('?')
        .and_then(|(_, query)| {
            query.split('&').find_map(|part| {
                let (key, value) = part.split_once('=')?;
                (key == "limit")
                    .then(|| value.parse::<u64>().ok())
                    .flatten()
            })
        })
        .unwrap_or(20)
        .clamp(1, 100)
}

fn read_http_request(stream: &mut TcpStream) -> Result<(String, String, String), String> {
    let mut bytes = Vec::new();
    let mut header_end = None;
    let mut content_length = 0usize;
    loop {
        let mut chunk = [0u8; 4096];
        let count = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("请求过大".into());
        }
        if header_end.is_none() {
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let end = position + 4;
                let headers = String::from_utf8_lossy(&bytes[..end]);
                content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        (key.eq_ignore_ascii_case("content-length"))
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                header_end = Some(end);
            }
        }
        if let Some(end) = header_end {
            if bytes.len() >= end + content_length {
                break;
            }
        }
    }
    let end = header_end.ok_or_else(|| "HTTP 请求头不完整".to_string())?;
    let head = String::from_utf8_lossy(&bytes[..end]);
    let mut request_line = head.lines().next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let target = request_line.next().unwrap_or_default().to_string();
    if method.is_empty() || target.is_empty() {
        return Err("HTTP 请求行无效".into());
    }
    let body_end = (end + content_length).min(bytes.len());
    let body = String::from_utf8_lossy(&bytes[end..body_end]).into_owned();
    Ok((method, target, body))
}

fn write_web_response<T: serde::Serialize + ?Sized>(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &T,
) -> Result<(), String> {
    let body = if content_type.starts_with("text/html") {
        CHATGPT_WEB_UI.to_string()
    } else {
        serde_json::to_string(body).map_err(|error| error.to_string())?
    };
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Bad Request",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

const CHATGPT_WEB_UI: &str = r##"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>ChatGPT 自动确认 · Mahayana CLI</title>
<style>
:root{color-scheme:dark}body{font:15px system-ui;margin:0;background:#111827;color:#eef2ff}main{max-width:760px;margin:0 auto;padding:28px}section{background:#1f2937;border:1px solid #374151;border-radius:16px;padding:20px;margin:14px 0}h1{margin:0 0 8px;font-size:25px}p{color:#aebbd0}.grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px}.metric{background:#111827;border-radius:10px;padding:12px}.metric small{display:block;color:#9ca3af}.metric strong{display:block;margin-top:4px;font-size:17px}button{border:0;border-radius:9px;padding:10px 14px;margin:4px 6px 4px 0;background:#4f46e5;color:white;cursor:pointer}button.secondary{background:#374151}pre{white-space:pre-wrap;background:#111827;border-radius:10px;padding:12px;max-height:320px;overflow:auto}.safe{color:#86efac}.warning{color:#fbbf24}</style></head>
<body><main><section><h1>ChatGPT 自动确认</h1><p>Mahayana CLI 独立后台运行，通过 ChatGPT 本机 IPC 监听授权请求并回传「允许一次」。辅助功能扫描仅作为旧版 ChatGPT 的兼容通道。</p><p class="safe">不切换任务 · 不激活窗口 · 不移动系统鼠标 · 授权详情不写入审计日志</p></section>
<section><div class="grid"><div class="metric"><small>监听</small><strong id="running">读取中…</strong></div><div class="metric"><small>ChatGPT</small><strong id="app">读取中…</strong></div><div class="metric"><small>辅助功能</small><strong id="ax">读取中…</strong></div><div class="metric"><small>模式</small><strong id="mode">读取中…</strong></div></div></section>
<section><button id="start">启动自动确认</button><button id="stop" class="secondary">停止</button><button id="scan" class="secondary">立即扫描</button><button id="refresh" class="secondary">刷新</button><pre id="message">等待 CLI 返回…</pre></section>
<section><h2>本地审计</h2><pre id="audit">读取中…</pre></section></main>
<script>
const $=id=>document.getElementById(id);const pretty=v=>JSON.stringify(v,null,2);
async function call(path,method='GET',body){const r=await fetch(path,{method,headers:{'content-type':'application/json'},body:body===undefined?undefined:JSON.stringify(body)});const v=await r.json();if(!r.ok)throw new Error(v.message||'请求失败');return v}
async function refresh(){try{const s=await call('/api/status');$('running').textContent=s.running?'运行中':'已停止';$('app').textContent=s.applicationRunning?'已发现':'未发现';$('ax').textContent=s.accessibilityGranted?'已授权':'需要授权';$('mode').textContent=s.backgroundOnly?'严格后台':'兼容模式';$('message').textContent=pretty(s);const a=await call('/api/audit?limit=20');$('audit').textContent=pretty(a.events||a)}catch(e){$('message').textContent=e.message}}
async function action(path,body){try{$('message').textContent=pretty(await call(path,'POST',body));await refresh()}catch(e){$('message').textContent=e.message}}
$('start').onclick=()=>action('/api/start',{approveAll:true,intervalMs:750});$('stop').onclick=()=>action('/api/stop');$('scan').onclick=()=>action('/api/scan');$('refresh').onclick=refresh;refresh();setInterval(refresh,2000);
</script></body></html>"##;

fn call_mcp_tool(
    store: &mut StateStore,
    plugin_id: &str,
    request: &Value,
    stdout: &mut impl Write,
) -> Result<Value, (i64, String)> {
    let name = request
        .pointer("/params/name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "tools/call requires params.name".into()))?;
    let arguments = request
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(native_result) =
        run_chatgpt_native_tool(plugin_id, name, &arguments).map_err(|error| (-32000, error))?
    {
        return Ok(native_mcp_result(name, native_result));
    }
    let output = store
        .engine
        .call_tool(plugin_id, name, arguments)
        .map_err(|error| (-32602, error))?;
    if let Some(token) = request.pointer("/params/_meta/progressToken") {
        for progress in &output.progress {
            write_json_line(stdout,&json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":token,"progress":progress.progress,"total":progress.total,"message":progress.message}})).map_err(|error|(-32000,error))?;
        }
    }
    store.save().map_err(|error| (-32000, error))?;
    Ok(output.result)
}

fn run_chatgpt_native_tool(
    plugin_id: &str,
    tool: &str,
    arguments: &Value,
) -> Result<Option<Value>, String> {
    if plugin_id != "chatgpt-auto-confirm" || !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let command = match tool {
        "start" => "start",
        "stop" => "stop",
        "status" => "status",
        "scan_once" => "scan",
        "audit_log" => "audit",
        "diagnose" => "diagnose",
        _ => return Ok(None),
    };
    let runtime = native_runtime_path().ok_or_else(|| {
        "ChatGPT 自动确认的 macOS 原生运行时缺失；请从插件根目录运行或重新安装插件包".to_string()
    })?;
    let mut process = Command::new(&runtime);
    process.arg(command);
    match command {
        "start" | "sweep" => {
            process.arg(serde_json::to_string(arguments).map_err(|error| error.to_string())?)
        }
        "audit" => process.arg(
            arguments
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(1, 100)
                .to_string(),
        ),
        _ => &mut process,
    };
    let output = process
        .output()
        .map_err(|error| format!("启动 ChatGPT 原生运行时失败: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            format!(
                "ChatGPT 原生运行时没有返回 JSON{}",
                if stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", stderr.trim())
                }
            )
        })?;
    let result: Value = serde_json::from_str(line)
        .map_err(|error| format!("ChatGPT 原生运行时返回了无效 JSON: {error}"))?;
    if !output.status.success() && result.get("ok") != Some(&Value::Bool(false)) {
        return Err(format!("ChatGPT 原生运行时退出码 {}", output.status));
    }
    Ok(Some(result))
}

fn native_runtime_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("CHATGPT_AUTO_CONFIRM_NATIVE") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(path) = env::current_exe() {
        if let Some(cli_directory) = path.parent() {
            if let Some(plugin_directory) = cli_directory.parent() {
                candidates.push(
                    plugin_directory
                        .join("runtime")
                        .join("macos")
                        .join("chatgpt-auto-confirm"),
                );
            }
        }
    }
    if let Ok(directory) = env::current_dir() {
        candidates.push(
            directory
                .join("runtime")
                .join("macos")
                .join("chatgpt-auto-confirm"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn native_mcp_result(tool: &str, payload: Value) -> Value {
    let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let text = if ok {
        format!("Mahayana CLI 已直接执行 ChatGPT 自动确认 {tool}。")
    } else {
        let message = payload
            .get("message")
            .or_else(|| payload.get("errorCode"))
            .and_then(Value::as_str)
            .unwrap_or("原生运行时返回失败");
        format!("Mahayana CLI 执行 ChatGPT 自动确认 {tool} 失败：{message}")
    };
    json!({
        "content": [{"type":"text","text":text}],
        "structuredContent": payload,
        "isError": !ok
    })
}

fn read_resource(plugin_id: &str, request: &Value) -> Result<Value, (i64, String)> {
    let uri = request
        .pointer("/params/uri")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "resources/read requires params.uri".into()))?;
    if uri == resource_uri(plugin_id) {
        return Ok(
            json!({"contents":[{"uri":uri,"mimeType":"text/html;profile=mcp-app","text":home_html(plugin_id).map_err(|error|(-32000,error))?}]}),
        );
    }
    let content = content_resources(plugin_id)
        .map_err(|error| (-32000, error))?
        .into_iter()
        .find(|(candidate, _)| candidate == uri)
        .map(|(_, text)| text)
        .ok_or_else(|| (-32002, format!("resource not found: {uri}")))?;
    Ok(json!({"contents":[{"uri":uri,"mimeType":"text/markdown","text":content}]}))
}

struct StateStore {
    path: PathBuf,
    engine: OfficialMiniAppEngine,
}

impl StateStore {
    fn load(plugin_id: &str) -> Result<Self, String> {
        let path = state_path(plugin_id)?;
        let source = fs::read_to_string(&path).unwrap_or_default();
        Ok(Self {
            path,
            engine: OfficialMiniAppEngine::from_state_json(&source)?,
        })
    }
    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&self.path, self.engine.state_json()?).map_err(|error| error.to_string())
    }
}

fn state_path(plugin_id: &str) -> Result<PathBuf, String> {
    let root = env::var_os("FABUSHI_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("MAHAYANA_HOME").map(PathBuf::from))
        .unwrap_or(
            env::current_dir()
                .map_err(|error| error.to_string())?
                .join(".mahayana-state"),
        );
    Ok(root.join("plugins").join(plugin_id).join("state.json"))
}

fn resource_uri(plugin_id: &str) -> String {
    format!("ui://fabushi/{plugin_id}/home-v1.html")
}

fn take_option(args: &mut Vec<String>, name: &str) -> Option<String> {
    let index = args
        .iter()
        .position(|arg| arg == name || arg.starts_with(&format!("{name}=")))?;
    let option = args.remove(index);
    if let Some((_, value)) = option.split_once('=') {
        return Some(value.into());
    }
    (index < args.len()).then(|| args.remove(index))
}

fn infer_plugin_id(executable: &str) -> Option<String> {
    let stem = Path::new(executable).file_stem()?.to_str()?;
    OFFICIAL_PLUGIN_IDS
        .iter()
        .find(|id| stem.starts_with(**id))
        .map(|id| (*id).into())
}

fn print_json(value: &Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}
fn write_error(output: &mut impl Write, id: Value, code: i64, message: &str) -> Result<(), String> {
    write_json_line(
        output,
        &json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    )
}
fn write_json_line(output: &mut impl Write, value: &Value) -> Result<(), String> {
    writeln!(output, "{value}").map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())
}
