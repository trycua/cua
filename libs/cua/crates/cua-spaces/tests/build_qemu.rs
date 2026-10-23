//! A real build: a three-step Dockerfile on a QEMU Linux Space, then the
//! cache (a rebuild runs nothing, a changed file rebuilds from its COPY),
//! a Space made from the tag, and the failure path.
//!
//! Opt-in (it boots VMs and pulls the Linux image): set `CUA_BUILD_E2E=1` on
//! a machine with the local runtime, for the macOS/nightly lane. The test
//! uses its own cua home and removes every Space and checkpoint it makes.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_spaces::Spaces;
use cua_spaces::build::{BuildEvent, BuildOptions};

fn enabled() -> bool {
    if std::env::var("CUA_BUILD_E2E").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!("skipped: set CUA_BUILD_E2E=1 (boots a QEMU Linux Space)");
    false
}

/// The Dockerfile; `FROM` is `ubuntu:24.04` unless `CUA_BUILD_E2E_BASE` says
/// otherwise (CI sets `linux` with `CUA_IMAGE_LINUX` pointing at a disk it
/// fetched, so the build needs no registry).
fn dockerfile() -> String {
    let base = std::env::var("CUA_BUILD_E2E_BASE").unwrap_or_else(|_| "ubuntu:24.04".into());
    format!("FROM {base}\n{BODY}")
}

const BODY: &str = "ARG GREETING=hi
ENV APP=/srv/app
WORKDIR /srv/app
COPY hello.txt ./
RUN echo \"$GREETING\" > greeting && chmod 600 greeting
USER cua
RUN whoami > /tmp/who
";

fn options(ctx: &Path, tag: &str, events: Arc<Mutex<Vec<BuildEvent>>>) -> BuildOptions {
    BuildOptions {
        dockerfile: Some(dockerfile()),
        context_dir: Some(ctx.to_path_buf()),
        tag: Some(tag.into()),
        cpus: Some(2),
        memory_mb: Some(2048),
        step_timeout: Some(Duration::from_secs(600)),
        progress: Some(Arc::new(move |e: &BuildEvent| {
            events.lock().unwrap().push(e.clone())
        })),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn builds_caches_forks_and_fails_cleanly() {
    if !enabled() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let ctx = tempfile::tempdir().unwrap();
    std::fs::write(ctx.path().join("hello.txt"), "hello\n").unwrap();
    // SAFETY: set before any other thread in this test reads it.
    unsafe { std::env::set_var("CUA_HOME", home.path()) };
    let spaces = Spaces::builder().home(home.path()).build();
    let tag = format!("e2e-{}", std::process::id());
    let events = Arc::new(Mutex::new(Vec::new()));

    let t = Instant::now();
    let first = spaces
        .build(options(ctx.path(), &tag, events.clone()))
        .await
        .unwrap();
    let cold = t.elapsed();
    assert!(!first.cached, "{first:?}");
    assert_eq!(first.config.env["APP"], "/srv/app");
    assert_eq!(first.config.workdir.as_deref(), Some("/srv/app"));
    assert_eq!(first.config.user.as_deref(), Some("cua"));

    // Cache hit: nothing runs, and it is fast.
    let t = Instant::now();
    let again = spaces
        .build(options(ctx.path(), &tag, Default::default()))
        .await
        .unwrap();
    let hit = t.elapsed();
    assert!(
        again.cached && again.steps.iter().all(|s| s.cached),
        "{again:?}"
    );
    assert_eq!(again.checkpoint, first.checkpoint);
    eprintln!("PERF cache miss {cold:?}, cache hit {hit:?}");

    // A Space from the tag: files, owners, env, workdir.
    let created = spaces
        .create(cua_spaces::SpaceCreate {
            image: Some(format!("build:{tag}")),
            name: Some(format!("{tag}-a")),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
    let space = spaces.space(&created.id).await.unwrap();
    let out = space
        .bash(
            "pwd; echo $APP; whoami; sudo -n cat greeting; stat -c '%U %a' greeting; stat -c '%U' hello.txt; cat /tmp/who",
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(out.success(), "{}", out.render());
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines[0], "/srv/app", "{}", out.stdout);
    assert_eq!(lines[1], "/srv/app");
    assert_eq!(lines[2], "cua");
    assert_eq!(lines[3], "hi");
    assert_eq!(lines[4], "root 600");
    assert_eq!(lines[5], "root");
    assert_eq!(lines[6], "cua");
    spaces.delete(&created.id).await.unwrap();

    // A changed context file rebuilds the COPY and what follows only.
    std::fs::write(ctx.path().join("hello.txt"), "changed\n").unwrap();
    let changed = spaces
        .build(options(ctx.path(), &tag, Default::default()))
        .await
        .unwrap();
    let cached: Vec<bool> = changed.steps.iter().map(|s| s.cached).collect();
    // FROM, ENV and WORKDIR cached; COPY and everything after it not.
    assert_eq!(cached[..4], [true, true, true, false], "{cached:?}");
    assert!(cached[3..].iter().all(|c| !c), "{cached:?}");
    assert_ne!(changed.checkpoint, first.checkpoint);

    // A failing step: its command, exit code and output; earlier layers stay.
    let err = spaces
        .build(BuildOptions {
            base: Some(
                std::env::var("CUA_BUILD_E2E_BASE").unwrap_or_else(|_| "ubuntu:24.04".into()),
            ),
            steps: vec![cua_spaces::build::BuildStep::Run {
                command: "echo about to fail; exit 7".into(),
                user: None,
                workdir: None,
                env: Default::default(),
            }],
            ..Default::default()
        })
        .await
        .unwrap_err();
    let cua_spaces::Error::Build {
        exit_code, output, ..
    } = &err
    else {
        panic!("{err:?}")
    };
    assert_eq!(*exit_code, Some(7));
    assert!(output.contains("about to fail"), "{output}");
    assert!(
        spaces
            .list()
            .unwrap()
            .iter()
            .all(|s| !s.name.starts_with("build-")),
        "the builder was deleted"
    );

    // Clean up everything this test made.
    for b in spaces.builds().unwrap() {
        spaces.delete_build(&b.key).await.unwrap();
    }
    assert!(spaces.builds().unwrap().is_empty());
}
