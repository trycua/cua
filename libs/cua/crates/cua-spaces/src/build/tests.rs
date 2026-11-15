//! The engine against a fake builder: cache hits and misses, resuming from
//! the deepest layer, failure cleanup and the commands a step runs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::engine::{
    self, BuildEvent, BuildHost, BuilderSpec, ExecOutcome, ExecSpec, LineSink, RunOpts,
};
use super::plan::{self, Shape};
use super::recipe::{BuildRecipe, BuildStep};
use super::store;
use crate::error::{Error, Result};

#[derive(Default)]
struct State {
    layers: BTreeSet<String>,
    builders: Vec<String>,
    deleted_builders: Vec<String>,
    /// Every command run, with the builder it ran in.
    execs: Vec<(String, ExecSpec)>,
    puts: Vec<(String, String)>,
    /// Layer a builder was forked from (None: started from the base).
    started_from: Vec<Option<String>>,
    /// Commands containing this text fail with the exit code.
    fail_on: Option<(String, i32)>,
    /// Layers whose checkpoint cannot be forked (their files are gone).
    broken: BTreeSet<String>,
    /// The base image's resolved digest.
    digest: Option<String>,
    next: u32,
}

struct Fake {
    home: tempfile::TempDir,
    st: Mutex<State>,
}

impl Fake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            home: tempfile::tempdir().unwrap(),
            st: Mutex::new(State::default()),
        })
    }

    fn execs(&self) -> Vec<ExecSpec> {
        self.st
            .lock()
            .unwrap()
            .execs
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// Shell lines of the commands that ran (`sh -c <line>`), as root
    /// through sudo or directly.
    fn lines(&self) -> Vec<String> {
        self.execs()
            .iter()
            .filter(|e| e.program != "id")
            .map(|e| {
                let mut all = vec![e.program.clone()];
                all.extend(e.args.clone());
                all.last().cloned().unwrap_or_default()
            })
            .collect()
    }
}

#[async_trait]
impl BuildHost for Fake {
    fn home(&self) -> &Path {
        self.home.path()
    }
    async fn has_layer(&self, name: &str) -> bool {
        self.st.lock().unwrap().layers.contains(name)
    }
    async fn start_base(&self, spec: &BuilderSpec) -> Result<String> {
        let mut s = self.st.lock().unwrap();
        s.next += 1;
        let id = format!("local:{}-{}", spec.name, s.next);
        s.builders.push(id.clone());
        s.started_from.push(None);
        Ok(id)
    }
    async fn start_layer(&self, layer: &str, name: &str) -> Result<String> {
        let mut s = self.st.lock().unwrap();
        assert!(s.layers.contains(layer), "fork of a missing layer {layer}");
        if s.broken.contains(layer) {
            return Err(Error::NotFound(format!("{layer}: its files are gone")));
        }
        s.next += 1;
        let id = format!("local:{name}-{}", s.next);
        s.builders.push(id.clone());
        s.started_from.push(Some(layer.to_string()));
        Ok(id)
    }
    async fn exec(
        &self,
        builder: &str,
        spec: ExecSpec,
        on_line: &mut dyn LineSink,
    ) -> Result<ExecOutcome> {
        let mut s = self.st.lock().unwrap();
        s.execs.push((builder.to_string(), spec.clone()));
        if spec.program == "id" {
            on_line.line("cua");
            return Ok(ExecOutcome {
                code: Some(0),
                ..Default::default()
            });
        }
        let text = format!("{} {}", spec.program, spec.args.join(" "));
        if let Some((needle, code)) = &s.fail_on
            && text.contains(needle.as_str())
        {
            for i in 0..60 {
                on_line.line(&format!("output line {i}"));
            }
            return Ok(ExecOutcome {
                code: Some(*code),
                ..Default::default()
            });
        }
        on_line.line(&format!("ran: {text}"));
        Ok(ExecOutcome {
            code: Some(0),
            ..Default::default()
        })
    }
    async fn put(&self, builder: &str, path: &str, _data: Vec<u8>, _mode: u32) -> Result<()> {
        self.st
            .lock()
            .unwrap()
            .puts
            .push((builder.to_string(), path.to_string()));
        Ok(())
    }
    async fn put_file(&self, builder: &str, path: &str, _host: &Path) -> Result<()> {
        self.st
            .lock()
            .unwrap()
            .puts
            .push((builder.to_string(), path.to_string()));
        Ok(())
    }
    async fn save_layer(&self, _builder: &str, name: &str) -> Result<()> {
        let mut s = self.st.lock().unwrap();
        if !s.layers.insert(name.to_string()) {
            return Err(Error::invalid("already a checkpoint"));
        }
        Ok(())
    }
    async fn drop_layer(&self, name: &str) {
        self.st.lock().unwrap().layers.remove(name);
    }
    async fn drop_builder(&self, builder: &str) {
        self.st
            .lock()
            .unwrap()
            .deleted_builders
            .push(builder.to_string());
    }
}

