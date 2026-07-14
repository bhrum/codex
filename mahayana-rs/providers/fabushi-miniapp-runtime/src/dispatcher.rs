use crate::delivery;
use crate::error::RuntimeError;
use crate::file_state;
use crate::network;
use crate::storage;
use serde_json::json;
use serde_json::Map;
use serde_json::Value;

const SUPPORTED_METHODS: &[&str] = &[
    "runtime.getStatus",
    "runtime.getAuthorizationState",
    "runtime.storage.configure",
    "runtime.storage.getStatus",
    "runtime.storage.put",
    "runtime.storage.get",
    "runtime.storage.delete",
    "runtime.storage.list",
    "runtime.storage.snapshot",
    "runtime.file.register",
    "runtime.file.updateState",
    "runtime.file.get",
    "runtime.file.list",
    "globalDharma.delivery.enqueue",
    "globalDharma.delivery.getJob",
    "globalDharma.delivery.listJobs",
    "globalDharma.delivery.nextRetry",
    "globalDharma.delivery.markAttempt",
    "globalDharma.delivery.recordReceipt",
    "globalDharma.delivery.listReceipts",
    "network.http.fetch",
    "network.udp.open",
    "network.udp.send",
    "network.udp.broadcast",
    "network.udp.close",
];

pub(crate) struct DispatchResult {
    pub(crate) response: Value,
    pub(crate) updates: Vec<Value>,
}

pub(crate) fn execute(request: Value) -> Result<Value, RuntimeError> {
    Ok(execute_with_updates(request)?.response)
}

pub(crate) fn execute_with_updates(request: Value) -> Result<DispatchResult, RuntimeError> {
    let request_type = request_type(&request)?;
    let extra = request.get("@extra").cloned();
    let params = request_params(&request)?;
    let mut result = dispatch_call(&request_type, params)?;
    result.response = attach_extra(result.response, extra);
    Ok(result)
}

pub(crate) fn dispatch_call(method: &str, params: Value) -> Result<DispatchResult, RuntimeError> {
    let (response, updates) = match method {
        "runtime.getStatus" => (runtime_status(), vec![]),
        "runtime.getAuthorizationState" => (
            json!({
                "@type": "runtime.authorizationState",
                "authorizationState": {
                    "@type": "authorizationStateReady",
                },
            }),
            vec![],
        ),
        "runtime.storage.configure" => {
            typed_batch("runtime.storage.configured", storage::configure(params)?)
        }
        "runtime.storage.getStatus" => {
            typed_batch("runtime.storage.status", storage::get_status(params)?)
        }
        "runtime.storage.put" => typed_batch("runtime.storage.stored", storage::put(params)?),
        "runtime.storage.get" => typed_batch("runtime.storage.record", storage::get(params)?),
        "runtime.storage.delete" => {
            typed_batch("runtime.storage.deleted", storage::delete(params)?)
        }
        "runtime.storage.list" => typed_batch("runtime.storage.records", storage::list(params)?),
        "runtime.storage.snapshot" => {
            typed_batch("runtime.storage.snapshot", storage::snapshot(params)?)
        }
        "runtime.file.register" => {
            typed_batch("runtime.file.registered", file_state::register(params)?)
        }
        "runtime.file.updateState" => {
            typed_batch("runtime.file.updated", file_state::update_state(params)?)
        }
        "runtime.file.get" => typed_batch("runtime.file.file", file_state::get(params)?),
        "runtime.file.list" => typed_batch("runtime.file.files", file_state::list(params)?),
        "globalDharma.delivery.enqueue" => typed_batch(
            "globalDharma.delivery.queued",
            delivery::enqueue_job(params)?,
        ),
        "globalDharma.delivery.getJob" => {
            typed_batch("globalDharma.delivery.job", delivery::get_job(params)?)
        }
        "globalDharma.delivery.listJobs" => {
            typed_batch("globalDharma.delivery.jobs", delivery::list_jobs(params)?)
        }
        "globalDharma.delivery.nextRetry" => typed_batch(
            "globalDharma.delivery.nextRetry",
            delivery::next_retry(params)?,
        ),
        "globalDharma.delivery.markAttempt" => typed_batch(
            "globalDharma.delivery.attemptMarked",
            delivery::mark_attempt(params)?,
        ),
        "globalDharma.delivery.recordReceipt" => typed_batch(
            "globalDharma.delivery.receiptRecorded",
            delivery::record_receipt(params)?,
        ),
        "globalDharma.delivery.listReceipts" => typed_batch(
            "globalDharma.delivery.receipts",
            delivery::list_receipts(params)?,
        ),
        "network.http.fetch" => (
            typed_response("network.http.response", network::http_fetch_json(params)?),
            vec![],
        ),
        "network.udp.open" => (
            typed_response("network.udp.opened", network::udp_open_json(params)?),
            vec![],
        ),
        "network.udp.send" => (
            typed_response("network.udp.sent", network::udp_send_json(params)?),
            vec![],
        ),
        "network.udp.broadcast" => (
            typed_response(
                "network.udp.broadcastSent",
                network::udp_broadcast_json(params)?,
            ),
            vec![],
        ),
        "network.udp.close" => (
            typed_response("network.udp.closed", network::udp_close_json(params)?),
            vec![],
        ),
        _ => {
            return Err(RuntimeError::new(
                "unknown_request",
                format!("unsupported runtime request: {method}"),
            ));
        }
    };
    Ok(DispatchResult { response, updates })
}

