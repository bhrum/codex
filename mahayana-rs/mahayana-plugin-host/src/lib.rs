//! Mahayana policy layered over Codex plugin discovery and MCP routing.

use codex_app_server_protocol::PluginRuntimePlatform;
use codex_app_server_protocol::PluginRuntimeVariant;
use codex_core_plugins::manifest::load_plugin_manifest;
use mahayana_platform_core::CommandDeclaration;
use mahayana_platform_core::HostPlatform;
use mahayana_platform_core::MahayanaPluginManifest;
use mahayana_platform_core::ManifestError;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct LocalPlugin {
    pub codex: codex_core_plugins::manifest::PluginManifest,
    pub mahayana: Option<MahayanaPluginManifest>,
}

impl LocalPlugin {
    pub fn load(plugin_root: &Path) -> Result<Self, PluginHostError> {
        let codex = load_plugin_manifest(plugin_root)
            .ok_or_else(|| PluginHostError::InvalidCodexManifest(plugin_root.to_path_buf()))?;
        let mahayana = MahayanaPluginManifest::load(plugin_root)?;
        Ok(Self { codex, mahayana })
    }

    pub fn command(&self, name: &str) -> Option<&CommandDeclaration> {
        self.mahayana.as_ref()?.commands.iter().find(|command| {
            command.name == name || command.aliases.iter().any(|alias| alias == name)
        })
    }

    pub fn gate_for_tool(&self, tool: &str) -> Option<&str> {
        let target = format!("tool:{tool}");
        self.mahayana
            .as_ref()?
            .gates
            .iter()
            .find(|gate| gate.target == target)
            .map(|gate| gate.entitlement.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedRuntime {
    pub variant_id: Option<String>,
    pub server: String,
}

pub fn select_runtime(
    platform: HostPlatform,
    mcp_servers: &[String],
    variants: &[PluginRuntimeVariant],
) -> Result<SelectedRuntime, PluginHostError> {
    select_runtime_with_availability(platform, mcp_servers, variants, |_| true)
}

/// Selects the highest-priority runtime whose declared MCP server can be
/// started by the current host. This lets packaged plugins prefer a bundled
/// CLI while retaining their account-backed HTTP runtime as a compatibility
/// fallback when an older/development install does not contain the binary.
pub fn select_runtime_with_availability(
    platform: HostPlatform,
    mcp_servers: &[String],
    variants: &[PluginRuntimeVariant],
    mut is_available: impl FnMut(&str) -> bool,
) -> Result<SelectedRuntime, PluginHostError> {
    let platform = app_server_platform(platform);
    let mut candidates = variants
        .iter()
        .filter(|variant| variant.platforms.contains(&platform))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.id.cmp(&right.id))
    });
    for selected in candidates {
        if !mcp_servers.contains(&selected.server) {
            return Err(PluginHostError::MissingRuntimeServer(
                selected.server.clone(),
            ));
        }
        if is_available(&selected.server) {
            return Ok(SelectedRuntime {
                variant_id: Some(selected.id.clone()),
                server: selected.server.clone(),
            });
        }
    }
    if !variants
        .iter()
        .any(|variant| variant.platforms.contains(&platform))
    {
        return match mcp_servers {
            [server] if is_available(server) => Ok(SelectedRuntime {
                variant_id: None,
                server: server.clone(),
            }),
            [server] => Err(PluginHostError::RuntimeUnavailable {
                platform,
                servers: vec![server.clone()],
            }),
            [] => Err(PluginHostError::NoRuntimeForPlatform(platform)),
            _ => Err(PluginHostError::AmbiguousLegacyRuntime),
        };
    }
    Err(PluginHostError::RuntimeUnavailable {
        platform,
        servers: variants
            .iter()
            .filter(|variant| variant.platforms.contains(&platform))
            .map(|variant| variant.server.clone())
            .collect(),
    })
}

fn app_server_platform(platform: HostPlatform) -> PluginRuntimePlatform {
    match platform {
        HostPlatform::Cli => PluginRuntimePlatform::Cli,
        HostPlatform::Desktop => PluginRuntimePlatform::Desktop,
        HostPlatform::Mobile => PluginRuntimePlatform::Mobile,
        HostPlatform::Web => PluginRuntimePlatform::Web,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginCommandInvocation {
    pub plugin_id: String,
    pub command: String,
    pub arguments: Value,
}

impl PluginCommandInvocation {
    pub fn parse_tui(source: &str) -> Result<Self, PluginHostError> {
        let source = source
            .strip_prefix('/')
            .ok_or(PluginHostError::InvalidCommandSyntax)?;
        let (qualified, remainder) = source
            .split_once(char::is_whitespace)
            .map_or((source, ""), |(qualified, remainder)| {
                (qualified, remainder.trim())
            });
        let (plugin_id, command) = qualified
            .split_once(':')
            .ok_or(PluginHostError::InvalidCommandSyntax)?;
        if plugin_id.is_empty() || command.is_empty() {
            return Err(PluginHostError::InvalidCommandSyntax);
        }
        let arguments = if remainder.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(remainder)
                .unwrap_or_else(|_| serde_json::json!({"input": remainder}))
        };
        Ok(Self {
            plugin_id: plugin_id.to_string(),
            command: command.to_string(),
            arguments,
        })
    }
}

pub fn command_index(
    manifests: impl IntoIterator<Item = (String, MahayanaPluginManifest)>,
) -> HashMap<String, (String, String)> {
    let mut index = HashMap::new();
    for (plugin_id, manifest) in manifests {
        for command in manifest.commands {
            index.insert(
                format!("{plugin_id}:{}", command.name),
                (plugin_id.clone(), command.tool.clone()),
            );
            for alias in command.aliases {
                index.insert(
                    format!("{plugin_id}:{alias}"),
                    (plugin_id.clone(), command.tool.clone()),
                );
            }
        }
    }
    index
}

#[derive(Debug, thiserror::Error)]
pub enum PluginHostError {
    #[error("Codex plugin manifest is missing or invalid at {0}")]
    InvalidCodexManifest(std::path::PathBuf),
    #[error(transparent)]
    MahayanaManifest(#[from] ManifestError),
    #[error("selected runtime references undeclared MCP server {0}")]
    MissingRuntimeServer(String),
    #[error("plugin has no MCP runtime for {0:?}")]
    NoRuntimeForPlatform(PluginRuntimePlatform),
    #[error("plugin MCP runtimes for {platform:?} are unavailable: {servers:?}")]
    RuntimeUnavailable {
        platform: PluginRuntimePlatform,
        servers: Vec<String>,
    },
    #[error("plugin has multiple MCP servers but no runtimeVariants selection")]
    AmbiguousLegacyRuntime,
    #[error("plugin command must use /<plugin-id>:<command> [JSON]")]
    InvalidCommandSyntax,
}

#[cfg(test)]
#[path = "plugin_host_tests.rs"]
mod tests;
