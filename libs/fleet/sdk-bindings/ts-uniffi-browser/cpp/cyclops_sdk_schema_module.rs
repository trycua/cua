#[allow(unused_imports)]
use uniffi_runtime_javascript::{self as js, uniffi as u, IntoJs, IntoRust};
use wasm_bindgen::prelude::wasm_bindgen;
extern "C" {
    fn uniffi_cyclops_sdk_schema_fn_clone_preservedjson(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_free_preservedjson(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_schema_fn_constructor_preservedjson_from_json(
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_preservedjson_to_json(
        ptr: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_eq_eq(
        ptr: u::RustBuffer,
        other: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> i8;
    fn uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_eq_ne(
        ptr: u::RustBuffer,
        other: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> i8;
    fn uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_hash(
        ptr: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_checksum_method_preservedjson_to_json() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_preservedjson_from_json() -> u16;
    fn ffi_cyclops_sdk_schema_uniffi_contract_version() -> u32;
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_clone_preservedjson(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_clone_preservedjson(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_free_preservedjson(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_schema_fn_free_preservedjson(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_constructor_preservedjson_from_json(
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_constructor_preservedjson_from_json(
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_preservedjson_to_json(
    ptr: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_preservedjson_to_json(
            u64::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_eq_eq(
    ptr: js::ForeignBytes,
    other: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Int8 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_eq_eq(
            u::RustBuffer::into_rust(ptr),
            u::RustBuffer::into_rust(other),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_eq_ne(
    ptr: js::ForeignBytes,
    other: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Int8 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_eq_ne(
            u::RustBuffer::into_rust(ptr),
            u::RustBuffer::into_rust(other),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_hash(
    ptr: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::UInt64 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_hash(
            u::RustBuffer::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_preservedjson_to_json() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_preservedjson_to_json().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_preservedjson_from_json() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_preservedjson_from_json().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_schema_uniffi_contract_version() -> js::UInt32 {
    ffi_cyclops_sdk_schema_uniffi_contract_version().into_js()
}
