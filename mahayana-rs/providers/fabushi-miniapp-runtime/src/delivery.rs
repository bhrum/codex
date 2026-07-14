use crate::error::RuntimeError;
use once_cell::sync::Lazy;
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

static DELIVERY: Lazy<Mutex<DeliveryState>> = Lazy::new(|| Mutex::new(DeliveryState::default()));
static JOB_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static RECEIPT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct DeliveryState {
    generation: u64,
    jobs: HashMap<String, Value>,
    receipts: HashMap<String, Value>,
    retry_queue: VecDeque<String>,
}

pub(crate) fn enqueue_job(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let job_id = params
        .get("jobId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("gd_job_{}", JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed)));
    let now = now_millis();
    let endpoints = params
        .get("endpoints")
        .cloned()
        .or_else(|| params.get("endpointIds").cloned())
        .unwrap_or_else(|| json!([]));
    let max_attempts = params
        .get("maxAttempts")
        .and_then(read_u64_value)
        .unwrap_or(3)
        .clamp(1, 50);
    let priority = params
        .get("priority")
        .and_then(read_i64_value)
        .unwrap_or(0)
        .clamp(-1000, 1000);

    let mut state = lock_state()?;
    if state.jobs.contains_key(&job_id) {
        return Err(RuntimeError::new(
            "delivery_job_exists",
            format!("global dharma delivery job already exists: {job_id}"),
        ));
    }
    state.generation = state.generation.saturating_add(1);
    let job = json!({
        "jobId": job_id,
        "status": "queued",
        "packet": params.get("packet").cloned().unwrap_or(Value::Null),
        "endpoints": endpoints,
        "priority": priority,
        "attempts": 0,
        "maxAttempts": max_attempts,
        "nextAttemptAtMs": params.get("nextAttemptAtMs").and_then(read_u64_value).unwrap_or(now),
        "createdAtMs": now,
        "updatedAtMs": now,
        "generation": state.generation,
        "metadata": params.get("metadata").cloned().unwrap_or(Value::Null),
    });
    state.jobs.insert(job_id.clone(), job.clone());
    state.retry_queue.push_back(job_id.clone());

    let response = json!({
        "queued": true,
        "jobId": job_id,
        "job": job,
        "generation": state.generation,
        "source": "rust",
    });
    Ok((
        response,
        vec![delivery_update(
            "updateGlobalDharmaDeliveryQueued",
            &job,
            Value::Null,
        )],
    ))
}

pub(crate) fn get_job(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let job_id = required_string(&params, "jobId")?;
    let state = lock_state()?;
    let job = state.jobs.get(&job_id).cloned();
    Ok((
        json!({
            "jobId": job_id,
            "job": job,
            "found": job.is_some(),
            "generation": state.generation,
            "source": "rust",
        }),
        vec![],
    ))
}

pub(crate) fn list_jobs(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let status_filter = params
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let limit = params
        .get("limit")
        .and_then(read_u64_value)
        .unwrap_or(500)
        .min(5000) as usize;
    let state = lock_state()?;
    let mut jobs = state
        .jobs
        .values()
        .filter(|job| {
            status_filter
                .as_deref()
                .map(|status| job.get("status").and_then(Value::as_str) == Some(status))
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    jobs.sort_by(|a, b| {
        let ap = a.get("priority").and_then(Value::as_i64).unwrap_or(0);
        let bp = b.get("priority").and_then(Value::as_i64).unwrap_or(0);
        bp.cmp(&ap).then_with(|| {
            let at = a.get("createdAtMs").and_then(Value::as_u64).unwrap_or(0);
            let bt = b.get("createdAtMs").and_then(Value::as_u64).unwrap_or(0);
            at.cmp(&bt)
        })
    });
    jobs.truncate(limit);
    Ok((
        json!({
            "jobs": jobs,
            "generation": state.generation,
            "source": "rust",
        }),
        vec![],
    ))
}

pub(crate) fn mark_attempt(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let job_id = required_string(&params, "jobId")?;
    let success = params
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let retry_after_ms = params
        .get("retryAfterMs")
        .and_then(read_u64_value)
        .unwrap_or(15_000)
        .clamp(1_000, 24 * 60 * 60 * 1000);
    let now = now_millis();

    let mut state = lock_state()?;
    let mut job = state.jobs.get(&job_id).cloned().ok_or_else(|| {
        RuntimeError::new(
            "delivery_job_not_found",
            format!("delivery job not found: {job_id}"),
        )
    })?;
    let attempts = job
        .get("attempts")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1);
    let max_attempts = job.get("maxAttempts").and_then(Value::as_u64).unwrap_or(3);
    let next_status = if success {
        "sent"
    } else if attempts >= max_attempts {
        "failed"
    } else {
        "retry_scheduled"
    };
    let next_attempt_at = if success || attempts >= max_attempts {
        Value::Null
    } else {
        json!(now.saturating_add(retry_after_ms))
    };

    state.generation = state.generation.saturating_add(1);
    merge_job_fields(
        &mut job,
        json!({
            "status": next_status,
            "attempts": attempts,
            "lastEndpointId": params.get("endpointId").cloned().unwrap_or(Value::Null),
            "lastError": params.get("error").cloned().unwrap_or(Value::Null),
            "nextAttemptAtMs": next_attempt_at,
            "updatedAtMs": now,
            "generation": state.generation,
        }),
    );
    if next_status == "retry_scheduled" {
        state.retry_queue.push_back(job_id.clone());
    }
    state.jobs.insert(job_id.clone(), job.clone());

    let response = json!({
        "jobId": job_id,
        "job": job,
        "status": next_status,
        "retryScheduled": next_status == "retry_scheduled",
        "generation": state.generation,
        "source": "rust",
    });
    Ok((
        response,
        vec![delivery_update(
            "updateGlobalDharmaDeliveryAttempt",
            &job,
            Value::Null,
        )],
    ))
}

pub(crate) fn next_retry(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let now = params
        .get("nowMs")
        .and_then(read_u64_value)
        .unwrap_or_else(now_millis);
    let mut state = lock_state()?;
    let mut selected: Option<Value> = None;
    let mut selected_job_id: Option<String> = None;
    let queue_len = state.retry_queue.len();

    for _ in 0..queue_len {
        let Some(job_id) = state.retry_queue.pop_front() else {
            break;
        };
        let Some(job) = state.jobs.get(&job_id).cloned() else {
            continue;
        };
        let status = job.get("status").and_then(Value::as_str).unwrap_or("");
        if !matches!(status, "queued" | "retry_scheduled") {
            continue;
        }
        let due = job
            .get("nextAttemptAtMs")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if due <= now {
            selected = Some(job);
            selected_job_id = Some(job_id);
            break;
        }
        state.retry_queue.push_back(job_id);
    }

    if let (Some(job_id), Some(mut job)) = (selected_job_id, selected) {
        state.generation = state.generation.saturating_add(1);
        merge_job_fields(
            &mut job,
            json!({
                "status": "in_flight",
                "updatedAtMs": now,
                "generation": state.generation,
            }),
        );
        state.jobs.insert(job_id.clone(), job.clone());
        let response = json!({
            "jobId": job_id,
            "job": job,
            "found": true,
            "generation": state.generation,
            "source": "rust",
        });
        Ok((
            response,
            vec![delivery_update(
                "updateGlobalDharmaDeliveryStarted",
                &job,
                Value::Null,
            )],
        ))
    } else {
        Ok((
            json!({
                "job": Value::Null,
                "found": false,
                "generation": state.generation,
                "source": "rust",
            }),
            vec![],
        ))
    }
}

pub(crate) fn record_receipt(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let job_id = required_string(&params, "jobId")?;
    let endpoint_id = required_string(&params, "endpointId")?;
    let receipt_id = params
        .get("receiptId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "gd_receipt_{}",
                RECEIPT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            )
        });
    let status = params
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("delivered")
        .to_string();
    let now = now_millis();

    let mut state = lock_state()?;
    let mut job = state.jobs.get(&job_id).cloned().ok_or_else(|| {
        RuntimeError::new(
            "delivery_job_not_found",
            format!("delivery job not found: {job_id}"),
        )
    })?;
    state.generation = state.generation.saturating_add(1);
    let receipt = json!({
        "receiptId": receipt_id,
        "jobId": job_id,
        "endpointId": endpoint_id,
        "status": status,
        "payload": params.get("payload").cloned().unwrap_or(Value::Null),
        "receivedAtMs": now,
        "generation": state.generation,
    });
    state.receipts.insert(receipt_id.clone(), receipt.clone());
    merge_job_fields(
        &mut job,
        json!({
            "status": if status == "delivered" || status == "ok" { "delivered" } else { "receipt_failed" },
            "lastReceiptId": receipt_id,
            "updatedAtMs": now,
            "generation": state.generation,
        }),
    );
    state.jobs.insert(job_id.clone(), job.clone());

    let response = json!({
        "recorded": true,
        "receiptId": receipt_id,
        "jobId": job_id,
        "receipt": receipt,
        "job": job,
        "generation": state.generation,
        "source": "rust",
    });
    Ok((
        response,
        vec![delivery_update(
            "updateGlobalDharmaReceiptReceived",
            &job,
            receipt,
        )],
    ))
}

