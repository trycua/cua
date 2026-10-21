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

type Hook = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Odd {
    inner: FakeServiceManager,
    fail_stop: bool,
    fail_first_start: bool,
    down_after_start: bool,
    on_start: Option<Hook>,
}

impl Odd {
    fn running() -> Self {
        let odd = Self::default();
        odd.inner.start().unwrap();
        odd.inner.calls.lock().unwrap().clear();
        odd
    }
    fn calls(&self) -> Vec<String> {
        self.inner.calls.lock().unwrap().clone()
    }
}

impl ServiceManager for Odd {
    fn kind(&self) -> RunnerKind {
        self.inner.kind()
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        self.inner.install(spec)
    }
    fn start(&self) -> Result<()> {
        let first = !self.calls().iter().any(|c| c == "start");
        if let Some(hook) = &self.on_start {
            hook();
        }
        if self.down_after_start && first {
            self.inner.calls.lock().unwrap().push("start".into());
            return Ok(());
        }
        self.inner.start()?;
        if self.fail_first_start && first {
            return Err(Error::Service("fixture start refused".into()));
        }
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        if self.fail_stop {
            self.inner.calls.lock().unwrap().push("stop".into());
            return Err(Error::Service("fixture stop refused".into()));
        }
        self.inner.stop()
    }
    fn uninstall(&self) -> Result<()> {
        self.inner.uninstall()
    }
    fn state(&self) -> ServiceState {
        let mut s = self.inner.state();
        if self.down_after_start && self.calls().iter().any(|c| c == "start") {
            s.running = false;
        }
        s
    }
}

fn with(f: &Fixture, odd: Odd) -> (Host, Arc<Odd>) {
    let odd = Arc::new(odd);
    (Host::new(&f.home).with_service_manager(odd.clone()), odd)
}

fn version_of(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[tokio::test]
async fn a_failed_start_puts_the_previous_binary_back() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    let (host, odd) = with(
        &f,
        Odd {
            fail_first_start: true,
            ..Odd::running()
        },
    );
    let new = f.new_binary("0.2.0");
    let e = host.update_driver(opts(new, false)).await.unwrap_err();
    let text = e.to_string();
    assert!(text.contains("fixture start refused"), "{text}");
    assert!(text.contains("restored and restarted"), "{text}");
    assert!(version_of(&f.installed).contains("0.1.0"));
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
    assert_eq!(odd.calls(), ["stop", "start", "stop", "start"]);
}

#[tokio::test]
async fn a_service_that_is_down_after_start_is_rolled_back() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    let (host, _) = with(
        &f,
        Odd {
            down_after_start: true,
            ..Odd::running()
        },
    );
    let new = f.new_binary("0.2.0");
    let e = host.update_driver(opts(new, false)).await.unwrap_err();
    let text = e.to_string();
    assert!(text.contains("is not running"), "{text}");
    assert!(version_of(&f.installed).contains("0.1.0"));
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
}

#[tokio::test]
async fn a_failed_stop_changes_nothing() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    let (host, odd) = with(
        &f,
        Odd {
            fail_stop: true,
            ..Odd::running()
        },
    );
    let new = f.new_binary("0.2.0");
    let e = host.update_driver(opts(new, false)).await.unwrap_err();
    assert!(e.to_string().contains("fixture stop refused"), "{e}");
    assert!(version_of(&f.installed).contains("0.1.0"));
    assert_eq!(odd.calls(), ["stop"]);
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
}

#[tokio::test]
async fn a_swap_that_cannot_start_restarts_the_old_binary() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    let blocker = f.installed.with_file_name("cua-spacesd-previous");
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(blocker.join("keep"), "x").unwrap();
    let (host, odd) = with(&f, Odd::running());
    let new = f.new_binary("0.2.0");
    let e = host.update_driver(opts(new, false)).await.unwrap_err();
    let text = e.to_string();
    assert!(text.contains("could not move"), "{text}");
    assert!(text.contains("restored and restarted"), "{text}");
    assert!(version_of(&f.installed).contains("0.1.0"));
    assert_eq!(odd.calls(), ["stop", "start"]);
    assert_eq!(f.leftovers(), ["cua-spacesd-previous"]);
}

#[tokio::test]
async fn a_restore_that_fails_says_where_the_old_binary_is() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    let dest = f.installed.clone();
    let hook: Hook = Box::new(move || {
        let _ = std::fs::remove_file(&dest);
        std::fs::create_dir_all(dest.join("blocked")).unwrap();
    });
    let (host, odd) = with(
        &f,
        Odd {
            fail_first_start: true,
            on_start: Some(hook),
            ..Odd::running()
        },
    );
    let new = f.new_binary("0.2.0");
    let e = host.update_driver(opts(new, false)).await.unwrap_err();
    let text = e.to_string();
    assert!(text.contains("could not be put back"), "{text}");
    assert!(text.contains("cua-spacesd-previous"), "{text}");
    assert!(text.contains("is stopped"), "{text}");
    assert!(!text.contains("was restored"), "{text}");
    let backup = f.installed.with_file_name("cua-spacesd-previous");
    assert!(version_of(&backup).contains("0.1.0"));
    assert_eq!(odd.calls(), ["stop", "start", "stop"]);
}

#[tokio::test]
async fn stale_leftovers_from_a_crashed_run_do_not_matter() {
    let f = fixture("0.1.0");
    f.write_config(true, false, true);
    f.running();
    std::fs::write(f.installed.with_file_name("cua-spacesd-update"), "stale").unwrap();
    std::fs::write(f.installed.with_file_name("cua-spacesd-previous"), "stale").unwrap();
    let new = f.new_binary("0.2.0");
    f.host().update_driver(opts(new, false)).await.unwrap();
    assert!(version_of(&f.installed).contains("0.2.0"));
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
}

#[tokio::test]
async fn build_metadata_does_not_make_a_current_install_look_old() {
    let f = fixture("0.2.0");
    std::fs::write(
        &f.installed,
        "#!/bin/sh\necho \"cua-spacesd 0.2.0 (build 12)\"\n",
    )
    .unwrap();
    f.write_config(true, false, true);
    let new = f.new_binary("0.2.0+abc");
    let got = f.host().update_driver(opts(new, true)).await.unwrap();
    assert_eq!(
        got,
        DriverUpdate::Current {
            version: "0.2.0".into()
        }
    );
}

#[tokio::test]
async fn an_installed_version_that_cannot_be_read_is_not_reported_as_old() {
    let f = fixture("0.1.0");
    std::fs::write(&f.installed, "#!/bin/sh\necho garbage\n").unwrap();
    f.write_config(true, false, true);
    let new = f.new_binary("0.2.0");
    let e = f
        .host()
        .update_driver(opts(new.clone(), true))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("did not report one"), "{e}");
    assert!(f.leftovers().is_empty(), "{:?}", f.leftovers());
    let got = f.host().update_driver(opts(new, false)).await.unwrap();
    assert!(
        matches!(got, DriverUpdate::Updated { from: None, .. }),
        "{got:?}"
    );
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
