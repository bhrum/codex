use super::*;
use pretty_assertions::assert_eq;
use std::fs;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[test]
fn loads_minimal_extension_without_redefining_codex_identity() {
    let root = temporary_plugin_root();
    fs::create_dir_all(root.join(".mahayana")).expect("create extension directory");
    fs::create_dir_all(root.join("miniapp")).expect("create miniapp directory");
    fs::write(root.join("miniapp/index.html"), "<!doctype html>").expect("write entry");
    fs::write(
        root.join(".mahayana/plugin.json"),
        r#"{
          "schemaVersion": 1,
          "miniapp": {
            "entry": "./miniapp/index.html",
            "bridgeVersion": "1.0",
            "permissions": ["mcp.call", "commerce.purchase"]
          },
          "commands": [{"name": "forecast", "tool": "get_forecast", "aliases": ["weather"]}],
          "gates": [{"target": "tool:get_forecast", "entitlement": "weather.pro"}]
        }"#,
    )
    .expect("write extension");

    let manifest = MahayanaPluginManifest::load(&root)
        .expect("load extension")
        .expect("extension exists");

    assert_eq!(
        manifest.commands,
        vec![CommandDeclaration {
            name: "forecast".into(),
            tool: "get_forecast".into(),
            aliases: vec!["weather".into()],
        }]
    );
}

#[test]
fn rejects_codex_identity_fields_in_extension() {
    let root = temporary_plugin_root();
    fs::create_dir_all(root.join(".mahayana")).expect("create extension directory");
    fs::write(
        root.join(".mahayana/plugin.json"),
        r#"{"schemaVersion":1,"name":"duplicate-identity"}"#,
    )
    .expect("write extension");

    let error = MahayanaPluginManifest::load(&root).expect_err("unknown field must fail");
    assert!(matches!(error, ManifestError::Decode { .. }));
}

#[test]
fn cli_runtime_accepts_safe_path_commands_and_rejects_escaping_paths() {
    assert!(validate_cli_executable("node").is_ok());
    assert!(validate_cli_executable("node.exe").is_ok());
    assert!(validate_cli_executable("./runtime/cli/plugin").is_ok());
    assert!(validate_cli_executable("../outside/plugin").is_err());
    assert!(validate_cli_executable("/usr/bin/node").is_err());
    assert!(validate_cli_executable("node --eval").is_err());
}

fn temporary_plugin_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("mahayana-extension-{nonce}"));
    fs::create_dir_all(&root).expect("create plugin root");
    root
}
