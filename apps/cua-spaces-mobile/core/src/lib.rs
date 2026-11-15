// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The app core for the React Native app: `call(method, argsJson)` and its C
//! ABI. Every word the app shows that the desktop apps also show comes from
//! here (`cua_spaces_app_core::dispatch`), so the phone says what the Mac
//! says.

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use cua_spaces_app_core::dispatch;

pub mod device;
pub mod store;

/// Calls a core method; the result is its JSON, or the core's error text. `device.*` is this crate's (the phone's
/// device key); everything else is `cua_spaces_app_core::dispatch`.
pub fn call(method: &str, args: &str) -> Result<String, String> {
    match catch_unwind(AssertUnwindSafe(|| {
        if method.starts_with("device.") {
            device::call(method, args)
        } else if method.starts_with("store.") {
            store::call(method, args)
        } else {
            dispatch::call(method, args).map_err(|e| e.to_string())
        }
    })) {
        Ok(r) => r,
        Err(_) => Err(format!("{method}: the core panicked")),
    }
}

/// Every method `call` answers, as a JSON array.
pub fn methods() -> String {
    let all: Vec<&str> = dispatch::METHODS
        .iter()
        .copied()
        .chain(device::METHODS.iter().copied())
        .chain(store::METHODS.iter().copied())
        .collect();
    serde_json::to_string(&all).unwrap_or_else(|_| "[]".into())
}

/// The parity flows `[{name, flow, golden}]`, as JSON.
pub fn flows() -> String {
    serde_json::Value::Array(
        cua_spaces_app_core::parity::FLOWS
            .iter()
            .map(|(name, flow, golden)| serde_json::json!({ "name": name, "flow": flow, "golden": golden }))
            .collect(),
    )
    .to_string()
}

fn into_c(s: String) -> *mut c_char {
    // JSON never holds a NUL; if it somehow did, answer an empty string
    // rather than crash across the boundary.
    CString::new(s)
        .map(CString::into_raw)
        .unwrap_or(ptr::null_mut())
}

/// # Safety
/// `method` and `args` are NUL-terminated UTF-8. `error` is null or points
/// to a writable `char *`. Free the result and the error with
/// [`cua_core_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cua_core_call(
    method: *const c_char,
    args: *const c_char,
    error: *mut *mut c_char,
) -> *mut c_char {
    let read = |p: *const c_char| {
        if p.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(p) }.to_str().ok()
    };
    let (Some(method), Some(args)) = (read(method), read(args)) else {
        if !error.is_null() {
            unsafe { *error = into_c("the method and arguments must be UTF-8 text".into()) };
        }
        return ptr::null_mut();
    };
    match call(method, args) {
        Ok(json) => into_c(json),
        Err(e) => {
            if !error.is_null() {
                unsafe { *error = into_c(e) };
            }
            ptr::null_mut()
        }
    }
}

/// Every method name, as a JSON array. Free with [`cua_core_free`].
#[unsafe(no_mangle)]
pub extern "C" fn cua_core_methods() -> *mut c_char {
    into_c(methods())
}

/// The parity flows as JSON. Free with [`cua_core_free`].
#[unsafe(no_mangle)]
pub extern "C" fn cua_core_flows() -> *mut c_char {
    into_c(flows())
}

/// Frees a string this library returned.
///
/// # Safety
/// `s` came from this library and is freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cua_core_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_c_abi_round_trips_a_core_call() {
        let m = CString::new("spaces.listView").unwrap();
        let a =
            CString::new(r#"{"input":{"loaded":true,"incomplete":true,"reaching":1,"count":2}}"#)
                .unwrap();
        let mut err: *mut c_char = ptr::null_mut();
        let out = unsafe { cua_core_call(m.as_ptr(), a.as_ptr(), &mut err) };
        assert!(err.is_null());
        let text = unsafe { CStr::from_ptr(out) }.to_str().unwrap().to_string();
        unsafe { cua_core_free(out) };
        assert!(text.contains("Reaching 1 machine"), "{text}");
    }

    #[test]
    fn an_unknown_method_is_an_error_not_a_crash() {
        let m = CString::new("nope.nothing").unwrap();
        let a = CString::new("{}").unwrap();
        let mut err: *mut c_char = ptr::null_mut();
        let out = unsafe { cua_core_call(m.as_ptr(), a.as_ptr(), &mut err) };
        assert!(out.is_null());
        assert!(!err.is_null());
        unsafe { cua_core_free(err) };
    }
}
