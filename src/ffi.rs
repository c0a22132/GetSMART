use std::ffi::{CStr, CString, c_char};

use serde::Serialize;

use crate::error::ErrorCode;
use crate::{GetSmartError, get_smart, list_devices};

#[derive(Debug, Serialize)]
struct SuccessEnvelope<T> {
    ok: bool,
    data: T,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: ErrorCode,
    message: String,
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    ok: bool,
    error: ErrorBody,
}

static VERSION_BYTES: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

fn encode_json<T>(result: Result<T, GetSmartError>) -> *mut c_char
where
    T: Serialize,
{
    let payload = match result {
        Ok(data) => serde_json::to_string(&SuccessEnvelope { ok: true, data }),
        Err(error) => serde_json::to_string(&ErrorEnvelope {
            ok: false,
            error: ErrorBody {
                code: error.code(),
                message: error.to_string(),
            },
        }),
    }
    .unwrap_or_else(|serialization_error| {
        format!(
            "{{\"ok\":false,\"error\":{{\"code\":\"internal_error\",\"message\":\"{}\"}}}}",
            serialization_error
        )
    });

    CString::new(payload)
        .expect("JSON payloads never contain interior NUL bytes")
        .into_raw()
}

fn handle_ffi_call<T>(f: impl FnOnce() -> Result<T, GetSmartError>) -> *mut c_char
where
    T: Serialize,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => encode_json(result),
        Err(_) => encode_json::<()>(Err(GetSmartError::internal("panic across FFI boundary"))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn getsmart_list_devices_json() -> *mut c_char {
    handle_ffi_call(list_devices)
}

#[unsafe(no_mangle)]
pub extern "C" fn getsmart_get_smart_json(device_id: *const c_char) -> *mut c_char {
    handle_ffi_call(|| {
        if device_id.is_null() {
            return Err(GetSmartError::InvalidArgument(
                "device_id pointer must not be null".to_owned(),
            ));
        }

        let device_id = unsafe { CStr::from_ptr(device_id) }.to_str()?;
        get_smart(device_id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn getsmart_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }

    unsafe {
        drop(CString::from_raw(ptr));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn getsmart_version() -> *const c_char {
    VERSION_BYTES.as_ptr().cast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_success_envelope_uses_snake_case_keys() {
        let ptr = getsmart_list_devices_json();
        assert!(!ptr.is_null());

        let payload = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("payload must be valid UTF-8")
            .to_owned();
        getsmart_free_string(ptr);

        assert!(payload.contains("\"ok\":"));
    }

    #[test]
    fn ffi_error_envelope_is_serialized() {
        let ptr = getsmart_get_smart_json(std::ptr::null());
        assert!(!ptr.is_null());

        let payload = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("payload must be valid UTF-8")
            .to_owned();
        getsmart_free_string(ptr);

        assert!(payload.contains("\"ok\":false"));
        assert!(payload.contains("invalid_argument"));
    }
}
