use cyclops_sdk_schema::{
    OSGymSandboxTemplateSpec, OSGymSandboxTemplateSpecBuilder, OSGymSandboxWarmPoolSpec,
    OSGymSandboxWarmPoolSpecBuilder, SandboxService, SandboxServiceBuilder, SandboxTemplateRef,
    SandboxTemplateRefBuilder, SchemaBuildError, VmTemplate, VmTemplateBuilder,
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
    assert_eq!(template.vm_template.container_disk_image, "image");
    assert_eq!(pool.autoscaling, None);
}

#[test]
fn required_fields_use_stable_error() {
    let error = VmTemplateBuilder::new().build().unwrap_err();
    assert_eq!(
        error.to_string(),
        "VmTemplate is missing required field container_disk_image"
    );
    assert!(matches!(
        error,
        SchemaBuildError::MissingRequiredField { record_type, field }
            if record_type == "VmTemplate" && field == "container_disk_image"
    ));
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
        first.build().unwrap().container_disk_image,
        "registry.example/first:latest"
    );
    assert_eq!(
        second.build().unwrap().container_disk_image,
        "registry.example/second:latest"
    );
}