fn shape() -> Shape {
    Shape {
        kind: "vm".into(),
        cpus: 2,
        memory_mb: 4096,
        headless: true,
    }
}

fn opts(events: Option<Arc<Mutex<Vec<BuildEvent>>>>) -> RunOpts {
    RunOpts {
        tag: None,
        no_cache: false,
        step_timeout: Duration::from_secs(60),
        sink: events.map(|e| {
            Arc::new(move |ev: &BuildEvent| e.lock().unwrap().push(ev.clone())) as engine::BuildSink
        }),
    }
}

fn run_step(c: &str) -> BuildStep {
    BuildStep::Run {
        command: c.into(),
        user: None,
        workdir: None,
        env: Default::default(),
    }
}

fn recipe(steps: Vec<BuildStep>) -> BuildRecipe {
    BuildRecipe {
        base: "ubuntu:24.04".into(),
        steps,
    }
}

async fn build(
    fake: &Arc<Fake>,
    r: &BuildRecipe,
    ctx: Option<&Path>,
    o: &RunOpts,
) -> Result<super::BuildResult> {
    let digest = fake.st.lock().unwrap().digest.clone();
    let p = plan::plan(r, ctx, shape(), digest.as_deref())?;
    engine::run(&**fake, &p, o).await
}

fn three_steps() -> BuildRecipe {
    recipe(vec![
        BuildStep::Env {
            vars: [("APP".to_string(), "/srv/app".to_string())].into(),
        },
        run_step("echo one"),
        run_step("echo two"),
        run_step("echo three"),
    ])
}

