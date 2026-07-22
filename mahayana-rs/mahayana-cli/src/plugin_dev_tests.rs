use super::*;

fn temporary_repository(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mahayana-cli-{label}-{}", uuid::Uuid::new_v4()))
}

#[test]
fn init_writes_the_plugin_beside_the_marketplace() {
    let repository = temporary_repository("marketplace-layout");

    let result = init_repository(
        &repository,
        "Example Plugin",
        Some("示例插件"),
        PluginTemplate::Conversational,
    )
    .expect("initialize plugin");

    assert_eq!(result["plugin"], "example-plugin");
    assert!(
        repository
            .join(".agents/plugins/plugins/example-plugin/.codex-plugin/plugin.json")
            .is_file()
    );
    assert!(!repository.join("plugins/example-plugin").exists());
    let marketplace: Value = serde_json::from_str(
        &fs::read_to_string(repository.join(".agents/plugins/marketplace.json"))
            .expect("read marketplace"),
    )
    .expect("parse marketplace");
    assert_eq!(
        marketplace.pointer("/plugins/0/source/path"),
        Some(&json!("./.agents/plugins/plugins/example-plugin"))
    );
    validate_path(&repository).expect("validate repository marketplace");

    fs::remove_dir_all(repository).expect("remove test repository");
}

#[test]
fn desktop_approval_profile_declares_scoped_permissions_and_tools() {
    let repository = temporary_repository("desktop-approval");

    let result = init_repository(
        &repository,
        "ChatGPT Auto Confirm",
        Some("ChatGPT 自动确认"),
        PluginTemplate::DesktopApproval,
    )
    .expect("initialize desktop approval plugin");

    assert_eq!(result["profile"], "desktop-approval");
    let plugin_root = repository.join(".agents/plugins/plugins/chatgpt-auto-confirm");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(plugin_root.join(".mahayana/plugin.json"))
            .expect("read Mahayana manifest"),
    )
    .expect("parse Mahayana manifest");
    assert_eq!(
        manifest.pointer("/miniapp/permissions/3"),
        Some(&json!("desktop.chatgpt.approvals"))
    );
    assert!(
        fs::read_to_string(plugin_root.join("worker/src/index.ts"))
            .expect("read worker")
            .contains("desktop.chatgpt-approvals.start")
    );
    assert!(plugin_root.join("server/index.mjs").is_file());
    assert!(
        plugin_root
            .join("test/standalone-runtime.test.mjs")
            .is_file()
    );
    let codex_manifest: Value = serde_json::from_str(
        &fs::read_to_string(plugin_root.join(".codex-plugin/plugin.json"))
            .expect("read Codex manifest"),
    )
    .expect("parse Codex manifest");
    assert!(codex_manifest.get("runtimeVariants").is_none());
    assert_eq!(
        manifest.pointer("/supportedPlatforms"),
        Some(&json!(["cli", "desktop"]))
    );
    let mcp: Value = serde_json::from_str(
        &fs::read_to_string(plugin_root.join(".mcp.json")).expect("read MCP manifest"),
    )
    .expect("parse MCP manifest");
    assert_eq!(
        mcp.pointer("/mcpServers/chatgpt-auto-confirm-local/command"),
        Some(&json!("node"))
    );
    assert!(mcp.pointer("/mcpServers/chatgpt-auto-confirm").is_none());
    validate_path(&plugin_root).expect("validate desktop approval plugin");

    fs::remove_dir_all(repository).expect("remove test repository");
}

#[test]
fn generated_plugin_tests_run_through_the_cli_workbench() {
    let repository = temporary_repository("plugin-test");
    init_repository(
        &repository,
        "CLI Tested Plugin",
        Some("CLI 测试插件"),
        PluginTemplate::Conversational,
    )
    .expect("initialize plugin");

    let report = test_path(&repository.join(".agents/plugins/plugins/cli-tested-plugin"))
        .expect("run plugin tests");

    assert_eq!(report.pointer("/suites/0/executed"), Some(&json!(true)));
    assert_eq!(report.pointer("/suites/0/status"), Some(&json!("passed")));
    fs::remove_dir_all(repository).expect("remove test repository");
}

#[test]
fn plugin_site_distribution_is_served_by_the_generated_worker() {
    let repository = temporary_repository("site-distribution");
    init_repository(
        &repository,
        "Site Plugin",
        Some("站点插件"),
        PluginTemplate::Conversational,
    )
    .expect("initialize plugin");
    let plugin_root = repository.join(".agents/plugins/plugins/site-plugin");
    let wrangler = fs::read_to_string(plugin_root.join("wrangler.toml")).expect("read wrangler");
    assert!(wrangler.contains(".mahayana-distribution"));

    let distribution = prepare_site_distribution(
        &plugin_root,
        "site-plugin",
        "1.2.3",
        &"a".repeat(64),
        &[0x1f, 0x8b, 0x08, 0x00],
    )
    .expect("prepare distribution");
    assert!(distribution.join("index.html").is_file());
    assert!(distribution.join("mahayana/plugin.tar.gz").is_file());
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(distribution.join("mahayana/plugin.json")).expect("read site manifest"),
    )
    .expect("parse site manifest");
    assert_eq!(manifest["pluginId"], "site-plugin");
    assert_eq!(manifest["runtime"], "independent-worker-or-pages");

    fs::remove_dir_all(repository).expect("remove test repository");
}

#[test]
fn deployment_output_accepts_only_cloudflare_plugin_sites() {
    assert_eq!(
        deployment_url_from_output("Deployed https://site-plugin.bhrum.workers.dev"),
        Some("https://site-plugin.bhrum.workers.dev".to_string())
    );
    assert_eq!(
        deployment_url_from_output("https://site-plugin.pages.dev/"),
        Some("https://site-plugin.pages.dev".to_string())
    );
    assert_eq!(deployment_url_from_output("https://evil.example"), None);
}

#[test]
fn marketplace_install_extracts_a_verified_pages_bundle_into_the_repository() {
    let source_repository = temporary_repository("market-source");
    init_repository(
        &source_repository,
        "Market Plugin",
        Some("市场插件"),
        PluginTemplate::Conversational,
    )
    .expect("initialize source plugin");
    let source_plugin = source_repository.join(".agents/plugins/plugins/market-plugin");
    let archive = codex_core_plugins::plugin_bundle_archive::pack_plugin_bundle_tar_gz(
        &source_plugin,
        50 * 1024 * 1024,
    )
    .expect("pack plugin");

    let destination_repository = temporary_repository("market-destination");
    fs::create_dir_all(&destination_repository).expect("create destination repository");
    let receipt =
        install_marketplace_bundle(&destination_repository, "market-plugin", "1.0.0", &archive)
            .expect("install pages bundle");
    assert_eq!(receipt["installed"], true);
    assert_eq!(receipt["source"], "independent-pages-or-worker");
    assert!(
        destination_repository
            .join(".agents/plugins/plugins/market-plugin/.codex-plugin/plugin.json")
            .is_file()
    );
    validate_path(&destination_repository).expect("validate installed marketplace");

    fs::remove_dir_all(source_repository).expect("remove source repository");
    fs::remove_dir_all(destination_repository).expect("remove destination repository");
}
