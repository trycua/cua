use cyclops_sdk_schema::{
    ImageRefBuilder, OSGymSandboxTemplateSpec, OSGymSandboxTemplateSpecBuilder,
    OSGymSandboxWarmPoolSpec, OSGymSandboxWarmPoolSpecBuilder, RuntimeKind, SandboxService,
    SandboxServiceBuilder, SandboxTemplateRef, SandboxTemplateRefBuilder, SchemaBuildError,
    VmTemplate, VmTemplateBuilder, WarmPoolAutoscaling, WarmPoolAutoscalingBuilder,
};

#[test]
fn builders_cover_schema_records_and_optional_omission() {
    let service: SandboxService = SandboxServiceBuilder::new()
        .name("mcp".into())
        .target_port(3000)
        .build()
        .unwrap();
    let vm: VmTemplate = VmTemplateBuilder::new()
        .container_disk_image("image".into())
        .image_pull_secret("secret".into())
        .cpu_cores(4)
        .memory("8Gi".into())
        .services(vec![service.clone()])
        .build()
        .unwrap();
    let template: OSGymSandboxTemplateSpec = OSGymSandboxTemplateSpecBuilder::new()
        .vm_template(vm.clone())
        .build()
        .unwrap();
    let reference: SandboxTemplateRef = SandboxTemplateRefBuilder::new()
        .name("template".into())
        .build()
        .unwrap();
    let pool: OSGymSandboxWarmPoolSpec = OSGymSandboxWarmPoolSpecBuilder::new()
        .replicas(1)
        .sandbox_template_ref(reference)
        .build()
        .unwrap();

    assert_eq!(service.protocol, None);
    assert_eq!(vm.command, None);
    assert_eq!(vm.image_pull_secret.as_deref(), Some("secret"));
    assert_eq!(vm.cpu_cores, Some(4));
    assert_eq!(vm.memory.as_deref(), Some("8Gi"));
    assert_eq!(
        template.vm_template.container_disk_image.as_deref(),
        Some("image")
    );
    assert_eq!(pool.autoscaling, None);
}

#[test]
fn required_fields_use_stable_error() {
    let error = VmTemplateBuilder::new().build().unwrap_err();
    assert_eq!(
        error.to_string(),
        "VmTemplate requires exactly one of container_disk_image or image_ref for kubevirt"
    );
    assert!(matches!(error, SchemaBuildError::Invalid { .. }));
}

#[test]
fn generated_builders_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<VmTemplateBuilder>();
    assert_send_sync::<OSGymSandboxTemplateSpecBuilder>();
}

#[test]
fn builder_setters_preserve_prior_versions() {
    let base = VmTemplateBuilder::new();
    let first = base.container_disk_image("registry.example/first:latest".to_owned());
    let second = base.container_disk_image("registry.example/second:latest".to_owned());

    assert!(base.build().is_err());
    assert_eq!(
        first.build().unwrap().container_disk_image.as_deref(),
        Some("registry.example/first:latest")
    );
    assert_eq!(
        second.build().unwrap().container_disk_image.as_deref(),
        Some("registry.example/second:latest")
    );
}

#[test]
fn autoscaling_builder_supports_empty_and_immutable_optional_values() {
    let base = WarmPoolAutoscalingBuilder::new();
    let configured = base.min_pool_size(1).initial_pool_size(2).max_pool_size(5);

    assert_eq!(
        base.build().unwrap(),
        WarmPoolAutoscaling {
            min_pool_size: None,
            initial_pool_size: None,
            max_pool_size: None,
        }
    );
    assert_eq!(
        configured.build().unwrap(),
        WarmPoolAutoscaling {
            min_pool_size: Some(1),
            initial_pool_size: Some(2),
            max_pool_size: Some(5),
        }
    );
}

#[test]
fn kubevirt_template_accepts_one_image_ref() {
    let template = VmTemplateBuilder::new()
        .image_ref(
            ImageRefBuilder::new()
                .name("image-mzxw6ytboi".into())
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    assert_eq!(template.image_ref.unwrap().name, "image-mzxw6ytboi");
    assert!(template.container_disk_image.is_none());
}

#[test]
fn kubevirt_template_rejects_two_root_sources() {
    let error = VmTemplateBuilder::new()
        .container_disk_image("registry.example/vm@sha256:abc".into())
        .image_ref(
            ImageRefBuilder::new()
                .name("image-mzxw6ytboi".into())
                .build()
                .unwrap(),
        )
        .build()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "VmTemplate requires exactly one of container_disk_image or image_ref for kubevirt"
    );
}

#[test]
fn pod_runtime_rejects_image_ref() {
    let error = VmTemplateBuilder::new()
        .runtime(RuntimeKind::Gvisor)
        .image_ref(
            ImageRefBuilder::new()
                .name("image-mzxw6ytboi".into())
                .build()
                .unwrap(),
        )
        .build()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "VmTemplate image_ref is supported only for kubevirt"
    );
}

#[test]
fn pod_runtime_requires_container_disk_image() {
    let error = VmTemplateBuilder::new()
        .runtime(RuntimeKind::Macos)
        .build()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "VmTemplate requires container_disk_image for pod runtimes"
    );
}
