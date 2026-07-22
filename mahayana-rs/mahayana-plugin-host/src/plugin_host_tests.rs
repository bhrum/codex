use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[test]
fn runtime_selection_uses_codex_platform_priority() {
    let variants = vec![
        PluginRuntimeVariant {
            id: "http".into(),
            server: "remote".into(),
            platforms: vec![PluginRuntimePlatform::Cli, PluginRuntimePlatform::Desktop],
            priority: 10,
        },
        PluginRuntimeVariant {
            id: "local".into(),
            server: "local".into(),
            platforms: vec![PluginRuntimePlatform::Cli],
            priority: 20,
        },
    ];

    assert_eq!(
        select_runtime(
            HostPlatform::Cli,
            &["local".into(), "remote".into()],
            &variants,
        )
        .expect("select runtime"),
        SelectedRuntime {
            variant_id: Some("local".into()),
            server: "local".into(),
        }
    );
}

#[test]
fn runtime_selection_falls_back_when_bundled_cli_is_unavailable() {
    let variants = vec![
        PluginRuntimeVariant {
            id: "account-http".into(),
            server: "remote".into(),
            platforms: vec![PluginRuntimePlatform::Desktop],
            priority: 100,
        },
        PluginRuntimeVariant {
            id: "local-cli".into(),
            server: "local".into(),
            platforms: vec![PluginRuntimePlatform::Desktop],
            priority: 300,
        },
    ];

    assert_eq!(
        select_runtime_with_availability(
            HostPlatform::Desktop,
            &["local".into(), "remote".into()],
            &variants,
            |server| server == "remote",
        )
        .expect("select fallback runtime"),
        SelectedRuntime {
            variant_id: Some("account-http".into()),
            server: "remote".into(),
        }
    );
}

#[test]
fn namespaced_tui_command_keeps_json_arguments() {
    assert_eq!(
        PluginCommandInvocation::parse_tui(r#"/weather:forecast {"city":"北京"}"#)
            .expect("parse command"),
        PluginCommandInvocation {
            plugin_id: "weather".into(),
            command: "forecast".into(),
            arguments: json!({"city": "北京"}),
        }
    );
}

#[test]
fn official_plugins_use_the_codex_manifest_and_mahayana_extension_together() {
    let plugins_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../.agents/plugins/plugins");
    let mut plugin_names = fs::read_dir(&plugins_root)
        .expect("official plugins directory")
        .map(|entry| entry.expect("plugin directory").path())
        .filter(|path| path.is_dir())
        .map(|path| {
            let plugin = LocalPlugin::load(&path).expect("valid combined plugin manifests");
            assert!(plugin.mahayana.is_some(), "missing extension at {path:?}");
            plugin.codex.name
        })
        .collect::<Vec<_>>();
    plugin_names.sort();
    assert_eq!(
        plugin_names,
        vec![
            "bot-father",
            "chatgpt-auto-confirm",
            "computer-cleaner",
            "faliu-flashcards",
            "global-dharma",
            "hermes-installer",
            "mahayana-assistant",
            "platform-publish",
        ]
    );
}
