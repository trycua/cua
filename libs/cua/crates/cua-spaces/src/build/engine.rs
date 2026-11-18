//! The build engine: runs a [`Plan`](super::plan::Plan) against a
//! [`BuildHost`] (a builder Space with a shell, file upload and checkpoints),
//! reusing cached layers.
//!
//! The engine knows nothing about Spaces; [`BuildHost`] is what it needs of
//! one, so the cache, failure and cleanup logic is tested with a fake.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};

use super::base::python_script;
use super::context::{EntryKind, pack};
use super::plan::{Action, Plan};
use super::recipe::{BuildResult, StepResult};
use super::store;
use crate::error::{Error, Result};

/// Receives what a build is doing.
pub type BuildSink = Arc<dyn Fn(&BuildEvent) + Send + Sync>;

/// What a build reports, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildEvent {
    /// How the base image maps (when the mapping changed something), or a
    /// build waiting on an identical one already running.
    Note {
        /// A line for people.
        text: String,
    },
    /// A step starts (`cached`: served from a layer, nothing runs). Step 0
    /// is the base image.
    Step {
        /// Step number (0: `FROM`).
        index: u32,
        /// Number of steps after the base.
        total: u32,
        /// The step as a line.
        text: String,
        /// Served from cache.
        cached: bool,
    },
    /// A line a step printed.
    Output {
        /// Step number.
        index: u32,
        /// The line.
        line: String,
    },
    /// A step that ran finished.
    StepDone {
        /// Step number.
        index: u32,
        /// Seconds it took.
        seconds: f64,
        /// The layer's checkpoint, when it made one.
        checkpoint: Option<String>,
    },
    /// The build finished.
    Finished {
        /// Nothing ran.
        cached: bool,
        /// Seconds the build took.
        seconds: f64,
    },
}

/// A builder Space to start from a base image.
#[derive(Debug, Clone)]
pub struct BuilderSpec {
    /// Name of the builder Space.
    pub name: String,
    /// The Cua image.
    pub image: String,
    /// `vm` or `container`.
    pub kind: String,
    /// vCPUs.
    pub cpus: u32,
    /// Memory in MiB.
    pub memory_mb: u64,
    /// No desktop.
    pub headless: bool,
}

/// A command to run in the builder, as cua-spacesd takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecSpec {
    /// Program.
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Environment.
    pub env: std::collections::BTreeMap<String, String>,
    /// Working directory.
    pub cwd: Option<String>,
    /// OS user for cua-spacesd to switch to (it runs as root).
    pub user: Option<String>,
    /// Wall-clock limit.
    pub timeout: Duration,
}

/// How a command ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutcome {
    /// Exit code of a normal exit.
    pub code: Option<i32>,
    /// Supervisor error or signal text.
    pub error: Option<String>,
    /// Stopped by the limit.
    pub timed_out: bool,
}

impl ExecOutcome {
    /// Exit code 0.
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
}

/// Receives the lines a command prints (any `FnMut(&str)` is one).
pub trait LineSink: Send {
    /// One output line, without its newline.
    fn line(&mut self, line: &str);
}

impl<F: FnMut(&str) + Send> LineSink for F {
    fn line(&mut self, line: &str) {
        self(line)
    }
}

/// What the engine needs of a Space that builds.
#[async_trait]
pub trait BuildHost: Send + Sync {
    /// The cua home the build records live under.
    fn home(&self) -> &Path;
    /// Whether layer checkpoint `name` exists.
    async fn has_layer(&self, name: &str) -> bool;
    /// Starts a builder from a base image; its id.
    async fn start_base(&self, spec: &BuilderSpec) -> Result<String>;
    /// Starts a builder as a fork of layer checkpoint `layer`; its id.
    async fn start_layer(&self, layer: &str, name: &str) -> Result<String>;
    /// Runs a command in the builder, passing each output line.
    async fn exec(
        &self,
        builder: &str,
        spec: ExecSpec,
        on_line: &mut dyn LineSink,
    ) -> Result<ExecOutcome>;
    /// Writes a file in the builder.
    async fn put(&self, builder: &str, path: &str, data: Vec<u8>, mode: u32) -> Result<()>;
    /// Writes a host file in the builder.
    async fn put_file(&self, builder: &str, path: &str, host: &Path) -> Result<()>;
    /// Saves the builder as pinned layer checkpoint `name`.
    async fn save_layer(&self, builder: &str, name: &str) -> Result<()>;
    /// Deletes layer checkpoint `name` (unpinning it first).
    async fn drop_layer(&self, name: &str);
    /// Deletes the builder.
    async fn drop_builder(&self, builder: &str);
}

