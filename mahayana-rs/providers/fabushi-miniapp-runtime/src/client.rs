use crate::delivery_worker;
use crate::dispatcher;
use crate::error::RuntimeError;
use once_cell::sync::Lazy;
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

static CLIENTS: Lazy<Mutex<HashMap<u64, Arc<RuntimeClient>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static CLIENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct RuntimeClient {
    queue: Mutex<VecDeque<Value>>,
    queue_ready: Condvar,
    closed: AtomicBool,
}

impl RuntimeClient {
    fn new(client_id: u64) -> Self {
        let mut queue = VecDeque::new();
        queue.push_back(json!({
            "@type": "updateAuthorizationState",
            "authorizationState": {
                "@type": "authorizationStateReady",
            },
            "clientId": client_id,
        }));
        queue.push_back(json!({
            "@type": "updateRuntimeClientReady",
            "clientId": client_id,
            "clientCore": "fabushi-miniapp-runtime",
            "architecture": "tdlib-style-json-abi",
            "supportedMethods": dispatcher::supported_methods(),
        }));
        Self {
            queue: Mutex::new(queue),
            queue_ready: Condvar::new(),
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) fn enqueue(&self, value: Value) -> Result<(), RuntimeError> {
        if self.is_closed() {
            return Ok(());
        }
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| RuntimeError::new("runtime_lock_failed", "client queue lock failed"))?;
        queue.push_back(value);
        self.queue_ready.notify_one();
        Ok(())
    }

    fn receive(&self, timeout: Duration) -> Result<Option<Value>, RuntimeError> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| RuntimeError::new("runtime_lock_failed", "client queue lock failed"))?;
        if let Some(value) = queue.pop_front() {
            return Ok(Some(value));
        }
        if timeout.is_zero() || self.is_closed() {
            return Ok(None);
        }
        let (mut queue, _) = self
            .queue_ready
            .wait_timeout(queue, timeout)
            .map_err(|_| RuntimeError::new("runtime_lock_failed", "client queue lock failed"))?;
        Ok(queue.pop_front())
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.queue_ready.notify_all();
    }
}

pub(crate) fn create_client() -> u64 {
    let client_id = CLIENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let client = Arc::new(RuntimeClient::new(client_id));
    CLIENTS
        .lock()
        .expect("client registry lock")
        .insert(client_id, client);
    client_id
}

pub(crate) fn close_client(client_id: u64) -> Result<Value, RuntimeError> {
    let client = CLIENTS
        .lock()
        .map_err(|_| RuntimeError::new("runtime_lock_failed", "client registry lock failed"))?
        .remove(&client_id);
    if let Some(client) = client.as_ref() {
        client.close();
    }
    Ok(json!({
        "closed": client.is_some(),
        "clientId": client_id,
    }))
}

pub(crate) fn send(client_id: u64, request: Value) -> Result<Value, RuntimeError> {
    let client = get_client(client_id)?;
    let request_id = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let extra = request.get("@extra").cloned();
    let should_start_delivery_worker =
        request_method(&request).as_deref() == Some("globalDharma.delivery.enqueue");
    client.enqueue(attach_extra(
        json!({
            "@type": "updateRuntimeRequestAccepted",
            "clientId": client_id,
            "requestId": request_id,
        }),
        extra.clone(),
    ))?;

    let worker_client = client.clone();
    thread::spawn(move || {
        let response = match dispatcher::execute_with_updates(request) {
            Ok(result) => {
                for update in result.updates {
                    let _ = client.enqueue(update);
                }
                if should_start_delivery_worker {
                    delivery_worker::ensure_running(client_id, worker_client);
                }
                result.response
            }
            Err(error) => attach_extra(error.to_runtime_event(), extra),
        };
        let _ = client.enqueue(attach_request_id(response, client_id, request_id));
    });

    Ok(json!({
        "queued": true,
        "clientId": client_id,
        "requestId": request_id,
    }))
}

pub(crate) fn receive(client_id: u64, timeout_seconds: f64) -> Result<Option<Value>, RuntimeError> {
    get_client(client_id)?.receive(timeout_from_seconds(timeout_seconds))
}

fn get_client(client_id: u64) -> Result<Arc<RuntimeClient>, RuntimeError> {
    CLIENTS
        .lock()
        .map_err(|_| RuntimeError::new("runtime_lock_failed", "client registry lock failed"))?
        .get(&client_id)
        .cloned()
        .ok_or_else(|| {
            RuntimeError::new(
                "client_not_found",
                format!("runtime client not found: {client_id}"),
            )
        })
}

fn timeout_from_seconds(value: f64) -> Duration {
    if !value.is_finite() || value <= 0.0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(value.min(60.0))
}

fn request_method(request: &Value) -> Option<String> {
    request
        .get("@type")
        .or_else(|| request.get("type"))
        .or_else(|| request.get("method"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
            "@type": "runtime.update",
            "@extra": extra,
            "data": other,
        }),
    }
}

fn attach_request_id(value: Value, client_id: u64, request_id: u64) -> Value {
    match value {
        Value::Object(mut object) => {
            object.insert("clientId".to_string(), json!(client_id));
            object.insert("requestId".to_string(), json!(request_id));
            Value::Object(object)
        }
        other => json!({
            "@type": "runtime.response",
            "clientId": client_id,
            "requestId": request_id,
            "data": other,
        }),
    }
}