pub(crate) fn list_receipts(params: Value) -> Result<(Value, Vec<Value>), RuntimeError> {
    let job_filter = params
        .get("jobId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let limit = params
        .get("limit")
        .and_then(read_u64_value)
        .unwrap_or(500)
        .min(5000) as usize;
    let state = lock_state()?;
    let mut receipts = state
        .receipts
        .values()
        .filter(|receipt| {
            job_filter
                .as_deref()
                .map(|job_id| receipt.get("jobId").and_then(Value::as_str) == Some(job_id))
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    receipts.sort_by(|a, b| {
        let at = a.get("receivedAtMs").and_then(Value::as_u64).unwrap_or(0);
        let bt = b.get("receivedAtMs").and_then(Value::as_u64).unwrap_or(0);
        at.cmp(&bt)
    });
    receipts.truncate(limit);
    Ok((
        json!({
            "receipts": receipts,
            "generation": state.generation,
            "source": "rust",
        }),
        vec![],
    ))
}

fn delivery_update(update_type: &str, job: &Value, receipt: Value) -> Value {
    json!({
        "@type": update_type,
        "jobId": job.get("jobId").cloned().unwrap_or(Value::Null),
        "status": job.get("status").cloned().unwrap_or(Value::Null),
        "job": job,
        "receipt": receipt,
        "createdAtMs": now_millis(),
    })
}

fn merge_job_fields(job: &mut Value, patch: Value) {
    let Some(job) = job.as_object_mut() else {
        return;
    };
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            job.insert(key.clone(), value.clone());
        }
    }
}

fn lock_state() -> Result<std::sync::MutexGuard<'static, DeliveryState>, RuntimeError> {
    DELIVERY
        .lock()
        .map_err(|_| RuntimeError::new("delivery_lock_failed", "delivery queue lock failed"))
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

fn read_i64_value(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
