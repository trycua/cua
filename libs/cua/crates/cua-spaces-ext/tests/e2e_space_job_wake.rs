// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A finished `space_job` wakes the agent that started it, against a real
//! Linux Space (`CUA_E2E_SPACE_JOBS=1`): cua-spacesd runs in an alpine
//! container under a restart loop, the job starts with `notify` as an agent
//! member, and the supervisor runs until the agent has the job's result as a
//! task in the Space's thread.
//!
//! | env | meaning |
//! |---|---|
//! | `CUA_SPACESD_STATIC` | the static Linux cua-spacesd to run |
//! | `DOCKER_HOST` | the engine (the `docker` command must be on `PATH`) |

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use serde_json::{Value, json};

fn docker(args: &[&str]) -> String {
    let out = std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("the docker command");
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Removes the container however the test ends.
struct Container(String);
impl Drop for Container {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.0])
            .output();
    }
}

fn uniq() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            % 0xffffff
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_job_wakes_the_agent() {
    if std::env::var("CUA_E2E_SPACE_JOBS").as_deref() != Ok("1") {
        eprintln!("skipping: set CUA_E2E_SPACE_JOBS=1 to run");
        return;
    }
    let bin = std::path::PathBuf::from(
        std::env::var("CUA_SPACESD_STATIC").expect("set CUA_SPACESD_STATIC"),
    );
    let name = format!("cua-e2e-jobs-wake-{}", uniq());
    let _guard = Container(name.clone());
    let token = format!("tok-{}", uniq());
    docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "-p",
        "127.0.0.1::3211",
        "-e",
        &format!("CUA_ENV_TOKEN={token}"),
        "-v",
        &format!("{}:/cua-spacesd:ro", bin.display()),
        "alpine:3.20",
        "sh",
        "-c",
        "while true; do /cua-spacesd; sleep 1; done",
    ]);
    let addr = docker(&["port", &name, "3211"]);
    let addr = addr.lines().next().unwrap().to_string();

    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder()
            .home(home.path())
            .download_dir(home.path().join("downloads"))
            .operator_display(Arc::new(cua_spaces::operator::NoDisplay)),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let info = spaces
        .add(&format!("http://{addr}"), Some(token), Some("wake".into()))
        .await
        .expect("add the Space");
    let up = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(s) = spaces.space(&info.id).await
            && s.bash("true", Duration::from_secs(10))
                .await
                .is_ok_and(|o| o.success())
        {
            break;
        }
        assert!(Instant::now() < up, "the Space did not answer");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let mcp = McpServer::new(spaces.clone());
    let call = |caller: Caller, tool: &'static str, args: Value| {
        let mcp = mcp.clone();
        async move {
            let out = caller.scope(Box::pin(mcp.call(tool, args))).await;
            cua_spaces::extension::outcome_json(out).unwrap_or_else(|e| panic!("{tool}: {e}"))
        }
    };
    call(
        Caller::Owner,
        "agent_add",
        json!({"space": info.id, "name": "ada", "harness": "claude-code",
               "env_from_host": ["ANTHROPIC_API_KEY"]}),
    )
    .await;

    let started = call(
        Caller::Agent {
            name: "ada".into(),
            space: info.id.clone(),
        },
        "space_job",
        json!({"space": info.id, "action": "start", "name": "wake me",
               "command": "sleep 8; echo hello from the job", "notify": true}),
    )
    .await;
    assert_eq!(started["state"], "running", "{started}");
    assert_eq!(started["notify"]["registered"], true, "{started}");

    // The supervisor follows the job; the agent has the result as a task.
    let p = spaces.persistent();
    let until = Instant::now() + Duration::from_secs(90);
    let task = loop {
        p.tick().await;
        let inbox = call(
            Caller::Owner,
            "thread_inbox",
            json!({"space": info.id, "member": "ada"}),
        )
        .await;
        if let Some(t) = inbox["tasks"].as_array().and_then(|t| t.first()) {
            break t.clone();
        }
        assert!(Instant::now() < until, "ada was never woken");
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    let text = task["text"].as_str().unwrap();
    eprintln!("{text}");
    assert!(text.starts_with("JOB j"), "{text}");
    assert!(text.contains(" exited exit=0 "), "{text}");
    assert!(text.contains("| wake me"), "{text}");
    assert!(text.contains("hello from the job"), "{text}");
    assert!(
        p.watched_jobs().unwrap().is_empty(),
        "the job is no longer followed"
    );
}
