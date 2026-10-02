// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua keyvault status|init|unlock|lock` against a real broker served in
//! this test process on `$CUA_HOME/keyvault.sock` in a temp home, with a
//! fake presence gate (no Touch ID) and no OS key store (nothing reaches the
//! login keychain). The broker treats exactly this build of `cua` (by its
//! cdhash) as first party, the way a debug daemon does with
//! `CUA_KEYVAULT_TEST_REQUIREMENT`.
#![cfg(target_os = "macos")]

mod common;

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use common::{Home, Out};
use cua_keyvault::broker::{
    Backend, BrokerConfig, Captured, DeliveryOutcome, FakePresence, ImportSpec, Inventory,
};
use cua_keyvault::model::PayloadEntry;
use cua_keyvault::record::{self, CookieRecord};
use cua_keyvault::{Broker, TrustPolicy};
use tokio::io::AsyncWriteExt;

struct NoBackend;

#[async_trait::async_trait]
impl Backend for NoBackend {
    fn inventory(&self, _: &str, _: Option<&str>) -> cua_keyvault::Result<Inventory> {
        Err(cua_keyvault::Error::Unsupported("test".into()))
    }
    fn capture(&self, _: &ImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
        Err(cua_keyvault::Error::Unsupported("test".into()))
    }
    async fn deliver(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Vec<PayloadEntry>,
        _: u64,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        Err(cua_keyvault::Error::Unsupported("test".into()))
    }
    async fn wipe(&self, _: &str, _: &str) -> cua_keyvault::Result<Vec<String>> {
        Ok(vec![])
    }
}

/// Captures one item per site in the spec, plus a whole-app session item
/// when `whole_app`; records every spec it saw.
#[derive(Default)]
struct FakeImportBackend {
    specs: std::sync::Mutex<Vec<ImportSpec>>,
}

fn fake_captured(app: &str, site: Option<&str>) -> Captured {
    let n = match site {
        // A site is one cookie; the whole app is one file.
        Some(s) => record::cookie_record(&CookieRecord {
            creation_utc: None,
            expires_utc: 0,
            host_key: format!(".{s}"),
            http_only: true,
            last_update_utc: None,
            name: "session".into(),
            partition_key: None,
            last_access_utc: None,
            source_type: None,
            has_cross_site_ancestor: None,
            path: "/".into(),
            priority: None,
            same_site: 1,
            secure: true,
            source_port: None,
            source_scheme: None,
            value: b"FIXTURE-SECRET".to_vec(),
        }),
        None => record::file_record("session/app", 0o600, b"FIXTURE-SECRET"),
    }
    .unwrap();
    let (meta, payload) = n.into_item(app, app, "Default", "full");
    Captured { meta, payload }
}

#[async_trait::async_trait]
impl Backend for FakeImportBackend {
    fn inventory(&self, _: &str, _: Option<&str>) -> cua_keyvault::Result<Inventory> {
        Err(cua_keyvault::Error::Unsupported("test".into()))
    }
    fn capture(&self, spec: &ImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
        self.specs.lock().unwrap().push(spec.clone());
        let mut out: Vec<Captured> = spec
            .sites
            .iter()
            .map(|s| fake_captured(&spec.app, Some(&s.site)))
            .collect();
        if spec.whole_app {
            out.push(fake_captured(&spec.app, None));
        }
        Ok(out)
    }
    async fn deliver(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Vec<PayloadEntry>,
        _: u64,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        Err(cua_keyvault::Error::Unsupported("test".into()))
    }
    async fn wipe(&self, _: &str, _: &str) -> cua_keyvault::Result<Vec<String>> {
        Ok(vec![])
    }
}

fn cdhash(bin: &str) -> String {
    let o = std::process::Command::new("codesign")
        .args(["-dvvv", bin])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stderr)
        .lines()
        .find_map(|l| l.strip_prefix("CDHash=").map(str::to_string))
        .expect("the test cua is signed (linker ad hoc signature)")
}

struct Rig {
    home: Home,
    presence: Arc<FakePresence>,
    _server: tokio::task::JoinHandle<()>,
    _short: tempfile::TempDir,
}

async fn rig() -> Rig {
    rig_with_backend(Arc::new(NoBackend)).await
}

