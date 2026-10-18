// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Persistent agents end to end: a real `cua daemon`, a real local Docker
//! Space, a real harness (Claude Code) scripted by the mock provider
//! (cua-mock-llm). Opt-in: without `CUA_PERSISTENT_E2E=1` the test prints
//! why and passes. Run it through `tests/e2e/run-persistent-e2e.sh`, which
//! starts the mock provider next to the Space and passes:
//!
//! - `CUA_PERSISTENT_E2E_ENDPOINT` the model base URL as the Space sees it;
//! - `CUA_PERSISTENT_E2E_KEY` the key the mock expects (never printed);
//! - `CUA_PERSISTENT_E2E_IMAGE` the Space image;
//! - `CUA_PERSISTENT_E2E_EVIDENCE` a directory for the timings and logs.
//!
//! It proves, in order:
//!
//! 1. memory: the agent writes its memory in turn 1, the Space is deleted
//!    and created again (a fresh disk), and turn 2 reads the memory back
//!    from the home the daemon restored from the Cua Volume;
//! 2. the bridge: the agent's `notify_user` reaches the notifications feed,
//!    and its `volume_read` of another agent's folder is refused;
//! 3. routines with no client connected: a routine fires from the daemon
//!    alone, and fires again after the daemon restarts;
//! 4. pause and resume: pausing suspends the local Space and routines stop
//!    firing (the slot is refused); resuming brings the Space back and the
//!    agent answers again.
//!
//! Host safety: the daemon runs with a temporary `HOME` and `CUA_HOME` and
//! the file credential store; the only host effects are its own child
//! process (killed by handle, never by name) and the Docker containers it
//! creates (removed at the end).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_sdk::{Cua, PersistentAgentOptions, SpaceCreateOptions};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn docker_host() -> String {
    if let Some(h) = env("DOCKER_HOST") {
        return h;
    }
    let out = Command::new("docker")
        .args([
            "context",
            "inspect",
            "--format",
            "{{.Endpoints.docker.Host}}",
        ])
        .output()
        .expect("docker CLI");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn start_daemon(root: &Path, key: &str, log: &Path) -> Daemon {
    let cua_home = root.join("cua");
    let socket = cua_home.join("cua.sock");
    std::fs::create_dir_all(root.join("home")).unwrap();
    // A daemon that was killed leaves its socket file behind.
    let _ = std::fs::remove_file(&socket);
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_cua-spaces-cli"))
        .args([
            "daemon",
            "start",
            "--foreground",
            "--loopback",
            "off",
            "--socket",
        ])
        .arg(&socket)
        .env("HOME", root.join("home"))
        .env("CUA_HOME", &cua_home)
        .env("CUA_CREDENTIAL_STORE", "file")
        .env("CUA_TELEMETRY_ENABLED", "false")
        .env("CUA_ENV_TEST_SANDBOX", "1")
        .env("DOCKER_HOST", docker_host())
        .env("ANTHROPIC_API_KEY", key)
        .env("RUST_LOG", "info")
        .stdin(Stdio::null())
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .expect("start cua daemon");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !socket.exists() {
        assert!(
            Instant::now() < deadline,
            "the daemon never opened {socket:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    Daemon { child, socket }
}

fn connect(d: &Daemon) -> Arc<Cua> {
    Cua::connect(Some(d.socket.display().to_string()), None).unwrap()
}

/// Waits (bounded) until the daemon answers.
async fn ready(cua: &Arc<Cua>) {
    for _ in 0..120 {
        if cua.info().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("the daemon never answered");
}

struct Evidence(std::fs::File);
impl Evidence {
    fn line(&mut self, s: impl AsRef<str>) {
        println!("{}", s.as_ref());
        let _ = writeln!(self.0, "{}", s.as_ref());
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Waits (bounded) for a notification after `since_ms` that `pred` accepts.
async fn notification(
    cua: &Arc<Cua>,
    since_ms: u64,
    secs: u64,
    pred: impl Fn(&cua_sdk::NotificationInfo) -> bool,
) -> cua_sdk::NotificationInfo {
    let spaces = cua.spaces();
    for _ in 0..secs {
        for n in spaces.notifications(false, Some(since_ms)).await.unwrap() {
            if pred(&n) {
                return n;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let all = spaces.notifications(false, Some(since_ms)).await.unwrap();
    panic!("no matching notification in {secs}s; saw {all:#?}");
}

async fn create_space(cua: &Arc<Cua>, image: &str, name: &str) -> (String, u64) {
    let t0 = Instant::now();
    let r = cua
        .spaces()
        .create(SpaceCreateOptions {
            image: Some(image.into()),
            on: Some("local".into()),
            kind: Some("container".into()),
            runtime: Some("runc".into()),
            name: Some(name.into()),
            wait: Some(true),
            // A locally built Spaces image: say it runs cua-spacesd.
            spacesd: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    (r.space.expect("ready").id, t0.elapsed().as_millis() as u64)
}

fn docker_state(name: &str) -> String {
    let out = Command::new("docker")
        .env("DOCKER_HOST", docker_host())
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name={name}"),
            "--format",
            "{{.Names}} {{.State}}",
        ])
        .output()
        .expect("docker CLI");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn e2e_persistent_agent_lifecycle() {
    if env("CUA_PERSISTENT_E2E").as_deref() != Some("1") {
        eprintln!("skipped: set CUA_PERSISTENT_E2E=1 (run tests/e2e/run-persistent-e2e.sh)");
        return;
    }
    // The lifecycle's futures are large in a debug build: run it on a
    // thread with a stack sized for them rather than the test thread's.
    std::thread::Builder::new()
        .name("e2e_persistent".into())
        .stack_size(64 << 20)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .thread_stack_size(16 << 20)
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(Box::pin(lifecycle()))
        })
        .expect("spawn the e2e thread")
        .join()
        .unwrap_or_else(|p| std::panic::resume_unwind(p));
}

async fn lifecycle() {
    let endpoint = env("CUA_PERSISTENT_E2E_ENDPOINT").expect("CUA_PERSISTENT_E2E_ENDPOINT");
    let key = env("CUA_PERSISTENT_E2E_KEY").expect("CUA_PERSISTENT_E2E_KEY");
    let image = env("CUA_PERSISTENT_E2E_IMAGE").expect("CUA_PERSISTENT_E2E_IMAGE");
    let evidence_dir = PathBuf::from(env("CUA_PERSISTENT_E2E_EVIDENCE").unwrap_or_else(|| {
        std::env::temp_dir()
            .join("cua-persistent-e2e")
            .display()
            .to_string()
    }));
    std::fs::create_dir_all(&evidence_dir).unwrap();
    let mut ev = Evidence(std::fs::File::create(evidence_dir.join("persistent.log")).unwrap());
    let root = tempfile::tempdir().unwrap();
    let log = evidence_dir.join("daemon.log");
    let suffix = format!("{:06x}", now_ms() % 0xFF_FFFF);
    let space_name = format!("pe2e-{suffix}");
    let mut timings = serde_json::Map::new();

    let daemon = start_daemon(root.path(), &key, &log);
    let cua = connect(&daemon);
    ready(&cua).await;
    let spaces = cua.spaces();

    // 1. Memory across a Space release and recreate.
    let (space, ms) = create_space(&cua, &image, &space_name).await;
    ev.line(format!("space {space} ready in {ms} ms"));
    timings.insert("space_create_ms".into(), ms.into());
    spaces
        .persistent_agent_create(
            "ada".into(),
            "claude-code".into(),
            space.clone(),
            Some(PersistentAgentOptions {
                model: Some("claude-mock-1".into()),
                base_url: Some(endpoint.clone()),
                env_from_host: vec!["ANTHROPIC_API_KEY".into()],
                env: Default::default(),
            }),
        )
        .await
        .unwrap();
    let t = now_ms();
    let d = spaces
        .persistent_agent_send(
            "ada".into(),
            "Remember that my favorite color is teal. mock: shell mkdir -p ../claude-memory && \
             echo 'favorite color: teal' > ../claude-memory/MEMORY.md && cat \"$CLAUDE_CONFIG_DIR/settings.json\""
                .into(),
        )
        .await
        .unwrap();
    assert!(d.started);
    ev.line(format!("turn 1 run {} (install included)", d.run_id));
    let n = notification(&cua, t, 900, |n| {
        n.kind == "turn_ended" && n.agent.as_deref() == Some("ada")
    })
    .await;
    ev.line(format!(
        "turn 1 notification after {} ms: {}",
        n.at_ms - t,
        n.body
    ));
    assert!(
        n.body.contains("autoMemoryDirectory")
            && n.body.contains("cua-volume/agents/ada/claude-memory"),
        "Claude Code's auto memory points into the home: {}",
        n.body
    );
    let drive = cua_volume::Drive::open_local(&root.path().join("cua"));
    let (bytes, _) = drive
        .session(cua_volume::Context::user())
        .read("agents/ada/claude-memory/MEMORY.md", None)
        .await
        .expect("the home was saved to the drive after the turn");
    assert_eq!(
        String::from_utf8_lossy(&bytes).trim(),
        "favorite color: teal"
    );

    let t_release = Instant::now();
    spaces.delete(space.clone()).await.unwrap();
    let (space2, ms) = create_space(&cua, &image, &space_name).await;
    assert_eq!(space2, space, "the same Space id, a fresh disk");
    timings.insert("space_recreate_ms".into(), ms.into());
    let t = now_ms();
    let d = spaces
        .persistent_agent_send(
            "ada".into(),
            "What is my favorite color? mock: shell cat ../claude-memory/MEMORY.md".into(),
        )
        .await
        .unwrap();
    assert!(d.started, "the old run went with the old Space");
    let restored = d.restored.clone().unwrap();
    ev.line(format!(
        "home restored into the new Space: {} files, {} bytes, {} ms",
        restored.files, restored.bytes, restored.millis
    ));
    timings.insert("home_restore_ms".into(), restored.millis.into());
    let n = notification(&cua, t, 900, |n| {
        n.kind == "turn_ended" && n.agent.as_deref() == Some("ada")
    })
    .await;
    assert!(
        n.body.contains("favorite color: teal"),
        "memory survived: {}",
        n.body
    );
    ev.line(format!(
        "memory survived a Space release and recreate ({} ms from delete to the answer)",
        t_release.elapsed().as_millis()
    ));

    // 2. The bridge: notify_user and the drive's rules.
    drive
        .session(cua_volume::Context::user())
        .write(
            "agents/bob/private.md",
            b"bob only".to_vec(),
            cua_volume::Condition::None,
        )
        .await
        .unwrap();
    let t = now_ms();
    spaces
        .persistent_agent_send(
            "ada".into(),
            "mock: tool volume_read {\"path\": \"agents/bob/private.md\"}; tool notify_user {\"title\": \"Your research is ready\", \"body\": \"3 sources\"}".into(),
        )
        .await
        .unwrap();
    let n = notification(&cua, t, 300, |n| n.kind == "message").await;
    assert_eq!(n.title, "Your research is ready");
    assert_eq!(n.agent.as_deref(), Some("ada"));
    let n = notification(&cua, t, 300, |n| n.kind == "turn_ended").await;
    let (events, _) = drive.audit().tail(50).unwrap();
    assert!(
        events.iter().any(|e| e.action == "denied"
            && e.principal == "agent:ada"
            && e.path == "agents/bob/private.md"),
        "the refusal is audited: {events:#?}"
    );
    ev.line(format!(
        "bridge: notify_user delivered, volume_read of agents/bob refused ({})",
        n.body.lines().next().unwrap_or("")
    ));

    // 3. A routine fires from the daemon with no client connected, and
    //    again after the daemon restarts.
    let r = spaces
        .routine_add(
            "ada".into(),
            "heartbeat".into(),
            "mock: say routine ran".into(),
            Some(1),
            None,
            None,
        )
        .await
        .unwrap();
    let created = now_ms();
    drop(spaces);
    drop(cua);
    ev.line("client disconnected; waiting for the routine");
    let cua = connect(&daemon);
    let n = notification(&cua, created, 150, |n| {
        n.kind == "turn_ended" && n.body.contains("routine ran")
    })
    .await;
    let due = created + 60_000;
    let fired = cua.spaces().routines(Some("ada".into())).await.unwrap();
    let fired_at = fired
        .iter()
        .find(|x| x.id == r.id)
        .and_then(|x| x.last_fired_at.clone())
        .unwrap();
    ev.line(format!(
        "routine fired (last_fired_at {fired_at}); turn ended {} ms after the slot",
        n.at_ms.saturating_sub(due)
    ));
    timings.insert(
        "routine_turn_end_after_due_ms".into(),
        n.at_ms.saturating_sub(due).into(),
    );
    drop(cua);
    drop(daemon);
    ev.line("daemon stopped; starting it again");
    let daemon = start_daemon(root.path(), &key, &log);
    let cua = connect(&daemon);
    ready(&cua).await;
    let after_restart = now_ms();
    let listed = cua.spaces().routines(Some("ada".into())).await.unwrap();
    assert!(
        listed.iter().any(|x| x.id == r.id),
        "the routine survived the restart"
    );
    let n = notification(&cua, after_restart, 150, |n| {
        n.kind == "turn_ended" && n.body.contains("routine ran")
    })
    .await;
    ev.line(format!(
        "routine fired again after the restart ({} ms after it)",
        n.at_ms - after_restart
    ));

    // 4. Pause and resume.
    let spaces = cua.spaces();
    let p = spaces.agent_pause("ada".into()).await.unwrap();
    assert_eq!(p.space_state, "suspended");
    assert!(
        docker_state(&space_name).contains("paused"),
        "{}",
        docker_state(&space_name)
    );
    timings.insert("pause_ms".into(), p.millis.into());
    ev.line(format!(
        "paused in {} ms; container: {}",
        p.millis,
        docker_state(&space_name)
    ));
    let paused_at = now_ms();
    // The next slot is refused, not fired.
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let r = spaces.routines(Some("ada".into())).await.unwrap();
        let outcome = r[0].last_outcome.clone().unwrap_or_default();
        if outcome.contains("paused") {
            ev.line(format!("routine slot while paused: {outcome}"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no refused slot while paused: {outcome}"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(
        spaces
            .notifications(false, Some(paused_at))
            .await
            .unwrap()
            .iter()
            .all(|n| n.kind != "turn_ended"),
        "a paused agent does not run"
    );
    spaces
        .routine_set_enabled(r.id.clone(), false)
        .await
        .unwrap();
    let t0 = Instant::now();
    let t = now_ms();
    let res = spaces
        .agent_resume("ada".into(), Some("mock: say back".into()))
        .await
        .unwrap();
    assert!(!docker_state(&space_name).contains("paused"));
    let n = notification(&cua, t, 300, |n| {
        n.kind == "turn_ended" && n.body.contains("back")
    })
    .await;
    let ready = t0.elapsed().as_millis() as u64;
    ev.line(format!(
        "resumed: home back in {} ms, first answer {} ms after resume ({})",
        res.ready_ms,
        ready,
        n.body.lines().next().unwrap_or("")
    ));
    timings.insert("resume_ready_ms".into(), res.ready_ms.into());
    timings.insert("resume_to_first_answer_ms".into(), ready.into());

    spaces.persistent_agent_remove("ada".into()).await.unwrap();
    spaces.delete(space).await.unwrap();
    std::fs::write(
        evidence_dir.join("timings.json"),
        serde_json::to_vec_pretty(&timings).unwrap(),
    )
    .unwrap();
    ev.line(format!("timings: {}", serde_json::Value::Object(timings)));
}
