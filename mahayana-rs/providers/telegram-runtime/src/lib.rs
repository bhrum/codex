//! Stable C/JSON ABI for Dart FFI and other platform shells.

mod runtime;

pub use runtime::{
    close_client, create_client, create_persistent_client, execute_json, RuntimeError,
};

use serde_json::json;
use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
};

#[no_mangle]
pub extern "C" fn fabushi_telegram_create_client() -> u64 {
    create_client()
}

/// Linker anchor for platforms that resolve the remaining C ABI symbols at
/// runtime (notably Dart FFI with an iOS static library).
#[no_mangle]
pub extern "C" fn fabushi_telegram_force_link() -> u32 {
    let symbols = [
        fabushi_telegram_create_client as *const () as usize,
        fabushi_telegram_create_persistent_client as *const () as usize,
        fabushi_telegram_execute as *const () as usize,
        fabushi_telegram_close_client as *const () as usize,
        fabushi_telegram_free_string as *const () as usize,
    ];
    std::hint::black_box(symbols);
    1
}

#[no_mangle]
/// Creates a runtime whose encrypted state is restored from `database_path`.
///
/// # Safety
/// `database_path` must be a valid NUL-terminated UTF-8 string. `storage_key`
/// must point to `storage_key_length` readable bytes and remain alive until the
/// call returns. Exactly 32 key bytes are required.
pub unsafe extern "C" fn fabushi_telegram_create_persistent_client(
    database_path: *const c_char,
    storage_key: *const u8,
    storage_key_length: usize,
) -> *mut c_char {
    let response = catch_unwind(AssertUnwindSafe(|| {
        let path = read_utf8(database_path)?;
        if storage_key.is_null() {
            return Err(RuntimeError::NullRequestPointer);
        }
        // SAFETY: upheld by this function's FFI contract.
        let key = unsafe { std::slice::from_raw_parts(storage_key, storage_key_length) };
        let client_id = create_persistent_client(path, key)?;
        Ok::<_, RuntimeError>(json!({
            "ok": true,
            "data": {
                "@type": "client.created",
                "clientId": client_id,
                "persistentStorage": true,
            }
        }))
    }));
    let value = match response {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => error_response(error.code(), &error.to_string()),
        Err(_) => error_response(
            "rust_panic",
            "Telegram Rust runtime caught an internal panic.",
        ),
    };
    into_c_string(value.to_string())
}

#[no_mangle]
/// # Safety
/// `request_json` must point to a valid NUL-terminated UTF-8 string that stays
/// alive until this synchronous call returns.
pub unsafe extern "C" fn fabushi_telegram_execute(
    client_id: u64,
    request_json: *const c_char,
) -> *mut c_char {
    let response = catch_unwind(AssertUnwindSafe(|| {
        let request = read_utf8(request_json)?;
        Ok::<_, RuntimeError>(execute_json(client_id, request))
    }));
    match response {
        Ok(Ok(json)) => into_c_string(json),
        Ok(Err(error)) => {
            into_c_string(error_response("invalid_ffi_request", &error.to_string()).to_string())
        }
        Err(_) => into_c_string(
            error_response(
                "rust_panic",
                "Telegram Rust runtime caught an internal panic.",
            )
            .to_string(),
        ),
    }
}

#[no_mangle]
pub extern "C" fn fabushi_telegram_close_client(client_id: u64) -> *mut c_char {
    let response = match close_client(client_id) {
        Ok(()) => json!({"ok": true, "data": {"@type": "client.closed", "clientId": client_id}}),
        Err(error) => error_response(error.code(), &error.to_string()),
    };
    into_c_string(response.to_string())
}

#[no_mangle]
/// # Safety
/// `pointer` must be null or a pointer returned by this library that has not
/// already been freed.
pub unsafe extern "C" fn fabushi_telegram_free_string(pointer: *mut c_char) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: this function only accepts pointers returned by CString::into_raw
    // from this library, and the Dart bridge frees each response exactly once.
    drop(unsafe { CString::from_raw(pointer) });
}

fn read_utf8<'a>(pointer: *const c_char) -> Result<&'a str, RuntimeError> {
    if pointer.is_null() {
        return Err(RuntimeError::NullRequestPointer);
    }
    // SAFETY: the FFI contract requires a non-null, NUL-terminated UTF-8 string
    // that remains alive for the duration of this synchronous call.
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|_| RuntimeError::InvalidUtf8)
}

fn into_c_string(value: String) -> *mut c_char {
    CString::new(value)
        .expect("serialized JSON never contains raw NUL bytes")
        .into_raw()
}

fn error_response(code: &str, message: &str) -> serde_json::Value {
    json!({
        "ok": false,
        "errorCode": code,
        "message": message,
    })
}
