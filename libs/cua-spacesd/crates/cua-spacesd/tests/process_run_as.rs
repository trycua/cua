// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Commands and file operations as another guest user without `sudo`.
//!
//! The real `cua-spacesd` binary runs as an ordinary user with
//! `no_new_privs` set and no `sudo` on its PATH (what gVisor gives the
//! Spaces images), holding either the three capabilities the images grant it
//! (`CAP_SETUID`, `CAP_SETGID`, `CAP_KILL`, the same `setpriv` the container
//! supervisor uses) or none. The tests then drive it through the client as
//! the SDKs do and check the identity switch, the file ownership, the
//! environment, the supplementary groups, that nothing the agent starts
//! keeps a capability, the signal path, and the refusals: an agent without
//! the capabilities, an unknown user, and a viewer ticket.
//!
//! Needs Linux, `setpriv`, `useradd` and root or passwordless `sudo` (a
//! GitHub-hosted Linux runner has it). Without them each test prints why and
//! returns; set `CUA_REQUIRE_PRIVILEGED_TESTS=1` (the nightly lanes do) to
//! fail instead.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command as Proc, Stdio};
use std::time::Duration;

use cua_proto::env::v1 as pb;
use cua_spacesd_client::{
    Command, ConnectOptions, Error, FileAccess, RunAsMode, SpacesdClient, TransportPreference,
    UploadOptions,
};

const TOKEN: &str = "process-run-as-token-0123456789abcdef";

/// Runs `argv` as root (through `sudo -n` unless already root).
fn root(argv: &[&str]) -> std::io::Result<std::process::Output> {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        Proc::new(argv[0]).args(&argv[1..]).output()
    } else {
        Proc::new("sudo").arg("-n").args(argv).output()
    }
}

