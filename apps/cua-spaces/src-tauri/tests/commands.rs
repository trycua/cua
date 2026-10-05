// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The app's command layer (`cua_spaces_lib::core::AppCore`, which every
//! Tauri command calls) against SDK fakes: `cua_spacesd_client::testing::MockServer`
//! as a Space's spacesd, `cua_fleet::testing::FakeFleet` as Fleet, and
//! a fake local runtime. Hermetic: temp `~/.cua`, no daemon, no GUI, a
//! `FakeHost` teleport sender, no real user profile or keychain.

use async_trait::async_trait;
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::Provider;
use cua_spaces_lib::core::{AppCore, CoreConfig, SpaceCreateConfig, StreamOpts, StreamTargetArg};
use cua_spacesd_client::testing::{MockAuth, MockGateway, MockServer};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// The canonical Linux image: it carries cua-spacesd, so a create waits for it.
const IMAGE: &str = "ghcr.io/trycua/linux:24.04-disk";

fn core(home: &std::path::Path) -> Arc<AppCore> {
    AppCore::new(CoreConfig::hermetic(home))
}

async fn mock_space(token: &str, features: &[&str]) -> MockServer {
    let srv = MockServer::start(MockAuth {
        token: Some(token.into()),
        ..Default::default()
    })
    .await;
    srv.state.advertise(features);
    srv
}

