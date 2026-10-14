// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Loopback fixtures for the language-binding smoke tests (never shipped).
//!
//! Prints one JSON line with the endpoints, then serves until stdin closes
//! (or 15 minutes pass):
//!
//! ```json
//! {"env_url":"http://127.0.0.1:…","env_token":"fixture-token",
//!  "fleet_base_url":"http://127.0.0.1:…","fleet_token":"fake-fleet-token",
//!  "registry_mirror":"*=http://127.0.0.1:…",
//!  "spaces_url":"http://127.0.0.1:…","spaces_token":"…",
//!  "spaces_guest_home":"/tmp/…","spaces_downloads":"/tmp/…",
//!  "spaces_teleport_home":"/tmp/…","teleport_host_home":"/tmp/…",
//!  "daemon_socket":"/tmp/…/cua.sock","source_commit":"…"}
//! ```
//!
//! The env URL is a `MockServer` (simulated processes, in-memory files, a
//! scripted `/media` socket). The fake Fleet routes every claim's `env`
//! service to a second `MockServer`, so cloud sandboxes have a spacesd.
//! `registry_mirror` (a `CUA_REGISTRY_MIRRORS` value) serves the manifests
//! of `fixtures::sample_registry()`. The Spaces URL is a real cua-spacesd server
//! core in this process (`cua_spaces_e2e`), reporting Linux like the docs'
//! Spaces, confined before the line is printed: its child processes see a temp `HOME`, and Downloads and the
//! teleport import home are temp directories. `teleport_host_home` holds a
//! synthetic Firefox profile for `CuaConfig.teleport_home`.
//!
//! `daemon_socket` (unix) is a cua daemon server core in this process whose
//! Keyvault is test-only: a passphrase vault in a temp home with a file
//! generation anchor (never the OS key store), a fake user-presence gate in
//! place of Touch ID (the Keyvault tests' `FakePresence`), and one item, the
//! fixture Firefox profile's login selection, imported by the test user (a
//! first-party test identity). Nothing is approved on its own: a `teleport_app`
//! call files a consent request, and the line `approve` on stdin is the user
//! approving it in Cua (it approves the requests waiting then, or the next
//! one to arrive within 30 s).
//!
//! The binary refuses to serve (exit 2, a message on stderr) when its
//! checkout's HEAD is not the commit it was built from: rebuild with
//! `libs/cua/scripts/build-test-fixtures.sh`. Nothing touches the host's real
//! home, apps or keychain.

use cua_daemon::fixtures;
use std::io::Write;
use tokio::io::AsyncBufReadExt;

const TOKEN: &str = "fixture-token";

/// The commit this binary was built from (build.rs).
const SOURCE_COMMIT: &str = env!("CUA_FIXTURES_SOURCE_COMMIT");