fn ok(argv: &[&str]) -> String {
    let out = root(argv).unwrap();
    assert!(
        out.status.success(),
        "{argv:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Whether this machine can run the tests; says why not.
fn privileged() -> bool {
    let why = if root(&["true"]).map(|o| !o.status.success()).unwrap_or(true) {
        Some("neither root nor passwordless sudo")
    } else if Proc::new("setpriv").arg("--help").output().is_err() {
        Some("no setpriv")
    } else if Proc::new("useradd").arg("--help").output().is_err()
        && root(&["useradd", "--help"]).is_err()
    {
        Some("no useradd")
    } else {
        None
    };
    match why {
        None => true,
        Some(why) => {
            assert!(
                std::env::var_os("CUA_REQUIRE_PRIVILEGED_TESTS").is_none(),
                "privileged test needed but unavailable: {why}"
            );
            eprintln!("skipped: {why}");
            false
        }
    }
}

/// The two accounts and the group of one test, removed on drop.
struct Accounts {
    agent: String,
    other: String,
    group: String,
}

impl Accounts {
    fn new(tag: &str) -> Self {
        let pid = std::process::id();
        let a = Accounts {
            agent: format!("cra{tag}{pid}"),
            other: format!("crb{tag}{pid}"),
            group: format!("crg{tag}{pid}"),
        };
        ok(&["groupadd", &a.group]);
        ok(&["useradd", "-m", "-s", "/bin/sh", "-G", &a.group, &a.other]);
        ok(&["useradd", "-m", "-s", "/bin/sh", &a.agent]);
        a
    }
}

impl Drop for Accounts {
    fn drop(&mut self) {
        for user in [&self.agent, &self.other] {
            let _ = root(&["pkill", "-KILL", "-u", user]);
            let _ = root(&["userdel", "-r", "-f", user]);
        }
        let _ = root(&["groupdel", &self.group]);
    }
}

/// A running agent and a client for it.
struct Agent {
    client: SpacesdClient,
    port: u16,
    pid: String,
    dir: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = root(&["kill", "-KILL", &self.pid]);
        let _ = root(&["rm", "-rf", &self.dir.display().to_string()]);
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Starts the agent as `accounts.agent` with `no_new_privs`, a PATH with no
/// `sudo`, and the capabilities `caps` (a `setpriv` list, empty: none).
async fn start(accounts: &Accounts, caps: &str) -> Agent {
    let tmp = tempfile::tempdir().unwrap();
    // A directory the agent's account can reach (the build tree may not be).
    let dir = tmp.path().join("agent");
    let dir_s = dir.display().to_string();
    ok(&["install", "-d", "-m", "0755", &dir_s]);
    ok(&[
        "install",
        "-m",
        "0755",
        env!("CARGO_BIN_EXE_cua-spacesd"),
        &format!("{dir_s}/cua-spacesd"),
    ]);
    // The tools the tests use, and no sudo.
    let bin = dir.join("bin");
    ok(&["install", "-d", "-m", "0755", &bin.display().to_string()]);
    for tool in [
        "sh", "id", "env", "cat", "sleep", "stat", "grep", "ls", "tr", "touch", "mkdir", "chmod",
        "pgrep", "head", "printf", "true", "rm", "echo",
    ] {
        if let Some(path) = which(tool) {
            ok(&["ln", "-sf", &path, &format!("{}/{tool}", bin.display())]);
        }
    }
    let data = dir.join("data");
    ok(&[
        "install",
        "-d",
        "-m",
        "0700",
        "-o",
        &accounts.agent,
        &data.display().to_string(),
    ]);
    let port = free_port();
    let pid_file = dir.join("pid");
    let mut setpriv = format!(
        "setpriv --reuid {a} --regid {a} --clear-groups --no-new-privs",
        a = accounts.agent
    );
    if !caps.is_empty() {
        setpriv.push_str(&format!(" --inh-caps {caps} --ambient-caps {caps}"));
    }
    let script = format!(
        "echo $$ > {pid}; exec {setpriv} -- env -i PATH={bin} HOME={data} USER={a} \
         CUA_ENV_TOKEN={TOKEN} CUA_ENV_LISTEN=127.0.0.1:{port} CUA_ENV_DATA_DIR={data} \
         CUA_ENV_NO_DESKTOP=true CUA_ENV_NO_DRIVER=true CUA_ENV_NO_MCP=true CUA_SPACESD_NO_ACCESS_LOG=true \
         CUA_ENV_QUIC_PORT=0 {dir_s}/cua-spacesd serve",
        pid = pid_file.display(),
        bin = bin.display(),
        data = data.display(),
        a = accounts.agent,
    );
    let mut cmd = if unsafe { libc::geteuid() } == 0 {
        Proc::new("sh")
    } else {
        let mut c = Proc::new("sudo");
        c.args(["-n", "sh"]);
        c
    };
    cmd.args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().unwrap();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    let url = format!("http://127.0.0.1:{port}");
    let mut client = None;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let connect = tokio::time::timeout(
            Duration::from_secs(3),
            SpacesdClient::connect(
                ConnectOptions::parse(&url)
                    .unwrap()
                    .transport(TransportPreference::Native)
                    .probe(false)
                    .token(TOKEN),
            ),
        )
        .await;
        if let Ok(Ok(c)) = connect {
            if matches!(
                tokio::time::timeout(Duration::from_secs(3), c.capabilities()).await,
                Ok(Ok(_))
            ) {
                client = Some(c);
                break;
            }
        }
    }
    let pid = std::fs::read_to_string(&pid_file)
        .or_else(|_| Ok::<_, std::io::Error>(ok(&["cat", &pid_file.display().to_string()])))
        .unwrap()
        .trim()
        .to_owned();
    Agent {
        client: client.expect("the agent did not start"),
        port,
        pid,
        dir,
        _tmp: tmp,
    }
}

fn which(tool: &str) -> Option<String> {
    std::env::var("PATH")
        .ok()?
        .split(':')
        .map(|d| Path::new(d).join(tool))
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
}

const CAPS: &str = "+setuid,+setgid,+kill";

/// stdout of a shell line in the guest as `user` (the agent's when `None`).
async fn sh(agent: &Agent, user: Option<&str>, line: &str) -> String {
    let mut cmd = Command::shell(line).cwd("/");
    if let Some(u) = user {
        cmd = cmd.user(u);
    }
    let out = agent.client.run(cmd).await.unwrap();
    assert_eq!(out.status.code, Some(0), "{line}: {}", out.stderr_str());
    out.stdout_str().trim().to_owned()
}

fn host_id(user: &str, flag: &str) -> String {
    ok(&["id", flag, user])
}

#[tokio::test]
async fn an_agent_with_the_capabilities_switches_users_without_sudo() {
    if !privileged() {
        return;
    }
    let accounts = Accounts::new("c");
    let agent = start(&accounts, CAPS).await;
    let (a, b) = (accounts.agent.as_str(), accounts.other.as_str());

    // The agent says how it does it.
    assert_eq!(agent.client.run_as_mode().await, RunAsMode::Capabilities);
    let caps = agent.client.capabilities().await.unwrap();
    let feature = caps
        .features
        .iter()
        .find(|f| f.name == "process.run_as")
        .expect("process.run_as is advertised");
    assert!(feature.supported);
    assert_eq!(feature.attributes["mode"], "capabilities");
    assert_eq!(feature.attributes["signal_others"], "true");
    let fs_feature = caps
        .features
        .iter()
        .find(|f| f.name == "fs.run_as")
        .unwrap();
    assert_eq!(fs_feature.attributes["privilege"], "capabilities");

    // There is no sudo for it to fall back on, and it is not asked to.
    assert_eq!(
        sh(&agent, None, "command -v sudo || echo none").await,
        "none"
    );
    let handle = agent
        .client
        .spawn(Command::shell("id -u").cwd("/").user(b))
        .await
        .unwrap();
    assert!(!handle.via_sudo());
    assert_eq!(
        handle.wait().await.unwrap().stdout_str().trim(),
        host_id(b, "-u")
    );

    // Identity: uid, gid and supplementary groups are the user's, and the
    // default user (no `user`) is still the agent's own account.
    assert_eq!(sh(&agent, Some(b), "id -u").await, host_id(b, "-u"));
    assert_eq!(sh(&agent, Some(b), "id -g").await, host_id(b, "-g"));
    let groups = sh(&agent, Some(b), "id -G").await;
    let mut got: Vec<_> = groups.split_whitespace().collect();
    let want_s = host_id(b, "-G");
    let mut want: Vec<_> = want_s.split_whitespace().collect();
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want, "supplementary groups");
    assert!(
        sh(&agent, Some(b), "id -Gn")
            .await
            .contains(&accounts.group),
        "the user's own group list, not the agent's"
    );
    assert_eq!(sh(&agent, None, "id -un").await, a);
    assert!(
        !sh(&agent, None, "id -Gn").await.contains(&accounts.group),
        "the agent's groups are not the other user's"
    );

    // Environment: the user's HOME, USER, LOGNAME, SHELL; no agent token.
    assert_eq!(
        sh(
            &agent,
            Some(b),
            "printf '%s %s %s %s' \"$HOME\" \"$USER\" \"$LOGNAME\" \"$SHELL\""
        )
        .await,
        format!("/home/{b} {b} {b} /bin/sh")
    );
    assert_eq!(
        sh(&agent, Some(b), "env | grep -c '^CUA_ENV_TOKEN=' || true").await,
        "0"
    );
    // The default cwd is the user's home.
    let out = agent
        .client
        .run(Command::shell("pwd").user(b))
        .await
        .unwrap();
    assert_eq!(out.stdout_str().trim(), format!("/home/{b}"));

    // Nothing the agent starts keeps a capability, switched or not.
    for user in [Some(b), None] {
        for field in ["CapInh", "CapPrm", "CapEff", "CapAmb"] {
            let line = format!("grep '^{field}:' /proc/self/status");
            let got = sh(&agent, user, &line).await;
            assert!(got.ends_with("0000000000000000"), "{user:?} {got}");
        }
    }
    // root is reachable the same way.
    assert_eq!(sh(&agent, Some("root"), "id -u").await, "0");

    // Files: created by the user, owned by the user, with that user's
    // permission checks.
    let home_file = format!("/home/{b}/note.txt");
    agent
        .client
        .upload(
            &home_file,
            bytes::Bytes::from_static(b"hello"),
            UploadOptions {
                run_as: Some(b.to_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        sh(&agent, Some(b), &format!("stat -c '%U:%a' {home_file}")).await,
        format!("{b}:644")
    );
    let as_b = FileAccess::user(b);
    assert_eq!(
        agent
            .client
            .stat_with(&home_file, false, &as_b)
            .await
            .unwrap()
            .size,
        5
    );
    // The agent's account cannot read the other user's 0700 home; the other
    // user can.
    ok(&["chmod", "0700", &format!("/home/{b}")]);
    let denied = agent
        .client
        .stat_with(&home_file, false, &FileAccess::default())
        .await;
    assert!(denied.is_err(), "the agent's own account is not the user");
    assert!(agent
        .client
        .stat_with(&home_file, false, &as_b)
        .await
        .is_ok());
    // The other user cannot write where only the agent's account may.
    let err = agent
        .client
        .upload(
            &format!("{}/data/forbidden", agent.dir.display()),
            bytes::Bytes::from_static(b"x"),
            UploadOptions {
                run_as: Some(b.to_owned()),
                ..Default::default()
            },
        )
        .await
        .expect_err("the other user cannot write into the agent's data dir");
    assert!(
        matches!(err, Error::PermissionDenied(_)),
        "the kernel's check for the user: {err:?}"
    );

    // The file helper holds no privilege and none of the agent's
    // environment.
    let helper = sh(
        &agent,
        Some(b),
        "for p in $(pgrep -u $(id -u) -f 'fs-as --user'); do \
           grep '^CapEff:' /proc/$p/status; tr '\\0' '\\n' </proc/$p/environ | grep -c CUA_ENV_TOKEN || true; \
         done",
    )
    .await;
    assert!(helper.contains("CapEff:\t0000000000000000"), "{helper:?}");
    assert!(
        !helper.lines().any(|l| l == "1"),
        "token in the helper env: {helper:?}"
    );

    // Signals: the agent stops what it started as another user.
    let handle = agent
        .client
        .spawn(Command::shell("exec sleep 300").cwd("/").user(b))
        .await
        .unwrap();
    let pid = handle.pid();
    agent.client.kill_tree(pid, false).await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(10), handle.wait())
        .await
        .expect("the process ended")
        .unwrap();
    assert_eq!(
        out.status.signal,
        Some(pb::Signal::Kill),
        "{:?}",
        out.status
    );
    // A timeout is enforced the same way.
    let out = agent
        .client
        .run(
            Command::shell("exec sleep 300")
                .cwd("/")
                .user(b)
                .timeout(Duration::from_secs(1)),
        )
        .await
        .unwrap();
    assert!(out.status.timed_out, "{:?}", out.status);

    // An account that does not exist is the caller's mistake.
    let err = agent
        .client
        .run(Command::shell("true").user("no-such-user-xyz"))
        .await
        .expect_err("unknown user");
    assert!(err.to_string().contains("no-such-user-xyz"), "{err:?}");
}

#[tokio::test]
async fn an_agent_without_the_capabilities_refuses_and_says_why() {
    if !privileged() {
        return;
    }
    let accounts = Accounts::new("n");
    let agent = start(&accounts, "").await;
    let b = accounts.other.as_str();

    assert_eq!(agent.client.run_as_mode().await, RunAsMode::None);
    let caps = agent.client.capabilities().await.unwrap();
    let feature = caps
        .features
        .iter()
        .find(|f| f.name == "process.run_as")
        .unwrap();
    assert!(!feature.supported);
    assert!(
        feature.limitation.contains("no sudo"),
        "{}",
        feature.limitation
    );
    assert_eq!(feature.attributes["mode"], "none");
    assert_eq!(feature.attributes["signal_others"], "false");

    // The default user and the agent's own account need no privilege.
    assert_eq!(sh(&agent, None, "id -un").await, accounts.agent);
    assert_eq!(
        sh(&agent, Some(&accounts.agent), "id -un").await,
        accounts.agent
    );

    // Another user is refused, with the reason, and sudo is not tried.
    let err = agent
        .client
        .spawn(Command::shell("id -u").user(b))
        .await
        .expect_err("refused");
    assert!(
        matches!(&err, Error::PermissionDenied(d)
            if d.message.contains("cannot switch") && d.message.contains("CAP_SETUID")),
        "{err:?}"
    );
    let err = agent
        .client
        .upload(
            &format!("/home/{b}/x"),
            bytes::Bytes::from_static(b"x"),
            UploadOptions {
                run_as: Some(b.to_owned()),
                ..Default::default()
            },
        )
        .await
        .expect_err("refused");
    assert!(
        matches!(&err, Error::PermissionDenied(d) if d.message.contains("cannot act as")),
        "{err:?}"
    );
    // Not a half-way: nothing was created.
    assert!(!Path::new(&format!("/home/{b}/x")).exists());
}

#[tokio::test]
async fn a_viewer_ticket_cannot_pick_a_user() {
    if !privileged() {
        return;
    }
    let accounts = Accounts::new("v");
    let agent = start(&accounts, CAPS).await;
    let b = accounts.other.as_str();

    // The owner mints a ticket with file access.
    let ticket = agent
        .client
        .system()
        .create_viewer_ticket(pb::CreateViewerTicketRequest {
            files_root: format!("/home/{b}"),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .ticket;
    let viewer = SpacesdClient::connect(
        ConnectOptions::parse(&format!("http://127.0.0.1:{}", agent.port))
            .unwrap()
            .transport(TransportPreference::Native)
            .probe(false)
            .token(ticket),
    )
    .await
    .unwrap();

    // Processes are not a viewer's at all, as another user or as itself.
    for cmd in [
        Command::shell("id -u").user(b),
        Command::shell("id -u"),
        Command::shell("id -u").user("root"),
    ] {
        let err = viewer.spawn(cmd).await.expect_err("refused");
        assert!(
            matches!(&err, Error::PermissionDenied(d) if d.message.contains("viewer ticket")),
            "{err:?}"
        );
    }
    // Files: the viewer's own grant, never `run_as`.
    let err = viewer
        .stat_with(&format!("/home/{b}"), false, &FileAccess::user(b))
        .await
        .expect_err("refused");
    assert!(
        matches!(&err, Error::PermissionDenied(d) if d.message.contains("run_as is not allowed")),
        "{err:?}"
    );
    let err = viewer
        .upload(
            &format!("/home/{b}/viewer.txt"),
            bytes::Bytes::from_static(b"x"),
            UploadOptions {
                run_as: Some("root".into()),
                ..Default::default()
            },
        )
        .await
        .expect_err("refused");
    assert!(matches!(err, Error::PermissionDenied(_)), "{err:?}");
    assert!(!Path::new(&format!("/home/{b}/viewer.txt")).exists());
}
