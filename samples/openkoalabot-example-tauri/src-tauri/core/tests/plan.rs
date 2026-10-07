// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `Core::create_space` reaches the SDK with the call the wizard planned:
//! a recording fake `LocalRuntime` (no container, VM or network) sees the
//! prefixed image and the resources, and a cloud plan without cloud
//! credentials names the missing capability instead of creating anything.

use async_trait::async_trait;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use openkoalabot_example_core::plan::{SpacePlan, Target};
use openkoalabot_example_core::{Core, CoreConfig};
use std::sync::{Arc, Mutex};

/// Records every start and refuses it, so nothing runs.
#[derive(Default)]
struct Recorder {
    started: Mutex<Vec<LocalStartSpec>>,
}

#[async_trait]
impl LocalRuntime for Recorder {
    fn backend(&self) -> String {
        "recorder".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.started.lock().unwrap().push(spec.clone());
        Err(RuntimeError::Other(
            "recorder: not starting anything".into(),
        ))
    }
    async fn stop(&self, _: &str) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn delete(&self, _: &str) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn endpoints(&self, name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Err(RuntimeError::NotFound(name.into()))
    }
}

fn plan(image: &str, target: Target) -> SpacePlan {
    SpacePlan {
        image: image.into(),
        target,
        name: "cua-e2e-koala".into(),
        cpus: Some(3),
        memory_mb: Some(6144),
    }
}

#[tokio::test]
async fn a_local_plan_starts_the_prefixed_image_with_its_resources() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Recorder::default());
    let core = Core::new(CoreConfig::in_dir(dir.path()).with_local_runtime(rt.clone())).unwrap();
    let err = core
        .create_space(&plan("ghcr.io/trycua/linux:24.04", Target::Local))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("recorder"), "{err}");
    let started = rt.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1, "exactly one start");
    let spec = &started[0];
    assert_eq!(spec.name, "cua-e2e-koala");
    assert!(
        spec.image.ends_with("ghcr.io/trycua/linux:24.04"),
        "image {}",
        spec.image
    );
    assert!(
        spec.image.starts_with("container:") || spec.image == "ghcr.io/trycua/linux:24.04",
        "image {}",
        spec.image
    );
    assert_eq!(spec.cpus, 3);
    assert_eq!(spec.memory_mb, 6144);
    assert!(
        spec.env.contains_key("CUA_ENV_TOKEN"),
        "a fresh spacesd token"
    );
    assert!(core.list_spaces().unwrap().is_empty(), "nothing registered");
}

#[tokio::test]
async fn a_local_plan_without_a_local_runtime_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(CoreConfig::in_dir(dir.path())).unwrap();
    let err = core
        .create_space(&plan("ghcr.io/trycua/linux:24.04", Target::Local))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("local runtime"), "{err}");
}

#[tokio::test]
async fn a_cloud_plan_without_credentials_names_the_missing_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(CoreConfig::in_dir(dir.path())).unwrap();
    let err = core
        .create_space(&plan("ghcr.io/trycua/linux:24.04", Target::Cloud))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("CUA_CLIENT_ID"), "{err}");
}

#[tokio::test]
async fn an_unsupported_target_never_reaches_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Recorder::default());
    let core = Core::new(CoreConfig::in_dir(dir.path()).with_local_runtime(rt.clone())).unwrap();
    let err = core
        .create_space(&plan("ghcr.io/trycua/macos:26", Target::Cloud))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("does not run in the cloud"),
        "{err}"
    );
    assert!(rt.started.lock().unwrap().is_empty());
}
