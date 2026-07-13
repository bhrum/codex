use crate::error::RuntimeError;
use once_cell::sync::Lazy;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOCAL_STORE: Lazy<Mutex<LocalStore>> = Lazy::new(|| Mutex::new(LocalStore::default()));

#[derive(Default)]
struct LocalStore {
    generation: u64,
    collections: HashMap<String, HashMap<String, Value>>,
    snapshot_path: Option<PathBuf>,
}

pub(crate) fn configure(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let path = required_string(&params, "path")?;
    let load_existing = params
        .get("loadExisting")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut store = lock_store()?;
    store.snapshot_path = Some(PathBuf::from(&path));

    if load_existing {
        store.load_snapshot()?;
    } else {
        store.persist_snapshot()?;
    }

    let response = json!({
        "configured": true,
        "path": path,
        "generation": store.generation,
        "collectionCount": store.collections.len(),
        "recordCount": store.record_count(false),
        "source": "rust",
    });
    Ok((
        response.clone(),
        vec![storage_update("updateLocalStoreConfigured", &response)],
    ))
}

pub(crate) fn get_status(_params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let store = lock_store()?;
    Ok((
        json!({
            "generation": store.generation,
            "collectionCount": store.collections.len(),
            "recordCount": store.record_count(false),
            "snapshotPath": store.snapshot_path.as_ref().map(|path| path.to_string_lossy().to_string()),
            "source": "rust",
        }),
        vec![],
    ))
}