#[tokio::test]
async fn a_cold_build_makes_a_pinned_layer_per_filesystem_step() {
    let fake = Fake::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let r = build(&fake, &three_steps(), None, &opts(Some(events.clone())))
        .await
        .unwrap();
    assert!(!r.cached);
    // Base + three RUN layers; ENV made none.
    assert_eq!(fake.st.lock().unwrap().layers.len(), 4);
    assert_eq!(r.steps.len(), 5);
    assert_eq!(r.steps[0].text, "FROM ubuntu:24.04");
    assert!(r.steps.iter().all(|s| !s.cached));
    assert_eq!(
        r.steps[1].checkpoint, None,
        "ENV makes no layer: {:?}",
        r.steps[1]
    );
    assert_eq!(r.checkpoint, r.steps[4].checkpoint.clone().unwrap());
    assert!(r.checkpoint.starts_with("build-"));
    assert_eq!(r.config.env["APP"], "/srv/app");
    assert_eq!(r.config.driver_user, "cua");
    // One builder, deleted at the end.
    let st = fake.st.lock().unwrap();
    assert_eq!(st.builders.len(), 1);
    assert_eq!(st.deleted_builders, st.builders);
    assert_eq!(st.started_from, [None]);
    // Progress: steps in order, then Finished.
    let ev = events.lock().unwrap();
    let steps: Vec<u32> = ev
        .iter()
        .filter_map(|e| match e {
            BuildEvent::Step { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(steps, [0, 1, 2, 3, 4]);
    assert!(matches!(
        ev.last(),
        Some(BuildEvent::Finished { cached: false, .. })
    ));
    assert!(
        ev.iter()
            .any(|e| matches!(e, BuildEvent::Output { line, .. } if line.contains("echo two")))
    );
}

#[tokio::test]
async fn an_identical_rebuild_is_a_cache_hit_that_starts_no_builder() {
    let fake = Fake::new();
    let first = build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap();
    let before = fake.execs().len();
    let again = build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap();
    assert!(again.cached);
    assert!(again.steps.iter().all(|s| s.cached));
    assert_eq!(again.checkpoint, first.checkpoint);
    assert_eq!(again.key, first.key);
    assert_eq!(fake.execs().len(), before, "nothing ran");
    assert_eq!(
        fake.st.lock().unwrap().builders.len(),
        1,
        "no second builder"
    );
    assert_eq!(again.config.driver_user, "cua");
}

#[tokio::test]
async fn a_changed_step_rebuilds_from_the_deepest_unchanged_layer() {
    let fake = Fake::new();
    let first = build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap();
    let mut changed = three_steps();
    changed.steps[2] = run_step("echo TWO");
    let r = build(&fake, &changed, None, &opts(None)).await.unwrap();
    assert!(!r.cached);
    // ENV (1), "echo one" (2) cached; "echo TWO" (3) and "echo three" (4) ran.
    let cached: Vec<bool> = r.steps.iter().map(|s| s.cached).collect();
    assert_eq!(cached, [true, true, true, false, false]);
    assert_ne!(r.checkpoint, first.checkpoint);
    let st = fake.st.lock().unwrap();
    // The second builder forked the "echo one" layer.
    assert_eq!(st.started_from.len(), 2);
    assert_eq!(st.started_from[1], first.steps[2].checkpoint);
    drop(st);
    let ran: Vec<String> = fake
        .lines()
        .into_iter()
        .filter(|l| l.contains("TWO") || l.contains("three"))
        .collect();
    assert_eq!(ran.len(), 3, "echo three twice and echo TWO once: {ran:?}");
}

#[tokio::test]
async fn a_cached_layer_that_cannot_be_forked_is_dropped_and_rebuilt() {
    let fake = Fake::new();
    let first = build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap();
    // "echo two" is the layer the next build would start from.
    let two = first.steps[3].checkpoint.clone().unwrap();
    fake.st.lock().unwrap().broken.insert(two.clone());
    let mut changed = three_steps();
    changed.steps[3] = run_step("echo four");
    let r = build(&fake, &changed, None, &opts(None)).await.unwrap();
    let cached: Vec<bool> = r.steps.iter().map(|s| s.cached).collect();
    // Back to the layer before it: "echo two" and "echo four" ran.
    assert_eq!(cached, [true, true, true, false, false]);
    let st = fake.st.lock().unwrap();
    assert!(st.layers.contains(&two), "rebuilt under the same key");
    assert_eq!(st.started_from.last().unwrap(), &first.steps[2].checkpoint);
}

#[tokio::test]
async fn a_container_build_saves_only_the_base_and_the_result() {
    let fake = Fake::new();
    let r = recipe(vec![
        run_step("echo one"),
        run_step("echo two"),
        run_step("echo three"),
    ]);
    let mut shape = shape();
    shape.kind = "container".into();
    let p = plan::plan(&r, None, shape, None).unwrap();
    let res = engine::run(&*fake, &p, &opts(None)).await.unwrap();
    assert_eq!(fake.st.lock().unwrap().layers.len(), 2, "base and result");
    let saved: Vec<bool> = res.steps.iter().map(|s| s.checkpoint.is_some()).collect();
    assert_eq!(saved, [true, false, false, true]);
    // Cached again: no builder.
    let again = engine::run(&*fake, &p, &opts(None)).await.unwrap();
    assert!(again.cached);
}

#[tokio::test]
async fn no_cache_rebuilds_every_layer() {
    let fake = Fake::new();
    build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap();
    let mut o = opts(None);
    o.no_cache = true;
    let r = build(&fake, &three_steps(), None, &o).await.unwrap();
    assert!(!r.cached);
    assert!(r.steps.iter().all(|s| !s.cached));
    assert_eq!(fake.st.lock().unwrap().layers.len(), 4);
    assert_eq!(fake.st.lock().unwrap().started_from.len(), 2);
}

#[tokio::test]
async fn a_failed_step_names_the_command_and_leaves_no_checkpoint_for_it() {
    let fake = Fake::new();
    fake.st.lock().unwrap().fail_on = Some(("echo two".into(), 7));
    let e = build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap_err();
    let Error::Build {
        step,
        command,
        exit_code,
        output,
    } = &e
    else {
        panic!("{e:?}")
    };
    assert_eq!(*step, 3);
    assert_eq!(command, "RUN echo two");
    assert_eq!(*exit_code, Some(7));
    // The last 40 lines.
    assert!(
        output.contains("output line 59") && !output.contains("output line 19\n"),
        "{output}"
    );
    assert_eq!(output.lines().count(), 40);
    let msg = e.to_string();
    assert!(msg.contains("step 3") && msg.contains("exit 7"), "{msg}");
    {
        let st = fake.st.lock().unwrap();
        // Base and "echo one" stay cached; the failed step's layer is not saved.
        assert_eq!(st.layers.len(), 2);
        assert_eq!(st.deleted_builders, st.builders, "the builder is deleted");
    }
    // Fixed, the next build resumes after the cached layers.
    fake.st.lock().unwrap().fail_on = None;
    let r = build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap();
    let cached: Vec<bool> = r.steps.iter().map(|s| s.cached).collect();
    assert_eq!(cached, [true, true, true, false, false]);
}

#[tokio::test]
async fn a_timeout_and_a_supervisor_error_are_reported() {
    // An exit-less outcome is a failure with its reason in the output.
    struct Stuck(Arc<Fake>);
    #[async_trait]
    impl BuildHost for Stuck {
        fn home(&self) -> &Path {
            self.0.home()
        }
        async fn has_layer(&self, n: &str) -> bool {
            self.0.has_layer(n).await
        }
        async fn start_base(&self, s: &BuilderSpec) -> Result<String> {
            self.0.start_base(s).await
        }
        async fn start_layer(&self, l: &str, n: &str) -> Result<String> {
            self.0.start_layer(l, n).await
        }
        async fn exec(&self, b: &str, s: ExecSpec, f: &mut dyn LineSink) -> Result<ExecOutcome> {
            if s.program == "id" {
                return self.0.exec(b, s, f).await;
            }
            Ok(ExecOutcome {
                code: None,
                error: Some("executable not found".into()),
                timed_out: true,
            })
        }
        async fn put(&self, b: &str, p: &str, d: Vec<u8>, m: u32) -> Result<()> {
            self.0.put(b, p, d, m).await
        }
        async fn put_file(&self, b: &str, p: &str, h: &Path) -> Result<()> {
            self.0.put_file(b, p, h).await
        }
        async fn save_layer(&self, b: &str, n: &str) -> Result<()> {
            self.0.save_layer(b, n).await
        }
        async fn drop_layer(&self, n: &str) {
            self.0.drop_layer(n).await
        }
        async fn drop_builder(&self, b: &str) {
            self.0.drop_builder(b).await
        }
    }
    let fake = Fake::new();
    let p = plan::plan(&recipe(vec![run_step("sleep 9999")]), None, shape(), None).unwrap();
    let e = engine::run(&Stuck(fake.clone()), &p, &opts(None))
        .await
        .unwrap_err();
    let Error::Build {
        output, exit_code, ..
    } = &e
    else {
        panic!("{e:?}")
    };
    assert_eq!(*exit_code, None);
    assert!(
        output.contains("ran out of time") && output.contains("executable not found"),
        "{output}"
    );
}

#[tokio::test]
async fn a_moved_base_tag_rebuilds_every_layer() {
    let fake = Fake::new();
    fake.st.lock().unwrap().digest = Some("sha256:one".into());
    build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap();
    // Same digest: cached.
    assert!(
        build(&fake, &three_steps(), None, &opts(None))
            .await
            .unwrap()
            .cached
    );
    fake.st.lock().unwrap().digest = Some("sha256:two".into());
    let r = build(&fake, &three_steps(), None, &opts(None))
        .await
        .unwrap();
    assert!(!r.cached && r.steps.iter().all(|s| !s.cached));
    // The new digest is recorded: the next build is cached.
    assert!(
        build(&fake, &three_steps(), None, &opts(None))
            .await
            .unwrap()
            .cached
    );
}

#[tokio::test]
async fn commands_run_as_the_steps_user_in_its_directory_with_its_env() {
    let fake = Fake::new();
    let r = recipe(vec![
        BuildStep::Env {
            vars: [("APP".to_string(), "/srv/app".to_string())].into(),
        },
        BuildStep::Workdir {
            path: "/srv/app".into(),
        },
        run_step("echo as root"),
        BuildStep::User { user: "cua".into() },
        BuildStep::Run {
            command: "whoami > /tmp/who".into(),
            user: None,
            workdir: None,
            env: [("GREETING".to_string(), "hi".to_string())].into(),
        },
        BuildStep::User {
            user: "app:staff".into(),
        },
        run_step("id"),
    ]);
    build(&fake, &r, None, &opts(None)).await.unwrap();
    let execs: Vec<ExecSpec> = fake
        .execs()
        .into_iter()
        .filter(|e| e.program != "id")
        .collect();
    // WORKDIR: mkdir as root (the driver is `cua`, so through sudo).
    assert_eq!(execs[0].program, "sudo");
    assert!(
        execs[0].args.join(" ").contains("mkdir -p -- /srv/app"),
        "{:?}",
        execs[0]
    );
    // RUN as root, in /srv/app, with ENV.
    let root = &execs[1];
    assert_eq!(root.program, "sudo");
    assert_eq!(
        &root.args[..8],
        [
            "-n",
            "-H",
            "--preserve-env",
            "-u",
            "root",
            "--",
            "/bin/sh",
            "-c"
        ]
    );
    assert!(root.args.contains(&"/srv/app".to_string()));
    assert_eq!(root.env["APP"], "/srv/app");
    // USER cua is the driver: no switch, the cwd is the workdir.
    let cua = &execs[2];
    assert_eq!(cua.program, "/bin/sh");
    assert_eq!(cua.args, ["-c", "whoami > /tmp/who"]);
    assert_eq!(cua.cwd.as_deref(), Some("/srv/app"));
    assert_eq!(cua.env["GREETING"], "hi");
    assert_eq!(cua.env["APP"], "/srv/app");
    assert_eq!(cua.user, None);
    // Another user, with a group.
    let other = &execs[3];
    assert_eq!(other.program, "sudo");
    assert!(
        other.args.join(" ").contains("-u app -g staff"),
        "{:?}",
        other.args
    );
}

#[tokio::test]
async fn a_driver_that_is_root_switches_users_itself() {
    let (p, a, u, c) = engine::as_user_in(
        "root",
        "app",
        "make".into(),
        vec!["all".into()],
        Some("/w".into()),
    );
    assert_eq!(
        (p.as_str(), a, u.as_deref(), c.as_deref()),
        ("make", vec!["all".to_string()], Some("app"), Some("/w"))
    );
    let (p, _, u, _) = engine::as_user_in("root", "root", "x".into(), vec![], None);
    assert_eq!((p.as_str(), u), ("x", None));
    let (p, a, u, _) = engine::as_user_in("cua", "1000:1000", "x".into(), vec![], None);
    assert_eq!(p, "sudo");
    assert!(a.join(" ").contains("-u #1000 -g #1000"), "{a:?}");
    assert_eq!(u, None);
}

#[tokio::test]
async fn copy_uploads_a_tar_and_applies_owner_and_mode_as_root() {
    let ctx = tempfile::tempdir().unwrap();
    std::fs::write(ctx.path().join("hello.txt"), "hi").unwrap();
    let fake = Fake::new();
    let r = recipe(vec![
        BuildStep::Workdir {
            path: "/srv/app".into(),
        },
        BuildStep::Copy {
            sources: vec!["hello.txt".into()],
            dest: "./".into(),
            owner: Some("cua:cua".into()),
            mode: Some("640".into()),
            unpack: false,
        },
    ]);
    build(&fake, &r, Some(ctx.path()), &opts(None))
        .await
        .unwrap();
    let st = fake.st.lock().unwrap();
    assert_eq!(st.puts.len(), 2, "the tar and the path list: {:?}", st.puts);
    assert!(st.puts.iter().any(|(_, p)| p.ends_with(".tar")));
    drop(st);
    let line = fake
        .lines()
        .into_iter()
        .find(|l| l.contains("tar -xpf"))
        .unwrap();
    assert!(line.contains("chown -h -- cua:cua"), "{line}");
    assert!(line.contains("MODE=640"), "{line}");
}

#[tokio::test]
async fn a_changed_context_file_rebuilds_only_the_copy_and_after() {
    let ctx = tempfile::tempdir().unwrap();
    std::fs::write(ctx.path().join("hello.txt"), "one").unwrap();
    let fake = Fake::new();
    let r = recipe(vec![
        run_step("apt-get update"),
        BuildStep::Copy {
            sources: vec!["hello.txt".into()],
            dest: "/srv/".into(),
            owner: None,
            mode: None,
            unpack: false,
        },
        run_step("echo after"),
    ]);
    build(&fake, &r, Some(ctx.path()), &opts(None))
        .await
        .unwrap();
    assert!(
        build(&fake, &r, Some(ctx.path()), &opts(None))
            .await
            .unwrap()
            .cached
    );
    std::fs::write(ctx.path().join("hello.txt"), "two").unwrap();
    let again = build(&fake, &r, Some(ctx.path()), &opts(None))
        .await
        .unwrap();
    let cached: Vec<bool> = again.steps.iter().map(|s| s.cached).collect();
    assert_eq!(cached, [true, true, false, false]);
}

#[tokio::test]
async fn records_tags_and_deletion_keep_shared_layers() {
    let fake = Fake::new();
    let mut o = opts(None);
    o.tag = Some("app".into());
    let a = build(&fake, &three_steps(), None, &o).await.unwrap();
    assert_eq!(a.tag.as_deref(), Some("app"));
    assert_eq!(
        store::resolve_ref(fake.home(), "app").as_deref(),
        Some(a.checkpoint.as_str())
    );
    // A second build shares the first three layers.
    let mut changed = three_steps();
    changed.steps[3] = run_step("echo different");
    o.tag = Some("app2".into());
    let b = build(&fake, &changed, None, &o).await.unwrap();
    let recs = store::load_records(fake.home());
    assert_eq!(recs.len(), 2);
    let ra = store::load_record(fake.home(), &a.key).unwrap();
    assert_eq!(ra.config.env["APP"], "/srv/app");
    assert_eq!(ra.layers.len(), 4);
    assert_eq!(
        ra.layers[..3],
        store::load_record(fake.home(), &b.key).unwrap().layers[..3]
    );
}

#[tokio::test]
async fn heavy_inputs_are_planned_before_any_builder_starts() {
    let fake = Fake::new();
    let e = build(
        &fake,
        &recipe(vec![BuildStep::Copy {
            sources: vec!["missing".into()],
            dest: "/x".into(),
            owner: None,
            mode: None,
            unpack: false,
        }]),
        Some(Path::new("/nonexistent-cua-context")),
        &opts(None),
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("step 1"), "{e}");
    assert!(
        fake.st.lock().unwrap().builders.is_empty(),
        "no builder for a bad recipe"
    );
}

#[tokio::test]
async fn identical_builds_in_flight_share_one_run() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let runs = Arc::new(AtomicU32::new(0));
    let result = || super::BuildResult {
        checkpoint: "build-x".into(),
        tag: None,
        key: "k".repeat(64),
        cached: false,
        steps: vec![],
        config: Default::default(),
        seconds: 0.0,
    };
    let make = |runs: Arc<AtomicU32>, fail: bool| {
        let r = result();
        move || async move {
            runs.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(150)).await;
            if fail {
                Err(Error::Build {
                    step: 2,
                    command: "RUN x".into(),
                    exit_code: Some(3),
                    output: "boom".into(),
                })
            } else {
                Ok(r)
            }
        }
    };
    let (a, lead_a) = engine::shared("flight-ok".into(), make(runs.clone(), false));
    let (b, lead_b) = engine::shared("flight-ok".into(), make(runs.clone(), false));
    assert!(lead_a && !lead_b);
    let (ra, rb) = tokio::join!(a, b);
    assert_eq!(ra.unwrap().checkpoint, "build-x");
    assert_eq!(rb.unwrap().checkpoint, "build-x");
    assert_eq!(runs.load(Ordering::SeqCst), 1, "one run for both");
    // Done: the next one runs again.
    let (c, lead_c) = engine::shared("flight-ok".into(), make(runs.clone(), false));
    assert!(lead_c);
    c.await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    // A failure reaches every waiter; the key is free afterwards.
    let (a, _) = engine::shared("flight-bad".into(), make(runs.clone(), true));
    let (b, lead) = engine::shared("flight-bad".into(), make(runs.clone(), true));
    assert!(!lead);
    let (ra, rb) = tokio::join!(a, b);
    for r in [ra, rb] {
        assert!(matches!(
            &*r.unwrap_err(),
            Error::Build {
                exit_code: Some(3),
                ..
            }
        ));
    }
    let (d, lead_d) = engine::shared("flight-bad".into(), make(runs.clone(), false));
    assert!(lead_d);
    d.await.unwrap();
    // Different keys do not share.
    let (e1, _) = engine::shared("k1".into(), make(runs.clone(), false));
    let (e2, l2) = engine::shared("k2".into(), make(runs.clone(), false));
    assert!(l2);
    let _ = tokio::join!(e1, e2);
}

#[test]
fn recipes_round_trip_through_json_with_stable_names() {
    let r = three_steps();
    let j = serde_json::to_string(&r).unwrap();
    assert!(j.contains("\"op\":\"run\""), "{j}");
    let back: BuildRecipe = serde_json::from_str(&j).unwrap();
    assert_eq!(back, r);
    let step: BuildStep =
        serde_json::from_str(r#"{"op":"copy","sources":["a"],"dest":"/b"}"#).unwrap();
    assert!(matches!(
        step,
        BuildStep::Copy {
            unpack: false,
            owner: None,
            ..
        }
    ));
}

#[test]
fn dockerfiles_become_recipes() {
    let text = "ARG BASE=ubuntu\nFROM ${BASE}:24.04\nARG GREETING=hi\nENV APP=/srv/app\nWORKDIR /srv/app\nCOPY hello.txt ./\nRUN echo \"$GREETING\" > greeting && chmod 600 greeting\nUSER cua\nRUN whoami > /tmp/who\nCMD [\"sleep\", \"infinity\"]\nENTRYPOINT /entry.sh";
    let r = BuildRecipe::from_dockerfile(text, &BTreeMap::new()).unwrap();
    assert_eq!(r.base, "ubuntu:24.04");
    assert_eq!(r.steps.len(), 8);
    assert_eq!(
        r.steps[3],
        BuildStep::Run {
            command: "echo \"$GREETING\" > greeting && chmod 600 greeting".into(),
            user: None,
            workdir: None,
            env: [("GREETING".to_string(), "hi".to_string())].into(),
        }
    );
    assert_eq!(
        r.steps[7],
        BuildStep::Entrypoint {
            argv: vec!["/bin/sh".into(), "-c".into(), "/entry.sh".into()]
        }
    );
    // Build arguments override defaults and are checked.
    let args: BTreeMap<String, String> = [("GREETING".to_string(), "yo".to_string())].into();
    let r2 = BuildRecipe::from_dockerfile(text, &args).unwrap();
    assert!(matches!(&r2.steps[3], BuildStep::Run { env, .. } if env["GREETING"] == "yo"));
    let bad: BTreeMap<String, String> = [("NOPE".to_string(), "1".to_string())].into();
    let e = BuildRecipe::from_dockerfile(text, &bad).unwrap_err();
    assert!(e.to_string().contains("NOPE"), "{e}");
    // ENV beats ARG of the same name in RUN.
    let r3 =
        BuildRecipe::from_dockerfile("FROM linux\nARG X=1\nENV X=2\nRUN echo", &BTreeMap::new())
            .unwrap();
    assert!(matches!(&r3.steps[1], BuildStep::Run { env, .. } if env.is_empty()));
    // `$PATH` in ENV is Docker's default PATH.
    let r4 = BuildRecipe::from_dockerfile("FROM linux\nENV PATH=/opt/bin:$PATH", &BTreeMap::new())
        .unwrap();
    assert!(
        matches!(&r4.steps[0], BuildStep::Env { vars } if vars["PATH"].starts_with("/opt/bin:/usr/local/sbin"))
    );
    // Platform is checked.
    assert!(
        BuildRecipe::from_dockerfile("FROM --platform=linux/s390x linux", &BTreeMap::new())
            .is_err()
    );
    assert!(
        BuildRecipe::from_dockerfile("FROM --platform=linux/arm64 linux", &BTreeMap::new()).is_ok()
            || BuildRecipe::from_dockerfile("FROM --platform=linux/amd64 linux", &BTreeMap::new())
                .is_ok()
    );
}

#[test]
fn bases_map_to_cua_images_or_say_what_works() {
    use super::classify_base;
    let b = classify_base("ubuntu:24.04").unwrap();
    assert!(b.image.starts_with("ghcr.io/trycua/linux"), "{}", b.image);
    assert_eq!(b.python, None);
    let p = classify_base("python:3.12-slim-bookworm").unwrap();
    assert_eq!(p.python.as_deref(), Some("3.12"));
    assert!(p.note.unwrap().contains("uv"));
    assert!(
        classify_base("docker.io/library/python:3.11.9")
            .unwrap()
            .python
            .as_deref()
            == Some("3.11.9")
    );
    let d = classify_base("debian:bookworm").unwrap();
    assert!(d.note.unwrap().contains("Ubuntu"));
    assert_eq!(
        classify_base("ghcr.io/trycua/linux:24.04").unwrap().image,
        "ghcr.io/trycua/linux:24.04"
    );
    assert!(
        classify_base("linux")
            .unwrap()
            .image
            .starts_with("ghcr.io/trycua/linux")
    );
    let e = classify_base("nvcr.io/nvidia/pytorch:22.12-py3")
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("nvcr.io/nvidia/pytorch") && e.contains("cua-spacesd") && e.contains("ubuntu"),
        "{e}"
    );
    assert!(classify_base("scratch").is_err());
    assert!(classify_base("ubuntu:jammy").is_err());
    assert!(classify_base("").is_err());
    assert!(super::python_script("3.12").contains("uv python install"));
    assert_eq!(super::python_version("3.13t"), "3.13");
    assert_eq!(super::python_version("latest"), "3");
}

#[tokio::test]
async fn python_bases_install_python_in_the_base_layer() {
    let fake = Fake::new();
    let r = BuildRecipe {
        base: "python:3.12".into(),
        steps: vec![run_step("pip --version")],
    };
    build(&fake, &r, None, &opts(None)).await.unwrap();
    let lines = fake.lines();
    assert!(
        lines.iter().any(|l| l.contains("uv python install")),
        "{lines:?}"
    );
    let first = lines
        .iter()
        .position(|l| l.contains("uv python install"))
        .unwrap();
    let pip = lines
        .iter()
        .position(|l| l.contains("pip --version"))
        .unwrap();
    assert!(first < pip);
}

#[allow(dead_code)]
fn _unused(_: PathBuf) {}

fn config(
    env: &[(&str, &str)],
    workdir: Option<&str>,
    user: Option<&str>,
    driver: &str,
) -> super::ImageConfig {
    super::ImageConfig {
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        workdir: workdir.map(str::to_string),
        user: user.map(str::to_string),
        driver_user: driver.into(),
        ..Default::default()
    }
}

#[test]
fn an_images_defaults_wrap_a_shell_line() {
    // Nothing configured: the line is the line.
    assert_eq!(config(&[], None, None, "cua").wrap_shell("ls -l"), "ls -l");
    assert!(config(&[], None, None, "cua").is_default());
    // Env and workdir.
    let c = config(
        &[("APP", "/srv/app"), ("MSG", "it's here")],
        Some("/srv/app"),
        None,
        "cua",
    );
    assert_eq!(
        c.wrap_shell("echo $APP"),
        "export APP=/srv/app; export MSG='it'\\''s here'; cd -- /srv/app && echo $APP"
    );
    // The image's user is the one cua-spacesd runs as: no switch.
    let c = config(&[], Some("/srv/app"), Some("cua"), "cua");
    assert_eq!(c.wrap_shell("pwd"), "cd -- /srv/app && pwd");
    // Another user: sudo, the directory entered after the switch.
    let c = config(&[("A", "1")], Some("/srv/app"), Some("app:staff"), "cua");
    assert_eq!(
        c.wrap_shell("id -un"),
        "export A=1; exec sudo -n -H --preserve-env -u app -g staff -- /bin/sh -c 'cd -- /srv/app && id -un'"
    );
    // Root, numeric ids, and an unknown driver (no switch can be made).
    let c = config(&[], None, Some("0"), "cua");
    assert!(c.wrap_shell("id").contains("-u root"));
    let c = config(&[], None, Some("1000"), "cua");
    assert!(
        c.wrap_shell("id").contains("-u '#1000'"),
        "{}",
        c.wrap_shell("id")
    );
    let c = config(&[], None, Some("app"), "");
    assert_eq!(c.wrap_shell("id"), "id");
}

#[test]
fn a_space_made_from_a_build_runs_commands_with_the_images_defaults() {
    use crate::{Space, SpaceId};
    let space = Space::generic(
        SpaceId::Local {
            name: "built".into(),
        },
        "built",
        Default::default(),
    );
    let plain = space.shell_command("echo hi");
    assert_eq!(
        (plain.program.as_str(), plain.args.as_slice()),
        (
            "/bin/sh",
            ["-c".to_string(), "echo hi".to_string()].as_slice()
        )
    );
    let space = space.with_image_config(config(
        &[("APP", "/srv/app")],
        Some("/srv/app"),
        None,
        "cua",
    ));
    let cmd = space.shell_command("echo $APP");
    assert_eq!(cmd.program, "/bin/sh");
    assert_eq!(
        cmd.args[1],
        "export APP=/srv/app; cd -- /srv/app && echo $APP"
    );
    assert!(space.image_config().is_some());
}
