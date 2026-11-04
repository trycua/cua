//! The agentless fallback for sandboxes without cua-spacesd
//! (`Sandbox.guest_sh` / `guest_screenshot` / `guest_display`) through the
//! embedded SDK, over a fake local runtime: output streams in order with
//! stdout and stderr apart, the exit code is kept, a PNG comes back, the
//! display URL is redacted, and a runtime without the fallback fails with a
//! typed `Unsupported`. Nothing here starts a VM or touches `~/.cua`.

use async_trait::async_trait;
use cua_daemon::{Runtime, RuntimeConfig};
use cua_sandbox_core::{
    GuestDisplay, GuestOutput, GuestScreenshot, InstanceStatus, LocalEndpoints, LocalInstance,
    LocalRuntime, LocalStartSpec, LocalSummary, RuntimeError, RuntimeResult,
};
use cua_sdk::{Cua, CuaError, ImageFormat, SandboxCreateOptions};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

/// A Lume-like runtime: `agentless` turns the fallback on.
struct Fake {
    agentless: bool,
    vms: Mutex<HashMap<String, InstanceStatus>>,
    scripts: Mutex<Vec<(String, Option<Duration>)>>,
}

impl Fake {
    fn new(agentless: bool) -> Arc<Self> {
        Arc::new(Self {
            agentless,
            vms: Mutex::default(),
            scripts: Mutex::default(),
        })
    }
}

fn instance(name: &str) -> LocalInstance {
    LocalInstance {
        name: name.into(),
        backend: "lume".into(),
        status: InstanceStatus::Running,
        endpoints: LocalEndpoints {
            host: "127.0.0.1".into(),
            vnc: Some("127.0.0.1:5901".into()),
            ..Default::default()
        },
    }
}

#[async_trait]
impl LocalRuntime for Fake {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        self.vms
            .lock()
            .unwrap()
            .insert(spec.name.clone(), InstanceStatus::Running);
        Ok(instance(&spec.name))
    }
    async fn stop(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Ok(instance(name))
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        self.vms
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }
    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(instance(name).endpoints)
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.vms.lock().unwrap().remove(name);
        Ok(())
    }
    async fn guest_exec(
        &self,
        name: &str,
        script: &str,
        timeout: Option<Duration>,
        sink: tokio::sync::mpsc::Sender<GuestOutput>,
    ) -> RuntimeResult<i64> {
        if !self.agentless {
            return Err(RuntimeError::Unsupported {
                backend: "fake".into(),
                op: format!("agentless exec in {name}"),
            });
        }
        self.scripts
            .lock()
            .unwrap()
            .push((script.to_string(), timeout));
        for c in [
            GuestOutput::Stdout(b"one\n".to_vec()),
            GuestOutput::Stderr(b"warn\n".to_vec()),
            GuestOutput::Stdout(b"two\n".to_vec()),
        ] {
            sink.send(c).await.unwrap();
        }
        Ok(3)
    }
    async fn guest_screenshot(&self, name: &str) -> RuntimeResult<GuestScreenshot> {
        if !self.agentless {
            return Err(RuntimeError::Unsupported {
                backend: "fake".into(),
                op: format!("agentless screenshots of {name}"),
            });
        }
        Ok(GuestScreenshot {
            png: b"\x89PNG fake".to_vec(),
            width: 1024,
            height: 768,
            via: "vnc".into(),
        })
    }
    async fn guest_display(&self, name: &str) -> RuntimeResult<GuestDisplay> {
        Ok(GuestDisplay {
            url: "vnc://:s3cret@127.0.0.1:5901".into(),
            via: "vnc".into(),
            open_command: Some(vec!["lume".into(), "attach".into(), name.into()]),
        })
    }
}

async fn sdk(fake: Arc<Fake>) -> (Arc<Cua>, tempfile::TempDir) {
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        local: Some(fake),
        env_probe_timeout: Some(Duration::from_secs(1)),
        ..Default::default()
    })
    .unwrap();
    (Cua::from_runtime(runtime), dirs)
}

fn opts(name: &str) -> SandboxCreateOptions {
    SandboxCreateOptions {
        name: Some(name.into()),
        ..SandboxCreateOptions::new("local", "container:img")
    }
}

#[tokio::test]
async fn guest_sh_screenshot_and_display_go_through_the_runtime() {
    let fake = Fake::new(true);
    let (cua, _dirs) = sdk(fake.clone()).await;
    let sb = cua
        .sandboxes()
        .create(opts("cua-e2e-agentless"))
        .await
        .unwrap();

    // Streaming keeps the order and the streams apart.
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let code = sb
        .guest_sh_streaming("sw_vers".into(), Some(5_000), tx)
        .await
        .unwrap();
    assert_eq!(code, 3);
    let mut seen = Vec::new();
    for _ in 0..8 {
        match rx.recv().await {
            Some(c) => seen.push(c),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec![
            GuestOutput::Stdout(b"one\n".to_vec()),
            GuestOutput::Stderr(b"warn\n".to_vec()),
            GuestOutput::Stdout(b"two\n".to_vec()),
        ]
    );
    assert_eq!(
        fake.scripts.lock().unwrap()[0],
        ("sw_vers".to_string(), Some(Duration::from_secs(5)))
    );

    // Buffered, with the exec contract's ProcessOutput.
    let out = sb.guest_sh("uname".into(), None).await.unwrap();
    assert_eq!(out.exit.code, Some(3));
    assert!(!out.exit.success);
    assert_eq!(out.stdout, b"one\ntwo\n");
    assert_eq!(out.stderr, b"warn\n");
    assert_eq!(
        fake.scripts.lock().unwrap()[1].1,
        Some(Duration::from_secs(120)),
        "default timeout"
    );

    let shot = sb.guest_screenshot().await.unwrap();
    assert_eq!((shot.width, shot.height), (1024, 768));
    assert_eq!(shot.format, ImageFormat::Png);
    assert!(shot.image.starts_with(b"\x89PNG"));

    let d = sb.guest_display().await.unwrap();
    assert_eq!(d.via(), "vnc");
    assert_eq!(d.url(), "vnc://****@127.0.0.1:5901");
    assert_eq!(d.url_with_password(), "vnc://:s3cret@127.0.0.1:5901");
    for shown in [format!("{d:?}"), format!("{d:#?}"), d.to_string()] {
        assert!(!shown.contains("s3cret"), "leaked in {shown}");
    }
    assert_eq!(d.open_command(), ["lume", "attach", "cua-e2e-agentless"]);
}

#[tokio::test]
async fn a_runtime_without_the_fallback_is_a_typed_unsupported() {
    let (cua, _dirs) = sdk(Fake::new(false)).await;
    let sb = cua
        .sandboxes()
        .create(opts("cua-e2e-no-agentless"))
        .await
        .unwrap();
    let e = sb.guest_sh("true".into(), None).await.unwrap_err();
    assert!(matches!(e, CuaError::Unsupported(_)), "{e:?}");
    let e = sb.guest_screenshot().await.unwrap_err();
    assert!(matches!(e, CuaError::Unsupported(_)), "{e:?}");
}

#[tokio::test]
async fn a_direct_sandbox_has_no_fallback() {
    let (cua, _dirs) = sdk(Fake::new(true)).await;
    let sb = cua
        .sandboxes()
        .connect_url(
            "http://127.0.0.1:9".into(),
            Some("t".into()),
            Some("cua-e2e-direct".into()),
        )
        .await
        .unwrap();
    let e = sb.guest_screenshot().await.unwrap_err();
    assert!(matches!(e, CuaError::Unsupported(_)), "{e:?}");
}