/// Exits when the checkout this binary was built from has moved on: a stale
/// fixture must fail the suites, never quietly answer with old behavior.
fn refuse_when_stale() {
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    // No git (a source export) or no stamp: nothing to compare.
    let (Some(head), false) = (head, SOURCE_COMMIT.is_empty()) else {
        return;
    };
    if head != SOURCE_COMMIT {
        eprintln!(
            "cua-test-fixtures is stale: built from {SOURCE_COMMIT}, the checkout is at {head}. \
             Rebuild it with libs/cua/scripts/build-test-fixtures.sh"
        );
        std::process::exit(2);
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    refuse_when_stale();
    let env = fixtures::start_env(Some(TOKEN), None).await;
    // Every fake cloud claim's `env` service reaches this MockServer spacesd
    // (its own instance, token-less: it accepts the token the SDK installs).
    let claim_spacesd = fixtures::start_env(None, None).await;
    // Image manifests for `CUA_REGISTRY_MIRRORS`: resolving the docs' images
    // needs no network (and no Docker Hub rate limit).
    let registry = fixtures::start_registry_http(fixtures::sample_registry()).await;
    let fleet = fixtures::start_fleet_http_with_spacesd(
        cua_fleet::testing::FakeFleet::new(),
        claim_spacesd.url.clone(),
    )
    .await;
    // The docs' Spaces are Linux sandboxes: report Linux on Linux and macOS
    // hosts, so the teleport catalog and plans match what the pages show. A
    // Windows host has no /bin/sh for a Linux Space's shell, so there the
    // Space reports Windows and runs cmd.exe.
    let driver =
        cua_spaces_e2e::driver_with(|c| c.os_override = (!cfg!(windows)).then(|| "linux".into()))
            .await;
    driver.confine_direct().await;
    let host_home = tempfile::tempdir().expect("temp host home");
    cua_spaces_e2e::write_firefox_profile(host_home.path());
    #[cfg(unix)]
    let daemon = test_daemon::start(host_home.path()).await;
    let line = serde_json::json!({
        "env_url": env.url,
        "env_token": TOKEN,
        "fleet_base_url": fleet.base_url,
        "fleet_token": fixtures::FAKE_FLEET_TOKEN,
        "registry_mirror": registry.mirror(),
        "spaces_url": driver.url,
        "spaces_token": cua_spaces_e2e::TOKEN,
        "spaces_guest_home": driver.home,
        "spaces_downloads": driver.downloads,
        "spaces_teleport_home": driver.teleport_home,
        "teleport_host_home": host_home.path(),
        "source_commit": SOURCE_COMMIT,
    });
    #[cfg(unix)]
    let line = {
        let mut line = line;
        line["daemon_socket"] = serde_json::json!(daemon.socket);
        line
    };
    let mut out = std::io::stdout();
    writeln!(out, "{line}").unwrap();
    out.flush().unwrap();
    let commands = async {
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        // Bounded: returns on EOF or error.
        while let Ok(Some(command)) = lines.next_line().await {
            match command.trim() {
                #[cfg(unix)]
                "approve" => daemon.approve_waiting().await,
                "" => {}
                other => eprintln!("cua-test-fixtures: unknown command {other:?}"),
            }
        }
    };
    tokio::select! {
        _ = commands => {}
        _ = tokio::time::sleep(std::time::Duration::from_secs(900)) => {}
    }
    #[cfg(unix)]
    daemon.stop().await;
    drop((env, claim_spacesd, registry, fleet, driver, host_home));
}

/// A cua daemon server core with a test-only Keyvault (see the module docs).
#[cfg(unix)]
mod test_daemon {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use cua_daemon::server::{self, DaemonHandle, ServerConfig};
    use cua_daemon::{Runtime, RuntimeConfig};
    use cua_keyvault::CallerIdentity;
    use cua_keyvault::broker::{ApproveOptions, Broker, FakePresence, ImportSpec, InitRequest};
    use cua_spaces_ext::daemon::CuaSpacesDaemon;

    pub struct TestDaemon {
        /// `cua.sock` (the Keyvault's own socket sits beside it).
        pub socket: String,
        broker: Arc<Broker>,
        /// The person at the keyboard: a first-party test identity, as the
        /// Cua app is to the real Keyvault.
        user: CallerIdentity,
        handle: DaemonHandle,
        _runtime: Runtime,
        _home: tempfile::TempDir,
    }

    pub async fn start(teleport_home: &Path) -> TestDaemon {
        // Short: macOS caps Unix socket paths at 104 bytes.
        let home = tempfile::Builder::new()
            .prefix("cuafx")
            .tempdir_in("/tmp")
            .or_else(|_| tempfile::Builder::new().prefix("cuafx").tempdir())
            .expect("temp daemon home");
        // The Cua Spaces daemon extension with the fake presence gate (never
        // the OS key store or Touch ID).
        let runtime = Runtime::new(RuntimeConfig {
            state_dir: Some(home.path().join("sbx")),
            spaces_home: Some(home.path().to_path_buf()),
            teleport_home: Some(teleport_home.to_path_buf()),
            env_probe_timeout: Some(Duration::from_secs(10)),
            providers: Some(Vec::new()),
            extensions: vec![Arc::new(CuaSpacesDaemon::with_test_presence(Arc::new(
                FakePresence::new(true),
            )))],
            ..Default::default()
        })
        .expect("daemon runtime");
        let broker = runtime
            .attached::<cua_spaces_ext::daemon::Attached>()
            .and_then(|a| a.keyvault())
            .expect("the daemon hosts a Keyvault")
            .broker();
        let user = CallerIdentity::for_tests("com.trycua.cua", true);
        broker
            .init(
                &user,
                InitRequest {
                    os_protector: false,
                    passphrase: Some("cua-test-fixtures".into()),
                    recovery_key: false,
                },
            )
            .await
            .expect("create the test Keyvault");
        // The user imports the fixture profile's session into the vault, as
        // they would on the Keyvault page; teleport moves only what is here.
        let report = broker
            .import(
                &user,
                ImportSpec {
                    app: "firefox".into(),
                    whole_app: true,
                    ..Default::default()
                },
            )
            .await
            .expect("import the fixture Firefox session");
        assert!(report.saved > 0, "the fixture profile yields a vault item");
        let socket = home.path().join("cua.sock");
        let handle = server::start(
            runtime.clone(),
            ServerConfig {
                socket_path: Some(socket.clone()),
                // The Space env passthrough rides the loopback listener.
                loopback: Some(std::net::SocketAddr::from(([127, 0, 0, 1], 0))),
                token: cua_daemon::random_token(),
                discovery_path: None,
                bridge_ticket_ttl: Duration::from_secs(60),
            },
        )
        .await
        .expect("start the daemon");
        TestDaemon {
            socket: socket.display().to_string(),
            broker,
            user,
            handle,
            _runtime: runtime,
            _home: home,
        }
    }

    impl TestDaemon {
        /// The user approves the teleport requests waiting in Cua: those
        /// pending now, or the next to arrive within 30 s.
        pub async fn approve_waiting(&self) {
            for _ in 0..300 {
                let pending = match self.broker.list_pending(&self.user).await {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("cua-test-fixtures: list pending requests: {e}");
                        return;
                    }
                };
                if !pending.is_empty() {
                    for p in pending {
                        if let Err(e) = self
                            .broker
                            .approve(&self.user, &p.id, ApproveOptions::default())
                            .await
                        {
                            eprintln!("cua-test-fixtures: approve {}: {e}", p.id);
                        }
                    }
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            eprintln!("cua-test-fixtures: no teleport request arrived to approve");
        }

        pub async fn stop(self) {
            self.handle.shutdown();
            // Bounded: the listeners stop at once.
            let _ = tokio::time::timeout(Duration::from_secs(10), self.handle.wait()).await;
        }
    }
}
