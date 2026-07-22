use super::plugin_dev_template::template_files;
use codex_core_plugins::plugin_bundle_archive::unpack_plugin_bundle_tar_gz;
use mahayana_miniapp_protocol::AppSummary;
use mahayana_miniapp_protocol::CompiledContent;
use mahayana_miniapp_protocol::ContentCompiler;
use mahayana_miniapp_protocol::ContentSource;
use mahayana_miniapp_protocol::SourceKind;
use mahayana_platform_core::HostPlatform;
use mahayana_plugin_host::LocalPlugin;
use serde_json::Value;
use serde_json::json;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

pub use super::plugin_dev_template::PluginTemplate;

const SITE_DISTRIBUTION_DIR: &str = ".mahayana-distribution";

pub fn supported_marketplace_platforms(plugin_path: &Path) -> Result<Vec<String>, String> {
    let plugin = LocalPlugin::load(plugin_path).map_err(|error| error.to_string())?;
    if let Some(extension) = plugin.mahayana
        && !extension.supported_platforms.is_empty()
    {
        return Ok(extension
            .supported_platforms
            .into_iter()
            .map(|platform| match platform {
                HostPlatform::Cli => "cli",
                HostPlatform::Desktop => "desktop",
                HostPlatform::Mobile => "mobile",
                HostPlatform::Web => "web",
            })
            .map(str::to_string)
            .collect());
    }
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(plugin_path.join(".codex-plugin/plugin.json"))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let variants = manifest
        .get("runtimeVariants")
        .and_then(Value::as_array)
        .ok_or_else(|| "plugin runtimeVariants must be an array".to_string())?;
    let mut platforms = Vec::new();
    for platform in variants
        .iter()
        .filter_map(|variant| variant.get("platforms").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
    {
        if matches!(platform, "cli" | "desktop" | "mobile" | "web")
            && !platforms.iter().any(|existing| existing == platform)
        {
            platforms.push(platform.to_string());
        }
    }
    if platforms.is_empty() {
        return Err("plugin does not declare a supported marketplace platform".into());
    }
    Ok(platforms)
}

pub fn install_marketplace_bundle(
    repository: &Path,
    plugin_id: &str,
    version: &str,
    archive: &[u8],
) -> Result<Value, String> {
    let repository = absolute_path(repository)?;
    let plugin_id = normalized_name(plugin_id)?;
    let plugins_root = repository.join(".agents/plugins/plugins");
    let destination = plugins_root.join(&plugin_id);
    if destination.exists() {
        return Err(format!(
            "插件 {plugin_id} 已安装；请先明确卸载或使用更新流程"
        ));
    }
    fs::create_dir_all(&plugins_root).map_err(|error| error.to_string())?;
    let staging = plugins_root.join(format!(".install-{}-{}", plugin_id, uuid::Uuid::new_v4()));
    let result = (|| {
        unpack_plugin_bundle_tar_gz(archive, &staging, 100 * 1024 * 1024)
            .map_err(|error| error.to_string())?;
        let validation = validate_plugin(&staging)?;
        fs::rename(&staging, &destination).map_err(|error| error.to_string())?;
        update_marketplace(
            &repository.join(".agents/plugins/marketplace.json"),
            &plugin_id,
        )?;
        Ok(json!({
            "installed": true,
            "pluginId": plugin_id,
            "version": version,
            "source": "independent-pages-or-worker",
            "pluginRoot": destination,
            "validation": validation,
        }))
    })();
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    if result.is_err() && destination.exists() {
        let _ = fs::remove_dir_all(&destination);
    }
    result
}

pub fn prepare_site_distribution(
    plugin_path: &Path,
    plugin_id: &str,
    version: &str,
    package_sha256: &str,
    archive: &[u8],
) -> Result<PathBuf, String> {
    let wrangler = fs::read_to_string(plugin_path.join("wrangler.toml"))
        .map_err(|_| "插件必须包含 wrangler.toml 才能作为独立 Worker/Pages 发布".to_string())?;
    if !wrangler.contains(SITE_DISTRIBUTION_DIR) {
        return Err(format!(
            "wrangler.toml 必须声明 [assets] directory = \"{SITE_DISTRIBUTION_DIR}\""
        ));
    }
    let distribution = plugin_path.join(SITE_DISTRIBUTION_DIR);
    let mahayana = distribution.join("mahayana");
    fs::create_dir_all(&mahayana).map_err(|error| error.to_string())?;
    fs::write(mahayana.join("plugin.tar.gz"), archive).map_err(|error| error.to_string())?;
    let manifest = json!({
        "schemaVersion": 1,
        "pluginId": plugin_id,
        "version": version,
        "packagePath": "/mahayana/plugin.tar.gz",
        "packageSha256": package_sha256,
        "packageSize": archive.len(),
        "runtime": "independent-worker-or-pages",
    });
    fs::write(
        mahayana.join("plugin.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let title = escape_html(plugin_id);
    let version = escape_html(version);
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{title} · 大乘插件</title><main style=\"font:16px system-ui;max-width:680px;margin:12vh auto;padding:24px\"><h1>{title}</h1><p>独立部署的大乘插件，版本 {version}。</p><p><a href=\"/mahayana/plugin.tar.gz\">下载并安装插件内容</a></p><p><a href=\"/mahayana/plugin.json\">查看可验证清单</a></p></main>"
    );
    fs::write(distribution.join("index.html"), html).map_err(|error| error.to_string())?;
    Ok(distribution)
}

pub fn deploy_plugin_site(plugin_path: &Path) -> Result<String, String> {
    let package: Value = serde_json::from_str(
        &fs::read_to_string(plugin_path.join("package.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if package
        .pointer("/scripts/deploy")
        .and_then(Value::as_str)
        .is_none()
    {
        return Err("插件 package.json 必须声明 deploy 脚本".into());
    }
    let output = Command::new("npm")
        .args(["run", "deploy"])
        .current_dir(plugin_path)
        .output()
        .map_err(|error| format!("failed to start plugin deployment: {error}"))?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(format!("插件 Worker/Pages 部署失败：{}", combined.trim()));
    }
    deployment_url_from_output(&combined).ok_or_else(|| {
        "部署成功但未返回可验证的 workers.dev/pages.dev HTTPS 地址；请用 --deployment-url 指定"
            .to_string()
    })
}

fn deployment_url_from_output(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|word| {
        let candidate = word.trim_matches(|character: char| {
            matches!(
                character,
                '`' | '\'' | '"' | '(' | ')' | '[' | ']' | ',' | ';'
            )
        });
        let mut url = url::Url::parse(candidate).ok()?;
        let host = url.host_str()?.to_ascii_lowercase();
        if url.scheme() != "https"
            || !(host.ends_with(".workers.dev") || host.ends_with(".pages.dev"))
        {
            return None;
        }
        let path = url.path().trim_end_matches('/').to_string();
        url.set_path(&path);
        Some(url.to_string().trim_end_matches('/').to_string())
    })
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub struct RemoteRepository {
    pub marketplace: String,
    pub plugins: Vec<String>,
    pub has_local_runtimes: bool,
}

pub fn looks_like_github_source(source: &str) -> bool {
    source.starts_with("https://github.com/")
        || source.starts_with("ssh://git@github.com/")
        || source.starts_with("git@github.com:")
        || (source.split('/').count() == 2 && !source.contains(':'))
}

pub fn validate_github_source(source: &str) -> Result<RemoteRepository, String> {
    let source = github_clone_source(source)?;
    let checkout = std::env::temp_dir().join(format!("mahayana-plugin-{}", uuid::Uuid::new_v4()));
    let output = Command::new("git")
        .args(["clone", "--depth", "1", "--filter=blob:none", &source])
        .arg(&checkout)
        .output()
        .map_err(|error| format!("failed to start git clone: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "failed to clone plugin repository: {}",
            error.trim()
        ));
    }
    let result = inspect_remote_checkout(&checkout);
    let _ = fs::remove_dir_all(&checkout);
    result
}

fn github_clone_source(source: &str) -> Result<String, String> {
    if source.split('/').count() == 2 && !source.contains(':') {
        return Ok(format!("https://github.com/{source}"));
    }
    if let Ok(url) = url::Url::parse(source) {
        if url.host_str() != Some("github.com") {
            return Err("plugin install accepts GitHub repositories only".into());
        }
        if !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
            return Err("GitHub source must not embed credentials or query parameters".into());
        }
        return Ok(source.into());
    }
    if source.starts_with("git@github.com:") {
        return Ok(source.into());
    }
    Err("plugin install requires an HTTPS or SSH GitHub repository URL".into())
}

fn inspect_remote_checkout(checkout: &Path) -> Result<RemoteRepository, String> {
    let report = validate_path(checkout)?;
    let marketplace_path = checkout.join(".agents/plugins/marketplace.json");
    let marketplace: Value = serde_json::from_str(
        &fs::read_to_string(marketplace_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let marketplace_name = marketplace
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| "marketplace name is required".to_string())?
        .to_string();
    let plugins = marketplace
        .get("plugins")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry
                .pointer("/policy/installation")
                .and_then(Value::as_str)
                != Some("NOT_AVAILABLE")
        })
        .filter_map(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    if plugins.is_empty() {
        return Err("repository marketplace has no installable plugins".into());
    }
    let has_local_runtimes = report
        .get("plugins")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|plugin| {
            plugin
                .get("localServers")
                .and_then(Value::as_array)
                .is_some_and(|servers| !servers.is_empty())
        });
    Ok(RemoteRepository {
        marketplace: marketplace_name,
        plugins,
        has_local_runtimes,
    })
}

pub fn init_repository(
    repository: &Path,
    requested_name: &str,
    requested_title: Option<&str>,
    template: PluginTemplate,
) -> Result<Value, String> {
    let repository = absolute_path(repository)?;
    let name = normalized_name(requested_name)?;
    let title = requested_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(requested_name.trim());
    let plugin_root = repository.join(".agents/plugins/plugins").join(&name);
    let marketplace = repository.join(".agents/plugins/marketplace.json");
    if plugin_root.exists() {
        return Err(format!(
            "refusing to overwrite existing plugin directory {}",
            plugin_root.display()
        ));
    }
    let files = template_files(&name, title, template)?;
    for (relative, content) in &files {
        write_new(&plugin_root.join(relative), content)?;
    }
    let validation = match validate_plugin(&plugin_root) {
        Ok(validation) => validation,
        Err(error) => {
            let _ = fs::remove_dir_all(&plugin_root);
            return Err(error);
        }
    };
    if let Err(error) = update_marketplace(&marketplace, &name) {
        let _ = fs::remove_dir_all(&plugin_root);
        return Err(error);
    }
    Ok(json!({
        "created":true,
        "plugin":name,
        "profile":template.to_string(),
        "pluginRoot":plugin_root,
        "marketplace":marketplace,
        "files":files.keys().collect::<Vec<_>>(),
        "validation":validation,
    }))
}

#[cfg(test)]
#[path = "plugin_dev_tests.rs"]
mod tests;

pub fn validate_path(path: &Path) -> Result<Value, String> {
    let path = absolute_path(path)?;
    if path.join(".codex-plugin/plugin.json").is_file() {
        return validate_plugin(&path);
    }
    let marketplace_path = path.join(".agents/plugins/marketplace.json");
    let marketplace: Value =
        serde_json::from_str(&fs::read_to_string(&marketplace_path).map_err(|error| {
            format!(
                "{} is neither a plugin nor a repository marketplace: {error}",
                path.display()
            )
        })?)
        .map_err(|error| format!("invalid marketplace: {error}"))?;
    let entries = marketplace
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| "marketplace plugins must be an array".to_string())?;
    let mut reports = Vec::new();
    for entry in entries {
        let relative = validate_marketplace_entry(entry)?;
        reports.push(validate_plugin(&resolve_relative(&path, relative)?)?);
    }
    Ok(json!({
        "valid":true,
        "marketplace":marketplace.get("name"),
        "plugins":reports,
    }))
}

