use crate::error::RuntimeError;
use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

pub(crate) fn free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(ptr);
    }
}

pub(crate) fn read_json_request(ptr: *const c_char) -> Result<Value, RuntimeError> {
    if ptr.is_null() {
        return Err(RuntimeError::new(
            "invalid_request",
            "request pointer is null",
        ));
    }
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|error| RuntimeError::new("invalid_utf8", error.to_string()))?;
    serde_json::from_str(text).map_err(|error| RuntimeError::new("invalid_json", error.to_string()))
}

pub(crate) fn ffi_result(mut run: impl FnMut() -> Result<Value, RuntimeError>) -> *mut c_char {
    let value = match run() {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(error) => error.to_response(),
    };
    json_to_c_string(value)
}

pub(crate) fn runtime_receive_result(
    mut run: impl FnMut() -> Result<Option<Value>, RuntimeError>,
) -> *mut c_char {
    match run() {
        Ok(Some(value)) => json_to_c_string(value),
        Ok(None) => ptr::null_mut(),
        Err(error) => json_to_c_string(error.to_runtime_event()),
    }
}

pub(crate) fn json_to_c_string(value: Value) -> *mut c_char {
    let text = value.to_string().replace('\0', "");
    CString::new(text)
        .unwrap_or_else(|_| {
            CString::new(
                "{\"ok\":false,\"errorCode\":\"internal_error\",\"message\":\"invalid response string\"}",
            )
            .unwrap()
        })
        .into_raw()
}
