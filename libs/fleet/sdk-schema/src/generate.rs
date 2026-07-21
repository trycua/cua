use crate::{
    OSGymSandbox, OSGymSandboxClaim, OSGymSandboxTemplate, OSGymSandboxWarmPool, OSGymWorkspacePool,
};
use anyhow::{Context, Result, bail};
use kube::CustomResourceExt;
use serde::Deserialize;
use serde_json::Value;
use std::{fs, path::Path};

pub fn render_crds() -> Result<String> {
    let resources = [
        serde_yaml::to_string(&OSGymSandbox::crd())?,
        serde_yaml::to_string(&OSGymSandboxTemplate::crd())?,
        serde_yaml::to_string(&OSGymSandboxWarmPool::crd())?,
        serde_yaml::to_string(&OSGymSandboxClaim::crd())?,
        serde_yaml::to_string(&OSGymWorkspacePool::crd())?,
    ];

    Ok(resources.join("---\n"))
}

pub fn write_crds(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let output = render_crds()?;

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create CRD output directory {}", parent.display())
        })?;
    }

    fs::write(path, output)
        .with_context(|| format!("failed to write CRD bundle to {}", path.display()))?;
    Ok(())
}

pub fn semantic_yaml_documents(input: &str) -> Result<Vec<Value>> {
    serde_yaml::Deserializer::from_str(input)
        .map(|document| {
            let yaml = serde_yaml::Value::deserialize(document)?;
            Ok(serde_json::to_value(yaml)?)
        })
        .collect()
}

pub fn check_crds(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let generated = semantic_yaml_documents(&render_crds()?)?;
    let checked_in =
        semantic_yaml_documents(&fs::read_to_string(path).with_context(|| {
            format!("failed to read checked-in CRD bundle at {}", path.display())
        })?)?;

    let changed_resources = changed_resource_names(&generated, &checked_in);
    if changed_resources.is_empty() {
        return Ok(());
    }

    bail!(
        "CRD bundle drift detected in {}:\n{}\nRun `cargo run --manifest-path cyclops-cs/Cargo.toml -p cyclops-sdk-schema --bin generate-crds -- --output {}` to regenerate it.",
        path.display(),
        changed_resources
            .iter()
            .map(|name| format!("- {name}"))
            .collect::<Vec<_>>()
            .join("\n"),
        path.display(),
    );
}

fn changed_resource_names(generated: &[Value], checked_in: &[Value]) -> Vec<String> {
    let expected_names = generated.iter().map(resource_name).collect::<Vec<_>>();
    let actual_names = checked_in.iter().map(resource_name).collect::<Vec<_>>();
    let mut names = Vec::new();

    for (index, name) in expected_names.iter().enumerate() {
        if generated.get(index) != checked_in.get(index) {
            names.push((*name).to_owned());
        }
    }
    for (index, name) in actual_names.iter().enumerate() {
        if checked_in.get(index) != generated.get(index) && !names.iter().any(|entry| entry == name)
        {
            names.push((*name).to_owned());
        }
    }

    names
}

fn resource_name(value: &Value) -> &str {
    value
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or("<unnamed document>")
}
