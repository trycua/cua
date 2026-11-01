// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Client/daemon version skew with the real binaries (CUA-2155). Opt-in:
//! `tests/e2e/run-upgrade-e2e.sh` fetches the released cua 0.7.0 (the one in
//! Cua Spaces 0.13.1), builds this branch's `cua` (cua-spaces-cli) and runs
//! these with `--ignored`. Each case copies the 0.7.0 fixture home into a
//! throwaway `HOME`/`CUA_HOME`, starts one daemon on its own socket, and
//! stops it when the case ends. No keychain, relay, telemetry or Space.
//!
//! - old CLI, new daemon: every recorded 0.7.0 command answers what 0.7.0
//!   answered (superset), and its writes work;
//! - new CLI, old daemon: the current command names work against 0.7.0, and
//!   the old daemon is not replaced;
//! - downgrade: after the new daemon saved, the 0.7.0 daemon reads it all.
//!
//! `CUA_UPGRADE_OLD_CUA` and `CUA_UPGRADE_NEW_CUA` name the two binaries.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn bin(var: &str) -> PathBuf {
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| panic!("set {var}")))
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade/0.7.0")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), &dst).unwrap();
        }
    }
}

/// A throwaway home holding a copy of the 0.7.0 fixture home.
struct Home {
    dir: tempfile::TempDir,
    daemon: Option<Child>,
}

impl Home {
    fn new() -> Home {
        // Short: the daemon's Unix socket lives in it.
        let base = std::env::var_os("CUA_UPGRADE_E2E_TMP")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&base).unwrap();
        let dir = tempfile::Builder::new()
            .prefix("up")
            .tempdir_in(base)
            .unwrap();
        copy_dir(&fixtures().join("home"), &dir.path().join(".cua"));
        Home { dir, daemon: None }
    }

    fn cua_home(&self) -> PathBuf {
        self.dir.path().join(".cua")
    }

    fn command(&self, exe: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(exe);
        let home = self.dir.path();
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("CUA_HOME", self.cua_home())
            .env("CUA_TEST", "1")
            .env("CUA_CREDENTIAL_STORE", "file")
            .env("CUA_NO_BROWSER", "1")
            .env("CUA_RELAY_URL", "http://127.0.0.1:9")
            .env("CUA_DAEMON_NO_RELAY", "1")
            .env("CUA_TELEMETRY", "0")
            .env("CUA_TELEMETRY_FORBID_NETWORK", "1")
            .env("CUA_TELEMETRY_ENDPOINT", "http://127.0.0.1:9/batch/")
            .env("CUA_LOG", "error")
            .args(args)
            .current_dir(home)
            .stdin(Stdio::null());
        c
    }

    /// Starts `exe`'s daemon on this home and waits for its socket.
    fn start_daemon(&mut self, exe: &Path) {
        assert!(self.daemon.is_none());
        let sock = self.cua_home().join("cua.sock");
        let _ = std::fs::remove_file(&sock);
        let child = self
            .command(
                exe,
                &["daemon", "start", "--foreground", "--loopback", "off"],
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start daemon");
        self.daemon = Some(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if sock.exists() && self.try_run(exe, &["daemon", "status"]).0 == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("the daemon did not come up");
    }

    /// Stops the daemon this home started (by its PID only).
    fn stop_daemon(&mut self) {
        if let Some(mut c) = self.daemon.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Runs `exe args` (60 s at most): (exit code, stdout, stderr).
    fn try_run(&self, exe: &Path, args: &[&str]) -> (i32, String, String) {
        let mut child = self
            .command(exe, args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn cua");
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("cua {args:?} timed out");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let o = child.wait_with_output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into(),
            String::from_utf8_lossy(&o.stderr).into(),
        )
    }

    fn run(&self, exe: &Path, args: &[&str]) -> (String, String) {
        let (code, out, err) = self.try_run(exe, args);
        assert_eq!(code, 0, "cua {args:?}\nstdout:\n{out}\nstderr:\n{err}");
        (out, err)
    }

    fn json(&self, exe: &Path, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let (out, _) = self.run(exe, &a);
        serde_json::from_str(out.trim())
            .unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): {out}"))
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        self.stop_daemon();
    }
}

/// `new` has every field of `old` with the same value; `next_fire` only
/// keeps its presence (it depends on the time of the call).
fn superset(old: &Value, new: &Value, at: &str, out: &mut Vec<String>) {
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            for (k, ov) in o {
                match n.get(k) {
                    None => out.push(format!("{at}.{k}: missing")),
                    Some(nv) if k == "next_fire" => {
                        if ov.is_null() != nv.is_null() {
                            out.push(format!("{at}.{k}: {ov} became {nv}"));
                        }
                    }
                    Some(nv) => superset(ov, nv, &format!("{at}.{k}"), out),
                }
            }
        }
        (Value::Array(o), Value::Array(n)) if o.len() == n.len() => {
            for (i, (ov, nv)) in o.iter().zip(n).enumerate() {
                superset(ov, nv, &format!("{at}[{i}]"), out);
            }
        }
        _ if old == new => {}
        _ => out.push(format!("{at}: {old} became {new}")),
    }
}

fn assert_superset(what: &str, old: &Value, new: &Value) {
    let mut d = Vec::new();
    superset(old, new, what, &mut d);
    assert!(d.is_empty(), "{what}:\n{}", d.join("\n"));
}

fn golden(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(fixtures().join("golden").join(name)).unwrap()).unwrap()
}