/// Per-run settings.
#[derive(Clone)]
pub(super) struct RunOpts {
    pub tag: Option<String>,
    pub no_cache: bool,
    pub step_timeout: Duration,
    pub sink: Option<BuildSink>,
}

const TAIL_LINES: usize = 40;

fn emit(sink: &Option<BuildSink>, e: BuildEvent) {
    if let Some(s) = sink {
        s(&e);
    }
}

/// POSIX single-quoting.
pub(super) fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,@%+".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// How to run `program args` as `user` in `cwd` when cua-spacesd runs as
/// `driver`: directly when `user` is empty or the driver; through the
/// OS-user switch when the driver is root; through `sudo -n -u` otherwise
/// (the directory entered after the switch, so a closed home is no
/// obstacle). Returns the spec's program, args, user and cwd.
pub fn as_user_in(
    driver: &str,
    user: &str,
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
) -> (String, Vec<String>, Option<String>, Option<String>) {
    let same = user.is_empty() || user == driver || (user == "root" && driver == "root");
    if same {
        return (program, args, None, cwd);
    }
    if driver == "root" {
        return (program, args, Some(user.to_string()), cwd);
    }
    let (name, group) = match user.split_once(':') {
        Some((n, g)) => (n, Some(g)),
        None => (user, None),
    };
    let target = |s: &str| {
        if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
            format!("#{s}")
        } else {
            s.to_string()
        }
    };
    let mut sudo: Vec<String> = vec!["-n".into(), "-H".into(), "--preserve-env".into()];
    let name = if name.is_empty() { "root" } else { name };
    sudo.extend(["-u".into(), target(name)]);
    if let Some(g) = group.filter(|g| !g.is_empty()) {
        sudo.extend(["-g".into(), target(g)]);
    }
    sudo.push("--".into());
    let cwd = cwd.unwrap_or_else(|| "/".into());
    sudo.extend([
        "/bin/sh".into(),
        "-c".into(),
        "cd -- \"$0\" || exit 1; exec \"$@\"".into(),
        cwd,
        program,
    ]);
    sudo.extend(args);
    ("sudo".into(), sudo, None, None)
}

struct Runner<'a> {
    host: &'a dyn BuildHost,
    builder: String,
    driver: String,
    opts: &'a RunOpts,
    index: u32,
    tail: VecDeque<String>,
}

