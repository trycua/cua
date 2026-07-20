use cyclops_sdk_schema::{
    Firmware, ImagePullPolicy, JsonValueError, OSGymSandboxClaimCondition,
    OSGymSandboxClaimSandbox, OSGymSandboxClaimStatus, OSGymSandboxStatus,
    OSGymSandboxWarmPoolStatus, OidcConfig, PreservedJson, RuntimeKind, SandboxService,
    ServiceProtocol, VmTemplate,
};
use schemars::schema_for;
use serde_json::json;
use std::{collections::HashMap, sync::Arc};

#[test]
fn preserved_json_accepts_valid_json() {
    let value = PreservedJson::parse(r#"{"exec":{"command":["true"]}}"#.into()).unwrap();
    assert_eq!(value.to_string(), r#"{"exec":{"command":["true"]}}"#);
}

#[test]
fn preserved_json_rejects_invalid_json() {
    let error = PreservedJson::parse("{not-json}".into()).unwrap_err();
    assert!(error.to_string().contains("valid JSON"));
}

#[test]
fn preserved_json_schema_keeps_the_crd_object_contract() {
    let schema = schema_for!(PreservedJson);
    let value = schema.as_value();

    assert_eq!(value.get("type"), Some(&json!("object")));
    assert_eq!(
        value.get("x-kubernetes-preserve-unknown-fields"),
        Some(&json!(true))
    );
}

#[test]
fn preserved_json_from_json_returns_a_foreign_object() {
    let value = PreservedJson::from_json(r#"{"exec":{"command":["true"]}}"#.into()).unwrap();

    assert_eq!(value.to_json(), r#"{"exec":{"command":["true"]}}"#);
}

#[test]
fn preserved_json_from_json_returns_a_structured_error() {
    let error = PreservedJson::from_json("{not-json}".into()).unwrap_err();

    assert!(matches!(error, JsonValueError::Invalid { .. }));
}

fn assert_non_object_json_is_rejected(value: &str) {
    let parse_error = PreservedJson::parse(value.into()).unwrap_err();
    assert!(matches!(parse_error, JsonValueError::Invalid { .. }));

    let constructor_error = PreservedJson::from_json(value.into()).unwrap_err();
    assert!(matches!(constructor_error, JsonValueError::Invalid { .. }));

    let deserialize_error = serde_json::from_str::<PreservedJson>(value).unwrap_err();
    assert!(deserialize_error.to_string().contains("JSON object"));
}

#[test]
fn preserved_json_rejects_arrays() {
    assert_non_object_json_is_rejected("[]");
}

#[test]
fn preserved_json_rejects_scalars() {
    assert_non_object_json_is_rejected("true");
}

#[test]
fn preserved_json_rejects_null() {
    assert_non_object_json_is_rejected("null");
}

#[test]
fn vm_template_round_trips_every_known_field() {
    let template = VmTemplate {
        container_disk_image: "registry.example/vm:latest".into(),
        command: Some(vec!["/bin/sh".into(), "-c".into(), "echo ready".into()]),
        runtime: Some(RuntimeKind::Gvisor),
        runtime_class_name: Some("gvisor".into()),
        node_selector: Some(HashMap::from([("cua.ai/gvisor".into(), "enabled".into())])),
        tolerations: Some(vec![Arc::new(
            PreservedJson::parse(r#"{"key":"cua.ai/macos","effect":"NoSchedule"}"#.into()).unwrap(),
        )]),
        image_pull_policy: Some(ImagePullPolicy::IfNotPresent),
        image_pull_secret: Some("registry-creds".into()),
        cpu_cores: Some(8),
        memory: Some("8Gi".into()),
        firmware: Some(Firmware::Efi),
        probes: Some(Arc::new(
            PreservedJson::parse(r#"{"readinessProbe":{"tcpSocket":{"port":22}}}"#.into()).unwrap(),
        )),
        services: Some(vec![SandboxService {
            name: "ssh".into(),
            target_port: 22,
            protocol: Some(ServiceProtocol::TCP),
        }]),
        oidc: Some(OidcConfig {
            credentials_secret: "workload-oidc".into(),
            token_url: "https://auth.example/token".into(),
            aws_region: Some("us-west-2".into()),
            aws_role_arn: Some("arn:aws:iam::123456789012:role/workload".into()),
            refresh_interval_seconds: Some(900),
        }),
    };

    let value = serde_json::to_value(&template).unwrap();
    assert_eq!(value["imagePullPolicy"], json!("IfNotPresent"));
    assert_eq!(value["tolerations"][0]["key"], json!("cua.ai/macos"));
    assert_eq!(
        value["probes"]["readinessProbe"]["tcpSocket"]["port"],
        json!(22)
    );
    assert_eq!(
        serde_json::from_value::<VmTemplate>(value).unwrap(),
        template
    );
}

#[test]
fn vm_template_rejects_non_object_opaque_fields() {
    let error = serde_json::from_value::<VmTemplate>(json!({
        "containerDiskImage": "registry.example/vm:latest",
        "probes": ["not", "an", "object"],
    }))
    .unwrap_err();

    assert!(error.to_string().contains("JSON object"));
}

#[test]
fn minimal_vm_template_omits_none_fields_and_round_trips() {
    let template = VmTemplate {
        container_disk_image: "registry.example/vm:latest".into(),
        command: None,
        runtime: None,
        runtime_class_name: None,
        node_selector: None,
        tolerations: None,
        image_pull_policy: None,
        image_pull_secret: None,
        cpu_cores: None,
        memory: None,
        firmware: None,
        probes: None,
        services: None,
        oidc: None,
    };

    let value = serde_json::to_value(&template).unwrap();
    assert_eq!(
        value,
        json!({"containerDiskImage": "registry.example/vm:latest"})
    );
    assert!(!value.to_string().contains("null"));
    assert_eq!(
        serde_json::from_value::<VmTemplate>(value).unwrap(),
        template
    );
}

#[test]
fn exported_status_records_are_typed_and_round_trip() {
    let sandbox_status = OSGymSandboxStatus {
        phase: Some("Ready".into()),
        runtime: Some("kubevirt".into()),
        ready: Some(true),
        vm_name: Some("sandbox-vm".into()),
        service: Some("sandbox-service".into()),
        message: Some("ready for claims".into()),
        reset_issued_at: Some("2026-07-19T12:00:00Z".into()),
        reset_vmi_uid: Some("vmi-uid".into()),
    };
    let warm_pool_status = OSGymSandboxWarmPoolStatus {
        replicas: Some(3),
        ready_replicas: Some(2),
        selector: Some("osgym.cua.ai/pool=default".into()),
    };
    let claim_status = OSGymSandboxClaimStatus {
        phase: Some("Bound".into()),
        conditions: Some(vec![OSGymSandboxClaimCondition {
            type_: Some("Bound".into()),
            status: Some("True".into()),
            reason: Some("SandboxAvailable".into()),
            message: Some("claim bound".into()),
            last_transition_time: Some("2026-07-19T12:00:00Z".into()),
        }]),
        sandbox: Some(OSGymSandboxClaimSandbox {
            name: Some("sandbox-1".into()),
            service: Some("sandbox-1-service".into()),
        }),
    };

    assert_eq!(
        serde_json::from_value::<OSGymSandboxStatus>(
            serde_json::to_value(&sandbox_status).unwrap()
        )
        .unwrap(),
        sandbox_status
    );
    assert_eq!(
        serde_json::from_value::<OSGymSandboxWarmPoolStatus>(
            serde_json::to_value(&warm_pool_status).unwrap(),
        )
        .unwrap(),
        warm_pool_status
    );
    assert_eq!(
        serde_json::from_value::<OSGymSandboxClaimStatus>(
            serde_json::to_value(&claim_status).unwrap()
        )
        .unwrap(),
        claim_status
    );
}
