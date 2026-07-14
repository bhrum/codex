use crate::error::RuntimeError;
use base64::engine::general_purpose;
use base64::Engine as _;
use once_cell::sync::Lazy;
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::net::UdpSocket;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

static UDP_SOCKETS: Lazy<Mutex<HashMap<String, UdpSocket>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static UDP_SOCKET_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn http_fetch_json(params: Value) -> Result<Value, RuntimeError> {
    let url = required_string(&params, "url")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(RuntimeError::new(
            "invalid_url",
            "network.http.fetch only supports http:// and https:// URLs",
        ));
    }

    let method = params
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .trim()
        .to_ascii_uppercase();
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
    ) {
        return Err(RuntimeError::new(
            "invalid_method",
            format!("unsupported HTTP method: {method}"),
        ));
    }

    let timeout_ms = read_u64(&params, "timeoutMs", 15_000).clamp(1_000, 120_000);
    let max_body_bytes =
        read_u64(&params, "maxBodyBytes", 2 * 1024 * 1024).clamp(1, 16 * 1024 * 1024) as usize;

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(timeout_ms))
        .build();
    let mut request = agent.request(&method, &url);

    if let Some(headers) = params.get("headers").and_then(Value::as_object) {
        for (key, value) in headers {
            request = request.set(key, value.as_str().unwrap_or(&value.to_string()));
        }
    }

    let body = request_body(&params)?;
    let response = match body.as_deref() {
        Some(bytes) => request.send_bytes(bytes),
        None => request.call(),
    };
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(error)) => {
            return Err(RuntimeError::new("network_error", error.to_string()));
        }
    };

    let status_code = response.status();
    let mut headers = serde_json::Map::new();
    for name in response.headers_names() {
        if let Some(value) = response.header(&name) {
            headers.insert(name, json!(value));
        }
    }

    let mut body_bytes = Vec::new();
    let mut reader = response.into_reader().take((max_body_bytes + 1) as u64);
    reader
        .read_to_end(&mut body_bytes)
        .map_err(|error| RuntimeError::new("read_failed", error.to_string()))?;
    if body_bytes.len() > max_body_bytes {
        return Err(RuntimeError::new(
            "response_too_large",
            format!("HTTP response exceeded {max_body_bytes}B limit"),
        ));
    }

    Ok(json!({
        "statusCode": status_code,
        "headers": headers,
        "body": String::from_utf8_lossy(&body_bytes).to_string(),
        "bodyBase64": general_purpose::STANDARD.encode(&body_bytes),
        "bodyBytes": body_bytes.len(),
        "bodyTextEncoding": "utf-8",
        "url": url,
        "source": "rust",
    }))
}