pub fn test_path(path: &Path) -> Result<Value, String> {
    let path = absolute_path(path)?;
    let validation = validate_path(&path)?;
    let plugin_roots = if path.join(".codex-plugin/plugin.json").is_file() {
        vec![path.clone()]
    } else {
        let marketplace_path = path.join(".agents/plugins/marketplace.json");
        let marketplace: Value =
            serde_json::from_str(&fs::read_to_string(&marketplace_path).map_err(|error| {
                format!("failed to read {}: {error}", marketplace_path.display())
            })?)
            .map_err(|error| format!("invalid marketplace: {error}"))?;
        marketplace
            .get("plugins")
            .and_then(Value::as_array)
            .ok_or_else(|| "marketplace plugins must be an array".to_string())?
            .iter()
            .map(validate_marketplace_entry)
            .map(|relative| relative.and_then(|relative| resolve_relative(&path, relative)))
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut suites = Vec::new();
    for plugin_root in plugin_roots {
        let plugin = LocalPlugin::load(&plugin_root).map_err(|error| error.to_string())?;
        let package_path = plugin_root.join("package.json");
        let test_script = fs::read_to_string(&package_path)
            .ok()
            .and_then(|source| serde_json::from_str::<Value>(&source).ok())
            .and_then(|package| {
                package
                    .pointer("/scripts/test")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let Some(test_script) = test_script else {
            suites.push(json!({
                "plugin":plugin.codex.name,
                "executed":false,
                "reason":"package.json scripts.test is not declared",
            }));
            continue;
        };
        let status = Command::new("npm")
            .arg("test")
            .current_dir(&plugin_root)
            .env("CI", "1")
            .status()
            .map_err(|error| {
                format!(
                    "failed to start npm test for {}: {error}",
                    plugin_root.display()
                )
            })?;
        if !status.success() {
            return Err(format!(
                "plugin test failed for {} with status {}",
                plugin.codex.name, status
            ));
        }
        suites.push(json!({
            "plugin":plugin.codex.name,
            "executed":true,
            "runner":"npm test",
            "script":test_script,
            "status":"passed",
        }));
    }
    Ok(json!({
        "valid":true,
        "validation":validation,
        "suites":suites,
    }))
}

pub(super) fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| format!("failed to resolve {}: {error}", path.display()))
}

fn validate_plugin(plugin_root: &Path) -> Result<Value, String> {
    let plugin = LocalPlugin::load(plugin_root).map_err(|error| error.to_string())?;
    let mcp_path = plugin_root.join(".mcp.json");
    let mcp: Value = serde_json::from_str(
        &fs::read_to_string(&mcp_path)
            .map_err(|error| format!("failed to read {}: {error}", mcp_path.display()))?,
    )
    .map_err(|error| format!("invalid .mcp.json: {error}"))?;
    let servers = mcp
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or_else(|| ".mcp.json requires mcpServers".to_string())?;
    if servers.is_empty() {
        return Err(".mcp.json requires at least one server".into());
    }
    let mut remote_servers = Vec::new();
    let mut local_servers = Vec::new();
    for (name, server) in servers {
        if let Some(endpoint) = server.get("url").and_then(Value::as_str) {
            let endpoint = url::Url::parse(endpoint)
                .map_err(|error| format!("MCP server {name} URL is invalid: {error}"))?;
            if endpoint.scheme() != "https" || endpoint.host_str().is_none() {
                return Err(format!("MCP server {name} must use an absolute HTTPS URL"));
            }
            if !endpoint.username().is_empty() || endpoint.password().is_some() {
                return Err(format!("MCP server {name} URL must not embed credentials"));
            }
            remote_servers.push(name);
        } else if server.get("command").is_some() {
            local_servers.push(name);
        } else {
            return Err(format!("MCP server {name} requires url or command"));
        }
    }
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(plugin_root.join(".codex-plugin/plugin.json"))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("0.0.0");
    let title = manifest
        .pointer("/interface/displayName")
        .and_then(Value::as_str)
        .unwrap_or(&plugin.codex.name);
    let compiled = plugin_root
        .join("content")
        .is_dir()
        .then(|| compile_content(plugin_root, &plugin.codex.name, title, version))
        .transpose()?;
    let cloudflare_template_files = [
        "wrangler.toml",
        "worker/src/index.ts",
        "server/index.mjs",
        "ui/index.html",
        "test/contract.test.mjs",
        "test/standalone-runtime.test.mjs",
        ".github/workflows/deploy-cloudflare.yml",
    ];
    let has_cloudflare_template = cloudflare_template_files
        .iter()
        .any(|relative| plugin_root.join(relative).is_file());
    if has_cloudflare_template {
        for required in cloudflare_template_files {
            if !plugin_root.join(required).is_file() {
                return Err(format!(
                    "plugin is missing required template file {required}"
                ));
            }
        }
    }
    let mut checks = vec!["manifest", "content", "mcp-config", "https"];
    if has_cloudflare_template {
        checks.extend(["cloudflare", "contract-test"]);
    }
    Ok(json!({
        "valid":true,
        "plugin":plugin.codex.name,
        "mahayanaExtension":plugin.mahayana.is_some(),
        "homeSchema":compiled.as_ref().map(|content|content.home.schema.as_str()),
        "contentRevision":compiled.as_ref().map(|content|content.home.revision.as_str()),
        "feedItems":compiled.as_ref().map(|content|content.home.feed.items.len()).unwrap_or(0),
        "resources":compiled.as_ref().map(|content|content.resources.len()).unwrap_or(0),
        "remoteServers":remote_servers,
        "localServers":local_servers,
        "localExecutionRequiresSeparateApproval":!local_servers.is_empty(),
        "checks":checks,
    }))
}

fn compile_content(
    plugin_root: &Path,
    name: &str,
    title: &str,
    version: &str,
) -> Result<CompiledContent, String> {
    let content = plugin_root.join("content");
    let mut sources = Vec::new();
    let welcome = content.join("welcome.md");
    if welcome.is_file() {
        sources.push(ContentSource {
            path: "content/welcome.md".into(),
            kind: SourceKind::Welcome,
            markdown: fs::read_to_string(welcome).map_err(|error| error.to_string())?,
        });
    }
    for (folder, kind) in [
        ("tips", SourceKind::Tip),
        ("announcements", SourceKind::Announcement),
        ("articles", SourceKind::Article),
    ] {
        let mut entries = fs::read_dir(content.join(folder))
            .map(|entries| entries.filter_map(Result::ok).collect::<Vec<_>>())
            .unwrap_or_default();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                continue;
            }
            sources.push(ContentSource {
                path: format!("content/{folder}/{}", entry.file_name().to_string_lossy()),
                kind,
                markdown: fs::read_to_string(path).map_err(|error| error.to_string())?,
            });
        }
    }
    ContentCompiler::compile(
        AppSummary {
            id: name.into(),
            title: title.into(),
            version: version.into(),
            source: None,
        },
        sources,
        Vec::new(),
    )
}

