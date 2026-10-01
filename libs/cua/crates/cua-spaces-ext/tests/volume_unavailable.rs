// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A Space that connects without a Cua Volume (its image has none, or the
//! mount failed) is ready all the same, and says why: `volume_sync_status`
//! lists it under `volume_errors` (what `cua volume status` prints) until
//! the Space goes. Hermetic: a fake local runtime whose one instance is a
//! mock cua-spacesd without the volume feature, and temp homes.

use async_trait::async_trait;
use cua_sandbox_core::placement::On;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::SpaceCreate;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct FakeRuntime {
    port: u16,
    running: Mutex<BTreeMap<String, bool>>,
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(&spec.name).await?,
        })
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().insert(name.into(), false);
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(name).await?,
        })
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().get(name) {
            Some(true) => Ok(InstanceStatus::Running),
            Some(false) => Ok(InstanceStatus::Stopped),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, self.port)].into(),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn a_space_without_a_volume_is_ready_and_says_why() {
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: srv.addr.port(),
        running: Mutex::new(BTreeMap::new()),
    });
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        cua_spaces::Spaces::builder().home(home.path()).sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        ),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let server = cua_spaces::mcp::McpServer::new(spaces.clone());
    let status = || {
        let server = &server;
        async move {
            let o = Box::pin(server.call("volume_sync_status", json!({}))).await;
            assert!(!o.is_error, "{:?}", o.content);
            serde_json::from_str::<Value>(o.content[0]["text"].as_str().unwrap()).unwrap()
        }
    };
    assert_eq!(status().await["volume_errors"], json!([]));

    // Ready: the create returns the Space even though no volume mounts.
    let info = spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
            spacesd: Some(true),
            name: Some("novol".into()),
            timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
    assert_eq!(info.id, "local:novol");

    // The attach runs in the background; its verdict shows within seconds.
    let mut errors = Value::Null;
    for _ in 0..100 {
        errors = status().await["volume_errors"].clone();
        if errors.as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let e = &errors[0];
    assert_eq!(e["space"], "local:novol", "{errors}");
    assert!(
        e["error"]
            .as_str()
            .is_some_and(|m| m.starts_with("no Cua Volume in this Space: ")),
        "{errors}"
    );
    assert_eq!(status().await["volumes"], json!([]));

    // The note goes with the Space.
    spaces.delete("local:novol").await.unwrap();
    let mut after = Value::Null;
    for _ in 0..50 {
        after = status().await["volume_errors"].clone();
        if after == json!([]) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(after, json!([]));
}
