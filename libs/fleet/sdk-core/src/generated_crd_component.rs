// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.
fn claim_spec_json(value: crd_types::ClaimSpec) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    if let Some(value) = value.bind_deadline {
        object.insert("bindDeadline".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.lifecycle {
        object.insert("lifecycle".into(), claim_spec_lifecycle_json(value)?);
    }
    object.insert(
        "sandboxTemplateRef".into(),
        sandbox_template_ref_json(value.sandbox_template_ref)?,
    );
    if let Some(value) = value.warmpool {
        object.insert("warmpool".into(), serde_json::Value::from(value));
    }
    Ok(serde_json::Value::Object(object))
}

fn claim_spec_lifecycle_json(
    value: crd_types::ClaimSpecLifecycle,
) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    if let Some(value) = value.auto_renew {
        object.insert("autoRenew".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.shutdown_policy {
        object.insert("shutdownPolicy".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.shutdown_time {
        object.insert("shutdownTime".into(), serde_json::Value::from(value));
    }
    Ok(serde_json::Value::Object(object))
}

fn pool_spec_json(value: crd_types::PoolSpec) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    if let Some(value) = value.autoscaling {
        object.insert("autoscaling".into(), pool_spec_autoscaling_json(value)?);
    }
    object.insert("replicas".into(), serde_json::Value::from(value.replicas));
    if let Some(value) = value.services {
        object.insert(
            "services".into(),
            serde_json::Value::Array(
                value
                    .into_iter()
                    .map(|item| Ok(pool_spec_service_json(item)?))
                    .collect::<Result<Vec<_>, String>>()?,
            ),
        );
    }
    object.insert("template".into(), pool_template_json(value.template)?);
    Ok(serde_json::Value::Object(object))
}

fn pool_spec_autoscaling_json(
    value: crd_types::PoolSpecAutoscaling,
) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    if let Some(value) = value.initial_pool_size {
        object.insert("initialPoolSize".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.max_pool_size {
        object.insert("maxPoolSize".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.min_pool_size {
        object.insert("minPoolSize".into(), serde_json::Value::from(value));
    }
    Ok(serde_json::Value::Object(object))
}

fn pool_spec_service_json(value: crd_types::PoolSpecService) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    object.insert("name".into(), serde_json::Value::from(value.name));
    if let Some(value) = value.protocol {
        object.insert("protocol".into(), pool_spec_service_protocol_json(value));
    }
    object.insert(
        "targetPort".into(),
        serde_json::Value::from(value.target_port),
    );
    Ok(serde_json::Value::Object(object))
}

fn pool_spec_service_protocol_json(value: crd_types::PoolSpecServiceProtocol) -> serde_json::Value {
    serde_json::Value::String(
        match value {
            crd_types::PoolSpecServiceProtocol::Tcp => "TCP",
            crd_types::PoolSpecServiceProtocol::Udp => "UDP",
        }
        .into(),
    )
}

fn pool_template_json(value: crd_types::PoolTemplate) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    if let Some(value) = value.command {
        object.insert(
            "command".into(),
            serde_json::Value::Array(
                value
                    .into_iter()
                    .map(|item| serde_json::Value::from(item))
                    .collect(),
            ),
        );
    }
    object.insert(
        "containerDiskImage".into(),
        serde_json::Value::from(value.container_disk_image),
    );
    if let Some(value) = value.cpu_cores {
        object.insert("cpuCores".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.firmware {
        object.insert("firmware".into(), pool_template_firmware_json(value));
    }
    if let Some(value) = value.image_pull_secret {
        object.insert("imagePullSecret".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.memory {
        object.insert("memory".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.node_selector {
        object.insert(
            "nodeSelector".into(),
            serde_json::Value::Object(
                value
                    .into_iter()
                    .map(|entry| (entry.key, serde_json::Value::from(entry.value)))
                    .collect(),
            ),
        );
    }
    if let Some(value) = value.oidc {
        object.insert("oidc".into(), pool_template_oidc_json(value)?);
    }
    if let Some(value) = value.probes {
        object.insert(
            "probes".into(),
            serde_json::from_str(&value).map_err(|error| error.to_string())?,
        );
    }
    if let Some(value) = value.runtime {
        object.insert("runtime".into(), pool_template_runtime_json(value));
    }
    if let Some(value) = value.runtime_class_name {
        object.insert("runtimeClassName".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.tolerations {
        object.insert(
            "tolerations".into(),
            serde_json::Value::Array(
                value
                    .into_iter()
                    .map(|item| Ok(serde_json::from_str(&item).map_err(|error| error.to_string())?))
                    .collect::<Result<Vec<_>, String>>()?,
            ),
        );
    }
    Ok(serde_json::Value::Object(object))
}

fn pool_template_firmware_json(value: crd_types::PoolTemplateFirmware) -> serde_json::Value {
    serde_json::Value::String(
        match value {
            crd_types::PoolTemplateFirmware::Bios => "bios",
            crd_types::PoolTemplateFirmware::Efi => "efi",
        }
        .into(),
    )
}

fn pool_template_oidc_json(
    value: crd_types::PoolTemplateOidc,
) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    if let Some(value) = value.aws_region {
        object.insert("awsRegion".into(), serde_json::Value::from(value));
    }
    if let Some(value) = value.aws_role_arn {
        object.insert("awsRoleArn".into(), serde_json::Value::from(value));
    }
    object.insert(
        "credentialsSecret".into(),
        serde_json::Value::from(value.credentials_secret),
    );
    if let Some(value) = value.refresh_interval_seconds {
        object.insert(
            "refreshIntervalSeconds".into(),
            serde_json::Value::from(value),
        );
    }
    object.insert("tokenUrl".into(), serde_json::Value::from(value.token_url));
    Ok(serde_json::Value::Object(object))
}

fn pool_template_runtime_json(value: crd_types::PoolTemplateRuntime) -> serde_json::Value {
    serde_json::Value::String(
        match value {
            crd_types::PoolTemplateRuntime::Kubevirt => "kubevirt",
            crd_types::PoolTemplateRuntime::Macos => "macos",
            crd_types::PoolTemplateRuntime::Gvisor => "gvisor",
        }
        .into(),
    )
}

fn sandbox_template_ref_json(
    value: crd_types::SandboxTemplateRef,
) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    object.insert("name".into(), serde_json::Value::from(value.name));
    Ok(serde_json::Value::Object(object))
}

fn pool_spec_from_wit(value: crd_types::PoolSpec) -> Result<super::PoolSpec, String> {
    serde_json::from_value(pool_spec_json(value)?).map_err(|error| error.to_string())
}

fn claim_spec_from_wit(value: crd_types::ClaimSpec) -> Result<super::ClaimSpec, String> {
    serde_json::from_value(claim_spec_json(value)?).map_err(|error| error.to_string())
}