fn names(v: &Value, list: &str, field: &str) -> Vec<String> {
    v[list]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a[field].as_str().unwrap().to_string())
        .collect()
}

#[test]
#[ignore = "opt-in: tests/e2e/run-upgrade-e2e.sh (real 0.7.0 and new binaries)"]
fn old_070_cli_against_a_new_daemon_then_downgrade() {
    let (old, new) = (bin("CUA_UPGRADE_OLD_CUA"), bin("CUA_UPGRADE_NEW_CUA"));
    let mut h = Home::new();
    h.start_daemon(&new);

    // Reads: what 0.7.0 answered over the same home.
    for (args, file) in [
        (&["agent", "persistent"][..], "cli_agent_persistent.json"),
        (&["agent", "routine", "ls"][..], "cli_agent_routine_ls.json"),
        (
            &["agent", "notifications"][..],
            "cli_agent_notifications.json",
        ),
        (
            &["agent", "notifications", "--unread"][..],
            "cli_agent_notifications_--unread.json",
        ),
        (
            &["volume", "ls", "agents/"][..],
            "cli_volume_ls_agents_.json",
        ),
    ] {
        let now = h.json(&old, args);
        assert_superset(&format!("{args:?}"), &golden(file), &now);
    }
    let (cat, _) = h.run(&old, &["volume", "cat", "agents/ada/memory/MEMORY.md"]);
    assert_eq!(
        cat,
        std::fs::read_to_string(fixtures().join("golden/cli_volume_cat.txt")).unwrap()
    );

    // Writes by the old CLI through the new daemon.
    h.run(
        &old,
        &[
            "agent",
            "create",
            "zed",
            "--harness",
            "hermes",
            "--in",
            "local:dev",
        ],
    );
    h.run(
        &old,
        &[
            "agent", "routine", "add", "zed", "--title", "Dawn", "--prompt", "wake", "--daily",
            "07:00",
        ],
    );
    let r = h.json(&old, &["agent", "routine", "ls"]);
    assert!(
        names(&r, "routines", "agent").contains(&"zed".to_string()),
        "{r}"
    );
    h.run(&old, &["agent", "notifications", "--mark-read"]);
    // The new CLI sees the same records under the current names.
    let ls = h.json(&new, &["agent", "ls"]);
    assert_eq!(
        names(&ls, "agents", "name"),
        ["ada", "scout", "rev", "zed"],
        "{ls}"
    );
    h.stop_daemon();

    // Downgrade: the 0.7.0 daemon over what the new daemon saved.
    h.start_daemon(&old);
    let a = h.json(&old, &["agent", "persistent"]);
    assert_superset(
        "agents after a downgrade",
        &golden("cli_agent_persistent.json")["agents"][0],
        &a["agents"][0],
    );
    assert_eq!(names(&a, "agents", "name"), ["ada", "scout", "rev", "zed"]);
    let r = h.json(&old, &["agent", "routine", "ls"]);
    assert_eq!(r["routines"].as_array().unwrap().len(), 4, "{r}");
    let n = h.json(&old, &["agent", "notifications", "--unread"]);
    assert!(
        n["notifications"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["read"] == true),
        "{n}"
    );
    h.run(&old, &["agent", "forget", "zed"]);
}

#[test]
#[ignore = "opt-in: tests/e2e/run-upgrade-e2e.sh (real 0.7.0 and new binaries)"]
fn new_cli_against_the_070_daemon() {
    let (old, new) = (bin("CUA_UPGRADE_OLD_CUA"), bin("CUA_UPGRADE_NEW_CUA"));
    let mut h = Home::new();
    h.start_daemon(&old);

    // The current names, answered by 0.7.0, and the daemon keeps running
    // (no restart behind the user's back).
    let pid = std::fs::read_to_string(h.cua_home().join("daemon.json")).unwrap();
    let ls = h.json(&new, &["agent", "ls"]);
    assert_eq!(
        names(&ls, "agents", "name"),
        ["ada", "scout", "rev"],
        "{ls}"
    );
    let legacy = h.json(&new, &["agent", "persistent"]);
    assert_eq!(
        names(&legacy, "agents", "name"),
        names(&ls, "agents", "name")
    );
    let s = h.json(&new, &["agent", "schedule", "ls"]);
    assert_superset("schedule ls", &golden("cli_agent_routine_ls.json"), &s);
    let n = h.json(&new, &["agent", "notifications"]);
    assert_superset("notifications", &golden("cli_agent_notifications.json"), &n);

    // Writes by the current names land in the 0.7.0 daemon.
    h.run(
        &new,
        &[
            "agent",
            "add",
            "neo",
            "--harness",
            "hermes",
            "--in",
            "local:dev",
        ],
    );
    h.run(
        &new,
        &[
            "agent", "schedule", "add", "neo", "--title", "Dawn", "--prompt", "wake", "--daily",
            "07:00",
        ],
    );
    let r = h.json(&old, &["agent", "routine", "ls"]);
    assert!(
        names(&r, "routines", "agent").contains(&"neo".to_string()),
        "{r}"
    );
    h.run(&new, &["agent", "rm", "neo"]);
    let a = h.json(&old, &["agent", "persistent"]);
    assert_eq!(names(&a, "agents", "name"), ["ada", "scout", "rev"]);
    assert_eq!(
        pid,
        std::fs::read_to_string(h.cua_home().join("daemon.json")).unwrap(),
        "the 0.7.0 daemon was replaced"
    );
}
