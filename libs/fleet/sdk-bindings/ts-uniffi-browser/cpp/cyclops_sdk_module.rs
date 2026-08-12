#[allow(unused_imports)]
use uniffi_runtime_javascript::{self as js, uniffi as u, IntoJs, IntoRust};
use wasm_bindgen::prelude::wasm_bindgen;
extern "C" {
    fn uniffi_cyclops_sdk_fn_clone_cyclopsclient(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_free_cyclopsclient(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_fn_constructor_cyclopsclient_connect(
        configuration: u::RustBuffer,
        http_client: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_constructor_cyclopsclient_connect_with_access_token_provider(
        configuration: u::RustBuffer,
        token_provider: u64,
        http_client: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_create_claim(
        ptr: u64,
        request: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_delete_claim(
        ptr: u64,
        claim: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_get_claim(
        ptr: u64,
        claim: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_list_claims(
        ptr: u64,
        namespace: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_wait_claim(
        ptr: u64,
        claim: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_create_pool(
        ptr: u64,
        request: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_delete_pool(
        ptr: u64,
        pool: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_get_pool(
        ptr: u64,
        pool: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_list_pools(
        ptr: u64,
        namespace: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_update_pool(
        ptr: u64,
        pool: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_method_cyclopsclient_service_request(
        ptr: u64,
        sandbox: u::RustBuffer,
        service: u::RustBuffer,
        path: u::RustBuffer,
        request: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_clone_accesstokenprovider(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_free_accesstokenprovider(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_fn_init_callback_vtable_accesstokenprovider(
        vtable: std::ptr::NonNull<
            v_table_callback_interface_access_token_provider::VTableRs,
        >,
    );
    fn uniffi_cyclops_sdk_fn_method_accesstokenprovider_get_access_token(
        ptr: u64,
        force_refresh: i8,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_clone_httpclient(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_free_httpclient(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_fn_init_callback_vtable_httpclient(
        vtable: std::ptr::NonNull<v_table_callback_interface_http_client::VTableRs>,
    );
    fn uniffi_cyclops_sdk_fn_method_httpclient_execute(
        ptr: u64,
        request: u::RustBuffer,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_clone_cyclopscredentials(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_fn_free_cyclopscredentials(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_fn_constructor_cyclopscredentials_new(
        client_id: u::RustBuffer,
        client_secret: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn ffi_cyclops_sdk_rust_future_poll_u8(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_u8(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_u8(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_u8(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u8;
    fn ffi_cyclops_sdk_rust_future_poll_i8(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_i8(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_i8(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_i8(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> i8;
    fn ffi_cyclops_sdk_rust_future_poll_u16(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_u16(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_u16(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_u16(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u16;
    fn ffi_cyclops_sdk_rust_future_poll_i16(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_i16(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_i16(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_i16(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> i16;
    fn ffi_cyclops_sdk_rust_future_poll_u32(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_u32(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_u32(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_u32(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u32;
    fn ffi_cyclops_sdk_rust_future_poll_i32(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_i32(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_i32(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_i32(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> i32;
    fn ffi_cyclops_sdk_rust_future_poll_u64(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_u64(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_u64(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_u64(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn ffi_cyclops_sdk_rust_future_poll_i64(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_i64(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_i64(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_i64(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> i64;
    fn ffi_cyclops_sdk_rust_future_poll_f32(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_f32(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_f32(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_f32(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> f32;
    fn ffi_cyclops_sdk_rust_future_poll_f64(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_f64(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_f64(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_f64(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> f64;
    fn ffi_cyclops_sdk_rust_future_poll_rust_buffer(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_rust_buffer(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_rust_buffer(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_rust_buffer(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn ffi_cyclops_sdk_rust_future_poll_void(
        handle: u64,
        callback: rust_future_continuation_callback::FnSig,
        callback_data: u64,
    );
    fn ffi_cyclops_sdk_rust_future_cancel_void(handle: u64);
    fn ffi_cyclops_sdk_rust_future_free_void(handle: u64);
    fn ffi_cyclops_sdk_rust_future_complete_void(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_claim() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_claim() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_claim() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_claims() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_wait_claim() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_pool() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_pool() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_pool() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_pools() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_update_pool() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_cyclopsclient_service_request() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_accesstokenprovider_get_access_token() -> u16;
    fn uniffi_cyclops_sdk_checksum_method_httpclient_execute() -> u16;
    fn uniffi_cyclops_sdk_checksum_constructor_cyclopsclient_connect() -> u16;
    fn uniffi_cyclops_sdk_checksum_constructor_cyclopsclient_connect_with_access_token_provider() -> u16;
    fn uniffi_cyclops_sdk_checksum_constructor_cyclopscredentials_new() -> u16;
    fn ffi_cyclops_sdk_uniffi_contract_version() -> u32;
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_clone_cyclopsclient(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_fn_clone_cyclopsclient(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_free_cyclopsclient(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_fn_free_cyclopsclient(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_constructor_cyclopsclient_connect(
    configuration: js::ForeignBytes,
    http_client: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_fn_constructor_cyclopsclient_connect(
            u::RustBuffer::into_rust(configuration),
            u64::into_rust(http_client),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_constructor_cyclopsclient_connect_with_access_token_provider(
    configuration: js::ForeignBytes,
    token_provider: js::Handle,
    http_client: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_fn_constructor_cyclopsclient_connect_with_access_token_provider(
            u::RustBuffer::into_rust(configuration),
            u64::into_rust(token_provider),
            u64::into_rust(http_client),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_create_claim(
    ptr: js::Handle,
    request: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_create_claim(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(request),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_delete_claim(
    ptr: js::Handle,
    claim: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_delete_claim(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(claim),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_get_claim(
    ptr: js::Handle,
    claim: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_get_claim(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(claim),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_list_claims(
    ptr: js::Handle,
    namespace: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_list_claims(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(namespace),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_wait_claim(
    ptr: js::Handle,
    claim: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_wait_claim(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(claim),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_create_pool(
    ptr: js::Handle,
    request: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_create_pool(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(request),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_delete_pool(
    ptr: js::Handle,
    pool: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_delete_pool(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(pool),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_get_pool(
    ptr: js::Handle,
    pool: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_get_pool(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(pool),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_list_pools(
    ptr: js::Handle,
    namespace: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_list_pools(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(namespace),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_update_pool(
    ptr: js::Handle,
    pool: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_update_pool(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(pool),
        )
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_service_request(
    ptr: js::Handle,
    sandbox: js::ForeignBytes,
    service: js::ForeignBytes,
    path: js::ForeignBytes,
    request: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_cyclopsclient_service_request(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(sandbox),
            u::RustBuffer::into_rust(service),
            u::RustBuffer::into_rust(path),
            u::RustBuffer::into_rust(request),
        )
        .into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_clone_accesstokenprovider(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_fn_clone_accesstokenprovider(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_free_accesstokenprovider(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_fn_free_accesstokenprovider(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_init_callback_vtable_accesstokenprovider(
    vtable: v_table_callback_interface_access_token_provider::VTableJs,
) {
    uniffi_cyclops_sdk_fn_init_callback_vtable_accesstokenprovider(
        std::ptr::NonNull::<
            v_table_callback_interface_access_token_provider::VTableRs,
        >::into_rust(vtable),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_accesstokenprovider_get_access_token(
    ptr: js::Handle,
    force_refresh: js::Int8,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_accesstokenprovider_get_access_token(
            u64::into_rust(ptr),
            i8::into_rust(force_refresh),
        )
        .into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_clone_httpclient(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_fn_clone_httpclient(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_free_httpclient(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_fn_free_httpclient(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_init_callback_vtable_httpclient(
    vtable: v_table_callback_interface_http_client::VTableJs,
) {
    uniffi_cyclops_sdk_fn_init_callback_vtable_httpclient(
        std::ptr::NonNull::<
            v_table_callback_interface_http_client::VTableRs,
        >::into_rust(vtable),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_fn_method_httpclient_execute(
    ptr: js::Handle,
    request: js::ForeignBytes,
) -> js::Handle {
    uniffi_cyclops_sdk_fn_method_httpclient_execute(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(request),
        )
        .into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_clone_cyclopscredentials(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_fn_clone_cyclopscredentials(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_free_cyclopscredentials(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_fn_free_cyclopscredentials(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_fn_constructor_cyclopscredentials_new(
    client_id: js::ForeignBytes,
    client_secret: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_fn_constructor_cyclopscredentials_new(
            u::RustBuffer::into_rust(client_id),
            u::RustBuffer::into_rust(client_secret),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_u8(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_u8(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_u8(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_u8(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_u8(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_u8(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_u8(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::UInt8 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_u8(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_i8(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_i8(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_i8(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_i8(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_i8(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_i8(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_i8(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Int8 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_i8(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_u16(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_u16(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_u16(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_u16(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_u16(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_u16(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_u16(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::UInt16 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_u16(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_i16(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_i16(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_i16(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_i16(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_i16(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_i16(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_i16(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Int16 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_i16(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_u32(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_u32(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_u32(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_u32(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_u32(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_u32(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_u32(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::UInt32 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_u32(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_i32(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_i32(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_i32(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_i32(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_i32(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_i32(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_i32(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Int32 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_i32(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_u64(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_u64(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_u64(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_u64(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_u64(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_u64(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_u64(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::UInt64 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_u64(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_i64(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_i64(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_i64(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_i64(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_i64(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_i64(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_i64(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Int64 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_i64(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_f32(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_f32(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_f32(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_f32(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_f32(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_f32(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_f32(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Float32 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_f32(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_f64(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_f64(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_f64(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_f64(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_f64(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_f64(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_f64(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Float64 {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_f64(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_rust_buffer(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_rust_buffer(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_rust_buffer(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_rust_buffer(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_rust_buffer(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_rust_buffer(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_rust_buffer(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        ffi_cyclops_sdk_rust_future_complete_rust_buffer(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_poll_void(
    handle: js::Handle,
    callback: rust_future_continuation_callback::JsCallbackFnRustFutureContinuationCallback,
    callback_data: js::Handle,
) {
    ffi_cyclops_sdk_rust_future_poll_void(
        u64::into_rust(handle),
        rust_future_continuation_callback::FnSig::into_rust(callback),
        u64::into_rust(callback_data),
    );
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_cancel_void(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_cancel_void(u64::into_rust(handle));
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_rust_future_free_void(handle: js::Handle) {
    ffi_cyclops_sdk_rust_future_free_void(u64::into_rust(handle));
}
#[wasm_bindgen]
pub fn ubrn_ffi_cyclops_sdk_rust_future_complete_void(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        ffi_cyclops_sdk_rust_future_complete_void(u64::into_rust(handle), &mut u_status_)
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_claim() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_claim().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_claim() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_claim().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_claim() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_claim().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_claims() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_claims().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_wait_claim() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_wait_claim().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_pool() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_pool().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_pool() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_pool().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_pool() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_pool().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_pools() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_pools().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_update_pool() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_update_pool().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_cyclopsclient_service_request() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_cyclopsclient_service_request().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_accesstokenprovider_get_access_token() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_accesstokenprovider_get_access_token().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_method_httpclient_execute() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_method_httpclient_execute().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_constructor_cyclopsclient_connect() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_constructor_cyclopsclient_connect().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_constructor_cyclopsclient_connect_with_access_token_provider() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_constructor_cyclopsclient_connect_with_access_token_provider()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_checksum_constructor_cyclopscredentials_new() -> js::UInt16 {
    uniffi_cyclops_sdk_checksum_constructor_cyclopscredentials_new().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_uniffi_contract_version() -> js::UInt32 {
    ffi_cyclops_sdk_uniffi_contract_version().into_js()
}
mod foreign_future_result_f64 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::Float64;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: f64,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: f64::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_i8 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::Int8;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: i8,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: i8::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_u8 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::UInt8;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: u8,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: u8::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_void {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_f32 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::Float32;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: f32,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: f32::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_dropped_callback_struct {
    use super::*;
    use super::foreign_future_dropped_callback as method_free;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn handle(this: &VTableJs) -> js::UInt64;
        #[wasm_bindgen(method, getter)]
        fn free(
            this: &VTableJs,
        ) -> method_free::JsCallbackFnForeignFutureDroppedCallback;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        handle: u64,
        free: method_free::FnSig,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                handle: u64::into_rust(v_.handle()),
                free: method_free::FnSig::into_rust(v_.free()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_u64 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::UInt64;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: u64,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: u64::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_u32 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::UInt32;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: u32,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: u32::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_u16 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::UInt16;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: u16,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: u16::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
#[allow(non_snake_case)]
mod v_table_callback_interface_http_client__free {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen]
        pub type JsCallbackFnVTableCallbackInterfaceHttpClientFree;
        #[wasm_bindgen(method)]
        pub fn call(
            this_: &JsCallbackFnVTableCallbackInterfaceHttpClientFree,
            ctx_: &JsCallbackFnVTableCallbackInterfaceHttpClientFree,
            handle: js::UInt64,
        );
    }
    thread_local! {
        static CALLBACK : js::ForeignCell <
        JsCallbackFnVTableCallbackInterfaceHttpClientFree > = js::ForeignCell::new();
    }
    impl IntoRust<JsCallbackFnVTableCallbackInterfaceHttpClientFree> for FnSig {
        fn into_rust(
            callback: JsCallbackFnVTableCallbackInterfaceHttpClientFree,
        ) -> Self {
            CALLBACK.with(|cell| cell.set(callback));
            implementation
        }
    }
    pub(super) type FnSig = extern "C" fn(handle: u64);
    extern "C" fn implementation(handle: u64) {
        CALLBACK
            .with(|cell_| {
                cell_.with_value(|callback_| callback_.call(callback_, handle.into_js()))
            });
    }
}
#[allow(non_snake_case)]
mod v_table_callback_interface_http_client__clone {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen]
        pub type JsCallbackFnVTableCallbackInterfaceHttpClientClone;
        #[wasm_bindgen(method)]
        pub fn call(
            this_: &JsCallbackFnVTableCallbackInterfaceHttpClientClone,
            ctx_: &JsCallbackFnVTableCallbackInterfaceHttpClientClone,
            handle: js::UInt64,
        ) -> js::UInt64;
    }
    thread_local! {
        static CALLBACK : js::ForeignCell <
        JsCallbackFnVTableCallbackInterfaceHttpClientClone > = js::ForeignCell::new();
    }
    impl IntoRust<JsCallbackFnVTableCallbackInterfaceHttpClientClone> for FnSig {
        fn into_rust(
            callback: JsCallbackFnVTableCallbackInterfaceHttpClientClone,
        ) -> Self {
            CALLBACK.with(|cell| cell.set(callback));
            implementation
        }
    }
    pub(super) type FnSig = extern "C" fn(handle: u64) -> u64;
    extern "C" fn implementation(handle: u64) -> u64 {
        let uniffi_result_ = CALLBACK
            .with(|cell_| {
                cell_.with_value(|callback_| callback_.call(callback_, handle.into_js()))
            });
        u64::into_rust(uniffi_result_)
    }
}
mod v_table_callback_interface_http_client {
    use super::*;
    use super::v_table_callback_interface_http_client__free as method_uniffi_free;
    use super::v_table_callback_interface_http_client__clone as method_uniffi_clone;
    use super::callback_interface_http_client_method0 as method_execute;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn uniffi_free(
            this: &VTableJs,
        ) -> method_uniffi_free::JsCallbackFnVTableCallbackInterfaceHttpClientFree;
        #[wasm_bindgen(method, getter)]
        fn uniffi_clone(
            this: &VTableJs,
        ) -> method_uniffi_clone::JsCallbackFnVTableCallbackInterfaceHttpClientClone;
        #[wasm_bindgen(method, getter)]
        fn execute(
            this: &VTableJs,
        ) -> method_execute::JsCallbackFnCallbackInterfaceHttpClientMethod0;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        uniffi_free: method_uniffi_free::FnSig,
        uniffi_clone: method_uniffi_clone::FnSig,
        execute: method_execute::FnSig,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                uniffi_free: method_uniffi_free::FnSig::into_rust(v_.uniffi_free()),
                uniffi_clone: method_uniffi_clone::FnSig::into_rust(v_.uniffi_clone()),
                execute: method_execute::FnSig::into_rust(v_.execute()),
            }
        }
    }
}
mod foreign_future_result_rust_buffer {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::ForeignBytes;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: u::RustBuffer,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: u::RustBuffer::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_i64 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::Int64;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: i64,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: i64::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_result_i32 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::Int32;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: i32,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: i32::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
#[allow(non_snake_case)]
mod v_table_callback_interface_access_token_provider__free {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen]
        pub type JsCallbackFnVTableCallbackInterfaceAccessTokenProviderFree;
        #[wasm_bindgen(method)]
        pub fn call(
            this_: &JsCallbackFnVTableCallbackInterfaceAccessTokenProviderFree,
            ctx_: &JsCallbackFnVTableCallbackInterfaceAccessTokenProviderFree,
            handle: js::UInt64,
        );
    }
    thread_local! {
        static CALLBACK : js::ForeignCell <
        JsCallbackFnVTableCallbackInterfaceAccessTokenProviderFree > =
        js::ForeignCell::new();
    }
    impl IntoRust<JsCallbackFnVTableCallbackInterfaceAccessTokenProviderFree> for FnSig {
        fn into_rust(
            callback: JsCallbackFnVTableCallbackInterfaceAccessTokenProviderFree,
        ) -> Self {
            CALLBACK.with(|cell| cell.set(callback));
            implementation
        }
    }
    pub(super) type FnSig = extern "C" fn(handle: u64);
    extern "C" fn implementation(handle: u64) {
        CALLBACK
            .with(|cell_| {
                cell_.with_value(|callback_| callback_.call(callback_, handle.into_js()))
            });
    }
}
#[allow(non_snake_case)]
mod v_table_callback_interface_access_token_provider__clone {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen]
        pub type JsCallbackFnVTableCallbackInterfaceAccessTokenProviderClone;
        #[wasm_bindgen(method)]
        pub fn call(
            this_: &JsCallbackFnVTableCallbackInterfaceAccessTokenProviderClone,
            ctx_: &JsCallbackFnVTableCallbackInterfaceAccessTokenProviderClone,
            handle: js::UInt64,
        ) -> js::UInt64;
    }
    thread_local! {
        static CALLBACK : js::ForeignCell <
        JsCallbackFnVTableCallbackInterfaceAccessTokenProviderClone > =
        js::ForeignCell::new();
    }
    impl IntoRust<JsCallbackFnVTableCallbackInterfaceAccessTokenProviderClone>
    for FnSig {
        fn into_rust(
            callback: JsCallbackFnVTableCallbackInterfaceAccessTokenProviderClone,
        ) -> Self {
            CALLBACK.with(|cell| cell.set(callback));
            implementation
        }
    }
    pub(super) type FnSig = extern "C" fn(handle: u64) -> u64;
    extern "C" fn implementation(handle: u64) -> u64 {
        let uniffi_result_ = CALLBACK
            .with(|cell_| {
                cell_.with_value(|callback_| callback_.call(callback_, handle.into_js()))
            });
        u64::into_rust(uniffi_result_)
    }
}
mod v_table_callback_interface_access_token_provider {
    use super::*;
    use super::v_table_callback_interface_access_token_provider__free as method_uniffi_free;
    use super::v_table_callback_interface_access_token_provider__clone as method_uniffi_clone;
    use super::callback_interface_access_token_provider_method0 as method_get_access_token;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn uniffi_free(
            this: &VTableJs,
        ) -> method_uniffi_free::JsCallbackFnVTableCallbackInterfaceAccessTokenProviderFree;
        #[wasm_bindgen(method, getter)]
        fn uniffi_clone(
            this: &VTableJs,
        ) -> method_uniffi_clone::JsCallbackFnVTableCallbackInterfaceAccessTokenProviderClone;
        #[wasm_bindgen(method, getter)]
        fn get_access_token(
            this: &VTableJs,
        ) -> method_get_access_token::JsCallbackFnCallbackInterfaceAccessTokenProviderMethod0;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        uniffi_free: method_uniffi_free::FnSig,
        uniffi_clone: method_uniffi_clone::FnSig,
        get_access_token: method_get_access_token::FnSig,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                uniffi_free: method_uniffi_free::FnSig::into_rust(v_.uniffi_free()),
                uniffi_clone: method_uniffi_clone::FnSig::into_rust(v_.uniffi_clone()),
                get_access_token: method_get_access_token::FnSig::into_rust(
                    v_.get_access_token(),
                ),
            }
        }
    }
}
mod foreign_future_result_i16 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        pub type VTableJs;
        #[wasm_bindgen(method, getter)]
        fn return_value(this: &VTableJs) -> js::Int16;
        #[wasm_bindgen(method, getter)]
        fn call_status(this: &VTableJs) -> js::RustCallStatus;
    }
    #[repr(C)]
    pub(super) struct VTableRs {
        return_value: i16,
        call_status: u::RustCallStatus,
    }
    impl IntoRust<VTableJs> for VTableRs {
        fn into_rust(v_: VTableJs) -> Self {
            Self {
                return_value: i16::into_rust(v_.return_value()),
                call_status: u::RustCallStatus::into_rust(v_.call_status()),
            }
        }
    }
    impl VTableJs {
        #[allow(unused)]
        pub(super) fn copy_into_return(self, rust: &mut VTableRs) {
            *rust = <VTableRs>::into_rust(self);
        }
    }
}
mod foreign_future_dropped_callback {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen]
        pub type JsCallbackFnForeignFutureDroppedCallback;
        #[wasm_bindgen(method)]
        pub fn call(
            this_: &JsCallbackFnForeignFutureDroppedCallback,
            ctx_: &JsCallbackFnForeignFutureDroppedCallback,
            handle: js::UInt64,
        );
    }
    thread_local! {
        static CALLBACK : js::ForeignCell < JsCallbackFnForeignFutureDroppedCallback > =
        js::ForeignCell::new();
    }
    impl IntoRust<JsCallbackFnForeignFutureDroppedCallback> for FnSig {
        fn into_rust(callback: JsCallbackFnForeignFutureDroppedCallback) -> Self {
            CALLBACK.with(|cell| cell.set(callback));
            implementation
        }
    }
    pub(super) type FnSig = extern "C" fn(handle: u64);
    extern "C" fn implementation(handle: u64) {
        CALLBACK
            .with(|cell_| {
                cell_.with_value(|callback_| callback_.call(callback_, handle.into_js()))
            });
    }
}
mod foreign_future_complete_i16 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteI16)]
    pub struct JsCallbackFnForeignFutureCompleteI16 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteI16 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteI16)]
    impl JsCallbackFnForeignFutureCompleteI16 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_i16::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_i16::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_i16::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteI16> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteI16 {
            JsCallbackFnForeignFutureCompleteI16::new(self)
        }
    }
}
mod foreign_future_complete_u8 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteU8)]
    pub struct JsCallbackFnForeignFutureCompleteU8 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteU8 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteU8)]
    impl JsCallbackFnForeignFutureCompleteU8 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_u8::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_u8::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_u8::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteU8> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteU8 {
            JsCallbackFnForeignFutureCompleteU8::new(self)
        }
    }
}
mod foreign_future_complete_i8 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteI8)]
    pub struct JsCallbackFnForeignFutureCompleteI8 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteI8 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteI8)]
    impl JsCallbackFnForeignFutureCompleteI8 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_i8::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_i8::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_i8::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteI8> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteI8 {
            JsCallbackFnForeignFutureCompleteI8::new(self)
        }
    }
}
mod foreign_future_complete_f64 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteF64)]
    pub struct JsCallbackFnForeignFutureCompleteF64 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteF64 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteF64)]
    impl JsCallbackFnForeignFutureCompleteF64 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_f64::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_f64::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_f64::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteF64> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteF64 {
            JsCallbackFnForeignFutureCompleteF64::new(self)
        }
    }
}
mod foreign_future_complete_u32 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteU32)]
    pub struct JsCallbackFnForeignFutureCompleteU32 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteU32 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteU32)]
    impl JsCallbackFnForeignFutureCompleteU32 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_u32::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_u32::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_u32::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteU32> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteU32 {
            JsCallbackFnForeignFutureCompleteU32::new(self)
        }
    }
}
mod foreign_future_complete_u16 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteU16)]
    pub struct JsCallbackFnForeignFutureCompleteU16 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteU16 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteU16)]
    impl JsCallbackFnForeignFutureCompleteU16 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_u16::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_u16::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_u16::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteU16> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteU16 {
            JsCallbackFnForeignFutureCompleteU16::new(self)
        }
    }
}
mod foreign_future_complete_i64 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteI64)]
    pub struct JsCallbackFnForeignFutureCompleteI64 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteI64 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteI64)]
    impl JsCallbackFnForeignFutureCompleteI64 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_i64::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_i64::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_i64::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteI64> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteI64 {
            JsCallbackFnForeignFutureCompleteI64::new(self)
        }
    }
}
mod foreign_future_complete_u64 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteU64)]
    pub struct JsCallbackFnForeignFutureCompleteU64 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteU64 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteU64)]
    impl JsCallbackFnForeignFutureCompleteU64 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_u64::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_u64::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_u64::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteU64> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteU64 {
            JsCallbackFnForeignFutureCompleteU64::new(self)
        }
    }
}
mod foreign_future_complete_i32 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteI32)]
    pub struct JsCallbackFnForeignFutureCompleteI32 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteI32 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteI32)]
    impl JsCallbackFnForeignFutureCompleteI32 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_i32::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_i32::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_i32::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteI32> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteI32 {
            JsCallbackFnForeignFutureCompleteI32::new(self)
        }
    }
}
mod foreign_future_complete_f32 {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteF32)]
    pub struct JsCallbackFnForeignFutureCompleteF32 {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteF32 {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteF32)]
    impl JsCallbackFnForeignFutureCompleteF32 {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_f32::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_f32::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_f32::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteF32> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteF32 {
            JsCallbackFnForeignFutureCompleteF32::new(self)
        }
    }
}
mod foreign_future_complete_void {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteVoid)]
    pub struct JsCallbackFnForeignFutureCompleteVoid {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteVoid {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteVoid)]
    impl JsCallbackFnForeignFutureCompleteVoid {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_void::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_void::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_void::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteVoid> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteVoid {
            JsCallbackFnForeignFutureCompleteVoid::new(self)
        }
    }
}
mod foreign_future_complete_rust_buffer {
    use super::*;
    #[wasm_bindgen(js_name = ForeignFutureCompleteRustBuffer)]
    pub struct JsCallbackFnForeignFutureCompleteRustBuffer {
        callback: FnSig,
    }
    impl JsCallbackFnForeignFutureCompleteRustBuffer {
        fn new(callback: FnSig) -> Self {
            Self { callback }
        }
    }
    #[wasm_bindgen(js_class = ForeignFutureCompleteRustBuffer)]
    impl JsCallbackFnForeignFutureCompleteRustBuffer {
        #[wasm_bindgen]
        pub fn call(
            &self,
            _ctx: &Self,
            callback_data: js::UInt64,
            result: foreign_future_result_rust_buffer::VTableJs,
        ) {
            (self
                .callback)(
                u64::into_rust(callback_data),
                foreign_future_result_rust_buffer::VTableRs::into_rust(result),
            )
        }
    }
    pub(super) type FnSig = extern "C" fn(
        callback_data: u64,
        result: foreign_future_result_rust_buffer::VTableRs,
    );
    impl IntoJs<JsCallbackFnForeignFutureCompleteRustBuffer> for FnSig {
        fn into_js(self) -> JsCallbackFnForeignFutureCompleteRustBuffer {
            JsCallbackFnForeignFutureCompleteRustBuffer::new(self)
        }
    }
}
mod callback_interface_access_token_provider_method0 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen]
        pub type JsCallbackFnCallbackInterfaceAccessTokenProviderMethod0;
        #[wasm_bindgen(method)]
        pub fn call(
            this_: &JsCallbackFnCallbackInterfaceAccessTokenProviderMethod0,
            ctx_: &JsCallbackFnCallbackInterfaceAccessTokenProviderMethod0,
            uniffi_handle: js::UInt64,
            force_refresh: js::Int8,
            uniffi_future_callback: foreign_future_complete_rust_buffer::JsCallbackFnForeignFutureCompleteRustBuffer,
            uniffi_callback_data: js::UInt64,
        ) -> foreign_future_dropped_callback_struct::VTableJs;
    }
    thread_local! {
        static CALLBACK : js::ForeignCell <
        JsCallbackFnCallbackInterfaceAccessTokenProviderMethod0 > =
        js::ForeignCell::new();
    }
    impl IntoRust<JsCallbackFnCallbackInterfaceAccessTokenProviderMethod0> for FnSig {
        fn into_rust(
            callback: JsCallbackFnCallbackInterfaceAccessTokenProviderMethod0,
        ) -> Self {
            CALLBACK.with(|cell| cell.set(callback));
            implementation
        }
    }
    pub(super) type FnSig = extern "C" fn(
        uniffi_handle: u64,
        force_refresh: i8,
        uniffi_future_callback: foreign_future_complete_rust_buffer::FnSig,
        uniffi_callback_data: u64,
        rs_return_: &mut foreign_future_dropped_callback_struct::VTableRs,
    );
    extern "C" fn implementation(
        uniffi_handle: u64,
        force_refresh: i8,
        uniffi_future_callback: foreign_future_complete_rust_buffer::FnSig,
        uniffi_callback_data: u64,
        rs_return_: &mut foreign_future_dropped_callback_struct::VTableRs,
    ) {
        let uniffi_result_ = CALLBACK
            .with(|cell_| {
                cell_
                    .with_value(|callback_| {
                        callback_
                            .call(
                                callback_,
                                uniffi_handle.into_js(),
                                force_refresh.into_js(),
                                uniffi_future_callback.into_js(),
                                uniffi_callback_data.into_js(),
                            )
                    })
            });
        uniffi_result_.copy_into_return(rs_return_);
    }
}
mod rust_future_continuation_callback {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen]
        pub type JsCallbackFnRustFutureContinuationCallback;
        #[wasm_bindgen(method)]
        pub fn call(
            this_: &JsCallbackFnRustFutureContinuationCallback,
            ctx_: &JsCallbackFnRustFutureContinuationCallback,
            data: js::UInt64,
            poll_result: js::Int8,
        );
    }
    thread_local! {
        static CALLBACK : js::ForeignCell < JsCallbackFnRustFutureContinuationCallback >
        = js::ForeignCell::new();
    }
    impl IntoRust<JsCallbackFnRustFutureContinuationCallback> for FnSig {
        fn into_rust(callback: JsCallbackFnRustFutureContinuationCallback) -> Self {
            CALLBACK.with(|cell| cell.set(callback));
            implementation
        }
    }
    pub(super) type FnSig = extern "C" fn(data: u64, poll_result: i8);
    extern "C" fn implementation(data: u64, poll_result: i8) {
        CALLBACK
            .with(|cell_| {
                cell_
                    .with_value(|callback_| {
                        callback_.call(callback_, data.into_js(), poll_result.into_js())
                    })
            });
    }
}
mod callback_interface_http_client_method0 {
    use super::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen]
        pub type JsCallbackFnCallbackInterfaceHttpClientMethod0;
        #[wasm_bindgen(method)]
        pub fn call(
            this_: &JsCallbackFnCallbackInterfaceHttpClientMethod0,
            ctx_: &JsCallbackFnCallbackInterfaceHttpClientMethod0,
            uniffi_handle: js::UInt64,
            request: js::ForeignBytes,
            uniffi_future_callback: foreign_future_complete_rust_buffer::JsCallbackFnForeignFutureCompleteRustBuffer,
            uniffi_callback_data: js::UInt64,
        ) -> foreign_future_dropped_callback_struct::VTableJs;
    }
    thread_local! {
        static CALLBACK : js::ForeignCell <
        JsCallbackFnCallbackInterfaceHttpClientMethod0 > = js::ForeignCell::new();
    }
    impl IntoRust<JsCallbackFnCallbackInterfaceHttpClientMethod0> for FnSig {
        fn into_rust(callback: JsCallbackFnCallbackInterfaceHttpClientMethod0) -> Self {
            CALLBACK.with(|cell| cell.set(callback));
            implementation
        }
    }
    pub(super) type FnSig = extern "C" fn(
        uniffi_handle: u64,
        request: u::RustBuffer,
        uniffi_future_callback: foreign_future_complete_rust_buffer::FnSig,
        uniffi_callback_data: u64,
        rs_return_: &mut foreign_future_dropped_callback_struct::VTableRs,
    );
    extern "C" fn implementation(
        uniffi_handle: u64,
        request: u::RustBuffer,
        uniffi_future_callback: foreign_future_complete_rust_buffer::FnSig,
        uniffi_callback_data: u64,
        rs_return_: &mut foreign_future_dropped_callback_struct::VTableRs,
    ) {
        let uniffi_result_ = CALLBACK
            .with(|cell_| {
                cell_
                    .with_value(|callback_| {
                        callback_
                            .call(
                                callback_,
                                uniffi_handle.into_js(),
                                request.into_js(),
                                uniffi_future_callback.into_js(),
                                uniffi_callback_data.into_js(),
                            )
                    })
            });
        uniffi_result_.copy_into_return(rs_return_);
    }
}
