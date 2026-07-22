use sha2::Digest;
use sha2::Sha256;
use url::Url;

/// Canonicalizes a repository source without preserving credentials, query
/// strings, or fragments. GitHub owner/repository paths are case-insensitive.
pub fn canonical_repository_source(source: &str) -> Result<String, MiniAppIdentityError> {
    let source = source.trim().trim_start_matches("git+");
    if source.is_empty() {
        return Err(MiniAppIdentityError::EmptySource);
    }
    let expanded = if let Some(value) = source.strip_prefix("git@") {
        let (host, path) = value
            .split_once(':')
            .ok_or_else(|| MiniAppIdentityError::InvalidSource(source.into()))?;
        format!("ssh://git@{host}/{path}")
    } else {
        source.to_string()
    };
    let mut url =
        Url::parse(&expanded).map_err(|_| MiniAppIdentityError::InvalidSource(source.into()))?;
    if !matches!(url.scheme(), "https" | "ssh" | "git") {
        return Err(MiniAppIdentityError::UnsupportedScheme(url.scheme().into()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| MiniAppIdentityError::InvalidSource(source.into()))?
        .to_ascii_lowercase();
    let mut path = url
        .path()
        .trim_matches('/')
        .trim_end_matches(".git")
        .to_string();
    if path.is_empty() || path.split('/').any(|part| part == ".." || part.is_empty()) {
        return Err(MiniAppIdentityError::InvalidSource(source.into()));
    }
    if host == "github.com" {
        path.make_ascii_lowercase();
    }
    url.set_username("")
        .map_err(|_| MiniAppIdentityError::InvalidSource(source.into()))?;
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Ok(format!("{host}/{path}"))
}

/// Creates a collision-resistant identity from the repository and manifest
/// name. It deliberately does not change when a plugin is upgraded/reinstalled.
pub fn plugin_instance_id(
    repository_source: &str,
    manifest_name: &str,
) -> Result<String, MiniAppIdentityError> {
    let source = canonical_repository_source(repository_source)?;
    let name = manifest_name.trim().to_ascii_lowercase();
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(MiniAppIdentityError::InvalidManifestName);
    }
    let digest = Sha256::digest(format!("{source}\0{name}").as_bytes());
    Ok(format!("{name}@{}", hex_prefix(&digest, 16)))
}

pub fn legacy_official_conversation_id(plugin_id: &str) -> Option<String> {
    matches!(
        plugin_id,
        "global-dharma"
            | "faliu-flashcards"
            | "platform-publish"
            | "hermes-installer"
            | "bot-father"
            | "mahayana-assistant"
            | "chatgpt-auto-confirm"
    )
    .then(|| format!("miniapp:{plugin_id}"))
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    bytes
        .iter()
        .take(length)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MiniAppIdentityError {
    #[error("repository source must not be empty")]
    EmptySource,
    #[error("repository source is invalid: {0}")]
    InvalidSource(String),
    #[error("repository source scheme is not allowed: {0}")]
    UnsupportedScheme(String),
    #[error("manifest name must use lowercase letters, numbers, and hyphens")]
    InvalidManifestName,
}

#[cfg(test)]
#[path = "miniapp_identity_tests.rs"]
mod tests;
