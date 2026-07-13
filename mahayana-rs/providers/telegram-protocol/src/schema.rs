use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

pub const TD_API_PINNED_COMMIT: &str = "a17f87c4cff7b90b278d12b91ba0614383aaee82";
pub const TD_API_EXPECTED_SHA256: &str =
    "a8166ef37efb1a1440357b81e8e26c68ea45a35901c0bcc8d69964487c98476f";
pub const TD_API_EXPECTED_TYPES: usize = 2_126;
pub const TD_API_EXPECTED_FUNCTIONS: usize = 1_001;
pub const TELEGRAM_API_EXPECTED_SHA256: &str =
    "eb841074c076c62effa1ef01523da9d9d8157430b7276e849a8e0ce7f4f71bf9";
pub const TELEGRAM_API_EXPECTED_TYPES: usize = 1_631;
pub const TELEGRAM_API_EXPECTED_FUNCTIONS: usize = 796;
pub const MTPROTO_API_EXPECTED_SHA256: &str =
    "bde4942be06dc112dfc655e2a8aa5163a4e3aa0d859d7840cfc76ec3a00654c4";
pub const MTPROTO_API_EXPECTED_TYPES: usize = 40;
pub const MTPROTO_API_EXPECTED_FUNCTIONS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaBaseline {
    pub name: &'static str,
    pub sha256: &'static str,
    pub types: usize,
    pub functions: usize,
}

pub const TD_API_BASELINE: SchemaBaseline = SchemaBaseline {
    name: "td_api.tl",
    sha256: TD_API_EXPECTED_SHA256,
    types: TD_API_EXPECTED_TYPES,
    functions: TD_API_EXPECTED_FUNCTIONS,
};

pub const TELEGRAM_API_BASELINE: SchemaBaseline = SchemaBaseline {
    name: "telegram_api.tl",
    sha256: TELEGRAM_API_EXPECTED_SHA256,
    types: TELEGRAM_API_EXPECTED_TYPES,
    functions: TELEGRAM_API_EXPECTED_FUNCTIONS,
};

