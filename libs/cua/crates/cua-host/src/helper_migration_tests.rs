//! Moving a host from the old LaunchAgent to the Cua Spaces app's helper,
//! against the fake relay, a fake old agent and a fake helper (temp homes;
//! nothing touches launchd).

use crate::helper_migration::{LegacyService, MigrationRecord, RECORD_FILE};
use crate::preflight::SessionProbe;
use crate::service::{FakeServiceManager, LegacyAgent, ServiceManager, ServiceSpec, ServiceState};
use crate::testing::FakeRelay;
use crate::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The old LaunchAgent: a plist file in the temp home, loaded or not.
struct FakeLegacy {
    plist: PathBuf,
    loaded: Mutex<bool>,
    calls: Mutex<Vec<&'static str>>,
}

impl FakeLegacy {
    fn new(dir: &Path) -> Arc<Self> {
        let plist = dir.join("LaunchAgents/com.trycua.spacesd.host.plist");
        std::fs::create_dir_all(plist.parent().unwrap()).unwrap();
        std::fs::write(&plist, "<plist/>").unwrap();
        Arc::new(Self {
            plist,
            loaded: Mutex::new(true),
            calls: Mutex::default(),
        })
    }
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}

impl LegacyService for FakeLegacy {
    fn found(&self) -> LegacyAgent {
        let loaded = *self.loaded.lock().unwrap();
        LegacyAgent {
            plist: self.plist.is_file(),
            loaded,
            running: loaded,
        }
    }
    fn plist(&self) -> PathBuf {
        self.plist.clone()
    }
    fn bootout(&self) -> Result<()> {
        self.calls.lock().unwrap().push("bootout");
        *self.loaded.lock().unwrap() = false;
        Ok(())
    }
    fn bootstrap(&self) -> Result<()> {
        self.calls.lock().unwrap().push("bootstrap");
        assert!(self.plist.is_file(), "bootstrapped without its plist");
        *self.loaded.lock().unwrap() = true;
        Ok(())
    }
}

/// How the fake helper behaves once started.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Helper {
    /// Runs and joins the relay with a new session.
    Works,
    /// Runs but never reaches the relay.
    NeverOnline,
    /// Exits right away (launchd would keep restarting it).
    Crashes,
    /// macOS refuses to register it.
    Refused,
}

struct FakeHelper {
    mode: Helper,
    relay: FakeRelay,
    machine: Mutex<Option<String>>,
    running: Mutex<bool>,
    spec: Mutex<Option<ServiceSpec>>,
    calls: Mutex<Vec<&'static str>>,
}

impl FakeHelper {
    fn new(mode: Helper, relay: &FakeRelay) -> Arc<Self> {
        Arc::new(Self {
            mode,
            relay: relay.clone(),
            machine: Mutex::default(),
            running: Mutex::default(),
            spec: Mutex::default(),
            calls: Mutex::default(),
        })
    }
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}

impl ServiceManager for FakeHelper {
    fn kind(&self) -> RunnerKind {
        RunnerKind::AppHelper
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        self.calls.lock().unwrap().push("install");
        if self.mode == Helper::Refused {
            return Err(Error::Service("requires-approval".into()));
        }
        *self.spec.lock().unwrap() = Some(spec.clone());
        Ok(())
    }
    fn start(&self) -> Result<()> {
        self.calls.lock().unwrap().push("start");
        *self.running.lock().unwrap() = self.mode != Helper::Crashes;
        if self.mode == Helper::Works
            && let Some(id) = self.machine.lock().unwrap().as_deref()
        {
            self.relay.set_session(id, true, Some(1_800_000_000));
        }
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        self.calls.lock().unwrap().push("stop");
        *self.running.lock().unwrap() = false;
        Ok(())
    }
    fn uninstall(&self) -> Result<()> {
        self.calls.lock().unwrap().push("uninstall");
        *self.running.lock().unwrap() = false;
        *self.spec.lock().unwrap() = None;
        Ok(())
    }
    fn state(&self) -> ServiceState {
        ServiceState {
            installed: self.spec.lock().unwrap().is_some(),
            running: *self.running.lock().unwrap(),
            kind: "app-helper".into(),
            detail: String::new(),
        }
    }
}

/// A GUI session, and permissions not granted (to the new identity).
struct Probe;