fn validate_marketplace_entry(entry: &Value) -> Result<&str, String> {
    let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
    if normalized_name(name)? != name {
        return Err(format!("marketplace plugin name is not normalized: {name}"));
    }
    if entry.pointer("/source/source").and_then(Value::as_str) != Some("local") {
        return Err(format!("marketplace plugin {name} must use local source"));
    }
    if !matches!(
        entry
            .pointer("/policy/installation")
            .and_then(Value::as_str),
        Some("AVAILABLE" | "INSTALLED_BY_DEFAULT" | "NOT_AVAILABLE")
    ) {
        return Err(format!(
            "marketplace plugin {name} has invalid installation policy"
        ));
    }
    if !matches!(
        entry
            .pointer("/policy/authentication")
            .and_then(Value::as_str),
        Some("ON_INSTALL" | "ON_USE")
    ) {
        return Err(format!(
            "marketplace plugin {name} has invalid authentication policy"
        ));
    }
    if entry.get("category").and_then(Value::as_str).is_none() {
        return Err(format!("marketplace plugin {name} requires category"));
    }
    entry
        .pointer("/source/path")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("marketplace plugin {name} requires source.path"))
}

fn update_marketplace(path: &Path, name: &str) -> Result<(), String> {
    let mut marketplace = if path.is_file() {
        serde_json::from_str::<Value>(&fs::read_to_string(path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid existing marketplace: {error}"))?
    } else {
        json!({"name":"mahayana-repository","interface":{"displayName":"大乘仓库插件"},"plugins":[]})
    };
    let plugins = marketplace
        .get_mut("plugins")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "marketplace plugins must be an array".to_string())?;
    if plugins
        .iter()
        .any(|plugin| plugin.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("marketplace already contains plugin {name}"));
    }
    plugins.push(json!({
        "name":name,
        "source":{"source":"local","path":format!("./.agents/plugins/plugins/{name}")},
        "policy":{"installation":"AVAILABLE","authentication":"ON_INSTALL"},
        "category":"Productivity"
    }));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(&marketplace).map_err(|error| error.to_string())? + "\n",
    )
    .map_err(|error| error.to_string())
}

fn write_new(path: &Path, content: &str) -> Result<(), String> {
    if path.exists() {
        return Err(format!("refusing to overwrite {}", path.display()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, content).map_err(|error| error.to_string())
}

fn resolve_relative(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if !relative.starts_with("./") {
        return Err(format!("plugin path must start with ./: {relative}"));
    }
    let path = Path::new(relative);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!("plugin path escapes marketplace root: {relative}"));
    }
    Ok(root.join(path))
}

fn normalized_name(value: &str) -> Result<String, String> {
    let mut normalized = String::new();
    let mut separator = false;
    for character in value.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            normalized.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    let normalized = normalized.trim_matches('-').to_string();
    let starts_with_letter = normalized
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase());
    if normalized.is_empty() || normalized.len() > 64 || !starts_with_letter {
        return Err(
            "plugin name must normalize to 1-64 kebab-case characters starting with a letter"
                .into(),
        );
    }
    Ok(normalized)
}