pub(crate) fn supported_methods() -> &'static [&'static str] {
    SUPPORTED_METHODS
}

fn runtime_status() -> Value {
    json!({
        "@type": "runtime.status",
        "clientCore": "fabushi-miniapp-runtime",
        "architecture": "tdlib-style-json-abi",
        "authorizationState": {
            "@type": "authorizationStateReady",
        },
        "kernelModules": [
            "local_consistency_storage",
            "file_state_registry",
            "global_dharma_delivery_queue",
            "delivery_receipts",
            "retry_scheduler"
        ],
        "supportedMethods": SUPPORTED_METHODS,
    })
}

fn request_type(request: &Value) -> Result<String, RuntimeError> {
    request
        .get("@type")
        .or_else(|| request.get("type"))
        .or_else(|| request.get("method"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            RuntimeError::new(
                "invalid_request",
                "runtime request must include a non-empty @type",
            )
        })
}

fn request_params(request: &Value) -> Result<Value, RuntimeError> {
    if let Some(params) = request.get("params") {
        if params.is_object() {
            return Ok(params.clone());
        }
        return Err(RuntimeError::new(
            "invalid_request",
            "params must be a JSON object when provided",
        ));
    }

    let object = request.as_object().ok_or_else(|| {
        RuntimeError::new("invalid_request", "runtime request must be a JSON object")
    })?;
    let mut params = Map::new();
    for (key, value) in object {
        if !matches!(key.as_str(), "@type" | "type" | "method" | "@extra") {
            params.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(params))
}

fn typed_batch(response_type: &str, batch: (Value, Vec<Value>)) -> (Value, Vec<Value>) {
    let (value, updates) = batch;
    (typed_response(response_type, value), updates)
}

fn typed_response(response_type: &str, value: Value) -> Value {
    match value {
        Value::Object(mut object) => {
            object.insert(
                "@type".to_string(),
                Value::String(response_type.to_string()),
            );
            Value::Object(object)
        }
        other => json!({
            "@type": response_type,
            "value": other,
        }),
    }
}

fn attach_extra(value: Value, extra: Option<Value>) -> Value {
    let Some(extra) = extra else {
        return value;
    };
    match value {
        Value::Object(mut object) => {
            object.insert("@extra".to_string(), extra);
            Value::Object(object)
        }
        other => json!({
            "@type": "runtime.response",
            "@extra": extra,
            "data": other,
        }),
    }
}
