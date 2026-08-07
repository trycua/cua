use cyclops_sdk::{
    CreatePoolRequest, CreatePoolRequestBuilder, CreateTemplateRequest,
    CreateTemplateRequestBuilder, SdkBuildError,
};
use cyclops_sdk_schema::{
    OSGymSandboxTemplateSpecBuilder, OSGymSandboxWarmPoolSpecBuilder, SandboxTemplateRefBuilder,
    VmTemplateBuilder,
};

#[test]
fn builders_cover_request_records() {
    let template_spec = OSGymSandboxTemplateSpecBuilder::new()
        .vm_template(
            VmTemplateBuilder::new()
                .container_disk_image("image".into())
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    let template_request: CreateTemplateRequest = CreateTemplateRequestBuilder::new()
        .namespace("default".into())
        .name("template".into())
        .spec(template_spec)
        .build()
        .unwrap();
    let pool_spec = OSGymSandboxWarmPoolSpecBuilder::new()
        .replicas(1)
        .sandbox_template_ref(
            SandboxTemplateRefBuilder::new()
                .name("template".into())
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    let pool_request: CreatePoolRequest = CreatePoolRequestBuilder::new()
        .namespace("default".into())
        .spec(pool_spec)
        .build()
        .unwrap();

    assert_eq!(template_request.namespace, "default");
    assert_eq!(pool_request.namespace, "default");
}

#[test]
fn request_required_fields_use_stable_error() {
    let error = CreatePoolRequestBuilder::new().build().unwrap_err();
    assert_eq!(
        error.to_string(),
        "CreatePoolRequest is missing required field namespace"
    );
    assert!(matches!(
        error,
        SdkBuildError::MissingRequiredField { record_type, field }
            if record_type == "CreatePoolRequest" && field == "namespace"
    ));
}
