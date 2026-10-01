use cyclops_sdk::{
    CreateClaimRequestBuilder, CreatePoolRequest, CreatePoolRequestBuilder,
    CreateSignedServiceUrlRequestBuilder, CreateTemplateRequest, CreateTemplateRequestBuilder,
    CreateUserApiKeyRequestBuilder, CyclopsTokenProviderConfigurationBuilder, Pool,
    ResourceMetadata, Sandbox, SdkBuildError, TemplateBuilder,
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

#[test]
fn builders_cover_remaining_frontend_sdk_records() {
    let configuration = CyclopsTokenProviderConfigurationBuilder::new()
        .base_url("https://api.example.test".into())
        .pool_poll_interval_ms(5_000)
        .pool_poll_limit(120)
        .claim_poll_interval_ms(5_000)
        .claim_poll_limit(120)
        .build()
        .unwrap();
    let user_key = CreateUserApiKeyRequestBuilder::new()
        .name("automation".into())
        .scope(Vec::new())
        .build()
        .unwrap();
    let template_spec = OSGymSandboxTemplateSpecBuilder::new()
        .vm_template(
            VmTemplateBuilder::new()
                .container_disk_image("image".into())
                .build()
                .unwrap(),
        )
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
    let metadata = ResourceMetadata {
        namespace: "default".into(),
        name: "template".into(),
        labels: None,
        creation_timestamp: None,
    };
    let template = TemplateBuilder::new()
        .api_version("osgym.cua.ai/v1alpha1".into())
        .kind("OSGymSandboxTemplate".into())
        .metadata(metadata.clone())
        .spec(template_spec)
        .build()
        .unwrap();
    let pool = Pool {
        api_version: "osgym.cua.ai/v1alpha1".into(),
        kind: "OSGymSandboxWarmPool".into(),
        metadata,
        spec: pool_spec,
        status: None,
    };
    let claim = CreateClaimRequestBuilder::new().pool(pool).build().unwrap();

    assert_eq!(configuration.pool_poll_interval_ms, 5_000);
    assert_eq!(configuration.claim_poll_limit, 120);
    assert_eq!(user_key.scope, Vec::<String>::new());
    assert_eq!(template.metadata.name, "template");
    assert!(claim.spec.is_none());
    assert!(claim.name.is_none());
}

#[test]
fn remaining_sdk_builders_use_stable_required_field_errors() {
    for (error, record_type, field) in [
        (
            CyclopsTokenProviderConfigurationBuilder::new()
                .build()
                .unwrap_err(),
            "CyclopsTokenProviderConfiguration",
            "base_url",
        ),
        (
            CreateUserApiKeyRequestBuilder::new().build().unwrap_err(),
            "CreateUserApiKeyRequest",
            "name",
        ),
        (
            TemplateBuilder::new().build().unwrap_err(),
            "Template",
            "api_version",
        ),
        (
            CreateClaimRequestBuilder::new().build().unwrap_err(),
            "CreateClaimRequest",
            "pool",
        ),
    ] {
        assert!(matches!(
            error,
            SdkBuildError::MissingRequiredField {
                record_type: actual_record,
                field: actual_field,
            } if actual_record == record_type && actual_field == field
        ));
    }
}

#[test]
fn signed_service_url_request_builder_constructs_request() {
    let sandbox = Sandbox {
        namespace: "tenant-a".into(),
        claim: "claim-a".into(),
        name: "sandbox-a".into(),
        services: vec!["mcp".into()],
    };

    let request = CreateSignedServiceUrlRequestBuilder::new()
        .sandbox(sandbox.clone())
        .service("mcp".into())
        .label("Customer demo".into())
        .expires_in_seconds(3600)
        .build()
        .unwrap();

    assert_eq!(request.sandbox, sandbox);
    assert_eq!(request.service, "mcp");
    assert_eq!(request.label.as_deref(), Some("Customer demo"));
    assert_eq!(request.expires_in_seconds, 3600);
}

#[test]
fn signed_service_url_request_builder_requires_sandbox_service_and_expiration() {
    let missing_sandbox = CreateSignedServiceUrlRequestBuilder::new()
        .service("mcp".into())
        .expires_in_seconds(3600)
        .build()
        .unwrap_err();
    assert!(matches!(
        missing_sandbox,
        SdkBuildError::MissingRequiredField { record_type, field }
            if record_type == "CreateSignedServiceUrlRequest" && field == "sandbox"
    ));

    let sandbox = Sandbox {
        namespace: "tenant-a".into(),
        claim: "claim-a".into(),
        name: "sandbox-a".into(),
        services: vec!["mcp".into()],
    };
    let missing_service = CreateSignedServiceUrlRequestBuilder::new()
        .sandbox(sandbox)
        .expires_in_seconds(3600)
        .build()
        .unwrap_err();
    assert!(matches!(
        missing_service,
        SdkBuildError::MissingRequiredField { record_type, field }
            if record_type == "CreateSignedServiceUrlRequest" && field == "service"
    ));

    let missing_expiration = CreateSignedServiceUrlRequestBuilder::new()
        .sandbox(Sandbox {
            namespace: "tenant-a".into(),
            claim: "claim-a".into(),
            name: "sandbox-a".into(),
            services: vec!["mcp".into()],
        })
        .service("mcp".into())
        .build()
        .unwrap_err();
    assert!(matches!(
        missing_expiration,
        SdkBuildError::MissingRequiredField { record_type, field }
            if record_type == "CreateSignedServiceUrlRequest" && field == "expires_in_seconds"
    ));
}