async fn rig_with_backend(backend: Arc<dyn Backend>) -> Rig {
    let mut home = Home::new();
    // A short home: the socket path (and the bind's staging path) must fit
    // in SUN_LEN.
    let short = tempfile::Builder::new()
        .prefix("kv")
        .tempdir_in("/tmp")
        .unwrap();
    let cua_home = short.path().join(".cua");
    home.set("HOME", short.path().display().to_string());
    home.set("CUA_HOME", cua_home.display().to_string());
    std::fs::create_dir_all(&cua_home).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cua_home, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let presence = Arc::new(FakePresence::new(true));
    let broker = Arc::new(
        Broker::new(
            BrokerConfig {
                dir: cua_home.join("keyvault"),
                keychain_path: Some(cua_home.join("never-created.keychain")),
                // A development daemon: passphrase-only.
                os_protector: false,
            },
            backend,
            presence.clone(),
        )
        .unwrap(),
    );
    let policy = TrustPolicy::for_tests(format!(
        "cdhash H\"{}\"",
        cdhash(env!("CARGO_BIN_EXE_cua-spaces-cli"))
    ));
    let listener = cua_keyvault::ipc::bind(&cua_home.join("keyvault.sock"))
        .await
        .unwrap();
    let server = tokio::spawn(cua_keyvault::ipc::serve(listener, broker, policy));
    // This test process is not the Cua-signed daemon: the debug cua may talk
    // to it only with the development opt-out.
    home.set("CUA_KEYVAULT_ALLOW_UNVERIFIED_DAEMON", "1");
    home.set("CUA_ENV_TEST_SANDBOX", "1");
    Rig {
        home,
        presence,
        _server: server,
        _short: short,
    }
}

async fn with_stdin(home: &Home, args: &[&str], input: &str) -> Out {
    let mut child = home.spawn(args);
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).await.unwrap();
    drop(stdin);
    let o = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("cua timed out")
        .unwrap();
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).into(),
        stderr: String::from_utf8_lossy(&o.stderr).into(),
    }
}

const PASSPHRASE: &str = "orbit lantern pickle harbor";

#[tokio::test]
async fn status_init_lock_unlock_with_a_passphrase() {
    let r = rig().await;
    let s = r.home.run(&["keyvault", "status", "--json"]).await;
    let v = s.ok().json();
    assert_eq!(v["initialized"], false);
    assert_eq!(v["os_protector_available"], false);
    assert_eq!(v["passphrase_available"], true);
    assert_eq!(v["caller_first_party"], true, "{v}");
    assert_eq!(v["server_verified"], false);

    // The OS key store is refused before any presence prompt, pointing at
    // the command that works.
    let o = r.home.run(&["keyvault", "init"]).await;
    assert_ne!(o.code, 0);
    assert!(
        o.stderr.contains("cua keyvault init --passphrase"),
        "{}",
        o.stderr
    );
    assert!(r.presence.asked.lock().unwrap().is_empty());

    // Too short: refused locally, and the passphrase is never echoed.
    let o = with_stdin(
        &r.home,
        &["keyvault", "init", "--passphrase-stdin"],
        "hunter2\n",
    )
    .await;
    assert_ne!(o.code, 0);
    assert!(o.stderr.contains("at least 12"), "{}", o.stderr);
    assert!(!o.stderr.contains("hunter2") && !o.stdout.contains("hunter2"));

    let o = with_stdin(
        &r.home,
        &["keyvault", "init", "--passphrase-stdin", "--json"],
        &format!("{PASSPHRASE}\n"),
    )
    .await;
    let v = o.ok().json();
    assert_eq!(v["created"], true);
    assert_eq!(v["protector"], "passphrase");
    assert_eq!(v["recovery_key"].as_str().unwrap().len(), 47);
    assert_eq!(r.presence.asked.lock().unwrap().len(), 1);
    assert!(!o.stdout.contains(PASSPHRASE) && !o.stderr.contains(PASSPHRASE));

    let st = r.home.run(&["keyvault", "status"]).await;
    assert!(
        st.ok().stdout.contains("Keyvault: unlocked"),
        "{}",
        st.stdout
    );
    assert!(st.stdout.contains("Unlocks with: passphrase, recovery key"));

    r.home.run(&["keyvault", "lock"]).await.ok();
    let v = r.home.run(&["keyvault", "status", "--json"]).await.json();
    assert_eq!(v["unlocked"], false);

    let o = with_stdin(
        &r.home,
        &["keyvault", "unlock", "--passphrase-stdin"],
        "not the passphrase\n",
    )
    .await;
    assert_ne!(o.code, 0);
    assert!(o.stderr.contains("does not unlock"), "{}", o.stderr);

    // Spaces are part of a passphrase; only the line ending is dropped.
    let o = with_stdin(
        &r.home,
        &["keyvault", "unlock", "--passphrase-stdin"],
        &format!("{PASSPHRASE}\r\n"),
    )
    .await;
    assert!(o.ok().stdout.contains("Keyvault unlocked."));
    let v = r.home.run(&["keyvault", "status", "--json"]).await.json();
    assert_eq!(v["unlocked"], true);
    assert_eq!(r.presence.asked.lock().unwrap().len(), 1);
}

