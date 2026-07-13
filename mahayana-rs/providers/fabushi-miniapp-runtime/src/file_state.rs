use crate::error::RuntimeError;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static FILES: Lazy<Mutex<FileRegistry>> = Lazy::new(|| Mutex::new(FileRegistry::default()));
static FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct FileRegistry {
    generation: u64,
    files: HashMap<String, Value>,
}

pub(crate) fn register(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let file_id = params
        .get("fileId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("file_{}", FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)));
    let state = params
        .get("state")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("registered")
        .to_string();

    validate_state(&state)?;
    let now = now_millis();
    let mut registry = lock_registry()?;
    let previous_revision = registry
        .files
        .get(&file_id)
        .and_then(|file| file.get("revision"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    registry.generation = registry.generation.saturating_add(1);
    let file = file_record(
        &file_id,
        &state,
        &params,
        previous_revision.saturating_add(1),
        registry.generation,
        now,
    );
    registry.files.insert(file_id.clone(), file.clone());

    let response = json!({
        "registered": true,
        "fileId": file_id,
        "file": file,
        "generation": registry.generation,
        "source": "rust",
    });
    Ok((response, vec![file_update(&file)]))
}

pub(crate) fn update_state(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let file_id = required_string(&params, "fileId")?;
    let state = required_string(&params, "state")?;
    validate_state(&state)?;
    let now = now_millis();
    let mut registry = lock_registry()?;
    let current = registry.files.get(&file_id).cloned().ok_or_else(|| {
        RuntimeError::new(
            "file_not_found",
            format!("runtime file not found: {file_id}"),
        )
    })?;
    let revision = current
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1);
    registry.generation = registry.generation.saturating_add(1);

    let mut merged = current.as_object().cloned().unwrap_or_default();
    merged.insert("state".to_string(), json!(state));
    merged.insert("revision".to_string(), json!(revision));
    merged.insert("registryGeneration".to_string(), json!(registry.generation));
    merged.insert("updatedAtMs".to_string(), json!(now));
    merge_optional(&mut merged, &params, "localPath");
    merge_optional(&mut merged, &params, "remoteId");
    merge_optional(&mut merged, &params, "remoteUrl");
    merge_optional(&mut merged, &params, "contentHash");
    merge_optional(&mut merged, &params, "mimeType");
    merge_optional(&mut merged, &params, "expectedBytes");
    merge_optional(&mut merged, &params, "downloadedBytes");
    merge_optional(&mut merged, &params, "uploadedBytes");
    merge_optional(&mut merged, &params, "error");
    merge_optional(&mut merged, &params, "metadata");
    let file = Value::Object(merged);
    registry.files.insert(file_id.clone(), file.clone());

    let response = json!({
        "updated": true,
        "fileId": file_id,
        "file": file,
        "generation": registry.generation,
        "source": "rust",
    });
    Ok((response, vec![file_update(&file)]))
}

pub(crate) fn get(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let file_id = required_string(&params, "fileId")?;
    let registry = lock_registry()?;
    let file = registry.files.get(&file_id).cloned();
    Ok((
        json!({
            "fileId": file_id,
            "file": file,
            "found": file.is_some(),
            "generation": registry.generation,
            "source": "rust",
        }),
        vec![],
    ))
}

pub(crate) fn list(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let state_filter = params
        .get("state")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(state) = state_filter.as_deref() {
        validate_state(state)?;
    }
    let limit = params
        .get("limit")
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(500)
        .min(5000) as usize;
    let registry = lock_registry()?;
    let mut files = registry
        .files
        .values()
        .filter(|file| {
            state_filter
                .as_deref()
                .map(|state| file.get("state").and_then(Value::as_str) == Some(state))
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    files.sort_by(|a, b| {
        let aid = a.get("fileId").and_then(Value::as_str).unwrap_or_default();
        let bid = b.get("fileId").and_then(Value::as_str).unwrap_or_default();
        aid.cmp(bid)
    });
    files.truncate(limit);
    Ok((
        json!({
            "files": files,
            "generation": registry.generation,
            "source": "rust",
        }),
        vec![],
    ))
}

fn file_record(
    file_id: &str,
    state: &str,
    params: &Value,
    revision: u64,
    generation: u64,
    now: u64,
) -> Value {
    let mut record = serde_json::Map::new();
    record.insert("fileId".to_string(), json!(file_id));
    record.insert("state".to_string(), json!(state));
    record.insert("revision".to_string(), json!(revision));
    record.insert("registryGeneration".to_string(), json!(generation));
    record.insert("createdAtMs".to_string(), json!(now));
    record.insert("updatedAtMs".to_string(), json!(now));
    for key in [
        "localPath",
        "remoteId",
        "remoteUrl",
        "contentHash",
        "mimeType",
        "expectedBytes",
        "downloadedBytes",
        "uploadedBytes",
        "error",
        "metadata",
    ] {
        merge_optional(&mut record, params, key);
    }
    Value::Object(record)
}

fn file_update(file: &Value) -> Value {
    json!({
        "@type": "updateFile",
        "file": file,
        "fileId": file.get("fileId").cloned().unwrap_or(Value::Null),
        "state": file.get("state").cloned().unwrap_or(Value::Null),
        "createdAtMs": now_millis(),
    })
}

fn merge_optional(record: &mut serde_json::Map<String, Value>, params: &Value, key: &str) {
    if let Some(value) = params.get(key) {
        record.insert(key.to_string(), value.clone());
    }
}

fn validate_state(state: &str) -> Result<(), RuntimeError> {
    match state {
        "registered" | "local" | "remote" | "queued" | "downloading" | "uploading" | "ready"
        | "paused" | "failed" | "deleted" => Ok(()),
        _ => Err(RuntimeError::new(
            "invalid_file_state",
            format!("unsupported file state: {state}"),
        )),
    }
}

fn lock_registry() -> Result<std::sync::MutexGuard<'static, FileRegistry>, RuntimeError> {
    FILES
        .lock()
        .map_err(|_| RuntimeError::new("file_registry_lock_failed", "file registry lock failed"))
}

fn required_string(params: &Value, key: &str) -> Result<String, RuntimeError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RuntimeError::new("invalid_request", format!("{key} cannot be empty")))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
