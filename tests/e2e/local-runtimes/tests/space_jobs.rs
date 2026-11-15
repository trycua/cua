//! Background jobs in a real Linux Space (`CUA_E2E_SPACE_JOBS=1`).
//!
//! For each image, the test creates a Space through the Spaces SDK the apps
//! use and:
//!
//! 1. runs the runtime-agnostic conformance checks (`space_jobs::conformance`:
//!    exit status, log offsets, follow, cancel, timeout, conditions, list);
//! 2. starts a two minute job that prints a line every five seconds, then
//!    drops every client connection to the Space: the job keeps running and a
//!    new connection reads its log from the offset the old one reached;
//! 3. checks the job ended `exited`, exit 0, with every tick in order.
//!
//! A second test (`a_job_outlives_a_daemon_restart`) runs cua-spacesd in a
//! plain container under a restart loop, kills it while a job runs, and checks
//! the job's log kept growing and its result is intact. It needs the `docker`
//! command on `PATH` (and `DOCKER_HOST` for the engine to use).
//!
//! The daemon side (a finished job waking an agent) is tested against the same
//! kind of Space in `cua-spaces-ext/tests/e2e_space_job_wake.rs`, because this
//! crate is MIT and may not depend on the source-available crates.
//!
//! | env | meaning |
//! |---|---|
//! | `CUA_HOME` | required, a temporary directory (never `~/.cua`); keep it short |
//! | `CUA_E2E_SPACE_JOBS_IMAGES` | comma separated images (default `alpine:3.20,python:3.12-slim`) |
//! | `CUA_E2E_OCI_ANY_RUNTIME` | the container engine (default `auto`; `runc` where there is no gVisor) |
//! | `CUA_SPACESD_STATIC` | the static Linux cua-spacesd to inject |

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_daemon::local::VmmLocal;
use cua_e2e_local_runtimes::{gated, init_tracing, run_id};
use cua_sandbox_core::placement::{Kind, On};
use cua_spaces::space_jobs::{self, conformance};
use cua_spaces::{Space, SpaceCreate, SpaceCreated, Spaces};
use cua_spaces_contract::inputs as i;

