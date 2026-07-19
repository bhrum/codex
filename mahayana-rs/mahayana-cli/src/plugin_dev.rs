use super::plugin_dev_template::template_files;
use mahayana_miniapp_protocol::{
    AppSummary, CompiledContent, ContentCompiler, ContentSource, SourceKind,
};
use mahayana_plugin_host::LocalPlugin;
use serde_json::{Value, json};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

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
) -> Result<Value, String> {
    let name = normalized_name(requested_name)?;
    let title = requested_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(requested_name.trim());
    let plugin_root = repository.join("plugins").join(&name);
    let marketplace = repository.join(".agents/plugins/marketplace.json");
    if plugin_root.exists() {
        return Err(format!(
            "refusing to overwrite existing plugin directory {}",
            plugin_root.display()
        ));
    }
    let files = template_files(&name, title)?;
    for (relative, content) in &files {
        write_new(&plugin_root.join(relative), content)?;
    }
    update_marketplace(&marketplace, &name)?;
    Ok(json!({
        "created":true,
        "plugin":name,
        "pluginRoot":plugin_root,
        "marketplace":marketplace,
        "files":files.keys().collect::<Vec<_>>(),
        "validation":validate_plugin(&plugin_root)?,
    }))
}

pub fn validate_path(path: &Path) -> Result<Value, String> {
    if path.join(".codex-plugin/plugin.json").is_file() {
        return validate_plugin(path);
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
        reports.push(validate_plugin(&resolve_relative(path, relative)?)?);
    }
    Ok(json!({
        "valid":true,
        "marketplace":marketplace.get("name"),
        "plugins":reports,
    }))
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
    let compiled = compile_content(plugin_root, &plugin.codex.name, title, version)?;
    for required in [
        "wrangler.toml",
        "worker/src/index.ts",
        "ui/index.html",
        "test/contract.test.mjs",
        ".github/workflows/deploy-cloudflare.yml",
    ] {
        if !plugin_root.join(required).is_file() {
            return Err(format!(
                "plugin is missing required template file {required}"
            ));
        }
    }
    Ok(json!({
        "valid":true,
        "plugin":plugin.codex.name,
        "mahayanaExtension":plugin.mahayana.is_some(),
        "homeSchema":compiled.home.schema,
        "contentRevision":compiled.home.revision,
        "feedItems":compiled.home.feed.items.len(),
        "resources":compiled.resources.len(),
        "remoteServers":remote_servers,
        "localServers":local_servers,
        "localExecutionRequiresSeparateApproval":!local_servers.is_empty(),
        "checks":["manifest","content","mcp-config","https","cloudflare","contract-test"],
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
        "source":{"source":"local","path":format!("./plugins/{name}")},
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
