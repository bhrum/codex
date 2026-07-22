use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

const SUPPORTED_SCHEMA_VERSION: u32 = 1;
const SUPPORTED_BRIDGE_VERSION: &str = "1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MahayanaPluginManifest {
    pub schema_version: u32,
    #[serde(default)]
    pub supported_platforms: Vec<crate::HostPlatform>,
    #[serde(default)]
    pub miniapp: Option<MiniAppDeclaration>,
    #[serde(default)]
    pub commands: Vec<CommandDeclaration>,
    #[serde(default)]
    pub runtime: Option<PluginRuntimeDeclaration>,
    #[serde(default)]
    pub gates: Vec<EntitlementGate>,
}

impl MahayanaPluginManifest {
    pub fn load(plugin_root: &Path) -> Result<Option<Self>, ManifestError> {
        let path = plugin_root.join(".mahayana/plugin.json");
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ManifestError::Read { path, source }),
        };
        let manifest = serde_json::from_str::<Self>(&source)
            .map_err(|source| ManifestError::Decode { path, source })?;
        manifest.validate(plugin_root)?;
        Ok(Some(manifest))
    }

    pub fn validate(&self, plugin_root: &Path) -> Result<(), ManifestError> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema(self.schema_version));
        }
        let mut platforms = HashSet::new();
        for platform in &self.supported_platforms {
            if !platforms.insert(*platform) {
                return Err(ManifestError::DuplicateSupportedPlatform(*platform));
            }
        }
        if let Some(miniapp) = &self.miniapp {
            if miniapp.bridge_version != SUPPORTED_BRIDGE_VERSION {
                return Err(ManifestError::UnsupportedBridge(
                    miniapp.bridge_version.clone(),
                ));
            }
            let entry = validate_relative_path(&miniapp.entry)?;
            let resolved = plugin_root.join(entry);
            if !resolved.is_file() {
                return Err(ManifestError::MissingMiniAppEntry(resolved));
            }
            let mut permissions = HashSet::new();
            for permission in &miniapp.permissions {
                if !permissions.insert(permission) {
                    return Err(ManifestError::DuplicatePermission(*permission));
                }
            }
        }
        if let Some(runtime) = &self.runtime {
            if let Some(cli) = &runtime.cli {
                validate_cli_executable(&cli.executable)?;
            }
            if let Some(wasm) = &runtime.wasm {
                validate_relative_path(&wasm.module)?;
                if wasm.export_name.trim().is_empty() {
                    return Err(ManifestError::InvalidRuntimeExport);
                }
            }
        }

        let mut commands = HashSet::new();
        for command in &self.commands {
            validate_segment(&command.name, "command")?;
            validate_tool_name(&command.tool)?;
            if !commands.insert(command.name.as_str()) {
                return Err(ManifestError::DuplicateCommand(command.name.clone()));
            }
            for alias in &command.aliases {
                validate_segment(alias, "command alias")?;
                if !commands.insert(alias.as_str()) {
                    return Err(ManifestError::DuplicateCommand(alias.clone()));
                }
            }
        }

        let mut targets = HashSet::new();
        for gate in &self.gates {
            let Some(tool) = gate.target.strip_prefix("tool:") else {
                return Err(ManifestError::InvalidGateTarget(gate.target.clone()));
            };
            validate_tool_name(tool)?;
            validate_entitlement(&gate.entitlement)?;
            if !targets.insert(gate.target.as_str()) {
                return Err(ManifestError::DuplicateGate(gate.target.clone()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeDeclaration {
    #[serde(default)]
    pub cli: Option<CliRuntimeDeclaration>,
    #[serde(default)]
    pub wasm: Option<WasmRuntimeDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CliRuntimeDeclaration {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WasmRuntimeDeclaration {
    pub module: String,
    #[serde(rename = "export")]
    pub export_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MiniAppDeclaration {
    pub entry: String,
    pub bridge_version: String,
    #[serde(default)]
    pub permissions: Vec<HostPermission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostPermission {
    #[serde(rename = "profile.basic")]
    ProfileBasic,
    #[serde(rename = "auth.delegatedToken")]
    AuthDelegatedToken,
    #[serde(rename = "mcp.call")]
    McpCall,
    #[serde(rename = "commerce.purchase")]
    CommercePurchase,
    #[serde(rename = "storage.local")]
    StorageLocal,
    #[serde(rename = "ui.control")]
    UiControl,
    #[serde(rename = "desktop.accessibility")]
    DesktopAccessibility,
    #[serde(rename = "desktop.chatgpt.approvals")]
    DesktopChatGptApprovals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandDeclaration {
    pub name: String,
    pub tool: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntitlementGate {
    pub target: String,
    pub entitlement: String,
}

fn validate_relative_path(value: &str) -> Result<PathBuf, ManifestError> {
    if !value.starts_with("./") {
        return Err(ManifestError::InvalidRelativePath(value.to_string()));
    }
    let path = Path::new(value);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(ManifestError::InvalidRelativePath(value.to_string()));
    }
    Ok(path.to_path_buf())
}

fn validate_cli_executable(value: &str) -> Result<(), ManifestError> {
    if value.starts_with("./") {
        validate_relative_path(value)?;
        return Ok(());
    }
    let is_safe_path_command = !value.is_empty()
        && value.len() <= 128
        && !value.contains(['/', '\\', '\r', '\n'])
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if is_safe_path_command {
        Ok(())
    } else {
        Err(ManifestError::InvalidRelativePath(value.to_string()))
    }
}

fn validate_segment(value: &str, kind: &'static str) -> Result<(), ManifestError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(ManifestError::InvalidIdentifier {
            kind,
            value: value.to_string(),
        })
    }
}

fn validate_tool_name(value: &str) -> Result<(), ManifestError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(ManifestError::InvalidIdentifier {
            kind: "MCP tool",
            value: value.to_string(),
        })
    }
}

fn validate_entitlement(value: &str) -> Result<(), ManifestError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(ManifestError::InvalidIdentifier {
            kind: "entitlement",
            value: value.to_string(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("failed to read Mahayana plugin manifest {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode Mahayana plugin manifest {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported Mahayana plugin schema version {0}")]
    UnsupportedSchema(u32),
    #[error("unsupported Mini App bridge version {0}")]
    UnsupportedBridge(String),
    #[error("Mini App entry does not exist: {0}")]
    MissingMiniAppEntry(PathBuf),
    #[error("plugin path must be a ./-prefixed path without parent traversal: {0}")]
    InvalidRelativePath(String),
    #[error("invalid {kind} identifier: {value}")]
    InvalidIdentifier { kind: &'static str, value: String },
    #[error("duplicate command or alias: {0}")]
    DuplicateCommand(String),
    #[error("duplicate Mini App permission: {0:?}")]
    DuplicatePermission(HostPermission),
    #[error("duplicate supported platform: {0:?}")]
    DuplicateSupportedPlatform(crate::HostPlatform),
    #[error("entitlement gate must target tool:<name>: {0}")]
    InvalidGateTarget(String),
    #[error("duplicate entitlement gate target: {0}")]
    DuplicateGate(String),
    #[error("WASM runtime export must not be empty")]
    InvalidRuntimeExport,
}

#[cfg(test)]
#[path = "extension_manifest_tests.rs"]
mod tests;