/// `--passphrase` reads the controlling terminal, never argv or a pipe:
/// without one it refuses and names `--passphrase-stdin`.
#[tokio::test]
async fn the_prompt_needs_a_terminal() {
    let r = rig().await;
    let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_cua-spaces-cli"));
    c.env_clear()
        .envs(&r.home.env)
        .args(["keyvault", "unlock", "--passphrase"])
        .current_dir(r.home.dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // SAFETY: setsid in the child before exec: no controlling terminal.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = c.spawn().unwrap();
    // Anything on stdin must be ignored by the prompt.
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(format!("{PASSPHRASE}\n").as_bytes())
        .await
        .unwrap();
    drop(stdin);
    let o = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success());
    assert!(err.contains("--passphrase-stdin"), "{err}");
}

#[tokio::test]
async fn no_daemon_is_not_running() {
    let home = Home::new();
    let o = home.run(&["keyvault", "status"]).await;
    assert_ne!(o.code, 0);
    assert!(o.stderr.contains("not running"), "{}", o.stderr);
}

/// `cua keyvault import-session`: no `--site` captures the whole app
/// session (`whole_app: true`, no sites); one or more `--site` captures
/// exactly those sites' cookies (`whole_app: false`) with the storage and
/// password flags threaded through -- never both at once, and never a
/// delivery (this command only seals it in the vault).
#[tokio::test]
async fn import_session_whole_app_or_named_sites() {
    let backend = Arc::new(FakeImportBackend::default());
    let r = rig_with_backend(backend.clone() as Arc<dyn Backend>).await;
    let o = with_stdin(
        &r.home,
        &["keyvault", "init", "--passphrase-stdin", "--json"],
        &format!("{PASSPHRASE}\n"),
    )
    .await;
    assert_eq!(o.ok().json()["created"], true);

    let o = r
        .home
        .run(&["keyvault", "import-session", "--app", "chrome", "--json"])
        .await;
    let v = o.ok().json();
    assert_eq!(v["imported"]["saved"], 1, "{v}");
    assert_eq!(v["imported"]["created"], 1, "{v}");
    {
        let specs = backend.specs.lock().unwrap();
        let last = specs.last().unwrap();
        assert_eq!(last.app, "chrome");
        assert!(last.whole_app, "{last:?}");
        assert!(last.sites.is_empty(), "{last:?}");
    }

    let o = r
        .home
        .run(&[
            "keyvault",
            "import-session",
            "--app",
            "chrome",
            "--site",
            "github.com",
            "--include-storage",
            "--json",
        ])
        .await;
    let v = o.ok().json();
    assert_eq!(v["imported"]["saved"], 1, "{v}");
    {
        let specs = backend.specs.lock().unwrap();
        let last = specs.last().unwrap();
        assert!(!last.whole_app, "{last:?}");
        assert_eq!(last.sites.len(), 1);
        assert_eq!(last.sites[0].site, "github.com");
        assert!(last.sites[0].include_storage);
        assert!(!last.sites[0].include_passwords);
    }

    // Text output names what it imported and never delivers anywhere.
    let o = r
        .home
        .run(&["keyvault", "import-session", "--app", "slack"])
        .await;
    let o = o.ok();
    assert!(o.stdout.contains("Sealed in the Keyvault"), "{}", o.stdout);
    assert!(o.stdout.contains("Saved 1 item: 1 new"), "{}", o.stdout);
    assert!(!o.stdout.contains("teleported") && !o.stdout.contains("delivered to"));
}
