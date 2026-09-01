#[allow(unused_imports)]
use uniffi_runtime_javascript::{self as js, uniffi as u, IntoJs, IntoRust};
use wasm_bindgen::prelude::wasm_bindgen;
extern "C" {
    fn uniffi_cyclops_sdk_schema_fn_clone_imagerefbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_free_imagerefbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_schema_fn_constructor_imagerefbuilder_new(
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_imagerefbuilder_build(
        ptr: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn uniffi_cyclops_sdk_schema_fn_method_imagerefbuilder_name(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_clone_sandboxservicebuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_free_sandboxservicebuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_schema_fn_constructor_sandboxservicebuilder_new(
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_build(
        ptr: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_name(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_protocol(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_target_port(
        ptr: u64,
        value: u16,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_clone_sandboxtemplaterefbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_free_sandboxtemplaterefbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_schema_fn_constructor_sandboxtemplaterefbuilder_new(
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_sandboxtemplaterefbuilder_build(
        ptr: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn uniffi_cyclops_sdk_schema_fn_method_sandboxtemplaterefbuilder_name(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_clone_vmtemplatebuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_free_vmtemplatebuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_schema_fn_constructor_vmtemplatebuilder_new(
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_build(
        ptr: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_command(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_container_disk_image(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_cpu_cores(
        ptr: u64,
        value: u32,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_firmware(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_pull_policy(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_pull_secret(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_ref(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_memory(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_nested_virtualization(
        ptr: u64,
        value: i8,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_node_selector(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_oidc(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_probes(
        ptr: u64,
        value: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_runtime(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_runtime_class_name(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_services(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_tolerations(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
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
    fn uniffi_cyclops_sdk_schema_fn_clone_osgymsandboxtemplatespecbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_free_osgymsandboxtemplatespecbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_schema_fn_constructor_osgymsandboxtemplatespecbuilder_new(
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_osgymsandboxtemplatespecbuilder_build(
        ptr: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn uniffi_cyclops_sdk_schema_fn_method_osgymsandboxtemplatespecbuilder_vm_template(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_clone_osgymsandboxwarmpoolspecbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_free_osgymsandboxwarmpoolspecbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_schema_fn_constructor_osgymsandboxwarmpoolspecbuilder_new(
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_autoscaling(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_build(
        ptr: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_replicas(
        ptr: u64,
        value: u32,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_sandbox_template_ref(
        ptr: u64,
        value: u::RustBuffer,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_clone_warmpoolautoscalingbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_free_warmpoolautoscalingbuilder(
        handle: u64,
        status_: &mut u::RustCallStatus,
    );
    fn uniffi_cyclops_sdk_schema_fn_constructor_warmpoolautoscalingbuilder_new(
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_build(
        ptr: u64,
        status_: &mut u::RustCallStatus,
    ) -> u::RustBuffer;
    fn uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_initial_pool_size(
        ptr: u64,
        value: u32,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_max_pool_size(
        ptr: u64,
        value: u32,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_min_pool_size(
        ptr: u64,
        value: u32,
        status_: &mut u::RustCallStatus,
    ) -> u64;
    fn uniffi_cyclops_sdk_schema_checksum_method_imagerefbuilder_build() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_imagerefbuilder_name() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_build() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_name() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_protocol() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_target_port() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_sandboxtemplaterefbuilder_build() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_sandboxtemplaterefbuilder_name() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_build() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_command() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_container_disk_image() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_cpu_cores() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_firmware() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_pull_policy() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_pull_secret() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_ref() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_memory() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_nested_virtualization() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_node_selector() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_oidc() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_probes() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_runtime() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_runtime_class_name() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_services() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_tolerations() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_preservedjson_to_json() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxtemplatespecbuilder_build() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxtemplatespecbuilder_vm_template() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_autoscaling() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_build() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_replicas() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_sandbox_template_ref() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_build() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_initial_pool_size() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_max_pool_size() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_min_pool_size() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_imagerefbuilder_new() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_sandboxservicebuilder_new() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_sandboxtemplaterefbuilder_new() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_vmtemplatebuilder_new() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_preservedjson_from_json() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_osgymsandboxtemplatespecbuilder_new() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_osgymsandboxwarmpoolspecbuilder_new() -> u16;
    fn uniffi_cyclops_sdk_schema_checksum_constructor_warmpoolautoscalingbuilder_new() -> u16;
    fn ffi_cyclops_sdk_schema_uniffi_contract_version() -> u32;
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_clone_imagerefbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_clone_imagerefbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_free_imagerefbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_schema_fn_free_imagerefbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_constructor_imagerefbuilder_new(
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_constructor_imagerefbuilder_new(&mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_imagerefbuilder_build(
    ptr: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_imagerefbuilder_build(
            u64::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_imagerefbuilder_name(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_imagerefbuilder_name(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_clone_sandboxservicebuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_clone_sandboxservicebuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_free_sandboxservicebuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_schema_fn_free_sandboxservicebuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_constructor_sandboxservicebuilder_new(
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_constructor_sandboxservicebuilder_new(
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_build(
    ptr: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_build(
            u64::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_name(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_name(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_protocol(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_protocol(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_target_port(
    ptr: js::Handle,
    value: js::UInt16,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_sandboxservicebuilder_target_port(
            u64::into_rust(ptr),
            u16::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_clone_sandboxtemplaterefbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_clone_sandboxtemplaterefbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_free_sandboxtemplaterefbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_schema_fn_free_sandboxtemplaterefbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_constructor_sandboxtemplaterefbuilder_new(
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_constructor_sandboxtemplaterefbuilder_new(
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_sandboxtemplaterefbuilder_build(
    ptr: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_sandboxtemplaterefbuilder_build(
            u64::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_sandboxtemplaterefbuilder_name(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_sandboxtemplaterefbuilder_name(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_clone_vmtemplatebuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_clone_vmtemplatebuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_free_vmtemplatebuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_schema_fn_free_vmtemplatebuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_constructor_vmtemplatebuilder_new(
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_constructor_vmtemplatebuilder_new(&mut u_status_)
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_build(
    ptr: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_build(
            u64::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_command(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_command(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_container_disk_image(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_container_disk_image(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_cpu_cores(
    ptr: js::Handle,
    value: js::UInt32,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_cpu_cores(
            u64::into_rust(ptr),
            u32::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_firmware(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_firmware(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_pull_policy(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_pull_policy(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_pull_secret(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_pull_secret(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_ref(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_image_ref(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_memory(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_memory(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_nested_virtualization(
    ptr: js::Handle,
    value: js::Int8,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_nested_virtualization(
            u64::into_rust(ptr),
            i8::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_node_selector(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_node_selector(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_oidc(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_oidc(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_probes(
    ptr: js::Handle,
    value: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_probes(
            u64::into_rust(ptr),
            u64::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_runtime(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_runtime(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_runtime_class_name(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_runtime_class_name(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_services(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_services(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_tolerations(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_vmtemplatebuilder_tolerations(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
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
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_clone_osgymsandboxtemplatespecbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_clone_osgymsandboxtemplatespecbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_free_osgymsandboxtemplatespecbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_schema_fn_free_osgymsandboxtemplatespecbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_constructor_osgymsandboxtemplatespecbuilder_new(
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_constructor_osgymsandboxtemplatespecbuilder_new(
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_osgymsandboxtemplatespecbuilder_build(
    ptr: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_osgymsandboxtemplatespecbuilder_build(
            u64::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_osgymsandboxtemplatespecbuilder_vm_template(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_osgymsandboxtemplatespecbuilder_vm_template(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_clone_osgymsandboxwarmpoolspecbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_clone_osgymsandboxwarmpoolspecbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_free_osgymsandboxwarmpoolspecbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_schema_fn_free_osgymsandboxwarmpoolspecbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_constructor_osgymsandboxwarmpoolspecbuilder_new(
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_constructor_osgymsandboxwarmpoolspecbuilder_new(
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_autoscaling(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_autoscaling(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_build(
    ptr: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_build(
            u64::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_replicas(
    ptr: js::Handle,
    value: js::UInt32,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_replicas(
            u64::into_rust(ptr),
            u32::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_sandbox_template_ref(
    ptr: js::Handle,
    value: js::ForeignBytes,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_osgymsandboxwarmpoolspecbuilder_sandbox_template_ref(
            u64::into_rust(ptr),
            u::RustBuffer::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_clone_warmpoolautoscalingbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_clone_warmpoolautoscalingbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_free_warmpoolautoscalingbuilder(
    handle: js::Handle,
    f_status_: &mut js::RustCallStatus,
) {
    let mut u_status_ = u::RustCallStatus::default();
    unsafe {
        uniffi_cyclops_sdk_schema_fn_free_warmpoolautoscalingbuilder(
            u64::into_rust(handle),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_constructor_warmpoolautoscalingbuilder_new(
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_constructor_warmpoolautoscalingbuilder_new(
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_build(
    ptr: js::Handle,
    f_status_: &mut js::RustCallStatus,
) -> js::ForeignBytes {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_build(
            u64::into_rust(ptr),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_initial_pool_size(
    ptr: js::Handle,
    value: js::UInt32,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_initial_pool_size(
            u64::into_rust(ptr),
            u32::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_max_pool_size(
    ptr: js::Handle,
    value: js::UInt32,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_max_pool_size(
            u64::into_rust(ptr),
            u32::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub fn ubrn_uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_min_pool_size(
    ptr: js::Handle,
    value: js::UInt32,
    f_status_: &mut js::RustCallStatus,
) -> js::Handle {
    let mut u_status_ = u::RustCallStatus::default();
    let value_ = unsafe {
        uniffi_cyclops_sdk_schema_fn_method_warmpoolautoscalingbuilder_min_pool_size(
            u64::into_rust(ptr),
            u32::into_rust(value),
            &mut u_status_,
        )
    };
    f_status_.copy_from(u_status_);
    value_.into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_imagerefbuilder_build() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_imagerefbuilder_build().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_imagerefbuilder_name() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_imagerefbuilder_name().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_build() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_build().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_name() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_name().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_protocol() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_protocol().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_target_port() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_sandboxservicebuilder_target_port()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_sandboxtemplaterefbuilder_build() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_sandboxtemplaterefbuilder_build().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_sandboxtemplaterefbuilder_name() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_sandboxtemplaterefbuilder_name().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_build() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_build().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_command() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_command().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_container_disk_image() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_container_disk_image()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_cpu_cores() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_cpu_cores().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_firmware() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_firmware().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_pull_policy() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_pull_policy()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_pull_secret() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_pull_secret()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_ref() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_image_ref().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_memory() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_memory().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_nested_virtualization() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_nested_virtualization()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_node_selector() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_node_selector().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_oidc() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_oidc().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_probes() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_probes().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_runtime() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_runtime().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_runtime_class_name() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_runtime_class_name()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_services() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_services().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_tolerations() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_vmtemplatebuilder_tolerations().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_preservedjson_to_json() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_preservedjson_to_json().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxtemplatespecbuilder_build() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxtemplatespecbuilder_build()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxtemplatespecbuilder_vm_template() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxtemplatespecbuilder_vm_template()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_autoscaling() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_autoscaling()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_build() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_build()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_replicas() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_replicas()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_sandbox_template_ref() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_osgymsandboxwarmpoolspecbuilder_sandbox_template_ref()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_build() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_build()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_initial_pool_size() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_initial_pool_size()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_max_pool_size() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_max_pool_size()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_min_pool_size() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_method_warmpoolautoscalingbuilder_min_pool_size()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_imagerefbuilder_new() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_imagerefbuilder_new().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_sandboxservicebuilder_new() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_sandboxservicebuilder_new().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_sandboxtemplaterefbuilder_new() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_sandboxtemplaterefbuilder_new()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_vmtemplatebuilder_new() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_vmtemplatebuilder_new().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_preservedjson_from_json() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_preservedjson_from_json().into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_osgymsandboxtemplatespecbuilder_new() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_osgymsandboxtemplatespecbuilder_new()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_osgymsandboxwarmpoolspecbuilder_new() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_osgymsandboxwarmpoolspecbuilder_new()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_uniffi_cyclops_sdk_schema_checksum_constructor_warmpoolautoscalingbuilder_new() -> js::UInt16 {
    uniffi_cyclops_sdk_schema_checksum_constructor_warmpoolautoscalingbuilder_new()
        .into_js()
}
#[wasm_bindgen]
pub unsafe fn ubrn_ffi_cyclops_sdk_schema_uniffi_contract_version() -> js::UInt32 {
    ffi_cyclops_sdk_schema_uniffi_contract_version().into_js()
}
