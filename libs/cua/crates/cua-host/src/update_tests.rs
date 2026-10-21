//! `Host::update_driver` against fake drivers (shell scripts that answer
//! `--version`) and a fake service manager: nothing is installed.

#![cfg(unix)]

use crate::service::FakeServiceManager;
use crate::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    installed: PathBuf,
    manager: Arc<FakeServiceManager>,
}

fn script(path: &Path, version: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::write(path, format!("#!/bin/sh\necho \"cua-spacesd {version}\"\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture(version: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".cua");
    let installed = home.join("host").join("cua-spacesd");
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    script(&installed, version);
    Fixture {
        home,
        installed,
        manager: Arc::new(FakeServiceManager::default()),
        _dir: dir,
    }
}

impl Fixture {
    fn host(&self) -> Host {
        Host::new(&self.home).with_service_manager(self.manager.clone())
    }

    fn write_config(&self, share_desktop: bool, provide_spaces: bool, sharing: bool) {
        let config = HostConfig {
            mode: "direct".into(),
            listen: Some("127.0.0.1:3211".into()),
            name: "mini".into(),
            runner: "process".into(),
            driver_bin: self.installed.clone(),
            sharing,
            share_desktop,
            provide_spaces,
            ..HostConfig::default()
        };
        let path = self.host().paths().config();
        std::fs::write(path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    }

    fn new_binary(&self, version: &str) -> PathBuf {
        let path = self.home.join("new-cua-spacesd");
        script(&path, version);
        path
    }

    fn running(&self) {
        let host = self.host();
        let config = host.config().unwrap().unwrap();
        self.manager.install(&host.service_spec(&config)).unwrap();
        self.manager.start().unwrap();
        self.manager.calls.lock().unwrap().clear();
    }

    fn calls(&self) -> Vec<String> {
        self.manager.calls.lock().unwrap().clone()
    }

    fn leftovers(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.installed.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != "cua-spacesd" && n != "config.json")
            .collect();
        names.sort();
        names
    }
}

fn opts(bin: PathBuf, check: bool) -> DriverUpdateOptions {
    DriverUpdateOptions {
        driver_bin: Some(bin),
        check,
    }
}

#[tokio::test]
async fn a_computer_that_was_never_set_up_is_an_error() {
    let f = fixture("0.1.0");
    let new = f.new_binary("0.2.0");
    let e = f.host().update_driver(opts(new, false)).await.unwrap_err();
    assert!(matches!(e, Error::NotFound(_)), "{e}");
    assert!(e.to_string().contains("cua host setup"), "{e}");
    assert!(f.calls().is_empty());
}

#[tokio::test]
async fn an_installed_version_that_matches_is_left_alone() {
    let f = fixture("0.2.0");
    f.write_config(true, false, true);
    f.running();
    let new = f.new_binary("0.2.0");
    let before = std::fs::read(&f.installed).unwrap();
    let got = f.host().update_driver(opts(new, false)).await.unwrap();
    assert_eq!(
        got,
        DriverUpdate::Current {
            version: "0.2.0".into()
        }
    );
    assert!(f.calls().is_empty());
    assert_eq!(std::fs::read(&f.installed).unwrap(), before);
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
}

#[tokio::test]
async fn check_reports_and_changes_nothing() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    f.running();
    let new = f.new_binary("0.2.0");
    let before = std::fs::read(&f.installed).unwrap();
    let config_before = std::fs::read(f.host().paths().config()).unwrap();
    let got = f.host().update_driver(opts(new, true)).await.unwrap();
    assert_eq!(
        got,
        DriverUpdate::Available {
            installed: Some("0.1.0".into()),
            latest: "0.2.0".into()
        }
    );
    assert!(f.calls().is_empty());
    assert_eq!(std::fs::read(&f.installed).unwrap(), before);
    assert_eq!(
        std::fs::read(f.host().paths().config()).unwrap(),
        config_before
    );
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
}

#[tokio::test]
async fn a_running_service_is_stopped_swapped_and_started_again() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    f.running();
    let new = f.new_binary("0.2.0");
    let got = f.host().update_driver(opts(new, false)).await.unwrap();
    assert_eq!(
        got,
        DriverUpdate::Updated {
            from: Some("0.1.0".into()),
            to: "0.2.0".into(),
            restarted: true
        }
    );
    assert_eq!(f.calls(), ["stop", "start"]);
    assert!(
        std::fs::read_to_string(&f.installed)
            .unwrap()
            .contains("0.2.0")
    );
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
    assert!(f.manager.state().running);
}

#[tokio::test]
async fn a_service_that_was_not_running_stays_stopped() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    let new = f.new_binary("0.2.0");
    let got = f.host().update_driver(opts(new, false)).await.unwrap();
    assert_eq!(
        got,
        DriverUpdate::Updated {
            from: Some("0.1.0".into()),
            to: "0.2.0".into(),
            restarted: false
        }
    );
    assert!(f.calls().is_empty());
}

#[tokio::test]
async fn update_works_with_both_sharing_options_off_and_touches_no_settings() {
    let f = fixture("0.1.0");
    f.write_config(false, false, false);
    f.running();
    let before = f.host().config().unwrap().unwrap();
    let new = f.new_binary("0.2.0");
    f.host().update_driver(opts(new, false)).await.unwrap();
    assert_eq!(f.host().config().unwrap().unwrap(), before);
    assert!(f.host().policy().unwrap().is_none());
    assert!(!f.host().paths().machine_token().exists());
    assert_eq!(f.calls(), ["stop", "start"]);
}

struct FailsToStart(FakeServiceManager);

impl ServiceManager for FailsToStart {
    fn kind(&self) -> RunnerKind {
        self.0.kind()
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        self.0.install(spec)
    }
    fn start(&self) -> Result<()> {
        let first = !self.0.calls.lock().unwrap().iter().any(|c| c == "start");
        self.0.start()?;
        if first {
            Err(Error::Service("fixture start refused".into()))
        } else {
            Ok(())
        }
    }
    fn stop(&self) -> Result<()> {
        self.0.stop()
    }
    fn uninstall(&self) -> Result<()> {
        self.0.uninstall()
    }
    fn state(&self) -> ServiceState {
        ServiceState {
            running: true,
            ..self.0.state()
        }
    }
}

#[tokio::test]
async fn a_failed_start_puts_the_previous_binary_back() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    let manager = Arc::new(FailsToStart(FakeServiceManager::default()));
    let host = Host::new(&f.home).with_service_manager(manager.clone());
    let new = f.new_binary("0.2.0");
    let e = host.update_driver(opts(new, false)).await.unwrap_err();
    assert!(matches!(e, Error::Service(_)), "{e}");
    let text = e.to_string();
    assert!(text.contains("fixture start refused"), "{text}");
    assert!(text.contains("restored"), "{text}");
    assert!(
        std::fs::read_to_string(&f.installed)
            .unwrap()
            .contains("0.1.0")
    );
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
}

#[tokio::test]
async fn a_missing_local_binary_is_refused_before_anything_changes() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    f.running();
    let e = f
        .host()
        .update_driver(opts(f.home.join("missing"), false))
        .await
        .unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    assert!(f.calls().is_empty());
}