#[tokio::test]
async fn add_by_address_lists_screenshots_windows_and_streams_direct() {
    let home = tempfile::tempdir().unwrap();
    let app = core(home.path());
    let srv = mock_space("s3cret", &["desktop_stream", "window_stream"]).await;
    let addr = srv.addr.to_string();

    // "Add Space by address": bare host:port + token.
    let row = app
        .add_space(&addr, Some("s3cret".into()), Some("Lab box".into()))
        .await
        .unwrap();
    assert_eq!(row.id, format!("direct:{addr}"));
    assert_eq!(row.name, "Lab box");
    assert_eq!(row.provider, Provider::Direct);
    assert!(row.reachable);
    assert_eq!(row.os.as_deref(), Some("linux"));
    assert!(row.features.contains(&"desktop_stream".to_string()));

    // The registry is the shared ~/.cua file (what `cua daemon` reads).
    let reg = std::fs::read_to_string(home.path().join("spaces.json")).unwrap();
    assert!(reg.contains(&row.id));
    assert!(!reg.contains("s3cret"), "no secrets in spaces.json");

    let rows = app.list_spaces().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].reachable);
    let one = app.space_info(&row.id).await.unwrap();
    assert_eq!(one, rows[0]);

    let shot = app.space_screenshot(&row.id, Some(640)).await.unwrap();
    assert!(shot.starts_with("data:image/"), "{shot}");
    assert_eq!(
        srv.state.observed.screenshots.lock().unwrap()[0].max_dimension,
        640
    );

    let windows = app.list_remote_windows(&row.id).await.unwrap();
    assert!(!windows.is_empty());
    assert!(windows
        .iter()
        .all(|w| !w.id.is_empty() && !w.app_id.is_empty()));

    // A direct Space's ticket URL goes straight to its spacesd.
    let t = app
        .open_stream(
            &row.id,
            StreamTargetArg::Display { display_id: None },
            StreamOpts::default(),
        )
        .await
        .unwrap();
    assert_eq!(t.via, "direct");
    assert_eq!(t.ws_url, format!("ws://{addr}/media?ticket=ticket-abc"));
    assert_eq!(t.wire_version, 2);
    assert!(!t.audio);
    app.close_stream(&row.id, &t.media_session_id)
        .await
        .unwrap();

    // Legacy spellings and names resolve too.
    app.remove_space("Lab box").await.unwrap();
    assert!(app.list_spaces().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_wrong_token_is_refused_and_nothing_is_registered() {
    let home = tempfile::tempdir().unwrap();
    let app = core(home.path());
    let srv = mock_space("right", &[]).await;
    let e = app
        .add_space(&srv.url(), Some("wrong".into()), None)
        .await
        .unwrap_err();
    assert!(e.to_lowercase().contains("unauth"), "{e}");
    assert!(app.list_spaces().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_space_without_desktop_stream_refuses_the_desktop_stream() {
    let home = tempfile::tempdir().unwrap();
    let app = core(home.path());
    let srv = mock_space("t", &[]).await;
    let row = app
        .add_space(&srv.url(), Some("t".into()), None)
        .await
        .unwrap();
    let e = app
        .open_stream(
            &row.id,
            StreamTargetArg::Display { display_id: None },
            StreamOpts::default(),
        )
        .await
        .unwrap_err();
    assert!(e.contains("desktop_stream"), "{e}");
}

#[tokio::test]
async fn stream_options_map_to_the_open_media_request() {
    let home = tempfile::tempdir().unwrap();
    let app = core(home.path());
    let srv = mock_space("t", &["window_stream", "audio.desktop"]).await;
    let row = app
        .add_space(&srv.url(), Some("t".into()), None)
        .await
        .unwrap();
    let t = app
        .open_stream(
            &row.id,
            StreamTargetArg::Window {
                window_id: "w-1".into(),
            },
            StreamOpts {
                max_fps: Some(15),
                audio: Some(true),
                codecs: Some(vec!["h264".into(), "png".into()]),
                policy: Some("view_only".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(t.audio);
    // Unknown names are refused before any request.
    let e = app
        .open_stream(
            &row.id,
            StreamTargetArg::Display { display_id: None },
            StreamOpts {
                codecs: Some(vec!["vp9".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(e.contains("desktop_stream") || e.contains("vp9"), "{e}");
}

#[tokio::test]
async fn an_unreachable_space_is_listed_within_the_probe_bound() {
    let home = tempfile::tempdir().unwrap();
    let app = core(home.path());
    let srv = mock_space("t", &[]).await;
    let row = app
        .add_space(&srv.url(), Some("t".into()), None)
        .await
        .unwrap();
    drop(srv);
    // A fresh core (no cached connection), as after an app restart.
    let app = core(home.path());
    let started = Instant::now();
    let rows = app.list_spaces().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, row.id);
    assert!(!rows[0].reachable);
    assert!(rows[0].error.is_some());
}

#[tokio::test]
async fn cloud_spaces_bind_through_the_gateway_and_delete_removes_the_sandbox() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    // The pool name is derived from namespace + runtime + image; compute it
    // with a probe core, then emulate the gateway for that sandbox.
    let probe = {
        let mut c = CoreConfig::hermetic(home.path());
        c.fleet_client = Some(fake.client());
        AppCore::new(c)
    };
    let pool = probe
        .spaces()
        .fleet_pool_name(cua_spaces::contract::inputs::FleetRuntime::Kubevirt, IMAGE);
    let srv = MockServer::start(MockAuth {
        token: None,
        gateway: Some(MockGateway {
            prefix: format!("/api/svc/{pool}/sbx-cua-e2e-app-claim-env"),
            bearer: "fake-fleet-token".into(),
            claim: "cua-e2e-app-claim".into(),
        }),
        prefix: None,
    })
    .await;
    let mut c = CoreConfig::hermetic(home.path());
    c.fleet_client = Some(fake.client_with_base(&srv.url()));
    let app = AppCore::new(c);

    let row = app
        .create_space(SpaceCreateConfig {
            on: Some("cloud".into()),
            image: Some(IMAGE.into()),
            runtime: Some("kubevirt".into()),
            name: Some("cua-e2e-app-claim".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(row.id, "cloud:cua-e2e-app-claim");
    assert_eq!(row.provider, Provider::Cloud);
    assert!(row.reachable);

    // Fleet media needs gateway headers: without a daemon the app says so
    // instead of handing the webview a URL it cannot open.
    srv.state.advertise(&["desktop_stream"]);
    let app2 = {
        let mut c = CoreConfig::hermetic(home.path());
        c.fleet_client = Some(fake.client_with_base(&srv.url()));
        AppCore::new(c)
    };
    let e = app2
        .open_stream(
            &row.id,
            StreamTargetArg::Display { display_id: None },
            StreamOpts::default(),
        )
        .await
        .unwrap_err();
    assert!(e.contains("daemon"), "{e}");

    let msg = app.delete_space(&row.id).await.unwrap();
    assert!(msg.to_lowercase().contains("deleted"), "{msg}");
    assert!(!fake.exists("claim", &pool, "cua-e2e-app-claim"));
    assert!(app.list_spaces().await.unwrap().is_empty());
}

#[tokio::test]
async fn runtime_image_pairing_errors_come_from_the_sdk() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let mut c = CoreConfig::hermetic(home.path());
    c.fleet_client = Some(fake.client());
    let app = AppCore::new(c);
    // gVisor cannot run a containerDisk: the SDK's one check, from the
    // image's manifest (here a fixture), not its tag.
    cua_fleet::testing::set_image_variant(IMAGE, cua_fleet::ImageVariant::ContainerDisk);
    let e = app
        .create_space(SpaceCreateConfig {
            on: Some("cloud".into()),
            image: Some(IMAGE.into()),
            runtime: Some("gvisor".into()),
            name: None,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.contains("containerDisk"), "{e}");
    assert!(fake.requests().is_empty(), "refused before any request");
    let e = app
        .create_space(SpaceCreateConfig {
            on: Some("cloud".into()),
            runtime: Some("firecracker".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.contains("firecracker"), "{e}");
    // A runtime the cloud does not offer lists the ones it does.
    let e = app
        .create_space(SpaceCreateConfig {
            on: Some("cloud".into()),
            runtime: Some("qemu".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.contains("kubevirt") || e.contains("gvisor"), "{e}");
    // A kind that contradicts the runtime is refused locally too.
    let e = app
        .create_space(SpaceCreateConfig {
            on: Some("local".into()),
            kind: Some("container".into()),
            runtime: Some("qemu".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.contains("qemu"), "{e}");
    assert!(fake.requests().is_empty(), "refused before any request");
}

#[tokio::test]
async fn without_cloud_auth_creates_say_how_to_sign_in() {
    let home = tempfile::tempdir().unwrap();
    let app = core(home.path());
    let status = app.fleet_status(false).await;
    assert!(!status.configured);
    let e = app
        .create_space(SpaceCreateConfig {
            on: Some("cloud".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.contains("sign in") || e.contains("CUA_CLIENT_ID"), "{e}");
    assert!(!e.to_lowercase().contains("claim"), "{e}");
}

#[tokio::test]
async fn default_location_is_stored_in_the_cua_home_config() {
    let home = tempfile::tempdir().unwrap();
    let app = core(home.path());
    let env_override = std::env::var("CUA_DEFAULT_ON").is_ok_and(|v| !v.trim().is_empty());

    let before = app.default_location().unwrap();
    assert_eq!(
        before.path,
        home.path().join("config.toml").display().to_string()
    );
    if !env_override {
        assert_eq!(
            (before.value.as_str(), before.source.as_str()),
            ("local", "default")
        );
        assert_eq!(before.env, None);
    }

    let after = app.set_default_location("cloud").unwrap();
    let toml = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert!(
        toml.contains("[default]") && toml.contains("on = \"cloud\""),
        "{toml}"
    );
    if !env_override {
        assert_eq!(
            (after.value.as_str(), after.source.as_str()),
            ("cloud", "config")
        );
        // No location given: the create goes to the default (the cloud,
        // which is not signed in here).
        let e = app
            .create_space(SpaceCreateConfig::default())
            .await
            .unwrap_err();
        assert!(e.contains("sign in") || e.contains("CUA_CLIENT_ID"), "{e}");
    }

    // An existing machine or an engine word is not a location.
    let e = app
        .set_default_location("direct:127.0.0.1:3211")
        .unwrap_err();
    assert!(e.contains("one machine"), "{e}");
    let e = app.set_default_location("fleet").unwrap_err();
    assert!(e.contains("cloud"), "{e}");
    assert_eq!(app.set_default_location("local").unwrap().value, "local");
}

/// A local runtime whose one "instance" is a MockServer on loopback.
struct FakeRuntime {
    port: u16,
    started: Mutex<Vec<LocalStartSpec>>,
    deleted: Mutex<Vec<String>>,
    running: Mutex<BTreeMap<String, bool>>,
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.started.lock().unwrap().push(spec.clone());
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
        self.deleted.lock().unwrap().push(name.into());
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
async fn local_container_spaces_are_created_with_a_fresh_token_and_deleted() {
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: srv.addr.port(),
        started: Mutex::new(vec![]),
        deleted: Mutex::new(vec![]),
        running: Mutex::new(BTreeMap::new()),
    });
    let home = tempfile::tempdir().unwrap();
    let mut c = CoreConfig::hermetic(home.path());
    c.local_runtime = Some(rt.clone());
    let app = AppCore::new(c);

    let row = app
        .create_space(SpaceCreateConfig {
            on: Some("local".into()),
            kind: Some("container".into()),
            name: Some("cua-e2e-local-app".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(row.id, "local:cua-e2e-local-app");
    assert_eq!(row.provider, Provider::Local);
    let spec = rt.started.lock().unwrap()[0].clone();
    assert_eq!(
        spec.image, "cua-e2e-local/linux:docker-local-arm64",
        "the configured Spaces container image"
    );
    assert_eq!(spec.env.get("CUA_ENV_TOKEN").map(String::len), Some(32));

    // macOS needs a configured Lume image; the error says how.
    let e = app
        .create_space(SpaceCreateConfig {
            on: Some("local".into()),
            runtime: Some("lume".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.contains("CUA_SPACES_MACOS_IMAGE"), "{e}");

    app.delete_space(&row.id).await.unwrap();
    assert_eq!(rt.deleted.lock().unwrap().as_slice(), ["cua-e2e-local-app"]);
}

#[tokio::test]
async fn teleport_consent_is_enforced_by_the_sdk_approval() {
    use cua_spaces_ext::teleport::providers::{ExportRegistry, FakeHost, FirefoxProvider};
    use cua_spaces_ext::teleport::AppSessions;

    let home = tempfile::tempdir().unwrap();
    // A generated Firefox profile under a temp HOME, read through FakeHost.
    let profile = home.path().join("fixture-profile");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("prefs.js"), "user_pref(\"x\", 1);\n").unwrap();
    std::fs::write(profile.join("cookies.sqlite"), b"synthetic cookies").unwrap();
    std::fs::write(profile.join("places.sqlite"), b"synthetic history").unwrap();
    let host = Arc::new(FakeHost::new().with_home(home.path()));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(
        FirefoxProvider::new()
            .with_host(host)
            .with_profile_dir(&profile),
    ));
    let mut c = CoreConfig::hermetic(home.path());
    c.app_sessions = Arc::new(AppSessions::from_registry(registry));
    let app = AppCore::new(c);

    let m = app.teleport_manifest("firefox", "full").await.unwrap();
    assert_eq!(m.provider_id, "firefox");
    assert_eq!(m.scope, "full_profile");
    assert!(m.supports_hotspot);
    let sensitive: Vec<String> = m
        .items
        .iter()
        .filter(|i| i.sensitive)
        .map(|i| i.rel_path.clone())
        .collect();
    assert!(!sensitive.is_empty(), "cookies are sensitive: {m:?}");

    let srv = mock_space("t", &["teleport.firefox"]).await;
    let row = app
        .add_space(&srv.url(), Some("t".into()), None)
        .await
        .unwrap();
    // Sensitive items without the consent sheet's acknowledgement: refused.
    let e = app
        .teleport_push("firefox", "full", &row.id, &sensitive, false)
        .await
        .unwrap_err();
    assert!(e.contains("sensitive"), "{e}");
    // A path the manifest never offered: refused.
    let e = app
        .teleport_push(
            "firefox",
            "full",
            &row.id,
            &["../../etc/passwd".into()],
            true,
        )
        .await
        .unwrap_err();
    assert!(e.contains("not in this manifest"), "{e}");
}

#[tokio::test]
async fn hotspot_and_daemon_state_are_honest_when_idle() {
    let home = tempfile::tempdir().unwrap();
    let app = core(home.path());
    let h = app.hotspot_status();
    assert!(!h.active && h.space_id.is_none());
    let h = app.stop_hotspot().await.unwrap();
    assert!(!h.active);
    let d = app.daemon_status(false).await;
    assert!(!d.connected);
    assert!(d.error.unwrap().contains("disabled"));
    let l = app.local_status().await;
    assert!(!l.available);
}

/// A local runtime whose instances download forever (the create a person
/// cancels): `start` reports some bytes, then never returns.
#[derive(Default)]
struct HangingRuntime {
    running: Mutex<BTreeMap<String, bool>>,
    deleted: Mutex<Vec<String>>,
    specs: Mutex<Vec<LocalStartSpec>>,
}

#[async_trait]
impl LocalRuntime for HangingRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        use cua_sandbox_core::progress::{report, Phase, Progress, Transfer};
        self.specs.lock().unwrap().push(spec.clone());
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        report(Progress::phase(Phase::Pulling).bytes(Transfer {
            done: 4_200_000_000,
            total: 23_900_000_000,
            per_second: Some(85e6),
        }));
        std::future::pending::<()>().await;
        unreachable!()
    }
    async fn stop(&self, _name: &str) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().get(name) {
            Some(_) => Ok(InstanceStatus::Running),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.deleted.lock().unwrap().push(name.into());
        match self.running.lock().unwrap().remove(name) {
            Some(_) => Ok(()),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn endpoints(&self, _name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(LocalEndpoints::default())
    }
    async fn gpu_support(&self) -> Vec<cua_sandbox_core::gpu::GpuSupport> {
        use cua_sandbox_core::gpu::{GpuOption, GpuSupport};
        vec![
            GpuSupport {
                runtime: "lume".into(),
                options: vec![
                    GpuOption {
                        id: "paravirtual".into(),
                        label: "GPU acceleration".into(),
                        experimental: true,
                        supported: false,
                        reason: "Needs macOS 15 or later".into(),
                        learn_more: Some("https://cua.ai/docs/lume/guides/gpu-passthrough".into()),
                        usd_per_hour: None,
                    },
                    GpuOption {
                        id: "second".into(),
                        label: "Not offered".into(),
                        experimental: false,
                        supported: true,
                        reason: String::new(),
                        learn_more: None,
                        usd_per_hour: None,
                    },
                ],
                reason: String::new(),
            },
            GpuSupport::none("container", "Docker on macOS has no GPU"),
        ]
    }
}

#[tokio::test]
async fn a_create_reports_bytes_and_is_cancelled_by_its_pending_id() {
    use cua_spaces_lib::commands::CreateProgressPayload;
    let home = tempfile::tempdir().unwrap();
    let rt = Arc::new(HangingRuntime::default());
    let mut c = CoreConfig::hermetic(home.path());
    c.local_runtime = Some(rt.clone());
    let app = AppCore::new(c);

    // The `spaces:create-progress` events the webview gets.
    let heard: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let h = heard.clone();
    let sink = cua_spaces::ProgressSink::new(move |p| {
        let payload = CreateProgressPayload::new("pending:ux", p);
        h.lock()
            .unwrap()
            .push(serde_json::to_value(payload).unwrap());
    });
    let a = app.clone();
    let create = tokio::spawn(async move {
        a.create_space_with_progress(
            SpaceCreateConfig {
                on: Some("local".into()),
                kind: Some("container".into()),
                name: Some("cua-e2e-cancel".into()),
                gpu: Some("paravirtual".into()),
                ..Default::default()
            },
            Some(sink),
            Some("pending:ux".into()),
        )
        .await
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while rt.running.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the create never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        rt.specs.lock().unwrap()[0].gpu.as_deref(),
        Some("paravirtual"),
        "the wizard's GPU option reaches the runtime"
    );
    {
        let heard = heard.lock().unwrap();
        let bytes = heard
            .iter()
            .find(|p| p["bytesDone"].is_u64())
            .unwrap_or_else(|| panic!("no bytes reported: {heard:?}"));
        assert_eq!(bytes["pendingId"], "pending:ux");
        assert_eq!(bytes["phase"], "pulling");
        assert_eq!(bytes["bytesDone"], 4_200_000_000u64);
        assert_eq!(bytes["bytesTotal"], 23_900_000_000u64);
        assert!(bytes["bytesPerSecond"].as_f64().unwrap() > 0.0);
        assert!(
            heard.iter().all(|p| p["space"] == "local:cua-e2e-cancel"),
            "{heard:?}"
        );
    }

    // Cancel by the pending id the webview knows.
    let outcome = app.cancel_create("pending:ux").await.unwrap();
    assert_eq!(
        outcome.state,
        cua_spaces::CancelState::Cancelled,
        "{outcome:?}"
    );
    assert_eq!(outcome.id, "local:cua-e2e-cancel");
    let e = create.await.unwrap().unwrap_err();
    assert!(
        e.starts_with(cua_spaces_lib::core::CANCELLED_PREFIX),
        "the webview tells a cancel from a failure: {e}"
    );
    assert_eq!(rt.deleted.lock().unwrap().as_slice(), ["cua-e2e-cancel"]);
    assert!(app.list_spaces().await.unwrap().is_empty());
    // Again: nothing left to do.
    let again = app.cancel_create("pending:ux").await.unwrap();
    assert_eq!(again.state, cua_spaces::CancelState::NotCreating);
}

#[tokio::test]
async fn the_wizard_gets_the_first_gpu_option_of_each_runtime_that_has_one() {
    let home = tempfile::tempdir().unwrap();
    let mut c = CoreConfig::hermetic(home.path());
    c.local_runtime = Some(Arc::new(HangingRuntime::default()));
    let app = AppCore::new(c);
    let gpus = app.gpu_support().await;
    assert_eq!(
        serde_json::to_value(&gpus).unwrap(),
        serde_json::json!([{
            "runtime": "lume",
            "id": "paravirtual",
            "label": "GPU acceleration",
            "experimental": true,
            "supported": false,
            "reason": "Needs macOS 15 or later",
            "learnMore": "https://cua.ai/docs/lume/guides/gpu-passthrough",
        }])
    );
}
