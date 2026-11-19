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
    // volume_mount_status says the same.
    let o = Box::pin(server.call("volume_mount_status", json!({}))).await;
    assert!(!o.is_error, "{:?}", o.content);
    let m: Value = serde_json::from_str(o.content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(m["volume_errors"], errors, "{m}");
    assert_eq!(m["volumes"], json!([]));

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

// ---------------------------------------------------------------- named volumes

fn spaces_with_drive(
    rt: Option<Arc<FakeRuntime>>,
    home: &std::path::Path,
    state: &std::path::Path,
) -> cua_spaces::Spaces {
    let mut b = cua_spaces::Spaces::builder().home(home);
    if let Some(rt) = rt {
        b = b.sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt)
                .state_dir(state)
                .build(),
        );
    }
    cua_spaces_ext::register(b, cua_volume::Drive::open_local(home), None).build()
}

fn mount(name: &str, path: &str, read_only: bool) -> cua_spaces::named_volumes::VolumeMount {
    cua_spaces::named_volumes::VolumeMount {
        name: name.into(),
        path: path.into(),
        read_only,
        subpath: String::new(),
    }
}

#[tokio::test]
async fn named_volumes_are_made_listed_and_deleted_safely() {
    use cua_spaces::registry::Credential;
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = spaces_with_drive(None, home.path(), state.path());
    let call = |op: &'static str, a: Value| {
        let spaces = spaces.clone();
        async move { spaces.call_extension("Cua Volume", op, a).await }
    };
    assert_eq!(
        call("volumes.list", json!({})).await.unwrap(),
        json!({"volumes": []})
    );
    call("volumes.create", json!({"name": "shared"}))
        .await
        .unwrap();
    call("volumes.create", json!({"name": "other"}))
        .await
        .unwrap();
    // A second create, and bad names, are refused with the reason.
    let e = call("volumes.create", json!({"name": "shared"}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("already exists"), "{e}");
    for bad in ["Shared", "a/b", "", "-x"] {
        assert!(
            call("volumes.create", json!({"name": bad})).await.is_err(),
            "{bad}"
        );
    }
    let l = call("volumes.list", json!({})).await.unwrap();
    assert_eq!(
        l["volumes"],
        json!([{"name": "other", "mounted_in": []}, {"name": "shared", "mounted_in": []}])
    );
    // Existence checks name the first missing volume.
    call("volumes.exists", json!({"names": ["shared", "other"]}))
        .await
        .unwrap();
    let e = call("volumes.exists", json!({"names": ["shared", "typo"]}))
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("typo") && e.to_string().contains("create it"),
        "{e}"
    );

    // Files of the volume are the user's to put there.
    let drive = cua_volume::Drive::open_local(home.path());
    let user = drive.session(cua_volume::Context::user());
    user.write(
        "volumes/shared/a.txt",
        b"a".to_vec(),
        cua_volume::Condition::None,
    )
    .await
    .unwrap();
    user.write(
        "volumes/shared/d/b.txt",
        b"b".to_vec(),
        cua_volume::Condition::None,
    )
    .await
    .unwrap();

    // A Space that mounts it: listed, and the delete is refused.
    let registry = spaces.registry();
    registry
        .upsert(
            cua_proto::daemon::v1::Space {
                id: "local:user-a".into(),
                name: "user-a".into(),
                volumes: vec![(&mount("shared", "/mnt/shared", true)).into()],
                ..Default::default()
            },
            Credential::default(),
        )
        .unwrap();
    assert_eq!(
        spaces.volume_mounts("local:user-a").unwrap(),
        vec![mount("shared", "/mnt/shared", true)]
    );
    let l = call("volumes.list", json!({})).await.unwrap();
    assert_eq!(
        l["volumes"][1],
        json!({"name": "shared", "mounted_in": ["local:user-a"]})
    );
    let e = call("volumes.delete", json!({"name": "shared"}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("local:user-a"), "{e}");
    assert_eq!(
        user.read("volumes/shared/a.txt", None).await.unwrap().0,
        b"a"
    );
    // An unmounted one goes at once; forcing deletes a mounted one.
    let d = call("volumes.delete", json!({"name": "other"}))
        .await
        .unwrap();
    assert_eq!(d["files"], 0);
    let d = call("volumes.delete", json!({"name": "shared", "force": true}))
        .await
        .unwrap();
    assert_eq!(d["files"], 2);
    assert!(user.read("volumes/shared/a.txt", None).await.is_err());
    // History stays: the file can come back.
    let h = user.history("volumes/shared/a.txt").await.unwrap();
    assert!(!h.is_empty());
    assert_eq!(
        call("volumes.list", json!({})).await.unwrap(),
        json!({"volumes": []})
    );
    let e = call("volumes.delete", json!({"name": "shared"}))
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("not found") || e.to_string().contains("shared"),
        "{e}"
    );
}

#[tokio::test]
async fn volumes_that_cannot_be_served_fail_the_create_and_leave_nothing() {
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: srv.addr.port(),
        running: Mutex::new(BTreeMap::new()),
    });
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = spaces_with_drive(Some(rt.clone()), home.path(), state.path());
    let create = |volumes: Vec<cua_spaces::named_volumes::VolumeMount>, name: &str| {
        let spaces = spaces.clone();
        let name = name.to_string();
        async move {
            spaces
                .create(SpaceCreate {
                    on: Some(On::Local),
                    image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
                    spacesd: Some(true),
                    name: Some(name),
                    timeout: Some(Duration::from_secs(20)),
                    volumes,
                    ..Default::default()
                })
                .await
        }
    };
    // Bad arguments never reach the runtime.
    let e = create(vec![mount("shared", "mnt/rel", false)], "bad1")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("absolute"), "{e}");
    let e = create(
        vec![mount("a", "/mnt/a", false), mount("b", "/mnt/a/b", false)],
        "bad2",
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("inside"), "{e}");
    // A volume that does not exist.
    let e = create(vec![mount("ghost", "/mnt/g", false)], "bad3")
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("ghost") && e.to_string().contains("does not exist"),
        "{e}"
    );
    assert!(rt.running.lock().unwrap().is_empty());
    // A guest with no Cua Volume mount cannot serve the volume: the create
    // fails and the Space it made is deleted.
    spaces
        .call_extension("Cua Volume", "volumes.create", json!({"name": "shared"}))
        .await
        .unwrap();
    let e = create(vec![mount("shared", "/mnt/s", false)], "novol2")
        .await
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("named volumes need the Cua Volume mount"),
        "{e}"
    );
    assert!(
        spaces.list().unwrap().is_empty(),
        "the failed Space is gone"
    );
    assert!(
        rt.running.lock().unwrap().is_empty(),
        "its sandbox is deleted"
    );
    // Reuse and volumes do not mix.
    let e = spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            reuse: true,
            volumes: vec![mount("shared", "/mnt/s", false)],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.to_string().contains("reuse"), "{e}");
}

#[tokio::test]
async fn a_runtime_without_the_volume_extension_refuses_volumes() {
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces::Spaces::builder().home(home.path()).build();
    let e = spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            name: Some("x".into()),
            volumes: vec![mount("shared", "/mnt/s", false)],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.to_string().contains("Cua Volume"), "{e}");
}