pub(crate) fn put(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let collection = required_string(&params, "collection")?;
    let key = required_string(&params, "key")?;
    let value = params.get("value").cloned().unwrap_or(Value::Null);
    let expected_revision = params.get("expectedRevision").and_then(read_u64_value);
    let now = now_millis();

    let mut store = lock_store()?;
    let current_revision = store
        .collections
        .get(&collection)
        .and_then(|items| items.get(&key))
        .and_then(|record| record.get("revision"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if let Some(expected) = expected_revision {
        if expected != current_revision {
            return Err(RuntimeError::new(
                "storage_revision_conflict",
                format!(
                    "record revision conflict for {collection}/{key}: expected {expected}, current {current_revision}"
                ),
            ));
        }
    }

    store.generation = store.generation.saturating_add(1);
    let record = json!({
        "collection": collection,
        "key": key,
        "value": value,
        "revision": current_revision.saturating_add(1),
        "storeGeneration": store.generation,
        "deleted": false,
        "updatedAtMs": now,
    });
    store
        .collections
        .entry(collection.clone())
        .or_default()
        .insert(key.clone(), record.clone());
    store.persist_snapshot()?;

    let response = json!({
        "stored": true,
        "collection": collection,
        "key": key,
        "record": record,
        "generation": store.generation,
        "source": "rust",
    });
    Ok((
        response.clone(),
        vec![storage_update("updateLocalStoreRecord", &response)],
    ))
}

pub(crate) fn get(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let collection = required_string(&params, "collection")?;
    let key = required_string(&params, "key")?;
    let include_deleted = params
        .get("includeDeleted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let store = lock_store()?;
    let record = store
        .collections
        .get(&collection)
        .and_then(|items| items.get(&key))
        .filter(|record| {
            include_deleted
                || !record
                    .get("deleted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .cloned();
    Ok((
        json!({
            "collection": collection,
            "key": key,
            "record": record,
            "found": record.is_some(),
            "generation": store.generation,
            "source": "rust",
        }),
        vec![],
    ))
}

pub(crate) fn delete(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let collection = required_string(&params, "collection")?;
    let key = required_string(&params, "key")?;
    let expected_revision = params.get("expectedRevision").and_then(read_u64_value);
    let now = now_millis();

    let mut store = lock_store()?;
    let current_revision = store
        .collections
        .get(&collection)
        .and_then(|items| items.get(&key))
        .and_then(|record| record.get("revision"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if let Some(expected) = expected_revision {
        if expected != current_revision {
            return Err(RuntimeError::new(
                "storage_revision_conflict",
                format!(
                    "record revision conflict for {collection}/{key}: expected {expected}, current {current_revision}"
                ),
            ));
        }
    }

    store.generation = store.generation.saturating_add(1);
    let record = json!({
        "collection": collection,
        "key": key,
        "value": Value::Null,
        "revision": current_revision.saturating_add(1),
        "storeGeneration": store.generation,
        "deleted": true,
        "updatedAtMs": now,
    });
    store
        .collections
        .entry(collection.clone())
        .or_default()
        .insert(key.clone(), record.clone());
    store.persist_snapshot()?;

    let response = json!({
        "deleted": true,
        "collection": collection,
        "key": key,
        "record": record,
        "generation": store.generation,
        "source": "rust",
    });
    Ok((
        response.clone(),
        vec![storage_update("updateLocalStoreRecord", &response)],
    ))
}

pub(crate) fn list(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let collection = required_string(&params, "collection")?;
    let include_deleted = params
        .get("includeDeleted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let limit = params
        .get("limit")
        .and_then(read_u64_value)
        .unwrap_or(500)
        .min(5000) as usize;
    let store = lock_store()?;
    let records = store
        .collections
        .get(&collection)
        .map(|items| {
            let mut values = items
                .values()
                .filter(|record| {
                    include_deleted
                        || !record
                            .get("deleted")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                })
                .cloned()
                .collect::<Vec<_>>();
            values.sort_by(|a, b| {
                let ak = a.get("key").and_then(Value::as_str).unwrap_or_default();
                let bk = b.get("key").and_then(Value::as_str).unwrap_or_default();
                ak.cmp(bk)
            });
            values.truncate(limit);
            values
        })
        .unwrap_or_default();

    Ok((
        json!({
            "collection": collection,
            "records": records,
            "generation": store.generation,
            "source": "rust",
        }),
        vec![],
    ))
}

pub(crate) fn snapshot(_params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let store = lock_store()?;
    Ok((store.snapshot_json(), vec![]))
}

fn lock_store() -> Result<std::sync::MutexGuard<'static, LocalStore>, RuntimeError> {
    LOCAL_STORE
        .lock()
        .map_err(|_| RuntimeError::new("storage_lock_failed", "local store lock failed"))
}

impl LocalStore {
    fn record_count(&self, include_deleted: bool) -> usize {
        self.collections
            .values()
            .map(|items| {
                items
                    .values()
                    .filter(|record| {
                        include_deleted
                            || !record
                                .get("deleted")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                    })
                    .count()
            })
            .sum()
    }

    fn snapshot_json(&self) -> Value {
        let mut collections = Map::new();
        for (name, items) in &self.collections {
            let mut records = Map::new();
            for (key, record) in items {
                records.insert(key.clone(), record.clone());
            }
            collections.insert(name.clone(), Value::Object(records));
        }
        json!({
            "version": 1,
            "generation": self.generation,
            "collections": collections,
            "recordCount": self.record_count(false),
            "snapshotPath": self.snapshot_path.as_ref().map(|path| path.to_string_lossy().to_string()),
            "source": "rust",
        })
    }

    fn persist_snapshot(&self) -> Result<(), RuntimeError> {
        let Some(path) = self.snapshot_path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| RuntimeError::new("storage_persist_failed", error.to_string()))?;
        }
        fs::write(path, self.snapshot_json().to_string())
            .map_err(|error| RuntimeError::new("storage_persist_failed", error.to_string()))
    }

    fn load_snapshot(&mut self) -> Result<(), RuntimeError> {
        let Some(path) = self.snapshot_path.as_ref() else {
            return Ok(());
        };
        if !path.exists() {
            return self.persist_snapshot();
        }
        let text = fs::read_to_string(path)
            .map_err(|error| RuntimeError::new("storage_load_failed", error.to_string()))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| RuntimeError::new("storage_load_failed", error.to_string()))?;
        self.generation = value.get("generation").and_then(Value::as_u64).unwrap_or(0);
        self.collections.clear();
        if let Some(collections) = value.get("collections").and_then(Value::as_object) {
            for (collection, records) in collections {
                let mut map = HashMap::new();
                if let Some(records) = records.as_object() {
                    for (key, record) in records {
                        map.insert(key.clone(), record.clone());
                    }
                }
                self.collections.insert(collection.clone(), map);
            }
        }
        Ok(())
    }
}

fn storage_update(update_type: &str, response: &Value) -> Value {
    json!({
        "@type": update_type,
        "generation": response.get("generation").cloned().unwrap_or(Value::Null),
        "record": response.get("record").cloned().unwrap_or(Value::Null),
        "collection": response.get("collection").cloned().unwrap_or(Value::Null),
        "key": response.get("key").cloned().unwrap_or(Value::Null),
        "createdAtMs": now_millis(),
    })
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

fn read_u64_value(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
