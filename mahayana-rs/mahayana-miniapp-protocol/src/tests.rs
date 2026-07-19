use super::*;
use serde_json::json;

fn app() -> AppSummary {
    AppSummary {
        id: "global-dharma".into(),
        title: "全球法布施".into(),
        version: "1.0.0".into(),
        source: None,
    }
}

#[test]
fn compiles_deterministic_home_and_lazy_resources() {
    let sources = vec![
        ContentSource {
            path: "content/articles/intro.md".into(),
            kind: SourceKind::Article,
            markdown: "---\nid: intro\nrevision: '1'\ntitle: 使用指南\npublishedAt: 2026-07-19\nsummary: 三种使用模式\ntags: [指南]\n---\n# 使用指南\n\n正文".into(),
        },
        ContentSource {
            path: "content/welcome.md".into(),
            kind: SourceKind::Welcome,
            markdown: "---\nid: welcome\nrevision: '1'\n---\n欢迎使用。".into(),
        },
    ];
    let first = ContentCompiler::compile(app(), sources.clone(), Vec::new()).unwrap();
    let second = ContentCompiler::compile(app(), sources, Vec::new()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.home.schema, HOME_SCHEMA);
    assert_eq!(
        first.home.feed.items[0].resource_uri,
        "mahayana://global-dharma/content/articles/intro"
    );
    assert_eq!(first.resources[0].1, "# 使用指南\n\n正文");
}

#[test]
fn reads_home_from_standard_mcp_result() {
    let compiled = ContentCompiler::compile(
        app(),
        [ContentSource {
            path: "content/welcome.md".into(),
            kind: SourceKind::Welcome,
            markdown: "---\nid: welcome\nrevision: '1'\n---\n欢迎。".into(),
        }],
        Vec::new(),
    )
    .unwrap();
    let result = json!({"structuredContent": compiled.home});
    assert!(HomeDocument::from_tool_result(&result).unwrap().is_some());
    assert!(
        HomeDocument::from_tool_result(&json!({"content": []}))
            .unwrap()
            .is_none()
    );
}

#[test]
fn rejects_oversized_first_screen() {
    let result = ContentCompiler::compile(
        app(),
        [ContentSource {
            path: "content/welcome.md".into(),
            kind: SourceKind::Welcome,
            markdown: format!(
                "---\nid: welcome\nrevision: '1'\n---\n{}",
                "佛".repeat(MAX_HOME_BYTES)
            ),
        }],
        Vec::new(),
    );
    assert!(result.unwrap_err().contains("at most 32768"));
}

#[test]
fn parses_chat_disposition() {
    let disposition = ChatDisposition::from_tool_result(
        &json!({"structuredContent":{"handled":false,"mode":"idle"}}),
    )
    .unwrap();
    assert!(!disposition.handled);
    assert_eq!(disposition.mode.as_deref(), Some("idle"));
}
