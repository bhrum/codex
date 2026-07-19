use super::RuntimeHandle;
use codex_tui::ComposerAction;
use codex_tui::ComposerInput;
use codex_tui::enable_terminal_modes;
use codex_tui::restore_terminal_modes;
use crossterm::cursor::Hide;
use crossterm::cursor::Show;
use crossterm::event;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use crossterm::execute;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Alignment;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Margin;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use ratatui::widgets::Padding;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::io;
use std::io::IsTerminal;
use std::time::Duration;
use std::time::Instant;

type ChatTerminal = Terminal<CrosstermBackend<io::Stdout>>;

const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const RECEIVE_POLL_INTERVAL_MS: u64 = 0;

pub(super) fn run(runtime: &RuntimeHandle, selected: Option<String>) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("交互式对话需要终端；非交互环境请使用 mahayana send".into());
    }

    let mut terminal = TerminalSession::enter()?;
    let mut requested_id = selected;
    loop {
        let conversations = load_conversations(runtime)?;
        if conversations.is_empty() {
            return Err("当前没有可用联系人".into());
        }
        let conversation = match requested_id.take() {
            Some(id) => conversations
                .iter()
                .find(|conversation| conversation.id == id)
                .cloned()
                .ok_or_else(|| format!("联系人不存在：{id}"))?,
            None => match pick_conversation(terminal.terminal_mut(), runtime, &conversations)? {
                Some(index) => conversations[index].clone(),
                None => return Ok(()),
            },
        };

        match chat(terminal.terminal_mut(), runtime, conversation)? {
            ChatExit::Contacts => {}
            ChatExit::Quit => return Ok(()),
        }
    }
}

struct TerminalSession {
    terminal: ChatTerminal,
}

impl TerminalSession {
    fn enter() -> Result<Self, String> {
        enable_terminal_modes().map_err(|error| format!("无法启用终端对话模式：{error}"))?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = restore_terminal_modes();
            return Err(format!("无法打开终端对话界面：{error}"));
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).map_err(|error| {
            let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
            let _ = restore_terminal_modes();
            error.to_string()
        })?;
        Ok(Self { terminal })
    }

    fn terminal_mut(&mut self) -> &mut ChatTerminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let _ = restore_terminal_modes();
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConversationItem {
    id: String,
    title: String,
    peer_type: String,
    pinned: bool,
    unread_count: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct PluginCommandItem {
    plugin_id: String,
    command: String,
    tool: String,
    input_schema: Value,
}

impl PluginCommandItem {
    fn from_value(value: &Value) -> Result<Self, String> {
        let required = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("插件命令缺少 {name}"))
        };
        Ok(Self {
            plugin_id: required("pluginId")?,
            command: required("command")?,
            tool: required("tool")?,
            input_schema: value
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
        })
    }

    fn qualified(&self) -> String {
        format!("/{}:{}", self.plugin_id, self.command)
    }

    fn argument_hint(&self) -> &'static str {
        match self
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.len())
        {
            Some(0) | None => "无参数",
            Some(1) => "文本或 JSON 参数",
            Some(_) => "JSON 参数",
        }
    }
}

impl ConversationItem {
    fn from_value(value: &Value) -> Result<Self, String> {
        Ok(Self {
            id: value
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "联系人缺少编号".to_string())?
                .to_string(),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("未命名")
                .to_string(),
            peer_type: value
                .pointer("/peer/type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            pinned: value
                .get("pinned")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            unread_count: value
                .get("unreadCount")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        })
    }

    fn uses_ai(&self) -> bool {
        // Every built-in mini-app currently uses the same embedded Agent
        // backend as Codex. Human and Telegram contacts never expose model
        // settings even though they share this exact chat surface.
        matches!(self.peer_type.as_str(), "codexAi" | "miniApp")
    }

    fn kind_label(&self) -> &'static str {
        match self.peer_type.as_str() {
            "codexAi" => "AI",
            "miniApp" if self.uses_ai() => "AI 小程序",
            "miniApp" => "小程序",
            "telegramContact" => "Telegram 联系人",
            "mahayanaContact" => "联系人",
            _ => "对话",
        }
    }
}