impl Runner<'_> {
    /// Runs a command as `user` (empty: root) and fails with the step's
    /// command, exit code and last output lines.
    async fn run_as(
        &mut self,
        what: &str,
        argv: Vec<String>,
        env: std::collections::BTreeMap<String, String>,
        cwd: Option<String>,
        user: &str,
    ) -> Result<()> {
        // Root means root: cua-spacesd runs as the image's desktop user.
        let target = if user.is_empty() { "root" } else { user };
        let mut it = argv.into_iter();
        let program = it.next().unwrap_or_default();
        let (program, args, os_user, cwd) =
            as_user_in(&self.driver, target, program, it.collect(), cwd);
        let spec = ExecSpec {
            program,
            args,
            env,
            cwd,
            user: os_user,
            timeout: self.opts.step_timeout,
        };
        self.tail.clear();
        let index = self.index;
        let sink = self.opts.sink.clone();
        let tail = &mut self.tail;
        let out = self
            .host
            .exec(&self.builder, spec, &mut |line: &str| {
                if tail.len() == TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line.to_string());
                emit(
                    &sink,
                    BuildEvent::Output {
                        index,
                        line: line.to_string(),
                    },
                );
            })
            .await?;
        if out.success() {
            return Ok(());
        }
        let mut output = self.tail.iter().cloned().collect::<Vec<_>>().join("\n");
        if out.timed_out {
            output.push_str(&format!(
                "\n(ran out of time after {}s)",
                self.opts.step_timeout.as_secs()
            ));
        }
        if let Some(e) = &out.error {
            output.push_str(&format!("\n({e})"));
        }
        Err(Error::Build {
            step: self.index,
            command: what.to_string(),
            exit_code: out.code,
            output,
        })
    }

    async fn root(&mut self, what: &str, script: String) -> Result<()> {
        self.run_as(
            what,
            vec!["/bin/sh".into(), "-c".into(), script],
            Default::default(),
            None,
            "",
        )
        .await
    }

    /// The user cua-spacesd runs as.
    async fn driver_user(host: &dyn BuildHost, builder: &str) -> Result<String> {
        let mut lines = Vec::new();
        let out = host
            .exec(
                builder,
                ExecSpec {
                    program: "id".into(),
                    args: vec!["-un".into()],
                    env: Default::default(),
                    cwd: None,
                    user: None,
                    timeout: Duration::from_secs(60),
                },
                &mut |l: &str| lines.push(l.trim().to_string()),
            )
            .await?;
        match (out.success(), lines.into_iter().find(|l| !l.is_empty())) {
            (true, Some(u)) => Ok(u),
            _ => Ok("root".to_string()),
        }
    }

    async fn apply(&mut self, action: &Action, text: &str) -> Result<()> {
        match action {
            Action::Base => Ok(()),
            Action::Run {
                argv,
                env,
                workdir,
                user,
            } => {
                self.run_as(text, argv.clone(), env.clone(), Some(workdir.clone()), user)
                    .await
            }
            Action::Mkdir { path } => {
                self.root(text, format!("mkdir -p -- {}", sh_quote(path)))
                    .await
            }
            Action::Copy {
                entries,
                owner,
                mode,
            } => {
                let n = self.index;
                // Archives (ADD) travel as themselves and unpack into dest.
                for e in entries {
                    if let EntryKind::Archive { host, .. } = &e.kind {
                        let tmp = format!("/tmp/.cua-build-{n}.archive");
                        self.host.put_file(&self.builder, &tmp, host).await?;
                        self.root(
                            text,
                            format!(
                                "set -e; mkdir -p -- {d}; tar -xf {t} -C {d}; rm -f {t}",
                                d = sh_quote(&e.dest),
                                t = sh_quote(&tmp)
                            ),
                        )
                        .await?;
                    }
                }
                let (tarball, paths) = pack(entries)?;
                if paths.is_empty() {
                    return Ok(());
                }
                let tmp_tar = format!("/tmp/.cua-build-{n}.tar");
                let tmp_list = format!("/tmp/.cua-build-{n}.list");
                self.host
                    .put(&self.builder, &tmp_tar, tarball, 0o644)
                    .await?;
                let mut script = format!(
                    "set -e; tar -xpf {t} -C / --no-same-owner --no-overwrite-dir",
                    t = sh_quote(&tmp_tar)
                );
                if owner.is_some() || mode.is_some() {
                    self.host
                        .put(&self.builder, &tmp_list, paths, 0o644)
                        .await?;
                    if let Some(o) = owner {
                        script.push_str(&format!(
                            "; xargs -r -0 chown -h -- {} < {}",
                            sh_quote(o),
                            sh_quote(&tmp_list)
                        ));
                    }
                    if let Some(m) = mode {
                        // Symlinks keep their own mode.
                        script.push_str(&format!(
                            "; MODE={} xargs -r -0 sh -c 'for p; do [ -L \"$p\" ] || chmod \"$MODE\" -- \"$p\"; done' sh < {}",
                            sh_quote(m),
                            sh_quote(&tmp_list)
                        ));
                    }
                }
                script.push_str(&format!(
                    "; rm -f {} {}",
                    sh_quote(&tmp_tar),
                    sh_quote(&tmp_list)
                ));
                self.root(text, script).await
            }
        }
    }
}

fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let rem = secs % 86400;
    // Civil from days (Howard Hinnant).
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Builds `plan`: finds the deepest cached layer, runs the layers after it
/// in one builder, and records the result. A failed step leaves no
/// checkpoint of its own (earlier layers stay cached) and the builder is
/// deleted either way.
pub(super) async fn run(host: &dyn BuildHost, plan: &Plan, opts: &RunOpts) -> Result<BuildResult> {
    let t0 = Instant::now();
    let total = (plan.texts.len() - 1) as u32;
    let home = host.home();
    if let Some(note) = &plan.base.note {
        emit(&opts.sink, BuildEvent::Note { text: note.clone() });
    }
    let known = store::load_base(home, &plan.layers[0].key);
    let replace = opts.no_cache;
    // The deepest layer already built.
    let mut resume: Option<usize> = None;
    if !replace {
        for k in (0..plan.layers.len()).rev() {
            if host.has_layer(&plan.layers[k].checkpoint()).await {
                resume = Some(k);
                break;
            }
        }
    }
    let last = plan.layers.len() - 1;
    let all_cached = resume == Some(last);
    // Step text indices of layers; steps run in text order.
    let mut results: Vec<StepResult> = Vec::new();
    let mut driver = known
        .as_ref()
        .map(|k| k.driver_user.clone())
        .filter(|u| !u.is_empty());
    let mut builder: Option<String> = None;
    let base_script = plan.base.python.as_deref().map(python_script);

    let outcome: Result<()> = async {
        // The layer the builder starts from. A cached layer whose checkpoint
        // cannot be forked (its files are gone) is dropped and the next one
        // down tried, down to the base image.
        let mut from = resume;
        if !all_cached {
            let b = loop {
                match from {
                    Some(k) => {
                        let name = format!("build-{}-{}", &plan.layers[k].key[..12], rand_suffix());
                        match host.start_layer(&plan.layers[k].checkpoint(), &name).await {
                            Ok(b) => break b,
                            Err(e) => {
                                tracing::warn!(
                                    layer = %plan.layers[k].checkpoint(),
                                    "a cached build layer cannot be forked ({e}); rebuilding from before it"
                                );
                                host.drop_layer(&plan.layers[k].checkpoint()).await;
                                let mut lower = None;
                                for j in (0..k).rev() {
                                    if host.has_layer(&plan.layers[j].checkpoint()).await {
                                        lower = Some(j);
                                        break;
                                    }
                                }
                                from = lower;
                            }
                        }
                    }
                    None => {
                        break host
                            .start_base(&BuilderSpec {
                                name: format!(
                                    "build-{}-{}",
                                    &plan.layers[0].key[..12],
                                    rand_suffix()
                                ),
                                image: plan.base.image.clone(),
                                kind: plan.shape.kind.clone(),
                                cpus: plan.shape.cpus,
                                memory_mb: plan.shape.memory_mb,
                                headless: plan.shape.headless,
                            })
                            .await?;
                    }
                }
            };
            store::note_builder(home, &b);
            builder = Some(b);
        } else if driver.is_none() {
            // Fully cached, but the user cua-spacesd runs as is unknown (the
            // record was lost): ask a throwaway fork of the last layer.
            let name = format!("build-{}-{}", &plan.layers[last].key[..12], rand_suffix());
            let b = host
                .start_layer(&plan.layers[last].checkpoint(), &name)
                .await?;
            let u = Runner::driver_user(host, &b).await;
            host.drop_builder(&b).await;
            let u = u?;
            store::save_base(
                home,
                &plan.layers[0].key,
                &store::BaseInfo {
                    driver_user: u.clone(),
                    image_digest: plan.base_digest.clone(),
                },
            )?;
            driver = Some(u);
        }
        if let Some(b) = &builder
            && (driver.is_none() || from.is_none())
        {
            let u = Runner::driver_user(host, b).await?;
            store::save_base(
                home,
                &plan.layers[0].key,
                &store::BaseInfo {
                    driver_user: u.clone(),
                    image_digest: plan.base_digest.clone(),
                },
            )?;
            driver = Some(u);
        }
        let first_new = if all_cached {
            plan.layers.len()
        } else {
            from.map(|k| k + 1).unwrap_or(0)
        };
        let mut r = Runner {
            host,
            builder: builder.clone().unwrap_or_default(),
            driver: driver.clone().unwrap_or_default(),
            opts,
            index: 0,
            tail: VecDeque::new(),
        };
        for (t, text) in plan.texts.iter().enumerate() {
            let layer = plan.layer_of[t];
            let index = t as u32;
            // Metadata steps and cached layers run nothing.
            let cached = match layer {
                Some(k) => k < first_new,
                None => {
                    // Metadata is as cached as the layers before it.
                    let before = plan.layer_of[..t].iter().rev().flatten().next().copied();
                    before.is_none_or(|k| k < first_new)
                }
            };
            emit(
                &opts.sink,
                BuildEvent::Step {
                    index,
                    total,
                    text: text.clone(),
                    cached,
                },
            );
            let k = match (layer, cached) {
                (Some(k), false) => k,
                _ => {
                    results.push(StepResult {
                        index,
                        text: text.clone(),
                        cached,
                        seconds: 0.0,
                        checkpoint: layer
                            .filter(|k| plan.shape.kind != "container" || *k == 0 || *k == last)
                            .map(|k| plan.layers[k].checkpoint()),
                    });
                    continue;
                }
            };
            let ts = Instant::now();
            r.index = index;
            let l = &plan.layers[k];
            if k == 0 {
                if let Some(script) = &base_script {
                    r.root(&format!("install Python for {}", plan.base.from), script.clone())
                        .await?;
                }
            } else {
                r.apply(&l.action, text).await?;
            }
            let name = l.checkpoint();
            // A VM layer is a checkpoint of milliseconds (its RAM is a shared
            // file). A container's is an export of its whole filesystem,
            // minutes for an image of a gigabyte, so a container build saves
            // the base and the result only.
            let save = plan.shape.kind != "container" || k == 0 || k == last;
            let mut saved = None;
            if save {
                if replace {
                    host.drop_layer(&name).await;
                }
                match host.save_layer(&r.builder, &name).await {
                    Ok(()) => {}
                    // Another build saved the same layer meanwhile: the same
                    // key is the same content.
                    Err(_) if host.has_layer(&name).await => {}
                    Err(e) => return Err(e),
                }
                saved = Some(name);
            }
            let seconds = ts.elapsed().as_secs_f64();
            emit(
                &opts.sink,
                BuildEvent::StepDone {
                    index,
                    seconds,
                    checkpoint: saved.clone(),
                },
            );
            results.push(StepResult {
                index,
                text: text.clone(),
                cached: false,
                seconds,
                checkpoint: saved,
            });
        }
        Ok(())
    }
    .await;
    if let Some(b) = &builder {
        host.drop_builder(b).await;
        store::forget_builder(home, b);
    }
    outcome?;

    let mut config = plan.config.clone();
    config.driver_user = driver.unwrap_or_default();
    let checkpoint = plan.layers[last].checkpoint();
    let record = store::BuildRecord {
        key: plan.key.clone(),
        checkpoint: checkpoint.clone(),
        layers: plan.layers.iter().map(|l| l.checkpoint()).collect(),
        config: config.clone(),
        created_at: now_rfc3339(),
    };
    store::save_record(home, &record)?;
    if let Some(tag) = &opts.tag {
        store::set_tag(home, tag, &plan.key)?;
    }
    let seconds = t0.elapsed().as_secs_f64();
    emit(
        &opts.sink,
        BuildEvent::Finished {
            cached: all_cached,
            seconds,
        },
    );
    Ok(BuildResult {
        checkpoint,
        tag: opts.tag.clone(),
        key: plan.key.clone(),
        cached: all_cached,
        steps: results,
        config,
        seconds,
    })
}