impl SessionProbe for Probe {
    fn gui_session(&self, _uid: u32) -> bool {
        true
    }
    fn console_user(&self) -> Option<String> {
        Some("ada".into())
    }
    fn permission_status(&self, _driver_bin: &Path) -> Option<(bool, bool)> {
        Some((false, false))
    }
}

struct Old {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    app: PathBuf,
    relay: FakeRelay,
    machine: String,
    token: String,
    legacy: Arc<FakeLegacy>,
}

fn quick() -> VerifyTiming {
    VerifyTiming {
        timeout: Duration::from_millis(600),
        stable: Duration::from_millis(50),
        offline_grace: Duration::from_millis(150),
        poll: Duration::from_millis(20),
    }
}

/// A host set up as Cua Spaces 0.9.4 / `cua host setup` left it: relay
/// mode, the launchd runner, `bin/cua-spacesd`, the old agent loaded and
/// online at the relay.
async fn old_install() -> Old {
    let relay = FakeRelay::start().await;
    relay.add_account("acct", "user-1", Some("ada@example.com"));
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let home = root.join(".cua");
    let driver = root.join("downloaded-cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    let mut opts = SetupOptions::relay(&relay.url);
    opts.name = Some("studio".into());
    opts.driver_bin = Some(driver);
    let status = Host::new(&home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .setup(opts, &StaticToken("acct".into()))
        .await
        .unwrap();
    let machine = status.machine_id.unwrap();
    // What a launchd setup writes.
    let host = Host::new(&home);
    let mut config = host.config().unwrap().unwrap();
    config.runner = "launchd".into();
    std::fs::write(
        host.paths().config(),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    assert!(host.paths().driver_bin().is_file());
    relay.set_session(&machine, true, Some(1_700_000_000));
    let token = std::fs::read_to_string(host.paths().machine_token()).unwrap();
    let app = root.join("Applications/Cua Spaces.app");
    Old {
        legacy: FakeLegacy::new(&root),
        _dir: dir,
        root,
        home,
        app,
        relay,
        machine,
        token,
    }
}

impl Old {
    fn host(&self, helper: &Arc<FakeHelper>) -> Host {
        *helper.machine.lock().unwrap() = Some(self.machine.clone());
        Host::new(&self.home)
            .with_app_helper(self.app.clone(), helper.clone(), self.legacy.clone())
            .with_preflight_probe(Arc::new(Probe))
            .with_verify_timing(quick())
    }
    fn record(&self) -> MigrationRecord {
        serde_json::from_slice(&std::fs::read(self.home.join("host").join(RECORD_FILE)).unwrap())
            .unwrap()
    }
}

#[tokio::test]
async fn an_old_install_moves_to_the_helper_and_keeps_the_machine() {
    let old = old_install().await;
    let registrations = old.relay.register_calls();
    let helper = FakeHelper::new(Helper::Works, &old.relay);
    let host = old.host(&helper);

    let before = host.status().await.unwrap();
    assert!(before.helper_available && before.legacy_service);
    assert_eq!(before.service.kind, "launchd", "the old runner until moved");

    let m = host.migrate_service(false, "0.10.0").await.unwrap();
    assert_eq!(m.outcome, "migrated", "{m:?}");
    // macOS asks the new identity once (only macOS has the panes).
    assert_eq!(m.permissions_needed, cfg!(target_os = "macos"));

    // Same machine: id, token, policy; never registered again.
    let config = host.config().unwrap().unwrap();
    assert_eq!(config.runner, "app-helper");
    assert_eq!(
        config.driver_bin,
        old.app.join("Contents/MacOS/CuaSpacesMac")
    );
    assert_eq!(config.machine_id.as_deref(), Some(old.machine.as_str()));
    assert_eq!(
        std::fs::read_to_string(host.paths().machine_token()).unwrap(),
        old.token
    );
    assert_eq!(old.relay.register_calls(), registrations);
    assert_eq!(
        std::fs::read_to_string(&host.paths().machine_id)
            .unwrap()
            .trim(),
        old.machine
    );
    // The helper runs `join` with the same files.
    let spec = helper.spec.lock().unwrap().clone().unwrap();
    assert!(spec.args.join(" ").starts_with("join --relay"), "{spec:?}");
    assert!(
        spec.args
            .contains(&host.paths().machine_token().to_string_lossy().into_owned())
    );

    // The old agent is gone: out of launchd, plist and binary deleted.
    assert_eq!(old.legacy.calls(), ["bootout"]);
    assert!(!old.legacy.plist.exists());
    assert!(!host.paths().driver_bin().exists());
    assert!(!host.paths().dir.join("legacy-agent.plist").exists());
    assert_eq!(old.record().state, "done");

    let after = host.status().await.unwrap();
    assert_eq!(after.service.kind, "app-helper");
    assert!(after.service.running);
    assert_eq!(after.online, Some(true));
    assert!(!after.legacy_service);
    assert!(
        after
            .permissions
            .iter()
            .all(|p| p.instructions.contains("Cua Spaces")
                && !p.instructions.contains("cua-spacesd")),
        "{:?}",
        after.permissions
    );

    // Done: a second run changes nothing.
    let again = host.migrate_service(false, "0.10.0").await.unwrap();
    assert_eq!(again.outcome, "not-needed");
}

async fn rolled_back(mode: Helper) -> (Old, Arc<FakeHelper>, ServiceMigration) {
    let old = old_install().await;
    let helper = FakeHelper::new(mode, &old.relay);
    let host = old.host(&helper);
    let m = host.migrate_service(false, "0.10.0").await.unwrap();
    (old, helper, m)
}

async fn assert_old_install_intact(old: &Old) {
    let host = Host::new(&old.home);
    let config = host.config().unwrap().unwrap();
    assert_eq!(config.runner, "launchd");
    assert_eq!(config.driver_bin, host.paths().driver_bin());
    assert!(host.paths().driver_bin().is_file(), "the old binary stays");
    assert!(old.legacy.plist.is_file(), "the old plist is back");
    assert!(
        *old.legacy.loaded.lock().unwrap(),
        "the old agent runs again"
    );
    assert_eq!(old.legacy.calls(), ["bootout", "bootstrap"]);
    assert_eq!(
        std::fs::read_to_string(host.paths().machine_token()).unwrap(),
        old.token
    );
    assert_eq!(old.record().state, "rolled-back");
}

#[tokio::test]
async fn a_helper_that_never_comes_online_is_rolled_back() {
    let (old, helper, m) = rolled_back(Helper::NeverOnline).await;
    assert_eq!(m.outcome, "rolled-back", "{m:?}");
    assert!(m.detail.contains("not online at the relay"), "{}", m.detail);
    assert!(m.detail.contains("The old host service runs as before"));
    assert_eq!(helper.calls().last(), Some(&"uninstall"));
    assert_old_install_intact(&old).await;
}

#[tokio::test]
async fn a_helper_that_crashes_is_rolled_back() {
    let (old, helper, m) = rolled_back(Helper::Crashes).await;
    assert_eq!(m.outcome, "rolled-back", "{m:?}");
    assert!(m.detail.contains("did not start"), "{}", m.detail);
    assert_eq!(helper.calls().last(), Some(&"uninstall"));
    assert_old_install_intact(&old).await;
}

#[tokio::test]
async fn a_helper_macos_refuses_is_rolled_back_and_not_retried_by_itself() {
    let (old, helper, m) = rolled_back(Helper::Refused).await;
    assert_eq!(m.outcome, "rolled-back", "{m:?}");
    assert!(m.detail.contains("requires-approval"), "{}", m.detail);
    assert_eq!(helper.calls(), ["install", "uninstall"]);
    assert_old_install_intact(&old).await;

    // The same version keeps the old agent; --force (or the next version)
    // tries again.
    let helper = FakeHelper::new(Helper::Works, &old.relay);
    let host = old.host(&helper);
    let kept = host.migrate_service(false, "0.10.0").await.unwrap();
    assert_eq!(kept.outcome, "kept");
    assert!(helper.calls().is_empty());
    let moved = host.migrate_service(true, "0.10.0").await.unwrap();
    assert_eq!(moved.outcome, "migrated", "{moved:?}");
}

#[tokio::test]
async fn an_old_agent_that_was_offline_only_has_to_keep_running() {
    let old = old_install().await;
    old.relay.set_session(&old.machine, false, None);
    // Runs, but the relay never sees it (as the old one did not).
    let helper = FakeHelper::new(Helper::NeverOnline, &old.relay);
    let m = old
        .host(&helper)
        .migrate_service(false, "0.10.0")
        .await
        .unwrap();
    assert_eq!(m.outcome, "migrated", "{m:?}");
}

#[tokio::test]
async fn a_paused_host_switches_without_starting_the_helper() {
    let old = old_install().await;
    let host = Host::new(&old.home);
    let mut config = host.config().unwrap().unwrap();
    config.paused_signed_out = true;
    std::fs::write(
        host.paths().config(),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    let helper = FakeHelper::new(Helper::Works, &old.relay);
    let host = old.host(&helper);
    let m = host.migrate_service(false, "0.10.0").await.unwrap();
    assert_eq!(m.outcome, "switched", "{m:?}");
    assert!(helper.calls().is_empty(), "nothing starts while paused");
    assert_eq!(host.config().unwrap().unwrap().runner, "app-helper");
    assert!(!old.legacy.plist.exists() && !host.paths().driver_bin().exists());

    // Signing in again resumes it as the helper.
    host.resume_signed_in("user-1").await.unwrap();
    assert_eq!(helper.calls(), ["install", "start"]);
}

#[tokio::test]
async fn leftovers_of_an_interrupted_move_are_removed() {
    let old = old_install().await;
    let helper = FakeHelper::new(Helper::Works, &old.relay);
    let host = old.host(&helper);
    let mut config = host.config().unwrap().unwrap();
    config.runner = "app-helper".into();
    std::fs::write(
        host.paths().config(),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    assert!(host.status().await.unwrap().legacy_service);
    let m = host.migrate_service(false, "0.10.0").await.unwrap();
    assert_eq!(m.outcome, "cleaned-up", "{m:?}");
    assert!(!old.legacy.plist.exists() && !host.paths().driver_bin().exists());
    assert!(!host.status().await.unwrap().legacy_service);
}

#[tokio::test]
async fn setup_with_the_app_installed_uses_the_helper_and_retires_the_old_agent() {
    let old = old_install().await;
    let registrations = old.relay.register_calls();
    let helper = FakeHelper::new(Helper::Works, &old.relay);
    let host = old.host(&helper);
    // `cua host setup` again (the runner is auto): it must not write the
    // old LaunchAgent, and needs no download.
    let mut opts = SetupOptions::relay(&old.relay.url);
    opts.driver_bin = Some(old.root.join("no-such-binary"));
    let s = host.setup(opts, &StaticToken("acct".into())).await.unwrap();
    assert_eq!(s.service.kind, "app-helper");
    assert_eq!(s.machine_id.as_deref(), Some(old.machine.as_str()));
    assert_eq!(old.relay.register_calls(), registrations + 1);
    assert_eq!(helper.calls(), ["install", "start"]);
    assert_eq!(old.legacy.calls(), ["bootout"]);
    assert!(!old.legacy.plist.exists() && !host.paths().driver_bin().exists());
    assert_eq!(host.config().unwrap().unwrap().runner, "app-helper");
}

#[tokio::test]
async fn without_the_app_nothing_moves() {
    let old = old_install().await;
    // No helper found (`CUA_SPACES_APP=none` is not needed: off macOS, and
    // on a Mac without the app, nothing is found; here no helper is wired).
    let host = Host::new(&old.home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .with_preflight_probe(Arc::new(Probe));
    if crate::service::find_app_helper().is_none() {
        let m = host.migrate_service(false, "0.10.0").await.unwrap();
        assert_eq!(m.outcome, "not-needed");
        assert_eq!(host.config().unwrap().unwrap().runner, "launchd");
        assert!(host.paths().driver_bin().is_file());
    }
}

#[test]
fn removing_the_host_removes_the_helper_and_old_leftovers() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let old = old_install().await;
        let helper = FakeHelper::new(Helper::Works, &old.relay);
        let host = old.host(&helper);
        assert_eq!(
            host.migrate_service(false, "0.10.0").await.unwrap().outcome,
            "migrated"
        );
        // A plist an old app wrote again (a downgrade, then an upgrade).
        std::fs::write(&old.legacy.plist, "<plist/>").unwrap();
        host.remove().await.unwrap();
        assert_eq!(helper.calls().last(), Some(&"uninstall"));
        assert!(!old.legacy.plist.exists());
        assert!(!host.paths().dir.exists());
    });
}