pub(crate) fn udp_open_json(params: Value) -> Result<Value, RuntimeError> {
    let port = read_u64(&params, "port", 0).min(65_535) as u16;
    let bind_address = params
        .get("bindAddress")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("0.0.0.0");
    let socket = UdpSocket::bind((bind_address, port))
        .map_err(|error| RuntimeError::new("udp_bind_failed", error.to_string()))?;
    let broadcast = params
        .get("broadcast")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    socket
        .set_broadcast(broadcast)
        .map_err(|error| RuntimeError::new("udp_broadcast_failed", error.to_string()))?;
    let local_addr = socket
        .local_addr()
        .map_err(|error| RuntimeError::new("udp_local_addr_failed", error.to_string()))?;
    let socket_id = format!(
        "rust_udp_{}",
        UDP_SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    UDP_SOCKETS
        .lock()
        .map_err(|_| RuntimeError::new("udp_lock_failed", "UDP socket registry lock failed"))?
        .insert(socket_id.clone(), socket);

    Ok(json!({
        "socketId": socket_id,
        "address": local_addr.ip().to_string(),
        "port": local_addr.port(),
        "broadcast": broadcast,
        "source": "rust",
    }))
}

pub(crate) fn udp_send_json(params: Value) -> Result<Value, RuntimeError> {
    let socket_id = required_string(&params, "socketId")?;
    let host = required_string(&params, "host")?;
    let port = read_udp_port(&params, "port")?;
    let payload = request_udp_payload(&params)?;
    let target = format!("{host}:{port}");
    let sent_bytes = {
        let sockets = UDP_SOCKETS
            .lock()
            .map_err(|_| RuntimeError::new("udp_lock_failed", "UDP socket registry lock failed"))?;
        let socket = sockets.get(&socket_id).ok_or_else(|| {
            RuntimeError::new(
                "socket_not_found",
                format!("UDP socket not found: {socket_id}"),
            )
        })?;
        socket
            .send_to(&payload, &target)
            .map_err(|error| RuntimeError::new("udp_send_failed", error.to_string()))?
    };

    Ok(json!({
        "socketId": socket_id,
        "host": host,
        "port": port,
        "sentBytes": sent_bytes,
        "source": "rust",
    }))
}

pub(crate) fn udp_broadcast_json(params: Value) -> Result<Value, RuntimeError> {
    let host = params
        .get("host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("255.255.255.255")
        .to_string();
    let port = read_udp_port(&params, "port")?;
    let payload = request_udp_payload(&params)?;
    let target = format!("{host}:{port}");
    let socket_id = params
        .get("socketId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let sent_bytes = if let Some(id) = socket_id.as_deref() {
        let sockets = UDP_SOCKETS
            .lock()
            .map_err(|_| RuntimeError::new("udp_lock_failed", "UDP socket registry lock failed"))?;
        let socket = sockets.get(id).ok_or_else(|| {
            RuntimeError::new("socket_not_found", format!("UDP socket not found: {id}"))
        })?;
        socket
            .set_broadcast(true)
            .map_err(|error| RuntimeError::new("udp_broadcast_failed", error.to_string()))?;
        socket
            .send_to(&payload, &target)
            .map_err(|error| RuntimeError::new("udp_send_failed", error.to_string()))?
    } else {
        let socket = UdpSocket::bind(("0.0.0.0", 0))
            .map_err(|error| RuntimeError::new("udp_bind_failed", error.to_string()))?;
        socket
            .set_broadcast(true)
            .map_err(|error| RuntimeError::new("udp_broadcast_failed", error.to_string()))?;
        socket
            .send_to(&payload, &target)
            .map_err(|error| RuntimeError::new("udp_send_failed", error.to_string()))?
    };

    Ok(json!({
        "socketId": socket_id,
        "host": host,
        "port": port,
        "sentBytes": sent_bytes,
        "temporarySocket": socket_id.is_none(),
        "source": "rust",
    }))
}

pub(crate) fn udp_close_json(params: Value) -> Result<Value, RuntimeError> {
    let socket_id = required_string(&params, "socketId")?;
    let removed = UDP_SOCKETS
        .lock()
        .map_err(|_| RuntimeError::new("udp_lock_failed", "UDP socket registry lock failed"))?
        .remove(&socket_id)
        .is_some();
    if !removed {
        return Err(RuntimeError::new(
            "socket_not_found",
            format!("UDP socket not found: {socket_id}"),
        ));
    }
    Ok(json!({ "closed": true, "socketId": socket_id, "source": "rust" }))
}

fn request_body(params: &Value) -> Result<Option<Vec<u8>>, RuntimeError> {
    if let Some(body) = params.get("body") {
        return Ok(Some(
            body.as_str()
                .unwrap_or(&body.to_string())
                .as_bytes()
                .to_vec(),
        ));
    }
    if let Some(encoded) = params.get("bodyBase64").and_then(Value::as_str) {
        return general_purpose::STANDARD
            .decode(encoded)
            .map(Some)
            .map_err(|error| RuntimeError::new("invalid_body", error.to_string()));
    }
    Ok(None)
}

fn request_udp_payload(params: &Value) -> Result<Vec<u8>, RuntimeError> {
    let encoded = params
        .get("data")
        .or_else(|| params.get("dataBase64"))
        .and_then(Value::as_str)
        .ok_or_else(|| RuntimeError::new("invalid_request", "data must be a base64 string"))?;
    if encoded.trim().is_empty() {
        return Err(RuntimeError::new("invalid_request", "data cannot be empty"));
    }
    general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| RuntimeError::new("invalid_request", error.to_string()))
}

fn required_string(params: &Value, key: &str) -> Result<String, RuntimeError> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RuntimeError::new("invalid_request", format!("{key} cannot be empty")))?;
    Ok(value.to_string())
}

fn read_u64(params: &Value, key: &str, fallback: u64) -> u64 {
    params
        .get(key)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(fallback)
}

fn read_udp_port(params: &Value, key: &str) -> Result<u16, RuntimeError> {
    let value = read_u64(params, key, 0);
    if value == 0 || value > 65_535 {
        return Err(RuntimeError::new(
            "invalid_request",
            "UDP port must be between 1 and 65535",
        ));
    }
    Ok(value as u16)
}
