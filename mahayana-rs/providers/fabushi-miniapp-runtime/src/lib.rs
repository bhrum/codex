mod client;
mod delivery;
mod delivery_worker;
mod dispatcher;
mod error;
mod ffi;
mod file_state;
mod network;
mod storage;

use std::os::raw::c_char;

/// Executes one mini-app host request through the same Rust dispatcher that is
/// exported through the Flutter FFI ABI.  Product shells such as Mahayana CLI
/// use this entry point instead of carrying a second implementation of the
/// mini-app protocol.
pub fn execute_json(request_json: &str) -> Result<String, String> {
    let request =
        serde_json::from_str(request_json).map_err(|error| format!("invalid_json: {error}"))?;
    dispatcher::execute(request)
        .map(|response| response.to_string())
        .map_err(|error| format!("{}: {}", error.code, error.message))
}

/// Lists the host operations implemented by this runtime.  This is deliberately
/// shared by the CLI and FFI layers so a web mini-app sees the same contract on
/// every product surface.
pub fn supported_methods() -> &'static [&'static str] {
    dispatcher::supported_methods()
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_free_string(ptr: *mut c_char) {
    ffi::free_string(ptr);
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_create_client() -> u64 {
    client::create_client()
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_send(client_id: u64, request_json: *const c_char) -> *mut c_char {
    ffi::ffi_result(|| client::send(client_id, ffi::read_json_request(request_json)?))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_receive(client_id: u64, timeout_seconds: f64) -> *mut c_char {
    ffi::runtime_receive_result(|| client::receive(client_id, timeout_seconds))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_execute(request_json: *const c_char) -> *mut c_char {
    ffi::ffi_result(|| dispatcher::execute(ffi::read_json_request(request_json)?))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_close(client_id: u64) -> *mut c_char {
    ffi::ffi_result(|| client::close_client(client_id))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_close_client(client_id: u64) -> *mut c_char {
    ffi::ffi_result(|| client::close_client(client_id))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_http_fetch_json(request_json: *const c_char) -> *mut c_char {
    ffi::ffi_result(|| network::http_fetch_json(ffi::read_json_request(request_json)?))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_udp_open_json(request_json: *const c_char) -> *mut c_char {
    ffi::ffi_result(|| network::udp_open_json(ffi::read_json_request(request_json)?))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_udp_send_json(request_json: *const c_char) -> *mut c_char {
    ffi::ffi_result(|| network::udp_send_json(ffi::read_json_request(request_json)?))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_udp_broadcast_json(request_json: *const c_char) -> *mut c_char {
    ffi::ffi_result(|| network::udp_broadcast_json(ffi::read_json_request(request_json)?))
}

#[no_mangle]
pub extern "C" fn fabushi_runtime_udp_close_json(request_json: *const c_char) -> *mut c_char {
    ffi::ffi_result(|| network::udp_close_json(ffi::read_json_request(request_json)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::ffi::{CStr, CString};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn execute_dispatches_typed_requests_with_extra() {
        let response = dispatcher::execute(json!({
            "@type": "runtime.getStatus",
            "@extra": "req_001",
        }))
        .expect("runtime status response");

        assert_eq!(response["@type"], "runtime.status");
        assert_eq!(response["@extra"], "req_001");
        assert_eq!(response["architecture"], "tdlib-style-json-abi");
    }

    #[test]
    fn execute_accepts_params_object() {
        let response = dispatcher::execute(json!({
            "@type": "runtime.getAuthorizationState",
            "params": {},
        }))
        .expect("authorization state response");

        assert_eq!(response["@type"], "runtime.authorizationState");
        assert_eq!(
            response["authorizationState"]["@type"],
            "authorizationStateReady"
        );
    }

    #[test]
    fn client_send_emits_updates_and_response() {
        let client_id = client::create_client();

        let initial = client::receive(client_id, 0.0)
            .expect("receive initial")
            .expect("initial auth update");
        assert_eq!(initial["@type"], "updateAuthorizationState");

        let ready = client::receive(client_id, 0.0)
            .expect("receive ready")
            .expect("ready update");
        assert_eq!(ready["@type"], "updateRuntimeClientReady");

        let ack = client::send(
            client_id,
            json!({
                "@type": "runtime.getStatus",
                "@extra": "req_async",
            }),
        )
        .expect("send request");
        assert_eq!(ack["queued"], true);

        let accepted = client::receive(client_id, 0.0)
            .expect("receive accepted")
            .expect("accepted update");
        assert_eq!(accepted["@type"], "updateRuntimeRequestAccepted");
        assert_eq!(accepted["@extra"], "req_async");

        let deadline = Instant::now() + Duration::from_secs(2);
        let response = loop {
            if let Some(value) = client::receive(client_id, 0.05).expect("receive response") {
                break value;
            }
            assert!(Instant::now() < deadline, "timed out waiting for response");
        };
        assert_eq!(response["@type"], "runtime.status");
        assert_eq!(response["@extra"], "req_async");
        assert_eq!(response["clientId"], client_id);
        assert!(response["requestId"].as_u64().is_some());
    }

    #[test]
    fn receive_times_out_to_none() {
        let client_id = client::create_client();
        let _ = client::receive(client_id, 0.0).expect("auth update");
        let _ = client::receive(client_id, 0.0).expect("ready update");

        assert!(client::receive(client_id, 0.0)
            .expect("empty receive")
            .is_none());
    }

    #[test]
    fn storage_put_get_delete_list_snapshot_and_conflict() {
        let collection = format!("test_storage_{}", now_micros());
        let stored = dispatcher::execute(json!({
            "@type": "runtime.storage.put",
            "collection": collection,
            "key": "alpha",
            "value": {"n": 1},
            "expectedRevision": 0,
        }))
        .expect("put record");
        assert_eq!(stored["@type"], "runtime.storage.stored");
        assert_eq!(stored["record"]["revision"], 1);

        let conflict = dispatcher::execute(json!({
            "@type": "runtime.storage.put",
            "collection": collection,
            "key": "alpha",
            "value": {"n": 2},
            "expectedRevision": 0,
        }))
        .expect_err("revision conflict");
        assert_eq!(conflict.code, "storage_revision_conflict");

        let fetched = dispatcher::execute(json!({
            "@type": "runtime.storage.get",
            "collection": collection,
            "key": "alpha",
        }))
        .expect("get record");
        assert_eq!(fetched["record"]["value"]["n"], 1);

        let listed = dispatcher::execute(json!({
            "@type": "runtime.storage.list",
            "collection": collection,
        }))
        .expect("list records");
        assert_eq!(listed["records"].as_array().unwrap().len(), 1);

        let deleted = dispatcher::execute(json!({
            "@type": "runtime.storage.delete",
            "collection": collection,
            "key": "alpha",
            "expectedRevision": 1,
        }))
        .expect("delete record");
        assert_eq!(deleted["record"]["deleted"], true);

        let after_delete = dispatcher::execute(json!({
            "@type": "runtime.storage.get",
            "collection": collection,
            "key": "alpha",
        }))
        .expect("get deleted record");
        assert_eq!(after_delete["found"], false);

        let snapshot = dispatcher::execute(json!({
            "@type": "runtime.storage.snapshot",
        }))
        .expect("snapshot");
        assert_eq!(snapshot["@type"], "runtime.storage.snapshot");
        assert_eq!(snapshot["version"], 1);
    }

    #[test]
    fn file_registry_register_update_get_list() {
        let file_id = format!("test_file_{}", now_micros());
        let registered = dispatcher::execute(json!({
            "@type": "runtime.file.register",
            "fileId": file_id,
            "state": "queued",
            "localPath": "/tmp/source.txt",
            "expectedBytes": 42,
        }))
        .expect("register file");
        assert_eq!(registered["file"]["state"], "queued");
        assert_eq!(registered["file"]["revision"], 1);

        let updated = dispatcher::execute(json!({
            "@type": "runtime.file.updateState",
            "fileId": file_id,
            "state": "ready",
            "remoteUrl": "https://example.test/file.txt",
        }))
        .expect("update file");
        assert_eq!(updated["file"]["state"], "ready");
        assert_eq!(updated["file"]["revision"], 2);

        let fetched = dispatcher::execute(json!({
            "@type": "runtime.file.get",
            "fileId": file_id,
        }))
        .expect("get file");
        assert_eq!(fetched["found"], true);
        assert_eq!(
            fetched["file"]["remoteUrl"],
            "https://example.test/file.txt"
        );

        let listed = dispatcher::execute(json!({
            "@type": "runtime.file.list",
            "state": "ready",
        }))
        .expect("list files");
        assert!(listed["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["fileId"] == file_id));
    }

    #[test]
    fn delivery_queue_retry_attempt_receipt_flow() {
        let job_id = format!("test_job_{}", now_micros());
        let queued = dispatcher::execute(json!({
            "@type": "globalDharma.delivery.enqueue",
            "jobId": job_id,
            "packet": {"text": "南无阿弥陀佛"},
            "endpoints": [{"id": "endpoint_1", "transport": "p2p"}],
            "maxAttempts": 2,
        }))
        .expect("enqueue delivery job");
        assert_eq!(queued["job"]["status"], "queued");

        let next = dispatcher::execute(json!({
            "@type": "globalDharma.delivery.nextRetry",
        }))
        .expect("next retry");
        assert_eq!(next["found"], true);
        assert_eq!(next["job"]["status"], "in_flight");

        let attempted = dispatcher::execute(json!({
            "@type": "globalDharma.delivery.markAttempt",
            "jobId": job_id,
            "endpointId": "endpoint_1",
            "success": true,
        }))
        .expect("mark attempt");
        assert_eq!(attempted["job"]["status"], "sent");

        let receipt = dispatcher::execute(json!({
            "@type": "globalDharma.delivery.recordReceipt",
            "jobId": job_id,
            "endpointId": "endpoint_1",
            "status": "delivered",
            "payload": {"ok": true},
        }))
        .expect("record receipt");
        assert_eq!(receipt["recorded"], true);
        assert_eq!(receipt["job"]["status"], "delivered");

        let receipts = dispatcher::execute(json!({
            "@type": "globalDharma.delivery.listReceipts",
            "jobId": job_id,
        }))
        .expect("list receipts");
        assert_eq!(receipts["receipts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn delivery_worker_consumes_queued_job_and_emits_updates() {
        let client_id = client::create_client();
        let _ = client::receive(client_id, 0.0).expect("auth update");
        let _ = client::receive(client_id, 0.0).expect("ready update");
        let job_id = format!("test_worker_job_{}", now_micros());

        client::send(
            client_id,
            json!({
                "@type": "globalDharma.delivery.enqueue",
                "@extra": "worker_enqueue",
                "jobId": job_id,
                "packet": {"text": "worker"},
                "endpoints": [{"id": "missing_p2p", "transport": "p2p"}],
                "maxAttempts": 1,
                "retryAfterMs": 1,
            }),
        )
        .expect("send enqueue");

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut saw_started = false;
        let mut saw_failed_attempt = false;
        while Instant::now() < deadline {
            if let Some(event) = client::receive(client_id, 0.05).expect("receive worker event") {
                if event["@type"] == "updateGlobalDharmaDeliveryWorkerStarted" {
                    saw_started = true;
                }
                if event["@type"] == "updateGlobalDharmaDeliveryAttempt"
                    && event["jobId"] == job_id
                    && event["status"] == "failed"
                {
                    saw_failed_attempt = true;
                    break;
                }
            }
        }
        assert!(saw_started, "worker did not start");
        assert!(saw_failed_attempt, "worker did not mark failed attempt");
        let _ = client::close_client(client_id);
    }

    #[test]
    fn ffi_execute_returns_wrapped_json() {
        let request = CString::new(r#"{"@type":"runtime.getStatus","@extra":"ffi"}"#).unwrap();
        let ptr = fabushi_runtime_execute(request.as_ptr());
        assert!(!ptr.is_null());
        let response = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string();
        fabushi_runtime_free_string(ptr);

        let value: Value = serde_json::from_str(&response).expect("json response");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["@type"], "runtime.status");
        assert_eq!(value["data"]["@extra"], "ffi");
    }

    #[test]
    fn legacy_http_ffi_keeps_wrapped_error_shape() {
        let request = CString::new(r#"{"url":"ftp://example.test"}"#).unwrap();
        let ptr = fabushi_runtime_http_fetch_json(request.as_ptr());
        assert!(!ptr.is_null());
        let response = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string();
        fabushi_runtime_free_string(ptr);

        let value: Value = serde_json::from_str(&response).expect("json response");
        assert_eq!(value["ok"], false);
        assert_eq!(value["errorCode"], "invalid_url");
    }

    fn now_micros() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
    }
}
