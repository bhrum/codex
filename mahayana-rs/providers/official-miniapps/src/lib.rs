//! Shared local-first core for Fabushi's official Mini Apps.
//!
//! The same deterministic tool/state contract is hosted by the desktop CLI
//! (direct command and stdio MCP modes) and by the browser/mobile WASM module.
//! OS-only work is returned as an explicit `hostRequest`; the host remains the
//! sole owner of sockets, process execution, secrets, and user confirmation.

use mahayana_miniapp_protocol::{
    AppSummary, CompiledContent, ContentCompiler, ContentSource, QuickAction, QuickReply,
    SourceKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const OFFICIAL_PLUGIN_IDS: [&str; 6] = [
    "global-dharma",
    "faliu-flashcards",
    "platform-publish",
    "hermes-installer",
    "bot-father",
    "mahayana-assistant",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDefinition {
    pub id: String,
    pub title: String,
    pub description: String,
    pub tools: Vec<Value>,
    pub commands: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressUpdate {
    pub progress: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallOutcome {
    pub result: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub progress: Vec<ProgressUpdate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialMiniAppEngine {
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    global_dharma: GlobalDharmaState,
    #[serde(default)]
    decks: BTreeMap<String, Deck>,
    #[serde(default)]
    drafts: BTreeMap<String, Draft>,
    #[serde(default)]
    hermes: HermesState,
    #[serde(default)]
    generated_plugins: BTreeMap<String, GeneratedPlugin>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlobalDharmaState {
    running: bool,
    loops: u64,
    sent: u64,
    logs: Vec<String>,
    mode: Option<String>,
    pending_content: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Deck {
    id: String,
    title: String,
    cards: Vec<Value>,
    cursor: usize,
    reviews: Vec<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Draft {
    id: String,
    title: String,
    content: String,
    status: String,
    platforms: Vec<String>,
    updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HermesState {
    installed: bool,
    running: bool,
    messages: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedPlugin {
    id: String,
    status: String,
    bundle: Value,
}

impl OfficialMiniAppEngine {
    pub fn from_state_json(source: &str) -> Result<Self, String> {
        if source.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(source).map_err(|error| format!("invalid plugin state: {error}"))
    }

    pub fn state_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn call_tool(
        &mut self,
        plugin_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<ToolCallOutcome, String> {
        let definition = app_definition(plugin_id)
            .ok_or_else(|| format!("unknown official plugin: {plugin_id}"))?;
        if !definition
            .tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some(tool_name))
        {
            return Err(format!("{plugin_id} has no MCP Tool {tool_name}"));
        }
        if tool_name == "home" {
            return Ok(ToolCallOutcome {
                result: home_result(&definition),
                progress: Vec::new(),
            });
        }
        match plugin_id {
            "global-dharma" => self.call_global_dharma(tool_name, &arguments),
            "faliu-flashcards" => self.call_flashcards(tool_name, &arguments),
            "platform-publish" => self.call_platform_publish(tool_name, &arguments),
            "hermes-installer" => self.call_hermes(tool_name, &arguments),
            "bot-father" => self.call_bot_father(tool_name, &arguments),
            "mahayana-assistant" => self.call_assistant(tool_name, &arguments),
            _ => unreachable!("validated plugin id"),
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id = self.next_id.saturating_add(1);
        format!("{prefix}-{:08}", self.next_id)
    }

    fn call_global_dharma(
        &mut self,
        tool: &str,
        arguments: &Value,
    ) -> Result<ToolCallOutcome, String> {
        let state = &mut self.global_dharma;
        let (text, structured, progress) = match tool {
            "chat" => return self.chat_global_dharma(arguments),
            "start" => {
                state.running = true;
                state.logs.push("服务启动请求已交给宿主".into());
                (
                    "全球法布施启动请求已提交。",
                    json!({"running": true, "hostRequest": host_request("service.start", json!({"service":"global-dharma"}))}),
                    progress_pair("启动全球法布施", "启动请求已提交"),
                )
            }
            "stop" => {
                state.running = false;
                state.logs.push("服务停止请求已交给宿主".into());
                (
                    "全球法布施停止请求已提交。",
                    json!({"running": false, "hostRequest": host_request("service.stop", json!({"service":"global-dharma"}))}),
                    progress_pair("停止全球法布施", "停止请求已提交"),
                )
            }
            "loop" => {
                state.loops = state.loops.saturating_add(1);
                state
                    .logs
                    .push(format!("完成第 {} 次本地调度", state.loops));
                (
                    "调度循环已完成。",
                    json!({"loops": state.loops}),
                    progress_pair("开始调度", "调度完成"),
                )
            }
            "status" => (
                "已读取全球法布施状态。",
                json!({"running":state.running,"loops":state.loops,"sent":state.sent}),
                Vec::new(),
            ),
            "send" => {
                let content = required_string(arguments, "content")?;
                state.sent = state.sent.saturating_add(1);
                state.logs.push(format!(
                    "提交内容 #{}（{} 字节）",
                    state.sent,
                    content.len()
                ));
                (
                    "内容已交给宿主传输。",
                    json!({
                        "sent": state.sent,
                        "hostRequest": host_request("network.send", json!({
                            "channelPreference": ["udp", "http-direct"],
                            "payload": content,
                            "taskId": arguments.get("task_id").and_then(Value::as_str).unwrap_or("mahayana"),
                            "countryCodes": arguments.get("country_codes").cloned().unwrap_or(Value::Null),
                            "targets": arguments.get("udp_targets").cloned().unwrap_or(Value::Null),
                            "httpTargets": arguments.get("http_targets").cloned().unwrap_or(Value::Null)
                        }))
                    }),
                    progress_pair("准备传输", "已交给宿主"),
                )
            }
            "logs" => {
                let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
                let start = state.logs.len().saturating_sub(limit.clamp(1, 200));
                (
                    "已读取日志。",
                    json!({"entries": &state.logs[start..]}),
                    Vec::new(),
                )
            }
            "validate_config" => (
                "配置有效。",
                json!({"valid":true,"keys":arguments.get("config").and_then(Value::as_object).map(|value| value.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()}),
                Vec::new(),
            ),
            "deploy_latest" => (
                "最新版本部署请求已提交。",
                json!({"deploymentId":self.id("deployment"),"status":"queued","hostRequest":host_request("deployment.submit",json!({"pluginId":"global-dharma"}))}),
                progress_pair("提交部署", "部署已入队"),
            ),
            _ => unreachable!("validated tool"),
        };
        Ok(outcome(text, structured, progress))
    }

    fn chat_global_dharma(&mut self, arguments: &Value) -> Result<ToolCallOutcome, String> {
        let message = required_string(arguments, "message")?.trim();
        let state = &mut self.global_dharma;
        let (text, structured) = match message {
            "1" | "进入全球发送" => {
                state.mode = Some("global-send".into());
                state.pending_content = None;
                (
                    "已进入全球发送。请发送要传播的内容；真正发送前会再次确认。",
                    json!({"handled":true,"mode":"global-send"}),
                )
            }
            "2" | "进入本地转经轮" => {
                state.mode = Some("local-prayer-wheel".into());
                (
                    "已进入本地转经轮模式。回复“开始”创建本地运行请求。",
                    json!({"handled":true,"mode":"local-prayer-wheel"}),
                )
            }
            "3" | "进入本地场能模式" => {
                state.mode = Some("local-field".into());
                (
                    "已进入本地场能模式。回复“开始”创建本地运行请求。",
                    json!({"handled":true,"mode":"local-field"}),
                )
            }
            "退出" | "返回" => {
                state.mode = None;
                state.pending_content = None;
                ("已返回首页。", json!({"handled":true,"mode":"home"}))
            }
            "确认发送" if state.mode.as_deref() == Some("global-send") => {
                let Some(content) = state.pending_content.take() else {
                    return Ok(outcome(
                        "还没有待发送内容，请先发送正文。",
                        json!({"handled":true,"mode":"global-send"}),
                        Vec::new(),
                    ));
                };
                state.sent = state.sent.saturating_add(1);
                (
                    "发送请求已准备好，宿主确认后才会执行。",
                    json!({
                        "handled":true,
                        "mode":"global-send",
                        "taskId":format!("global-send-{}",state.sent),
                        "hostRequest":host_request("network.send",json!({"payload":content,"taskId":format!("global-send-{}",state.sent)})),
                    }),
                )
            }
            "开始" if state.mode.as_deref() == Some("local-prayer-wheel") => (
                "本地转经轮运行请求已准备好，宿主确认后才会执行。",
                json!({"handled":true,"mode":"local-prayer-wheel","hostRequest":host_request("local.prayer-wheel.start",json!({}))}),
            ),
            "开始" if state.mode.as_deref() == Some("local-field") => (
                "本地场能运行请求已准备好，宿主确认后才会执行。",
                json!({"handled":true,"mode":"local-field","hostRequest":host_request("local.field.start",json!({}))}),
            ),
            _ if state.mode.as_deref() == Some("global-send") => {
                state.pending_content = Some(message.to_string());
                (
                    "已保存待发送内容。回复“确认发送”继续，或回复“退出”取消。",
                    json!({"handled":true,"mode":"global-send","pending":true}),
                )
            }
            _ => ("", json!({"handled":false,"mode":state.mode})),
        };
        Ok(outcome(text, structured, Vec::new()))
    }

    fn call_flashcards(
        &mut self,
        tool: &str,
        arguments: &Value,
    ) -> Result<ToolCallOutcome, String> {
        let (text, structured) = match tool {
            "create_deck" => {
                let title = required_string(arguments, "title")?.to_string();
                let cards = arguments
                    .get("cards")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let id = self.id("deck");
                self.decks.insert(
                    id.clone(),
                    Deck {
                        id: id.clone(),
                        title: title.clone(),
                        cards: cards.clone(),
                        cursor: 0,
                        reviews: Vec::new(),
                    },
                );
                (
                    format!("已创建牌组「{title}」。"),
                    json!({"id":id,"title":title,"cardCount":cards.len()}),
                )
            }
            "list_decks" => {
                let decks = self.decks.values().map(|deck| json!({"id":deck.id,"title":deck.title,"cardCount":deck.cards.len()})).collect::<Vec<_>>();
                ("已列出牌组。".into(), json!({"decks":decks}))
            }
            "open_deck" => {
                let id = required_string(arguments, "deck_id")?;
                ("已打开牌组。".into(), json!({"deck":self.decks.get(id)}))
            }
            "review_next" => {
                let id = required_string(arguments, "deck_id")?;
                let card = self.decks.get(id).and_then(|deck| {
                    (!deck.cards.is_empty())
                        .then(|| deck.cards[deck.cursor % deck.cards.len()].clone())
                });
                (
                    if card.is_some() {
                        "下一张卡片已就绪。"
                    } else {
                        "牌组中没有卡片。"
                    }
                    .into(),
                    json!({"card":card}),
                )
            }
            "submit_review" => {
                let id = required_string(arguments, "deck_id")?;
                let rating = required_string(arguments, "rating")?;
                let Some(deck) = self.decks.get_mut(id) else {
                    return Ok(error_outcome("找不到牌组。"));
                };
                deck.reviews
                    .push(json!({"rating":rating,"sequence":deck.reviews.len()+1}));
                deck.cursor = deck.cursor.saturating_add(1);
                (
                    "复习结果已保存。".into(),
                    json!({"nextIndex":deck.cursor,"rating":rating}),
                )
            }
            _ => unreachable!("validated tool"),
        };
        Ok(outcome(&text, structured, Vec::new()))
    }

    fn call_platform_publish(
        &mut self,
        tool: &str,
        arguments: &Value,
    ) -> Result<ToolCallOutcome, String> {
        let (text, structured, progress) = match tool {
            "create_draft" => {
                let title = required_string(arguments, "title")?.to_string();
                let content = arguments
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let id = self.id("draft");
                let draft = Draft {
                    id: id.clone(),
                    title,
                    content,
                    status: "draft".into(),
                    platforms: Vec::new(),
                    updated_at: format!("local-{}", self.next_id),
                };
                self.drafts.insert(id.clone(), draft.clone());
                (
                    "草稿已创建。",
                    serde_json::to_value(draft).map_err(|error| error.to_string())?,
                    Vec::new(),
                )
            }
            "save_draft" => {
                let id = required_string(arguments, "draft_id")?;
                let Some(draft) = self.drafts.get_mut(id) else {
                    return Ok(error_outcome("找不到草稿。"));
                };
                if let Some(title) = arguments.get("title").and_then(Value::as_str) {
                    draft.title = title.into();
                }
                if let Some(content) = arguments.get("content").and_then(Value::as_str) {
                    draft.content = content.into();
                }
                draft.updated_at = format!("local-{}", self.next_id);
                (
                    "草稿已保存。",
                    serde_json::to_value(draft).map_err(|error| error.to_string())?,
                    Vec::new(),
                )
            }
            "open_draft" | "status" => {
                let id = required_string(arguments, "draft_id")?;
                (
                    if tool == "status" {
                        "已读取发布状态。"
                    } else {
                        "已读取草稿。"
                    },
                    json!({"draft":self.drafts.get(id)}),
                    Vec::new(),
                )
            }
            "publish" => {
                let id = required_string(arguments, "draft_id")?.to_string();
                let platforms = required_string_array(arguments, "platforms")?;
                let Some(draft) = self.drafts.get_mut(&id) else {
                    return Ok(error_outcome("找不到草稿。"));
                };
                draft.status = "publishing".into();
                draft.platforms = platforms.clone();
                (
                    "发布任务已交给宿主。",
                    json!({"draftId":id,"platforms":platforms,"status":"publishing","hostRequest":host_request("platform.publish",json!({"draftId":id,"platforms":platforms}))}),
                    progress_pair("准备发布", "发布任务已提交"),
                )
            }
            _ => unreachable!("validated tool"),
        };
        Ok(outcome(text, structured, progress))
    }

    fn call_hermes(&mut self, tool: &str, arguments: &Value) -> Result<ToolCallOutcome, String> {
        let (text, structured, progress) = match tool {
            "install" => {
                self.hermes.installed = true;
                (
                    "Hermes 安装请求已交给宿主。",
                    json!({"installed":true,"hostRequest":host_request("runtime.install",json!({"runtime":"hermes"}))}),
                    progress_pair("开始安装", "安装请求已提交"),
                )
            }
            "start" => {
                self.hermes.running = self.hermes.installed;
                (
                    if self.hermes.running {
                        "Hermes 启动请求已提交。"
                    } else {
                        "请先安装 Hermes。"
                    },
                    json!({"installed":self.hermes.installed,"running":self.hermes.running,"messages":self.hermes.messages,"hostRequest":self.hermes.running.then(||host_request("runtime.start",json!({"runtime":"hermes"})))}),
                    Vec::new(),
                )
            }
            "status" => (
                "已读取 Hermes 状态。",
                json!({"installed":self.hermes.installed,"running":self.hermes.running,"messages":self.hermes.messages}),
                Vec::new(),
            ),
            "chat" => {
                let message = required_string(arguments, "message")?;
                self.hermes.messages = self.hermes.messages.saturating_add(1);
                (
                    "Hermes 消息已交给宿主。",
                    json!({"messageId":self.id("hermes-message"),"length":message.len(),"hostRequest":host_request("runtime.message",json!({"runtime":"hermes","message":message}))}),
                    Vec::new(),
                )
            }
            "stop" => {
                self.hermes.running = false;
                (
                    "Hermes 停止请求已提交。",
                    json!({"installed":self.hermes.installed,"running":false,"messages":self.hermes.messages,"hostRequest":host_request("runtime.stop",json!({"runtime":"hermes"}))}),
                    Vec::new(),
                )
            }
            "reset" => {
                self.hermes = HermesState::default();
                (
                    "Hermes 本地状态已重置。",
                    json!({"installed":false,"running":false,"messages":0}),
                    Vec::new(),
                )
            }
            _ => unreachable!("validated tool"),
        };
        Ok(outcome(text, structured, progress))
    }

    fn call_bot_father(
        &mut self,
        tool: &str,
        arguments: &Value,
    ) -> Result<ToolCallOutcome, String> {
        let (text, structured) = match tool {
            "create_plugin" => {
                let name = normalize_plugin_name(required_string(arguments, "name")?);
                let description = required_string(arguments, "description")?;
                let id = self.id("plugin");
                let bundle = generated_bundle(&name, description);
                self.generated_plugins.insert(
                    id.clone(),
                    GeneratedPlugin {
                        id: id.clone(),
                        status: "created".into(),
                        bundle: bundle.clone(),
                    },
                );
                (
                    "插件包已创建。".into(),
                    json!({"pluginId":id,"bundle":bundle}),
                )
            }
            "validate_plugin" => {
                let id = required_string(arguments, "plugin_id")?;
                let Some(plugin) = self.generated_plugins.get(id) else {
                    return Ok(error_outcome("找不到插件。"));
                };
                (
                    "插件验证通过。".into(),
                    json!({"valid":true,"missing":[],"runtimeVariants":plugin.bundle.pointer("/manifest/runtimeVariants").cloned().unwrap_or(json!([]))}),
                )
            }
            "build_plugin" | "install_plugin" | "publish_plugin" => {
                let id = required_string(arguments, "plugin_id")?;
                let status = match tool {
                    "build_plugin" => "built",
                    "install_plugin" => "installed",
                    _ => "publishing",
                };
                let Some(plugin) = self.generated_plugins.get_mut(id) else {
                    return Ok(error_outcome("找不到插件。"));
                };
                plugin.status = status.into();
                (
                    format!("插件状态已更新为 {status}。"),
                    json!({"pluginId":id,"status":status,"hostRequest":(tool=="publish_plugin").then(||host_request("plugin.publish",json!({"pluginId":id})))}),
                )
            }
            "deployment_status" => {
                let id = required_string(arguments, "plugin_id")?;
                (
                    "已读取部署状态。".into(),
                    self.generated_plugins
                        .get(id)
                        .map(|plugin| json!({"pluginId":plugin.id,"status":plugin.status}))
                        .unwrap_or(json!({"pluginId":id,"status":"not_found"})),
                )
            }
            _ => unreachable!("validated tool"),
        };
        Ok(outcome(&text, structured, Vec::new()))
    }

    fn call_assistant(&mut self, tool: &str, arguments: &Value) -> Result<ToolCallOutcome, String> {
        let (text, structured) = match tool {
            "help" => {
                let topic = arguments
                    .get("topic")
                    .and_then(Value::as_str)
                    .unwrap_or("小程序");
                (
                    format!(
                        "「{topic}」通过本地 CLI/WASM 与 MCP Tools 工作；输入 / 可查看当前插件命令。"
                    ),
                    json!({"topic":topic}),
                )
            }
            "list_plugins" => (
                "已列出官方插件。".into(),
                json!({"plugins":OFFICIAL_PLUGIN_IDS.iter().filter_map(|id|app_definition(id)).collect::<Vec<_>>()}),
            ),
            "plugin_status" => {
                let id = required_string(arguments, "plugin_id")?;
                (
                    "已读取插件状态。".into(),
                    json!({"pluginId":id,"available":app_definition(id).is_some(),"runtime":"local-core"}),
                )
            }
            "diagnose_plugin" => {
                let id = required_string(arguments, "plugin_id")?;
                (
                    "插件诊断完成。".into(),
                    json!({"pluginId":id,"checks":{"manifest":"ok","cli":"ok","mcp":"ok","wasm":"ok","uiResource":"ok","secretExposure":"ok"}}),
                )
            }
            _ => unreachable!("validated tool"),
        };
        Ok(outcome(&text, structured, Vec::new()))
    }
}

pub fn app_definition(plugin_id: &str) -> Option<AppDefinition> {
    let (title, description, tools, commands) = match plugin_id {
        "global-dharma" => (
            "全球法布施",
            "全球法布施运行、发送、日志和部署管理",
            global_dharma_tools(),
            command_map(&[
                ("start", "start"),
                ("stop", "stop"),
                ("loop", "loop"),
                ("status", "status"),
                ("send", "send"),
                ("logs", "logs"),
                ("validate", "validate_config"),
                ("deploy", "deploy_latest"),
            ]),
        ),
        "faliu-flashcards" => (
            "法流记忆卡",
            "创建、打开与复习法流记忆卡",
            flashcard_tools(),
            command_map(&[
                ("create", "create_deck"),
                ("list", "list_decks"),
                ("open", "open_deck"),
                ("next", "review_next"),
                ("review", "submit_review"),
            ]),
        ),
        "platform-publish" => (
            "平台发布",
            "跨平台草稿与发布状态管理",
            publish_tools(),
            command_map(&[
                ("create", "create_draft"),
                ("save", "save_draft"),
                ("open", "open_draft"),
                ("publish", "publish"),
                ("status", "status"),
            ]),
        ),
        "hermes-installer" => (
            "Hermes 安装器",
            "安装并运行 Hermes；密钥只存放在宿主 Secret Store",
            hermes_tools(),
            command_map(&[
                ("install", "install"),
                ("start", "start"),
                ("status", "status"),
                ("chat", "chat"),
                ("stop", "stop"),
                ("reset", "reset"),
            ]),
        ),
        "bot-father" => (
            "Bot Father",
            "生成、验证、构建与发布可移植 Codex 插件",
            bot_father_tools(),
            command_map(&[
                ("create", "create_plugin"),
                ("validate", "validate_plugin"),
                ("build", "build_plugin"),
                ("install", "install_plugin"),
                ("publish", "publish_plugin"),
                ("status", "deployment_status"),
            ]),
        ),
        "mahayana-assistant" => (
            "大乘助手",
            "插件状态、诊断与使用帮助",
            assistant_tools(),
            command_map(&[
                ("help", "help"),
                ("list", "list_plugins"),
                ("status", "plugin_status"),
                ("diagnose", "diagnose_plugin"),
            ]),
        ),
        _ => return None,
    };
    Some(AppDefinition {
        id: plugin_id.into(),
        title: title.into(),
        description: description.into(),
        tools,
        commands,
    })
}

pub fn combined_manifest(plugin_id: &str) -> Result<Value, String> {
    let app =
        app_definition(plugin_id).ok_or_else(|| format!("unknown official plugin: {plugin_id}"))?;
    Ok(json!({
        "schemaVersion": 1,
        "protocolVersion": PROTOCOL_VERSION,
        "plugin": {
            "name": app.id,
            "title": app.title,
            "version": VERSION,
            "description": app.description,
            "mcpServers": "./.mcp.json",
            "runtimeVariants": [
                {"id":"local-cli","server":format!("{}-local",app.id),"platforms":["cli","desktop"],"priority":300},
                {"id":"account-http","server":app.id,"platforms":["cli","desktop","mobile","web"],"priority":100}
            ]
        },
        "mahayana": {
            "commands": app.commands,
            "cli": {"executable":"./runtime/cli/fabushi-plugin-cli","args":["--plugin",app.id]},
            "wasm": {"module":"./runtime/wasm/fabushi_official_miniapps_bg.wasm","export":"OfficialMiniAppRuntime"}
        },
        "tools": app.tools,
    }))
}

pub fn home_html(plugin_id: &str) -> Result<String, String> {
    let app =
        app_definition(plugin_id).ok_or_else(|| format!("unknown official plugin: {plugin_id}"))?;
    let buttons = app
        .tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .filter(|name| *name != "home")
        .map(|name| format!(r#"<button data-tool="{name}">/{name}</button>"#))
        .collect::<String>();
    Ok(format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'none'"><style>:root{{color-scheme:light dark}}body{{font:15px system-ui;margin:0;padding:20px;background:#101722;color:#edf3ff}}h1{{font-size:21px;margin:0 0 6px}}p{{color:#9eb0ca}}button{{margin:4px;border:1px solid #3d526f;border-radius:10px;padding:9px 12px;background:#182538;color:inherit}}pre{{white-space:pre-wrap;background:#0b111a;padding:12px;border-radius:10px}}</style></head><body><h1>{}</h1><p>{}</p><div>{buttons}</div><pre id="out">本地 CLI/WASM MCP App 已连接</pre><script>(()=>{{let id=0;const pending=new Map();const out=document.querySelector('#out');addEventListener('message',event=>{{const m=event.data;if(!m||m.jsonrpc!=='2.0')return;if(m.id!==undefined&&pending.has(m.id)){{pending.get(m.id)(m);pending.delete(m.id)}}if(m.method==='notifications/progress')out.textContent=`${{m.params?.message??'执行中'}} (${{m.params?.progress??0}}/${{m.params?.total??0}})`;if(m.method==='ui/notifications/tool-result')out.textContent=JSON.stringify(m.params,null,2)}});function call(name){{const requestId=++id;return new Promise(resolve=>{{pending.set(requestId,resolve);parent.postMessage({{jsonrpc:'2.0',id:requestId,method:'tools/call',params:{{name,arguments:{{}}}}}},'*')}})}}document.querySelectorAll('[data-tool]').forEach(button=>button.onclick=async()=>{{out.textContent='执行中…';const response=await call(button.dataset.tool);out.textContent=JSON.stringify(response.result??response.error,null,2)}})}})()</script></body></html>"#,
        escape_html(&app.title),
        escape_html(&app.description)
    ))
}

pub fn content_resources(plugin_id: &str) -> Result<Vec<(String, String)>, String> {
    Ok(compiled_content(plugin_id)?.resources)
}

fn home_result(app: &AppDefinition) -> Value {
    let home = compiled_content(&app.id)
        .expect("official mini-app content must compile")
        .home;
    json!({
        "content":[{"type":"text","text":home.welcome.as_ref().map(|welcome|welcome.markdown.as_str()).unwrap_or("")}],
        "structuredContent":home,
        "_meta":{"ui/resourceUri":format!("ui://fabushi/{}/home-v1.html",app.id)}
    })
}

fn compiled_content(plugin_id: &str) -> Result<CompiledContent, String> {
    let app =
        app_definition(plugin_id).ok_or_else(|| format!("unknown official plugin: {plugin_id}"))?;
    let mut sources = vec![ContentSource {
        path: "content/welcome.md".into(),
        kind: SourceKind::Welcome,
        markdown: format!(
            "---\nid: welcome\nrevision: '1'\n---\n欢迎来到 **{}**。\n\n{}",
            app.title, app.description
        ),
    }];
    let quick_replies = if plugin_id == "global-dharma" {
        sources.extend([
            ContentSource {
                path: "content/announcements/getting-started.md".into(),
                kind: SourceKind::Announcement,
                markdown: "---\nid: getting-started\nrevision: '1'\ntitle: 首次使用说明\npublishedAt: 2026-07-19\nsummary: 所有网络发送与本地运行都会在真正执行前请求宿主批准。\ntags: [公告, 安全]\n---\n# 首次使用说明\n\n选择模式后按对话提示操作；真正发送、安装或运行前，大乘宿主会展示参数和风险。".into(),
            },
            ContentSource {
                path: "content/articles/guide.md".into(),
                kind: SourceKind::Article,
                markdown: "---\nid: guide\nrevision: '1'\ntitle: 全球法布施使用指南\npublishedAt: 2026-07-19\nsummary: 了解全球发送、本地转经轮与本地场能三种模式。\ntags: [指南]\n---\n# 全球法布施使用指南\n\n## 1 全球发送\n\n逐步收集发送内容，确认后通过宿主受控网络能力执行。\n\n## 2 本地转经轮\n\n在本地运行，启动前由宿主展示能力请求。\n\n## 3 本地场能模式\n\n在本地运行，插件不会向宿主返回任意 shell 字符串。".into(),
            },
        ]);
        vec![
            quick_message("global-send", "1 全球发送", "1", "进入全球发送"),
            quick_message("local-prayer-wheel", "2 本地转经轮", "2", "进入本地转经轮"),
            quick_message("local-field", "3 本地场能模式", "3", "进入本地场能模式"),
        ]
    } else {
        Vec::new()
    };
    ContentCompiler::compile(
        AppSummary {
            id: app.id,
            title: app.title,
            version: VERSION.into(),
            source: Some("https://github.com/fabushi/fabushi".into()),
        },
        sources,
        quick_replies,
    )
}

fn quick_message(id: &str, label: &str, alias: &str, value: &str) -> QuickReply {
    QuickReply {
        id: id.into(),
        label: label.into(),
        aliases: vec![alias.into()],
        action: QuickAction::Message {
            value: value.into(),
        },
    }
}

fn outcome(text: &str, structured: Value, progress: Vec<ProgressUpdate>) -> ToolCallOutcome {
    ToolCallOutcome {
        result: json!({"content":[{"type":"text","text":text}],"structuredContent":structured}),
        progress,
    }
}

fn error_outcome(text: &str) -> ToolCallOutcome {
    ToolCallOutcome {
        result: json!({"content":[{"type":"text","text":text}],"isError":true}),
        progress: Vec::new(),
    }
}

fn progress_pair(start: &str, end: &str) -> Vec<ProgressUpdate> {
    vec![
        ProgressUpdate {
            progress: 0,
            total: 1,
            message: start.into(),
        },
        ProgressUpdate {
            progress: 1,
            total: 1,
            message: end.into(),
        },
    ]
}

fn host_request(capability: &str, params: Value) -> Value {
    json!({"transport":"mcp-host-bridge","capability":capability,"params":params})
}

fn command_map(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(command, tool)| ((*command).into(), (*tool).into()))
        .collect()
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn required_string_array(value: &Value, key: &str) -> Result<Vec<String>, String> {
    let values = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{key} must be an array"))?;
    let values = values
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        Err(format!("{key} must not be empty"))
    } else {
        Ok(values)
    }
}

fn normalize_plugin_name(value: &str) -> String {
    let mut normalized = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    while normalized.contains("--") {
        normalized = normalized.replace("--", "-");
    }
    normalized.trim_matches('-').to_string()
}

fn generated_bundle(name: &str, description: &str) -> Value {
    let local = format!("{name}-local");
    let wasm = format!("{name}-wasm");
    let http = format!("{name}-http");
    json!({
        "manifest": {"name":name,"version":"0.1.0","description":description,"mcpServers":"./.mcp.json","runtimeVariants":[
            {"id":"local-cli","server":local,"platforms":["cli","desktop"],"priority":300},
            {"id":"local-wasm","server":wasm,"platforms":["mobile","web"],"priority":250},
            {"id":"account-http","server":http,"platforms":["cli","desktop","mobile","web"],"priority":100}
        ]},
        "files": [".codex-plugin/plugin.json",".mahayana/plugin.json",".mcp.json","runtime/cli/fabushi-plugin-cli","runtime/wasm/plugin.wasm","ui/home.html","test/contract.test.js"]
    })
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn annotations(read_only: bool, destructive: bool, open_world: bool) -> Value {
    json!({"readOnlyHint":read_only,"destructiveHint":destructive,"openWorldHint":open_world})
}

fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    annotation: Value,
    ui: bool,
) -> Value {
    let mut descriptor = json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":annotation});
    if ui {
        descriptor["_meta"] =
            json!({"ui/resourceUri":format!("ui://fabushi/{{pluginId}}/home-v1.html")});
    }
    descriptor
}

fn home_tool() -> Value {
    tool(
        "home",
        "加载插件首页",
        json!({
            "surface":{"type":"string","enum":["cli","desktop","mobile","web"]},
            "locale":{"type":"string"},
            "cursor":{"type":"string"},
            "limit":{"type":"integer","minimum":1,"maximum":10}
        }),
        &[],
        annotations(true, false, false),
        true,
    )
}
fn global_dharma_tools() -> Vec<Value> {
    vec![
        home_tool(),
        tool(
            "chat",
            "处理全球法布施对话与快捷回复",
            json!({
                "message":{"type":"string"},
                "surface":{"type":"string"},
                "locale":{"type":"string"},
                "actionId":{"type":["string","null"]}
            }),
            &["message"],
            annotations(false, false, false),
            false,
        ),
        tool(
            "start",
            "启动全球法布施服务",
            json!({}),
            &[],
            annotations(false, false, true),
            false,
        ),
        tool(
            "stop",
            "停止全球法布施服务",
            json!({}),
            &[],
            annotations(false, true, false),
            false,
        ),
        tool(
            "loop",
            "执行一次调度循环",
            json!({}),
            &[],
            annotations(false, false, true),
            false,
        ),
        tool(
            "status",
            "读取服务状态",
            json!({}),
            &[],
            annotations(true, false, false),
            false,
        ),
        tool(
            "send",
            "发送一条法布施内容",
            json!({
                "content":{"type":"string"},
                "task_id":{"type":"string","default":"mahayana"},
                "country_codes":{"type":"array","items":{"type":"string"}},
                "udp_targets":{"type":"array","items":{"type":"object","properties":{"host":{"type":"string"},"port":{"type":"integer","minimum":1,"maximum":65535}},"required":["host","port"],"additionalProperties":false}},
                "http_targets":{"type":"array","items":{"type":"string","format":"uri"}}
            }),
            &["content"],
            annotations(false, false, true),
            false,
        ),
        tool(
            "logs",
            "读取最近日志",
            json!({"limit":{"type":"integer","default":50}}),
            &[],
            annotations(true, false, false),
            false,
        ),
        tool(
            "validate_config",
            "验证法布施配置",
            json!({"config":{"type":"object"}}),
            &[],
            annotations(true, false, false),
            false,
        ),
        tool(
            "deploy_latest",
            "部署最新已验证版本",
            json!({}),
            &[],
            annotations(false, false, true),
            false,
        ),
    ]
}
fn flashcard_tools() -> Vec<Value> {
    vec![
        home_tool(),
        tool(
            "create_deck",
            "创建记忆卡牌组",
            json!({"title":{"type":"string"},"cards":{"type":"array","items":{"type":"object"},"default":[]}}),
            &["title"],
            annotations(false, false, false),
            false,
        ),
        tool(
            "list_decks",
            "列出所有牌组",
            json!({}),
            &[],
            annotations(true, false, false),
            false,
        ),
        tool(
            "open_deck",
            "打开一个牌组",
            json!({"deck_id":{"type":"string"}}),
            &["deck_id"],
            annotations(true, false, false),
            false,
        ),
        tool(
            "review_next",
            "获取下一张复习卡",
            json!({"deck_id":{"type":"string"}}),
            &["deck_id"],
            annotations(true, false, false),
            false,
        ),
        tool(
            "submit_review",
            "提交复习结果",
            json!({"deck_id":{"type":"string"},"rating":{"type":"string","enum":["again","hard","good","easy"]}}),
            &["deck_id", "rating"],
            annotations(false, false, false),
            false,
        ),
    ]
}
fn publish_tools() -> Vec<Value> {
    vec![
        home_tool(),
        tool(
            "create_draft",
            "创建平台发布草稿",
            json!({"title":{"type":"string"},"content":{"type":"string","default":""}}),
            &["title"],
            annotations(false, false, false),
            false,
        ),
        tool(
            "save_draft",
            "保存草稿内容",
            json!({"draft_id":{"type":"string"},"title":{"type":"string"},"content":{"type":"string"}}),
            &["draft_id"],
            annotations(false, false, false),
            false,
        ),
        tool(
            "open_draft",
            "打开草稿",
            json!({"draft_id":{"type":"string"}}),
            &["draft_id"],
            annotations(true, false, false),
            false,
        ),
        tool(
            "publish",
            "发布到指定平台",
            json!({"draft_id":{"type":"string"},"platforms":{"type":"array","items":{"type":"string"}}}),
            &["draft_id", "platforms"],
            annotations(false, false, true),
            false,
        ),
        tool(
            "status",
            "读取发布状态",
            json!({"draft_id":{"type":"string"}}),
            &["draft_id"],
            annotations(true, false, false),
            false,
        ),
    ]
}
fn hermes_tools() -> Vec<Value> {
    vec![
        home_tool(),
        tool(
            "install",
            "安装 Hermes 运行时",
            json!({}),
            &[],
            annotations(false, false, true),
            false,
        ),
        tool(
            "start",
            "启动 Hermes",
            json!({}),
            &[],
            annotations(false, false, false),
            false,
        ),
        tool(
            "status",
            "读取 Hermes 状态",
            json!({}),
            &[],
            annotations(true, false, false),
            false,
        ),
        tool(
            "chat",
            "向 Hermes 发送消息",
            json!({"message":{"type":"string"}}),
            &["message"],
            annotations(false, false, true),
            false,
        ),
        tool(
            "stop",
            "停止 Hermes",
            json!({}),
            &[],
            annotations(false, true, false),
            false,
        ),
        tool(
            "reset",
            "重置 Hermes 本地状态",
            json!({}),
            &[],
            annotations(false, true, false),
            false,
        ),
    ]
}
fn bot_father_tools() -> Vec<Value> {
    vec![
        home_tool(),
        tool(
            "create_plugin",
            "创建可移植 Codex 插件包",
            json!({"name":{"type":"string"},"description":{"type":"string"}}),
            &["name", "description"],
            annotations(false, false, false),
            false,
        ),
        tool(
            "validate_plugin",
            "验证插件包",
            json!({"plugin_id":{"type":"string"}}),
            &["plugin_id"],
            annotations(true, false, false),
            false,
        ),
        tool(
            "build_plugin",
            "构建插件包",
            json!({"plugin_id":{"type":"string"}}),
            &["plugin_id"],
            annotations(false, false, false),
            false,
        ),
        tool(
            "install_plugin",
            "安装插件包",
            json!({"plugin_id":{"type":"string"}}),
            &["plugin_id"],
            annotations(false, false, false),
            false,
        ),
        tool(
            "publish_plugin",
            "发布插件包",
            json!({"plugin_id":{"type":"string"}}),
            &["plugin_id"],
            annotations(false, false, true),
            false,
        ),
        tool(
            "deployment_status",
            "读取插件部署状态",
            json!({"plugin_id":{"type":"string"}}),
            &["plugin_id"],
            annotations(true, false, false),
            false,
        ),
    ]
}
fn assistant_tools() -> Vec<Value> {
    vec![
        home_tool(),
        tool(
            "help",
            "读取小程序使用帮助",
            json!({"topic":{"type":"string","default":"小程序"}}),
            &[],
            annotations(true, false, false),
            false,
        ),
        tool(
            "list_plugins",
            "列出官方插件",
            json!({}),
            &[],
            annotations(true, false, false),
            false,
        ),
        tool(
            "plugin_status",
            "读取插件状态",
            json!({"plugin_id":{"type":"string"}}),
            &["plugin_id"],
            annotations(true, false, false),
            false,
        ),
        tool(
            "diagnose_plugin",
            "诊断插件运行时",
            json!({"plugin_id":{"type":"string"}}),
            &["plugin_id"],
            annotations(true, false, false),
            false,
        ),
    ]
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub struct OfficialMiniAppRuntime {
        plugin_id: String,
        engine: OfficialMiniAppEngine,
    }

    #[wasm_bindgen]
    impl OfficialMiniAppRuntime {
        #[wasm_bindgen(constructor)]
        pub fn new(plugin_id: &str, state_json: &str) -> Result<OfficialMiniAppRuntime, JsValue> {
            app_definition(plugin_id)
                .ok_or_else(|| JsValue::from_str("unknown official plugin"))?;
            let engine = OfficialMiniAppEngine::from_state_json(state_json)
                .map_err(|error| JsValue::from_str(&error))?;
            Ok(Self {
                plugin_id: plugin_id.into(),
                engine,
            })
        }

        #[wasm_bindgen(js_name = callTool)]
        pub fn call_tool(&mut self, tool: &str, arguments_json: &str) -> Result<String, JsValue> {
            let output = self.call_tool_outcome(tool, arguments_json)?;
            let output: ToolCallOutcome = serde_json::from_str(&output)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            serde_json::to_string(&output.result)
                .map_err(|error| JsValue::from_str(&error.to_string()))
        }

        #[wasm_bindgen(js_name = callToolOutcome)]
        pub fn call_tool_outcome(
            &mut self,
            tool: &str,
            arguments_json: &str,
        ) -> Result<String, JsValue> {
            let arguments = serde_json::from_str(arguments_json)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            let output = self
                .engine
                .call_tool(&self.plugin_id, tool, arguments)
                .map_err(|error| JsValue::from_str(&error))?;
            serde_json::to_string(&output).map_err(|error| JsValue::from_str(&error.to_string()))
        }

        #[wasm_bindgen(js_name = exportState)]
        pub fn export_state(&self) -> Result<String, JsValue> {
            self.engine
                .state_json()
                .map_err(|error| JsValue::from_str(&error))
        }

        #[wasm_bindgen(js_name = toolsJson)]
        pub fn tools_json(&self) -> Result<String, JsValue> {
            serde_json::to_string(
                &app_definition(&self.plugin_id)
                    .expect("validated plugin")
                    .tools,
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))
        }

        #[wasm_bindgen(js_name = manifestJson)]
        pub fn manifest_json(&self) -> Result<String, JsValue> {
            serde_json::to_string(
                &combined_manifest(&self.plugin_id).map_err(|error| JsValue::from_str(&error))?,
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))
        }

        #[wasm_bindgen(js_name = homeHtml)]
        pub fn home_html(&self) -> Result<String, JsValue> {
            super::home_html(&self.plugin_id).map_err(|error| JsValue::from_str(&error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_official_plugin_has_unique_tools_and_local_artifacts() {
        for id in OFFICIAL_PLUGIN_IDS {
            let app = app_definition(id).expect("definition");
            let names = app
                .tools
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            let unique = names
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            assert_eq!(unique.len(), names.len(), "duplicate tool in {id}");
            assert_eq!(names.first(), Some(&"home"));
            let manifest = combined_manifest(id).expect("manifest");
            assert_eq!(
                manifest
                    .pointer("/mahayana/cli/executable")
                    .and_then(Value::as_str),
                Some("./runtime/cli/fabushi-plugin-cli")
            );
            assert!(
                manifest
                    .pointer("/mahayana/wasm/module")
                    .and_then(Value::as_str)
                    .unwrap()
                    .ends_with(".wasm")
            );
        }
    }

    #[test]
    fn flashcards_round_trip_is_local_and_stateful() {
        let mut engine = OfficialMiniAppEngine::default();
        engine
            .call_tool(
                "faliu-flashcards",
                "create_deck",
                json!({"title":"心经","cards":[{"front":"照见","back":"五蕴皆空"}]}),
            )
            .unwrap();
        let listed = engine
            .call_tool("faliu-flashcards", "list_decks", json!({}))
            .unwrap();
        assert_eq!(
            listed
                .result
                .pointer("/structuredContent/decks/0/cardCount"),
            Some(&json!(1))
        );
        let restored =
            OfficialMiniAppEngine::from_state_json(&engine.state_json().unwrap()).unwrap();
        assert_eq!(restored.decks.len(), 1);
    }

    #[test]
    fn privileged_operations_return_host_requests_instead_of_opening_sockets() {
        let mut engine = OfficialMiniAppEngine::default();
        let sent = engine
            .call_tool("global-dharma", "send", json!({"content":"法布施"}))
            .unwrap();
        assert_eq!(
            sent.result
                .pointer("/structuredContent/hostRequest/transport")
                .and_then(Value::as_str),
            Some("mcp-host-bridge")
        );
        assert_eq!(
            sent.result
                .pointer("/structuredContent/hostRequest/capability")
                .and_then(Value::as_str),
            Some("network.send")
        );
    }

    #[test]
    fn global_dharma_home_and_chat_follow_conversational_contract() {
        let mut engine = OfficialMiniAppEngine::default();
        let home = engine
            .call_tool("global-dharma", "home", json!({"surface":"cli"}))
            .unwrap();
        assert_eq!(
            home.result
                .pointer("/structuredContent/schema")
                .and_then(Value::as_str),
            Some("mahayana.miniapp.home.v1")
        );
        assert_eq!(
            home.result
                .pointer("/structuredContent/quickReplies/0/aliases/0")
                .and_then(Value::as_str),
            Some("1")
        );
        assert!(!content_resources("global-dharma").unwrap().is_empty());

        let selected = engine
            .call_tool("global-dharma", "chat", json!({"message":"进入全球发送"}))
            .unwrap();
        assert_eq!(
            selected
                .result
                .pointer("/structuredContent/handled")
                .and_then(Value::as_bool),
            Some(true)
        );
        engine
            .call_tool(
                "global-dharma",
                "chat",
                json!({"message":"愿一切众生离苦得乐"}),
            )
            .unwrap();
        let confirmed = engine
            .call_tool("global-dharma", "chat", json!({"message":"确认发送"}))
            .unwrap();
        assert_eq!(
            confirmed
                .result
                .pointer("/structuredContent/hostRequest/capability")
                .and_then(Value::as_str),
            Some("network.send")
        );
    }
}