type Flight = Shared<BoxFuture<'static, std::result::Result<BuildResult, Arc<Error>>>>;

/// Builds in flight in this process, by cua home and recipe key.
static FLIGHTS: LazyLock<Mutex<HashMap<String, Flight>>> = LazyLock::new(Default::default);

/// Runs `make()` once per `key` at a time: a caller that arrives while
/// another is running it gets that run's result (`leader` is false). The
/// run is a task of its own, so a caller that goes away does not cut it off
/// for the others.
pub(super) fn shared<F, Fut>(
    key: String,
    make: F,
) -> (
    impl std::future::Future<Output = std::result::Result<BuildResult, Arc<Error>>>,
    bool,
)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<BuildResult>> + Send + 'static,
{
    let mut flights = FLIGHTS.lock().expect("build flights");
    if let Some(f) = flights.get(&key) {
        return (f.clone(), false);
    }
    let fut = make();
    let k = key.clone();
    let task = tokio::spawn(async move {
        let r = fut.await.map_err(Arc::new);
        FLIGHTS.lock().expect("build flights").remove(&k);
        r
    });
    let f: Flight = async move {
        task.await.unwrap_or_else(|e| {
            Err(Arc::new(Error::Build {
                step: 0,
                command: String::new(),
                exit_code: None,
                output: format!("the build task stopped: {e}"),
            }))
        })
    }
    .boxed()
    .shared();
    flights.insert(key, f.clone());
    (f, true)
}

fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "{:x}{:x}",
        t & 0xffff,
        N.fetch_add(1, Ordering::Relaxed) & 0xff
    )
}