pub const MTPROTO_API_BASELINE: SchemaBaseline = SchemaBaseline {
    name: "mtproto_api.tl",
    sha256: MTPROTO_API_EXPECTED_SHA256,
    types: MTPROTO_API_EXPECTED_TYPES,
    functions: MTPROTO_API_EXPECTED_FUNCTIONS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaStats {
    pub types: usize,
    pub functions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaAudit {
    pub sha256: String,
    pub stats: SchemaStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeclarationKind {
    Type,
    Function,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDeclaration {
    pub name: String,
    pub constructor_id: Option<u32>,
    pub result_type: String,
    pub kind: DeclarationKind,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaCatalog {
    pub types: Vec<SchemaDeclaration>,
    pub functions: Vec<SchemaDeclaration>,
}

impl SchemaCatalog {
    pub fn stats(&self) -> SchemaStats {
        SchemaStats {
            types: self.types.len(),
            functions: self.functions.len(),
        }
    }

    pub fn explicit_constructor_count(&self) -> usize {
        self.types
            .iter()
            .chain(&self.functions)
            .filter(|declaration| declaration.constructor_id.is_some())
            .count()
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SchemaParseError {
    #[error("schema declaration is missing '=': {0}")]
    MissingResultType(String),
    #[error("schema declaration has no constructor/function name: {0}")]
    MissingName(String),
    #[error("schema declaration {name} has invalid constructor id {value}")]
    InvalidConstructorId { name: String, value: String },
    #[error("constructor id 0x{id:08x} is used by both {first} and {second}")]
    DuplicateConstructorId {
        id: u32,
        first: String,
        second: String,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SchemaError {
    #[error("TDLib schema digest changed: expected {expected}, found {actual}")]
    DigestMismatch { expected: String, actual: String },
    #[error(
        "TDLib schema surface changed: expected {expected_types} types/{expected_functions} functions, found {actual_types} types/{actual_functions} functions"
    )]
    SurfaceMismatch {
        expected_types: usize,
        expected_functions: usize,
        actual_types: usize,
        actual_functions: usize,
    },
    #[error("Telegram schema could not be parsed: {0}")]
    Parse(String),
}

pub fn parse_schema_catalog(schema: &str) -> Result<SchemaCatalog, SchemaParseError> {
    let mut kind = DeclarationKind::Type;
    let mut buffer = String::new();
    let mut catalog = SchemaCatalog {
        types: Vec::new(),
        functions: Vec::new(),
    };
    let mut constructor_ids = BTreeMap::<u32, String>::new();

    for raw_line in schema.lines() {
        let line = raw_line.trim();
        if line == "---types---" {
            kind = DeclarationKind::Type;
            buffer.clear();
            continue;
        }
        if line == "---functions---" {
            kind = DeclarationKind::Function;
            buffer.clear();
            continue;
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let without_comment = line.split_once("//").map_or(line, |(code, _)| code).trim();
        if without_comment.is_empty() {
            continue;
        }
        if !buffer.is_empty() {
            buffer.push(' ');
        }
        buffer.push_str(without_comment);

        while let Some(end) = buffer.find(';') {
            let raw = buffer[..end].trim().to_string();
            buffer = buffer[end + 1..].trim_start().to_string();
            if raw.is_empty() {
                continue;
            }
            let (left, result_type) = raw
                .rsplit_once('=')
                .ok_or_else(|| SchemaParseError::MissingResultType(raw.clone()))?;
            let name_token = left
                .split_whitespace()
                .next()
                .ok_or_else(|| SchemaParseError::MissingName(raw.clone()))?;
            let (name, constructor_id) = match name_token.split_once('#') {
                Some((name, value)) => {
                    let id = u32::from_str_radix(value, 16).map_err(|_| {
                        SchemaParseError::InvalidConstructorId {
                            name: name.to_string(),
                            value: value.to_string(),
                        }
                    })?;
                    (name.to_string(), Some(id))
                }
                None => (name_token.to_string(), None),
            };
            if name.is_empty() {
                return Err(SchemaParseError::MissingName(raw));
            }
            if let Some(id) = constructor_id {
                if let Some(first) = constructor_ids.get(&id) {
                    if !is_prefix_alias(first, &name) {
                        return Err(SchemaParseError::DuplicateConstructorId {
                            id,
                            first: first.clone(),
                            second: name,
                        });
                    }
                } else {
                    constructor_ids.insert(id, name.clone());
                }
            }
            let declaration = SchemaDeclaration {
                name,
                constructor_id,
                result_type: result_type.trim().to_string(),
                kind,
                raw,
            };
            match kind {
                DeclarationKind::Type => catalog.types.push(declaration),
                DeclarationKind::Function => catalog.functions.push(declaration),
            }
        }
    }
    Ok(catalog)
}

fn is_prefix_alias(first: &str, second: &str) -> bool {
    first.strip_suffix("Prefix") == Some(second) || second.strip_suffix("Prefix") == Some(first)
}

pub fn parse_td_api_schema(schema: &str) -> SchemaStats {
    #[derive(Clone, Copy)]
    enum Section {
        Types,
        Functions,
    }

    // td_api.tl starts directly with type declarations and only emits an
    // explicit `---functions---` marker later in the file.
    let mut section = Section::Types;
    let mut buffer = String::new();
    let mut types = 0;
    let mut functions = 0;

    for raw_line in schema.lines() {
        let line = raw_line.trim();
        if line == "---types---" {
            section = Section::Types;
            buffer.clear();
            continue;
        }
        if line == "---functions---" {
            section = Section::Functions;
            buffer.clear();
            continue;
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }

        let without_comment = line.split_once("//").map_or(line, |(code, _)| code).trim();
        if without_comment.is_empty() {
            continue;
        }
        if !buffer.is_empty() {
            buffer.push(' ');
        }
        buffer.push_str(without_comment);

        while let Some(end) = buffer.find(';') {
            let declaration = buffer[..end].trim();
            if declaration.contains('=') && !declaration.starts_with('#') {
                match section {
                    Section::Types => types += 1,
                    Section::Functions => functions += 1,
                }
            }
            buffer = buffer[end + 1..].trim_start().to_string();
        }
    }

    SchemaStats { types, functions }
}

pub fn audit_td_api_schema(schema_bytes: &[u8]) -> Result<SchemaAudit, SchemaError> {
    audit_schema(schema_bytes, TD_API_BASELINE)
}

pub fn audit_telegram_api_schema(schema_bytes: &[u8]) -> Result<SchemaAudit, SchemaError> {
    audit_schema(schema_bytes, TELEGRAM_API_BASELINE)
}

pub fn audit_mtproto_api_schema(schema_bytes: &[u8]) -> Result<SchemaAudit, SchemaError> {
    audit_schema(schema_bytes, MTPROTO_API_BASELINE)
}

pub fn audit_schema(
    schema_bytes: &[u8],
    baseline: SchemaBaseline,
) -> Result<SchemaAudit, SchemaError> {
    let actual_sha256 = hex::encode(Sha256::digest(schema_bytes));
    if actual_sha256 != baseline.sha256 {
        return Err(SchemaError::DigestMismatch {
            expected: baseline.sha256.to_string(),
            actual: actual_sha256,
        });
    }

    let schema = String::from_utf8_lossy(schema_bytes);
    let catalog =
        parse_schema_catalog(&schema).map_err(|error| SchemaError::Parse(error.to_string()))?;
    let stats = catalog.stats();
    if stats.types != baseline.types || stats.functions != baseline.functions {
        return Err(SchemaError::SurfaceMismatch {
            expected_types: baseline.types,
            expected_functions: baseline.functions,
            actual_types: stats.types,
            actual_functions: stats.functions,
        });
    }

    Ok(SchemaAudit {
        sha256: actual_sha256,
        stats,
    })
}