fn load_conversations(runtime: &RuntimeHandle) -> Result<Vec<ConversationItem>, String> {
    let response = runtime.execute(json!({"@type": "mahayana.conversation.list"}))?;
    response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Runtime 没有返回联系人列表".to_string())?
        .iter()
        .map(ConversationItem::from_value)
        .collect()
}

fn load_plugin_commands(
    runtime: &RuntimeHandle,
    conversation: &ConversationItem,
) -> Result<Vec<PluginCommandItem>, String> {
    let plugin_id = (conversation.peer_type == "miniApp")
        .then(|| conversation.id.strip_prefix("miniapp:"))
        .flatten();
    let Some(plugin_id) = plugin_id else {
        return Ok(Vec::new());
    };
    let response = runtime.execute(json!({
        "@type": "mahayana.plugin.commands",
        "pluginId": plugin_id,
    }))?;
    response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Runtime 没有返回插件命令列表".to_string())?
        .iter()
        .map(PluginCommandItem::from_value)
        .collect()
}

fn pick_conversation(
    terminal: &mut ChatTerminal,
    runtime: &RuntimeHandle,
    conversations: &[ConversationItem],
) -> Result<Option<usize>, String> {
    let mut selected = 0usize;
    let mut history_cache: HashMap<String, Vec<ChatMessage>> = HashMap::new();
    loop {
        if !conversations.is_empty() {
            let conv = &conversations[selected];
            if !history_cache.contains_key(&conv.id) {
                let msgs = load_history(runtime, conv).unwrap_or_else(|err| {
                    vec![ChatMessage {
                        kind: MessageKind::System,
                        text: format!("暂无对话记录或加载提示：{}", err),
                        streaming: false,
                    }]
                });
                history_cache.insert(conv.id.clone(), msgs);
            }
        }
        let current_messages = if !conversations.is_empty() {
            history_cache
                .get(&conversations[selected].id)
                .map(|m| m.as_slice())
                .unwrap_or(&[])
        } else {
            &[]
        };

        terminal
            .draw(|frame| {
                render_conversation_picker(frame, conversations, selected, current_messages)
            })
            .map_err(|error| error.to_string())?;
        match event::read().map_err(|error| error.to_string())? {
            Event::Key(key) if is_key_press(key) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = if selected == 0 {
                        conversations.len() - 1
                    } else {
                        selected - 1
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1) % conversations.len();
                }
                KeyCode::Home => selected = 0,
                KeyCode::End => selected = conversations.len() - 1,
                KeyCode::Enter => return Ok(Some(selected)),
                KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn render_conversation_picker(
    frame: &mut Frame<'_>,
    conversations: &[ConversationItem],
    selected: usize,
    current_messages: &[ChatMessage],
) {
    let area = frame.area();
    let main_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(main_rows[0]);

    // 左半区：联系人选单
    let left_block = Block::default()
        .title(" 联系人 (↑↓选择) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let items = conversations.iter().map(|conversation| {
        let pin = if conversation.pinned { "★ " } else { "  " };
        let unread = if conversation.unread_count > 0 {
            format!("  {} 条未读", conversation.unread_count)
        } else {
            String::new()
        };
        ListItem::new(Line::from(vec![
            Span::styled(pin, Style::default().fg(Color::Yellow)),
            Span::styled(
                conversation.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}{unread}", conversation.kind_label()),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
    });
    let list = List::new(items)
        .block(left_block)
        .highlight_symbol("› ")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, columns[0], &mut state);

    // 右半区：当前会话的对话内容预览
    let (selected_title, lines) = if let Some(conv) = conversations.get(selected) {
        (
            conv.title.as_str(),
            transcript_lines(conv, current_messages, false, Instant::now()),
        )
    } else {
        ("无", Vec::new())
    };
    let right_block = Block::default()
        .title(format!(" 对话预览: {} ", selected_title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(right_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, columns[1]);

    // 底部快捷键提示区
    let bottom_hint = Paragraph::new("↑↓/jk 切换联系人   Enter 进入完整对话   Esc/q 退出")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(bottom_hint, main_rows[1]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AiSettings {
    model: String,
    provider: String,
}

fn load_ai_settings(runtime: &RuntimeHandle) -> AiSettings {
    runtime
        .execute(json!({"@type": "mahayana.runtime.status"}))
        .map(|status| AiSettings {
            model: status
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("未知模型")
                .to_string(),
            provider: status
                .get("modelProvider")
                .and_then(Value::as_str)
                .unwrap_or("未知 Provider")
                .to_string(),
        })
        .unwrap_or_else(|_| AiSettings {
            model: "状态不可用".into(),
            provider: "状态不可用".into(),
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    User,
    Peer,
    System,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChatMessage {
    kind: MessageKind,
    text: String,
    streaming: bool,
}

fn load_history(
    runtime: &RuntimeHandle,
    conversation: &ConversationItem,
) -> Result<Vec<ChatMessage>, String> {
    let response = runtime.execute(json!({
        "@type": "mahayana.conversation.history",
        "conversationId": conversation.id,
        "limit": 100,
    }))?;
    response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Runtime 没有返回消息历史".to_string())?
        .iter()
        .map(|message| {
            let kind = match message.get("role").and_then(Value::as_str) {
                Some("user") => MessageKind::User,
                Some("system") => MessageKind::System,
                _ => MessageKind::Peer,
            };
            Ok(ChatMessage {
                kind,
                text: message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                streaming: false,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatExit {
    Contacts,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalChatCommand {
    Contacts,
    History,
    Quit,
}

fn local_chat_command(conversation: &ConversationItem, text: &str) -> Option<LocalChatCommand> {
    if conversation.peer_type == "miniApp" {
        return None;
    }
    match text {
        "/quit" | "/exit" => Some(LocalChatCommand::Quit),
        "/contacts" => Some(LocalChatCommand::Contacts),
        "/history" => Some(LocalChatCommand::History),
        _ => None,
    }
}

fn chat(
    terminal: &mut ChatTerminal,
    runtime: &RuntimeHandle,
    conversation: ConversationItem,
) -> Result<ChatExit, String> {
    let mut messages = load_history(runtime, &conversation)?;
    let plugin_commands = load_plugin_commands(runtime, &conversation).unwrap_or_default();
    let ai_settings = conversation.uses_ai().then(|| load_ai_settings(runtime));
    let mut composer =
        ComposerInput::new_with_placeholder(format!("给 {} 发送消息", conversation.title));
    composer.set_slash_commands_enabled(false);
    if plugin_commands.is_empty() {
        composer.set_hint_items(vec![
            ("enter", "发送"),
            ("shift+enter", "换行"),
            ("esc", "联系人"),
        ]);
    } else {
        composer.set_hint_items(vec![
            ("enter", "发送"),
            ("tab", "补全插件命令"),
            ("esc", "联系人"),
        ]);
    }
    let mut scroll_from_bottom = 0u16;

    loop {
        draw_chat(
            terminal,
            &conversation,
            ai_settings.as_ref(),
            &messages,
            &composer,
            &plugin_commands,
            false,
            scroll_from_bottom,
            Instant::now(),
        )?;
        if composer.flush_paste_burst_if_due() {
            continue;
        }
        if !event::poll(INPUT_POLL_INTERVAL).map_err(|error| error.to_string())? {
            continue;
        }
        match event::read().map_err(|error| error.to_string())? {
            Event::Paste(text) => {
                composer.handle_paste(text);
            }
            Event::Key(key) if is_key_press(key) => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(ChatExit::Quit);
                }
                match key.code {
                    KeyCode::Esc => return Ok(ChatExit::Contacts),
                    KeyCode::PageUp => {
                        scroll_from_bottom = scroll_from_bottom.saturating_add(10);
                        continue;
                    }
                    KeyCode::PageDown => {
                        scroll_from_bottom = scroll_from_bottom.saturating_sub(10);
                        continue;
                    }
                    KeyCode::End if key.modifiers.is_empty() => {
                        scroll_from_bottom = 0;
                        continue;
                    }
                    KeyCode::Tab if key.modifiers.is_empty() => {
                        if let Some(command) =
                            matching_plugin_commands(&composer.text(), &plugin_commands).first()
                        {
                            composer.replace_text(format!("{} ", command.qualified()));
                            continue;
                        }
                    }
                    _ => {}
                }
                if let ComposerAction::Submitted(text) = composer.input(key) {
                    let text = text.trim();
                    match local_chat_command(&conversation, text) {
                        Some(LocalChatCommand::Quit) => return Ok(ChatExit::Quit),
                        Some(LocalChatCommand::Contacts) => return Ok(ChatExit::Contacts),
                        Some(LocalChatCommand::History) => {
                            messages = load_history(runtime, &conversation)?;
                            scroll_from_bottom = 0;
                        }
                        None if !text.is_empty() => {
                            scroll_from_bottom = 0;
                            send_in_chat(
                                terminal,
                                runtime,
                                &conversation,
                                ai_settings.as_ref(),
                                &mut messages,
                                &composer,
                                &plugin_commands,
                                text,
                            )?;
                        }
                        None => {}
                    }
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn send_in_chat(
    terminal: &mut ChatTerminal,
    runtime: &RuntimeHandle,
    conversation: &ConversationItem,
    ai_settings: Option<&AiSettings>,
    messages: &mut Vec<ChatMessage>,
    composer: &ComposerInput,
    plugin_commands: &[PluginCommandItem],
    text: &str,
) -> Result<(), String> {
    messages.push(ChatMessage {
        kind: MessageKind::User,
        text: text.to_string(),
        streaming: false,
    });
    let accepted = match runtime.execute(json!({
        "@type": "mahayana.conversation.send",
        "conversationId": conversation.id,
        "text": text,
    })) {
        Ok(accepted) => accepted,
        Err(error) => {
            push_error(messages, error);
            return Ok(());
        }
    };
    let Some(operation_id) = accepted
        .get("operationId")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        push_error(messages, "Runtime 没有返回操作编号");
        return Ok(());
    };
    messages.push(ChatMessage {
        kind: MessageKind::Peer,
        text: String::new(),
        streaming: true,
    });
    let assistant_index = messages.len() - 1;
    let started_at = Instant::now();
    let mut interrupted = false;
    let mut usage_message_index: Option<usize> = None;
    let mut tool_progress_message_index: Option<usize> = None;

    loop {
        draw_chat(
            terminal,
            conversation,
            ai_settings,
            messages,
            composer,
            plugin_commands,
            true,
            0,
            started_at,
        )?;

        if event::poll(INPUT_POLL_INTERVAL).map_err(|error| error.to_string())? {
            match event::read().map_err(|error| error.to_string())? {
                Event::Key(key) if is_key_press(key) => match key.code {
                    KeyCode::Esc => {
                        if !interrupted {
                            interrupted = true;
                            if let Err(error) = runtime.interrupt(&operation_id) {
                                push_error(messages, error);
                                return Ok(());
                            }
                        }
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if !interrupted {
                            interrupted = true;
                            if let Err(error) = runtime.interrupt(&operation_id) {
                                push_error(messages, error);
                                return Ok(());
                            }
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        loop {
            let event = match runtime.receive(RECEIVE_POLL_INTERVAL_MS) {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(error) => {
                    finish_streaming_message(messages, assistant_index);
                    push_error(messages, error);
                    return Ok(());
                }
            };
            if event.get("operationId").and_then(Value::as_str) != Some(operation_id.as_str()) {
                continue;
            }
            match event.get("@type").and_then(Value::as_str) {
                Some("mahayana.message.delta") => {
                    if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                        messages[assistant_index].text.push_str(delta);
                    }
                }
                Some("mahayana.message.completed") => {
                    if event.pointer("/message/role").and_then(Value::as_str) != Some("user")
                        && let Some(text) = event.pointer("/message/text").and_then(Value::as_str)
                    {
                        messages[assistant_index].text = text.to_string();
                        messages[assistant_index].streaming = false;
                    }
                }
                Some("mahayana.model.usage.updated") => {
                    if let Some(summary) = super::format_usage_summary(&event) {
                        if let Some(index) = usage_message_index {
                            messages[index].text = summary;
                        } else {
                            messages.push(ChatMessage {
                                kind: MessageKind::System,
                                text: summary,
                                streaming: false,
                            });
                            usage_message_index = Some(messages.len() - 1);
                        }
                    }
                }
                Some("mahayana.plugin.progress") => {
                    let text = event
                        .get("message")
                        .and_then(Value::as_str)
                        .filter(|message| !message.is_empty())
                        .unwrap_or("MCP Tool 执行中…")
                        .to_string();
                    if let Some(index) = tool_progress_message_index {
                        messages[index].text = text;
                    } else {
                        messages.push(ChatMessage {
                            kind: MessageKind::System,
                            text,
                            streaming: true,
                        });
                        tool_progress_message_index = Some(messages.len() - 1);
                    }
                }
                Some("mahayana.approval.requested") => {
                    let decision = pick_approval(
                        terminal,
                        conversation,
                        ai_settings,
                        messages,
                        composer,
                        &event,
                        started_at,
                    )?;
                    if let Some(approval_id) = event.get("approvalId").and_then(Value::as_str)
                        && let Err(error) = runtime.resolve_approval(approval_id, decision)
                    {
                        finish_streaming_message(messages, assistant_index);
                        push_error(messages, error);
                        return Ok(());
                    }
                }
                Some("mahayana.operation.completed") => {
                    if let Some(index) = tool_progress_message_index {
                        messages[index].streaming = false;
                    }
                    finish_streaming_message(messages, assistant_index);
                    return Ok(());
                }
                Some("mahayana.operation.failed") => {
                    if let Some(index) = tool_progress_message_index {
                        messages[index].streaming = false;
                    }
                    finish_streaming_message(messages, assistant_index);
                    push_error(
                        messages,
                        event
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("操作失败"),
                    );
                    return Ok(());
                }
                Some("mahayana.operation.interrupted") => {
                    if let Some(index) = tool_progress_message_index {
                        messages[index].streaming = false;
                    }
                    finish_streaming_message(messages, assistant_index);
                    messages.push(ChatMessage {
                        kind: MessageKind::System,
                        text: "已停止生成。".into(),
                        streaming: false,
                    });
                    return Ok(());
                }
                _ => {}
            }
        }
    }
}

fn finish_streaming_message(messages: &mut [ChatMessage], assistant_index: usize) {
    let assistant = &mut messages[assistant_index];
    assistant.streaming = false;
    if assistant.text.trim().is_empty() {
        assistant.text = "（没有返回内容）".into();
    }
}

fn push_error(messages: &mut Vec<ChatMessage>, error: impl Into<String>) {
    messages.push(ChatMessage {
        kind: MessageKind::Error,
        text: error.into(),
        streaming: false,
    });
}

#[allow(clippy::too_many_arguments)]
fn pick_approval(
    terminal: &mut ChatTerminal,
    conversation: &ConversationItem,
    ai_settings: Option<&AiSettings>,
    messages: &[ChatMessage],
    composer: &ComposerInput,
    approval: &Value,
    started_at: Instant,
) -> Result<&'static str, String> {
    const OPTIONS: [(&str, &str); 4] = [
        ("允许本次", "accept"),
        ("本会话始终允许", "acceptForSession"),
        ("拒绝", "decline"),
        ("取消操作", "cancel"),
    ];
    let mut selected = 0usize;
    loop {
        terminal
            .draw(|frame| {
                render_chat(
                    frame,
                    conversation,
                    ai_settings,
                    messages,
                    composer,
                    &[],
                    true,
                    0,
                    started_at,
                );
                render_approval(frame, approval, &OPTIONS, selected);
            })
            .map_err(|error| error.to_string())?;
        match event::read().map_err(|error| error.to_string())? {
            Event::Key(key) if is_key_press(key) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = if selected == 0 {
                        OPTIONS.len() - 1
                    } else {
                        selected - 1
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % OPTIONS.len(),
                KeyCode::Enter => return Ok(OPTIONS[selected].1),
                KeyCode::Esc => return Ok("cancel"),
                _ => {}
            },
            _ => {}
        }
    }
}

fn render_approval(
    frame: &mut Frame<'_>,
    approval: &Value,
    options: &[(&str, &str)],
    selected: usize,
) {
    let area = centered_rect(70, 70, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" 需要确认 ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .padding(Padding::uniform(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(options.len() as u16),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(
            approval
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Codex 操作"),
        )
        .style(Style::default().add_modifier(Modifier::BOLD)),
        rows[0],
    );
    let details = approval
        .get("details")
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::to_string_pretty(value).ok())
        .unwrap_or_else(|| "此操作需要你的许可。".into());
    frame.render_widget(Paragraph::new(details).wrap(Wrap { trim: false }), rows[1]);
    let items = options
        .iter()
        .map(|(label, _)| ListItem::new(Line::from(*label)));
    let list = List::new(items).highlight_symbol("› ").highlight_style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, rows[2], &mut state);
    frame.render_widget(
        Paragraph::new("↑↓ 移动 · Enter 确认 · Esc 取消")
            .style(Style::default().fg(Color::DarkGray)),
        rows[3],
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_chat(
    terminal: &mut ChatTerminal,
    conversation: &ConversationItem,
    ai_settings: Option<&AiSettings>,
    messages: &[ChatMessage],
    composer: &ComposerInput,
    plugin_commands: &[PluginCommandItem],
    busy: bool,
    scroll_from_bottom: u16,
    started_at: Instant,
) -> Result<(), String> {
    terminal
        .draw(|frame| {
            render_chat(
                frame,
                conversation,
                ai_settings,
                messages,
                composer,
                plugin_commands,
                busy,
                scroll_from_bottom,
                started_at,
            )
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_chat(
    frame: &mut Frame<'_>,
    conversation: &ConversationItem,
    ai_settings: Option<&AiSettings>,
    messages: &[ChatMessage],
    composer: &ComposerInput,
    plugin_commands: &[PluginCommandItem],
    busy: bool,
    scroll_from_bottom: u16,
    started_at: Instant,
) {
    let area = frame.area();
    if area.width < 24 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("终端窗口太小，请放大后继续。")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }
    let header_height = if ai_settings.is_some() { 4 } else { 3 };
    let available_for_composer = area.height.saturating_sub(header_height + 2);
    let composer_height = composer
        .desired_height(area.width)
        .max(3)
        .min(available_for_composer.max(3));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(2),
            Constraint::Length(composer_height),
        ])
        .split(area);
    render_header(frame, rows[0], conversation, ai_settings);

    let body_area = rows[1].inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let lines = transcript_lines(conversation, messages, busy, started_at);
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let total_lines = paragraph.line_count(body_area.width) as u16;
    let maximum_scroll = total_lines.saturating_sub(body_area.height);
    let scroll = maximum_scroll.saturating_sub(scroll_from_bottom.min(maximum_scroll));
    frame.render_widget(paragraph.scroll((scroll, 0)), body_area);

    composer.render_ref(rows[2], frame.buffer_mut());
    if !busy {
        render_plugin_command_popup(frame, rows[1], &composer.text(), plugin_commands);
    }
    if !busy && let Some((x, y)) = composer.cursor_pos(rows[2]) {
        frame.set_cursor_position((x, y));
    }
}

fn matching_plugin_commands<'a>(
    input: &str,
    commands: &'a [PluginCommandItem],
) -> Vec<&'a PluginCommandItem> {
    let token = input.lines().next().unwrap_or_default();
    if !token.starts_with('/') || token.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let needle = token.to_ascii_lowercase();
    commands
        .iter()
        .filter(|command| {
            command
                .qualified()
                .to_ascii_lowercase()
                .starts_with(&needle)
        })
        .take(6)
        .collect()
}

fn render_plugin_command_popup(
    frame: &mut Frame<'_>,
    available_area: Rect,
    input: &str,
    commands: &[PluginCommandItem],
) {
    let matches = matching_plugin_commands(input, commands);
    if matches.is_empty() || available_area.height < 3 || available_area.width < 16 {
        return;
    }
    let height = (matches.len() as u16 + 2).min(available_area.height);
    let popup = Rect {
        x: available_area.x.saturating_add(1),
        y: available_area
            .y
            .saturating_add(available_area.height.saturating_sub(height)),
        width: available_area.width.saturating_sub(2),
        height,
    };
    frame.render_widget(Clear, popup);
    let items = matches.iter().map(|command| {
        ListItem::new(Line::from(vec![
            Span::styled(
                command.qualified(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {} · {}", command.tool, command.argument_hint()),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(" 插件命令 · Tab 补全 ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_symbol("› ")
        .highlight_style(Style::default().fg(Color::Cyan));
    let mut state = ListState::default().with_selected(Some(0));
    frame.render_stateful_widget(list, popup, &mut state);
}

fn render_header(
    frame: &mut Frame<'_>,
    area: Rect,
    conversation: &ConversationItem,
    ai_settings: Option<&AiSettings>,
) {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            conversation.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", conversation.kind_label()),
            Style::default().fg(Color::Cyan),
        ),
    ])];
    if let Some(settings) = ai_settings {
        lines.push(Line::from(vec![
            Span::styled("AI 设置  ", Style::default().fg(Color::DarkGray)),
            Span::styled("模型 ", Style::default().fg(Color::DarkGray)),
            Span::raw(settings.model.clone()),
            Span::styled("  ·  Provider ", Style::default().fg(Color::DarkGray)),
            Span::raw(settings.provider.clone()),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("会话  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                conversation.id.clone(),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    let header = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray))
            .padding(Padding::horizontal(2)),
    );
    frame.render_widget(header, area);
}

fn transcript_lines(
    conversation: &ConversationItem,
    messages: &[ChatMessage],
    busy: bool,
    started_at: Instant,
) -> Vec<Line<'static>> {
    if messages.is_empty() {
        return vec![Line::from(Span::styled(
            format!("开始与 {} 对话。", conversation.title),
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let mut lines = Vec::new();
    for message in messages {
        let (label, style) = match message.kind {
            MessageKind::User => (
                "› 你".to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            MessageKind::Peer => (
                format!("• {}", conversation.title),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            MessageKind::System => ("• 系统".to_string(), Style::default().fg(Color::Yellow)),
            MessageKind::Error => (
                "! 错误".to_string(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        };
        lines.push(Line::from(Span::styled(label, style)));
        if message.streaming && message.text.is_empty() {
            let spinner = spinner_frame(started_at);
            lines.push(Line::from(Span::styled(
                if busy {
                    format!("{spinner} 正在思考…  Esc 停止")
                } else {
                    format!("{spinner} 正在思考…")
                },
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let message_lines = message.text.split('\n');
            for line in message_lines {
                lines.push(Line::from(line.to_string()));
            }
            if message.streaming {
                lines.push(Line::from(Span::styled(
                    format!("{} 生成中…  Esc 停止", spinner_frame(started_at)),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        lines.push(Line::default());
    }
    lines
}

fn spinner_frame(started_at: Instant) -> &'static str {
    const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
    let index = (started_at.elapsed().as_millis() / 100) as usize % FRAMES.len();
    FRAMES[index]
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn is_key_press(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn recognizes_ai_and_human_conversations() {
        let ai = ConversationItem::from_value(&json!({
            "id": "codex:agent:assistant",
            "title": "Codex",
            "peer": {"type": "codexAi"}
        }))
        .unwrap();
        let miniapp = ConversationItem::from_value(&json!({
            "id": "miniapp:official.bot-father",
            "title": "机器人之父",
            "peer": {"type": "miniApp", "appId": "official.bot-father"}
        }))
        .unwrap();
        let contact = ConversationItem::from_value(&json!({
            "id": "mahayana:contact:42",
            "title": "联系人",
            "peer": {"type": "mahayanaContact", "contactId": "42"}
        }))
        .unwrap();

        assert!(ai.uses_ai());
        assert!(miniapp.uses_ai());
        assert!(!contact.uses_ai());
    }

    #[test]
    fn picker_data_does_not_depend_on_numeric_indices() {
        let conversation = ConversationItem::from_value(&json!({
            "id": "miniapp:official.flashcards",
            "title": "法流背诵卡",
            "peer": {"type": "miniApp", "appId": "official.flashcards"},
            "pinned": true,
            "unreadCount": 3
        }))
        .unwrap();

        assert_eq!(conversation.title, "法流背诵卡");
        assert!(conversation.pinned);
        assert_eq!(conversation.unread_count, 3);
    }

    #[test]
    fn miniapp_slash_commands_are_never_consumed_by_the_host() {
        let miniapp = ConversationItem::from_value(&json!({
            "id": "miniapp:official.global-dharma",
            "title": "全球法布施",
            "peer": {"type": "miniApp"}
        }))
        .unwrap();
        let contact = ConversationItem::from_value(&json!({
            "id": "mahayana:contact:42",
            "title": "联系人",
            "peer": {"type": "mahayanaContact"}
        }))
        .unwrap();

        assert_eq!(local_chat_command(&miniapp, "/quit"), None);
        assert_eq!(local_chat_command(&miniapp, "/history"), None);
        assert_eq!(local_chat_command(&miniapp, "/status"), None);
        assert_eq!(
            local_chat_command(&contact, "/quit"),
            Some(LocalChatCommand::Quit)
        );
    }

    #[test]
    fn plugin_command_completion_uses_qualified_projection() {
        let commands = vec![PluginCommandItem {
            plugin_id: "weather".into(),
            command: "forecast".into(),
            tool: "get_forecast".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}}
            }),
        }];

        let matches = matching_plugin_commands("/weather:fo", &commands);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].qualified(), "/weather:forecast");
        assert_eq!(matches[0].argument_hint(), "文本或 JSON 参数");
        assert!(matching_plugin_commands("普通消息", &commands).is_empty());
        assert!(matching_plugin_commands("/weather:forecast {}", &commands).is_empty());
    }

    #[test]
    fn conversation_picker_snapshot() {
        let conversations = vec![
            ConversationItem::from_value(&json!({
                "id": "miniapp:official.global-dharma",
                "title": "全球法布施",
                "peer": {"type": "miniApp"},
                "pinned": true,
                "unreadCount": 2
            }))
            .unwrap(),
            ConversationItem::from_value(&json!({
                "id": "miniapp:official.faliu-flashcards",
                "title": "法流记忆卡",
                "peer": {"type": "miniApp"}
            }))
            .unwrap(),
        ];
        let backend = TestBackend::new(64, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_conversation_picker(frame, &conversations, 0, &[]))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let snapshot = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect::<Vec<_>>()
            .join("\n");
        // Ratatui stores an empty continuation cell after each wide CJK glyph.
        // Compare compact text so the snapshot remains stable across backends.
        let compact = snapshot.replace(' ', "");

        assert!(compact.contains("全球法布施"), "{snapshot}");
        assert!(compact.contains("法流记忆卡"), "{snapshot}");
        assert!(compact.contains("开始与全球法布施对话"), "{snapshot}");
        assert!(
            compact.contains("↑↓/jk切换联系人Enter进入完整对话Esc/q退出"),
            "{snapshot}"
        );
    }
}
