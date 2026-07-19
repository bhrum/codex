use fabushi_official_miniapps::{
    OFFICIAL_PLUGIN_IDS, OfficialMiniAppEngine, PROTOCOL_VERSION, app_definition,
    combined_manifest, content_resources, home_html,
};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

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
        "help" | "--help" | "-h" => {
            let app = app_definition(&plugin_id).expect("validated plugin");
            println!(
                "{} local CLI/MCP\n\nUsage:\n  fabushi-plugin-cli --plugin {} --dump-manifest\n  fabushi-plugin-cli --plugin {} <command> [--json '{{...}}']\n  fabushi-plugin-cli --plugin {} mcp-serve\n\nCommands: {}",
                app.title,
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