fn spaces(home: &std::path::Path) -> Spaces {
    Spaces::builder()
        .home(home)
        .download_dir(home.join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .local_runtime(Arc::new(VmmLocal::default()))
        .probe_timeout(Duration::from_secs(20))
        .build()
}

async fn create(spaces: &Spaces, image: &str, name: &str) -> cua_spaces::SpaceInfo {
    let created = spaces
        .create(SpaceCreate {
            image: Some(image.into()),
            on: Some(On::Local),
            kind: Kind::Container,
            runtime: std::env::var("CUA_E2E_OCI_ANY_RUNTIME")
                .ok()
                .map(|r| cua_sandbox_core::placement::Runtime::parse(&r).expect("a runtime"))
                .unwrap_or_default(),
            name: Some(name.into()),
            cpus: Some(1),
            memory_mb: Some(1024),
            timeout: Some(Duration::from_secs(600)),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("create {image}: {e}"));
    let SpaceCreated::Ready { info, .. } = created else {
        panic!("{image} is not ready: {created:?}");
    };
    info
}

/// The Space's cua-spacesd pids, by name, as its own shell sees them.
async fn spacesd_pids(space: &Space) -> Vec<String> {
    let out = space
        .bash(
            "pidof cua-spacesd 2>/dev/null || pgrep -x cua-spacesd",
            Duration::from_secs(30),
        )
        .await
        .expect("pidof");
    out.stdout.split_whitespace().map(str::to_string).collect()
}

/// Answers once the Space's daemon does, for up to `within`.
async fn space_again(spaces: &Spaces, id: &str, within: Duration) -> Space {
    let until = Instant::now() + within;
    loop {
        if let Ok(space) = spaces.space(id).await
            && space
                .bash("true", Duration::from_secs(10))
                .await
                .is_ok_and(|o| o.success())
        {
            return space;
        }
        assert!(
            Instant::now() < until,
            "the Space did not answer in {within:?}"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_outlives_the_client_and_the_daemon() {
    if !gated("CUA_E2E_SPACE_JOBS") {
        return;
    }
    init_tracing();
    let home = std::env::var("CUA_HOME").expect("set CUA_HOME to a temporary directory");
    assert!(
        !home.is_empty() && !home.ends_with("/.cua"),
        "never the real cua home"
    );
    let reg = tempfile::tempdir().unwrap();
    let spaces = spaces(reg.path());
    let images = std::env::var("CUA_E2E_SPACE_JOBS_IMAGES")
        .unwrap_or_else(|_| "alpine:3.20,python:3.12-slim".into());
    for image in images.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let name = format!("e2e-jobs-{}-{}", run_id(), image.split(':').next().unwrap());
        let info = create(&spaces, image, &name).await;
        eprintln!("{image}: {}", info.id);
        let space = spaces.space(&info.id).await.unwrap();

        // 1. The conformance checks.
        let t = Instant::now();
        conformance::run(&space)
            .await
            .unwrap_or_else(|e| panic!("{image}: {e}"));
        eprintln!(
            "TIMING {image} conformance: {:.1}s",
            t.elapsed().as_secs_f64()
        );

        // 2. A two minute job, then every client goes away.
        let started = space
            .job_start(&i::SpaceJob {
                action: "start".into(),
                name: Some("two minute job".into()),
                command: Some(
                    "i=0; while [ $i -lt 24 ]; do echo tick $i; i=$((i+1)); sleep 5; done; echo finished"
                        .into(),
                ),
                ..Default::default()
            })
            .await
            .expect("start the long job");
        let job = started.job.clone();
        eprintln!("{image}: job {job} started");
        tokio::time::sleep(Duration::from_secs(12)).await;
        let seen = space
            .job_logs(&job, Some(0), 1000, Duration::ZERO)
            .await
            .unwrap();
        assert!(seen.text.contains("tick 0"), "{image}: {seen:?}");
        let offset = seen.next_offset;
        drop(space);
        spaces.forget_connection(&info.id).await.unwrap();
        tokio::time::sleep(Duration::from_secs(15)).await;

        let space = spaces.space(&info.id).await.unwrap();
        let after = space
            .job_logs(&job, Some(offset), 1000, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(after.status.state, "running", "{image}: {after:?}");
        assert!(
            after.text.contains("tick 3") || after.text.contains("tick 2"),
            "{image}: the job kept printing while no client was connected: {after:?}"
        );

        // 3. It ends exited 0 with every tick, in order.
        let end = Instant::now() + Duration::from_secs(180);
        let done = loop {
            let st = space.job_status(&job).await.unwrap();
            if st.finished() {
                break st;
            }
            assert!(Instant::now() < end, "{image}: the job did not end");
            tokio::time::sleep(Duration::from_secs(3)).await;
        };
        assert_eq!(
            (done.state.as_str(), done.exit_code),
            ("exited", Some(0)),
            "{image}: {done:?}"
        );
        assert!(
            done.elapsed_s >= 110,
            "{image}: ran {}s, expected about 120",
            done.elapsed_s
        );
        let full = space
            .job_logs(&job, Some(0), 100_000, Duration::ZERO)
            .await
            .unwrap();
        let ticks: Vec<&str> = full.text.lines().collect();
        let want: Vec<String> = (0..24)
            .map(|n| format!("tick {n}"))
            .chain(std::iter::once("finished".to_string()))
            .collect();
        assert_eq!(
            ticks, want,
            "{image}: the log has every line, once, in order"
        );
        eprintln!("{image}: {}", space_jobs::result_line(&done));
        drop(space);
        spaces.delete(&info.id).await.unwrap();
    }
}

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

#[tokio::test(flavor = "multi_thread")]
async fn a_job_outlives_a_daemon_restart() {
    if !gated("CUA_E2E_SPACE_JOBS") {
        return;
    }
    init_tracing();
    let bin = std::path::PathBuf::from(
        std::env::var("CUA_SPACESD_STATIC").expect("set CUA_SPACESD_STATIC"),
    );
    let name = format!("cua-e2e-jobs-restart-{}", run_id());
    let _guard = Container(name.clone());
    let token = format!("tok-{}", run_id());
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

    let reg = tempfile::tempdir().unwrap();
    let spaces = spaces(reg.path());
    let info = spaces
        .add(
            &format!("http://{addr}"),
            Some(token.clone()),
            Some("restart".into()),
        )
        .await
        .expect("add the Space");
    let space = space_again(&spaces, &info.id, Duration::from_secs(60)).await;

    let job = space
        .job_start(&i::SpaceJob {
            action: "start".into(),
            name: Some("survives a restart".into()),
            command: Some(
                "i=0; while [ $i -lt 12 ]; do echo tick $i; i=$((i+1)); sleep 5; done; echo finished"
                    .into(),
            ),
            ..Default::default()
        })
        .await
        .expect("start")
        .job;
    tokio::time::sleep(Duration::from_secs(12)).await;
    let before = spacesd_pids(&space).await;
    docker(&["exec", &name, "sh", "-c", "kill $(pidof cua-spacesd)"]);
    drop(space);
    spaces.forget_connection(&info.id).await.unwrap();
    let space = space_again(&spaces, &info.id, Duration::from_secs(60)).await;
    let after = spacesd_pids(&space).await;
    assert!(
        !after.is_empty() && after != before,
        "cua-spacesd did not restart: {before:?} -> {after:?}"
    );
    eprintln!("cua-spacesd {before:?} -> {after:?}");
    let mid = space
        .job_logs(&job, Some(0), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(mid.status.state, "running", "{mid:?}");
    assert!(mid.text.contains("tick 0"), "{mid:?}");

    let end = Instant::now() + Duration::from_secs(120);
    let done = loop {
        let st = space.job_status(&job).await.unwrap();
        if st.finished() {
            break st;
        }
        assert!(Instant::now() < end, "the job did not end");
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    assert_eq!(
        (done.state.as_str(), done.exit_code),
        ("exited", Some(0)),
        "{done:?}"
    );
    let full = space
        .job_logs(&job, Some(0), 100_000, Duration::ZERO)
        .await
        .unwrap();
    let want: Vec<String> = (0..12)
        .map(|n| format!("tick {n}"))
        .chain(std::iter::once("finished".to_string()))
        .collect();
    assert_eq!(full.text.lines().collect::<Vec<_>>(), want);
    eprintln!("{}", space_jobs::result_line(&done));
}
