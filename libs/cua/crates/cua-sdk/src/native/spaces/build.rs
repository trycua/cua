//! `Spaces.build`: an image from a Dockerfile or steps, made in a Space and
//! kept as pinned checkpoints (see `cua_spaces::build`). Here or through the
//! daemon's `SpaceService.BuildSpace`; one result either way.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Host, Spaces, daemon_err, dpb, via_json};
use crate::{CuaError, Result, native::run};

/// One build step. Values are literal: a Dockerfile's `$VAR`s are expanded
/// by [`BuildOptions::dockerfile`]'s parser, not here. `Run` and `Copy`
/// steps run as the `User`, in the `Workdir` and with the `Env` set before
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BuildStep {
    /// A shell command line (`/bin/sh -c`, or the last `Shell`).
    Run {
        /// The command line.
        command: String,
    },
    /// A command and arguments, run without a shell.
    RunExec {
        /// Program and arguments.
        argv: Vec<String>,
    },
    /// Files of the build context copied in (`COPY`; `unpack`: `ADD`).
    Copy {
        /// Paths (globs allowed) relative to the context directory.
        sources: Vec<String>,
        /// Destination in the image; relative to the current `Workdir`.
        dest: String,
        /// `user[:group]` that owns what lands (default root).
        owner: Option<String>,
        /// A `chmod` mode applied to what lands (default: the source's).
        mode: Option<String>,
        /// A local tar archive is unpacked into `dest`.
        unpack: bool,
    },
    /// Environment for later steps and for commands run in the image.
    Env {
        /// Variables.
        vars: HashMap<String, String>,
    },
    /// The working directory (created when missing).
    Workdir {
        /// Directory, relative to the current one.
        path: String,
    },
    /// The user later steps and the image's commands run as.
    User {
        /// Name or id, optionally `:group`.
        user: String,
    },
    /// Ports the image listens on.
    Expose {
        /// Ports (`80`, `443/tcp`).
        ports: Vec<String>,
    },
    /// The image's default command.
    Cmd {
        /// Program and arguments.
        argv: Vec<String>,
    },
    /// The image's entrypoint.
    Entrypoint {
        /// Program and arguments.
        argv: Vec<String>,
    },
    /// The shell `Run` uses.
    Shell {
        /// Program and flags (`["/bin/bash", "-c"]`).
        argv: Vec<String>,
    },
    /// Image labels.
    Label {
        /// Labels.
        labels: HashMap<String, String>,
    },
    /// Volume mount points.
    Volume {
        /// Paths.
        paths: Vec<String>,
    },
    /// The stop signal.
    StopSignal {
        /// Name or number.
        signal: String,
    },
    /// The health check, as written.
    Healthcheck {
        /// The `HEALTHCHECK` arguments.
        text: String,
    },
}

/// Options for [`Spaces::build`] and [`super::SpaceCreateOptions::build`].
/// Give the recipe as a `dockerfile` (text) or a `base` image; `steps` are
/// applied after it.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct BuildOptions {
    /// Dockerfile text. One stage; the instructions `cua build` documents
    /// (multi-stage, heredocs, BuildKit flags and `ONBUILD` are refused,
    /// naming the line).
    #[uniffi(default = None)]
    pub dockerfile: Option<String>,
    /// The build context: the directory `COPY` sources are relative to
    /// (`.dockerignore` applies). Default: none, and a `COPY` fails.
    #[uniffi(default = None)]
    pub context_dir: Option<String>,
    /// A base image, for a build made of `steps` alone: a Cua image,
    /// `ubuntu:<version>`, `debian:<version>` or `python:<version>`.
    #[uniffi(default = None)]
    pub base: Option<String>,
    /// Steps, after the Dockerfile's.
    #[uniffi(default = [])]
    pub steps: Vec<BuildStep>,
    /// Build arguments (`ARG` values).
    #[uniffi(default)]
    pub build_args: HashMap<String, String>,
    /// An alias for the result: `Spaces.fork`, `build:<tag>` images and
    /// `Spaces.create(build=...)` find the build by it.
    #[uniffi(default = None)]
    pub tag: Option<String>,
    /// `vm` (default), `container` or `microvm`.
    #[uniffi(default = None)]
    pub kind: Option<String>,
    /// Where: `local` (default). A build's checkpoints stay on the machine
    /// that took them.
    #[uniffi(default = None)]
    pub on: Option<String>,
    /// vCPUs of the build's machine, and so of Spaces made from it
    /// (default 2).
    #[uniffi(default = None)]
    pub cpus: Option<u32>,
    /// Memory in MiB (default 4096).
    #[uniffi(default = None)]
    pub memory_mb: Option<u64>,
    /// No desktop: only cua-spacesd runs (default true).
    #[uniffi(default = None)]
    pub headless: Option<bool>,
    /// Build every layer again instead of using cached ones.
    #[uniffi(default = false)]
    pub no_cache: bool,
    /// Budget for each step in milliseconds (default one hour).
    #[uniffi(default = None)]
    pub step_timeout_ms: Option<u64>,
}

/// What an image configures for commands run in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct BuildImageConfig {
    /// The `FROM` reference.
    #[serde(default)]
    pub base: String,
    /// Environment from `ENV` steps.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory, when a step set one.
    #[serde(default)]
    pub workdir: Option<String>,
    /// User, when a step set one.
    #[serde(default)]
    pub user: Option<String>,
    /// The user cua-spacesd runs as in the image.
    #[serde(default)]
    pub driver_user: String,
    /// Default command.
    #[serde(default)]
    pub cmd: Option<Vec<String>>,
    /// Entrypoint.
    #[serde(default)]
    pub entrypoint: Option<Vec<String>>,
    /// The shell `Run` used.
    #[serde(default)]
    pub shell: Option<Vec<String>>,
    /// Exposed ports.
    #[serde(default)]
    pub exposed: Vec<String>,
    /// Labels.
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Volumes.
    #[serde(default)]
    pub volumes: Vec<String>,
    /// Stop signal.
    #[serde(default)]
    pub stop_signal: Option<String>,
    /// Health check, as written.
    #[serde(default)]
    pub healthcheck: Option<String>,
}

/// One step of a finished build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct BuildStepResult {
    /// The step (0: the base image).
    pub index: u32,
    /// The step as a line (`RUN make`).
    pub text: String,
    /// Served from a cached layer: nothing ran.
    pub cached: bool,
    /// Seconds it took (0 when cached).
    pub seconds: f64,
    /// The layer's checkpoint, for a step that makes one.
    #[serde(default)]
    pub checkpoint: Option<String>,
}

/// The result of [`Spaces::build`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct BuildResult {
    /// The final layer's checkpoint: `Spaces.fork` and `cua sb fork` start
    /// Spaces from it.
    pub checkpoint: String,
    /// The tag, when one was asked for.
    #[serde(default)]
    pub tag: Option<String>,
    /// The recipe's cache key.
    pub key: String,
    /// Every layer was cached: nothing ran.
    pub cached: bool,
    /// One per step, base first.
    pub steps: Vec<BuildStepResult>,
    /// What commands run in the image get.
    pub config: BuildImageConfig,
    /// Seconds the build took.
    pub seconds: f64,
}

/// A build on this machine ([`Spaces::builds`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct BuildInfo {
    /// The final layer's checkpoint.
    pub checkpoint: String,
    /// The recipe's cache key.
    pub key: String,
    /// Tags naming it.
    pub tags: Vec<String>,
    /// The `FROM` reference.
    pub base: String,
    /// Layers it is made of (checkpoints), base first.
    pub layers: Vec<String>,
    /// RFC 3339 build time.
    pub created_at: String,
    /// Image configuration.
    pub config: BuildImageConfig,
}

/// What a build is doing ([`Spaces::build_with_progress`]).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BuildProgress {
    /// `note`, `step`, `output`, `step_done` or `finished`.
    pub event: String,
    /// The step (0: the base image).
    pub index: u32,
    /// Steps after the base.
    pub total: u32,
    /// The step as a line, a note, or an output line.
    pub text: String,
    /// `step`: served from a cached layer. `finished`: nothing ran.
    pub cached: bool,
    /// `step_done` and `finished`: seconds taken.
    pub seconds: f64,
    /// `step_done`: the layer's checkpoint.
    pub checkpoint: String,
}

impl From<&cua_spaces::build::BuildEvent> for BuildProgress {
    fn from(e: &cua_spaces::build::BuildEvent) -> Self {
        use cua_spaces::build::BuildEvent as E;
        let mut p = BuildProgress {
            event: String::new(),
            index: 0,
            total: 0,
            text: String::new(),
            cached: false,
            seconds: 0.0,
            checkpoint: String::new(),
        };
        match e {
            E::Note { text } => {
                p.event = "note".into();
                p.text = text.clone();
            }
            E::Step {
                index,
                total,
                text,
                cached,
            } => {
                p.event = "step".into();
                p.index = *index;
                p.total = *total;
                p.text = text.clone();
                p.cached = *cached;
            }
            E::Output { index, line } => {
                p.event = "output".into();
                p.index = *index;
                p.text = line.clone();
            }
            E::StepDone {
                index,
                seconds,
                checkpoint,
            } => {
                p.event = "step_done".into();
                p.index = *index;
                p.seconds = *seconds;
                p.checkpoint = checkpoint.clone().unwrap_or_default();
            }
            E::Finished { cached, seconds } => {
                p.event = "finished".into();
                p.cached = *cached;
                p.seconds = *seconds;
            }
        }
        p
    }
}

impl From<dpb::BuildProgress> for BuildProgress {
    fn from(p: dpb::BuildProgress) -> Self {
        BuildProgress {
            event: p.event,
            index: p.index,
            total: p.total,
            text: p.text,
            cached: p.cached,
            seconds: p.seconds,
            checkpoint: p.checkpoint,
        }
    }
}

/// Receives [`BuildProgress`] while [`Spaces::build_with_progress`] runs, in
/// order, ending with `finished`. Called on an SDK worker thread; return
/// quickly.
#[uniffi::export(with_foreign)]
pub trait BuildListener: Send + Sync {
    /// The build moved on.
    fn on_progress(&self, progress: BuildProgress);
}

fn config_from_json(json: &str) -> BuildImageConfig {
    serde_json::from_str(json).unwrap_or_default()
}

fn result_from_pb(r: dpb::BuildResult) -> BuildResult {
    BuildResult {
        checkpoint: r.checkpoint,
        tag: (!r.tag.is_empty()).then_some(r.tag),
        key: r.key,
        cached: r.cached,
        steps: r
            .steps
            .into_iter()
            .map(|s| BuildStepResult {
                index: s.index,
                text: s.text,
                cached: s.cached,
                seconds: s.seconds,
                checkpoint: (!s.checkpoint.is_empty()).then_some(s.checkpoint),
            })
            .collect(),
        config: config_from_json(&r.config_json),
        seconds: r.seconds,
    }
}

impl BuildOptions {
    fn core_steps(&self) -> Result<Vec<cua_spaces::build::BuildStep>> {
        self.steps.iter().map(via_json).collect()
    }

    /// Checks what can be checked before anything runs.
    fn validate(&self) -> Result<()> {
        let given = usize::from(self.dockerfile.is_some()) + usize::from(self.base.is_some());
        if given != 1 {
            return Err(CuaError::InvalidArgument(
                "a build needs exactly one of `dockerfile` and `base`".into(),
            ));
        }
        Ok(())
    }

    fn core_kind_on(
        &self,
    ) -> Result<(
        cua_sandbox_core::placement::Kind,
        Option<cua_sandbox_core::placement::On>,
    )> {
        use cua_sandbox_core::placement::{Kind, On};
        let place =
            |e: cua_sandbox_core::placement::PlacementError| CuaError::InvalidPlacement(e.message);
        let kind = Kind::parse(self.kind.as_deref().unwrap_or("")).map_err(place)?;
        let on = self
            .on
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .map(On::parse)
            .transpose()
            .map_err(place)?;
        Ok((kind, on))
    }

    fn to_core(
        &self,
        progress: Option<cua_spaces::build::BuildSink>,
    ) -> Result<cua_spaces::build::BuildOptions> {
        let (kind, on) = self.core_kind_on()?;
        Ok(cua_spaces::build::BuildOptions {
            recipe: None,
            dockerfile: self.dockerfile.clone(),
            context_dir: self.context_dir.clone().map(Into::into),
            base: self.base.clone(),
            steps: self.core_steps()?,
            build_args: self.build_args.clone().into_iter().collect(),
            tag: self.tag.clone().filter(|t| !t.is_empty()),
            kind,
            on,
            cpus: self.cpus,
            memory_mb: self.memory_mb,
            headless: self.headless,
            no_cache: self.no_cache,
            step_timeout: self.step_timeout_ms.map(Duration::from_millis),
            progress,
        })
    }

    fn to_request(&self) -> Result<dpb::BuildSpaceRequest> {
        let (kind, on) = self.core_kind_on()?;
        Ok(dpb::BuildSpaceRequest {
            dockerfile: self.dockerfile.clone().unwrap_or_default(),
            context_dir: self
                .context_dir
                .as_deref()
                .map(absolute)
                .unwrap_or_default(),
            base: self.base.clone().unwrap_or_default(),
            steps_json: if self.steps.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&self.steps).map_err(|e| CuaError::Internal(e.to_string()))?
            },
            build_args: self.build_args.clone(),
            tag: self.tag.clone().unwrap_or_default(),
            kind: kind.to_string(),
            location: on.map(|o| o.to_string()).unwrap_or_default(),
            cpus: self.cpus.unwrap_or(0),
            memory_mb: self.memory_mb.unwrap_or(0),
            headless: self.headless,
            no_cache: self.no_cache,
            step_timeout: self.step_timeout_ms.map(|ms| pbjson_types::Duration {
                seconds: (ms / 1000) as i64,
                nanos: ((ms % 1000) * 1_000_000) as i32,
            }),
        })
    }
}

/// A path as an absolute one: the daemon resolves it against its own
/// working directory, not the caller's.
fn absolute(p: &str) -> String {
    std::path::absolute(p)
        .map(|a| a.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string())
}

/// How long a build stream may stay silent: every step reports its output
/// as it prints, so a step's own budget plus a margin.
const BUILD_STREAM_MARGIN: Duration = Duration::from_secs(10 * 60);
const DEFAULT_STEP_BUDGET: Duration = Duration::from_secs(3600);

impl Spaces {
    pub(super) async fn build_impl(
        &self,
        options: BuildOptions,
        listener: Option<Arc<dyn BuildListener>>,
    ) -> Result<BuildResult> {
        options.validate()?;
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(spaces) => {
                    let sink = listener.map(|l| {
                        Arc::new(move |e: &cua_spaces::build::BuildEvent| l.on_progress(e.into()))
                            as cua_spaces::build::BuildSink
                    });
                    let core = options.to_core(sink)?;
                    let result = spaces.build(core).await?;
                    via_json(&result)
                }
                Host::Daemon(d) => {
                    let request = options.to_request()?;
                    let step_budget = options
                        .step_timeout_ms
                        .map(Duration::from_millis)
                        .unwrap_or(DEFAULT_STEP_BUDGET);
                    let mut stream = d
                        .spaces()
                        .build_space(request)
                        .await
                        .map_err(daemon_err)?
                        .into_inner();
                    use dpb::build_space_response::Event;
                    let idle = step_budget + BUILD_STREAM_MARGIN;
                    while let Some(msg) = tokio::time::timeout(idle, stream.message())
                        .await
                        .map_err(|_| {
                            CuaError::Timeout(format!(
                                "the cua daemon reported nothing about the build for {} min; see `cua daemon status` and `cua build --help`",
                                idle.as_secs() / 60
                            ))
                        })?
                        .map_err(daemon_err)?
                    {
                        match msg.event {
                            Some(Event::Progress(p)) => {
                                if let Some(l) = &listener {
                                    l.on_progress(p.into());
                                }
                            }
                            Some(Event::Result(r)) => return Ok(result_from_pb(r)),
                            None => {}
                        }
                    }
                    Err(CuaError::Internal(
                        "the daemon ended the build stream without a result".into(),
                    ))
                }
            }
        })
        .await
    }
}

#[uniffi::export]
impl Spaces {
    /// Builds an image from a Dockerfile or steps in a Space, as pinned
    /// layer checkpoints on this machine. Layers already built are reused (a
    /// rebuild of the same recipe is a cache hit that runs nothing; a
    /// changed step rebuilds from it on), and identical builds in flight
    /// share one run. The result's `checkpoint` is a fork source:
    /// `Spaces.fork(result.tag or result.checkpoint, name)` starts a Space
    /// from it in well under a second, with the image's environment,
    /// working directory and user. A failed step is an error that names
    /// the step, its exit code and its last output; earlier layers stay
    /// cached.
    pub async fn build(&self, options: BuildOptions) -> Result<BuildResult> {
        self.build_impl(options, None).await
    }

    /// [`Spaces::build`], reporting each step and the lines it prints to
    /// `listener` as they happen.
    pub async fn build_with_progress(
        &self,
        options: BuildOptions,
        listener: Arc<dyn BuildListener>,
    ) -> Result<BuildResult> {
        self.build_impl(options, Some(listener)).await
    }

    /// The builds on this machine, oldest first, with their tags.
    pub async fn builds(&self) -> Result<Vec<BuildInfo>> {
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(spaces) => via_json(&spaces.builds()?),
                Host::Daemon(d) => Ok(d
                    .spaces()
                    .list_builds(dpb::ListBuildsRequest {})
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .builds
                    .into_iter()
                    .map(|b| BuildInfo {
                        checkpoint: b.checkpoint,
                        key: b.key,
                        tags: b.tags,
                        base: b.base,
                        layers: b.layers,
                        created_at: b.created_at,
                        config: config_from_json(&b.config_json),
                    })
                    .collect()),
            }
        })
        .await
    }

    /// Deletes a build: its tags and the layer checkpoints no other build
    /// uses. `build` is a tag, a recipe key (or its first 12 characters) or
    /// a checkpoint. Returns the checkpoints deleted. Spaces already made
    /// from it keep running.
    pub async fn delete_build(&self, build: String) -> Result<Vec<String>> {
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(spaces) => Ok(spaces.delete_build(&build).await?),
                Host::Daemon(d) => Ok(d
                    .spaces()
                    .delete_build(dpb::DeleteBuildRequest { build })
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .deleted),
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_use_the_cores_json_names() {
        let s = BuildStep::Copy {
            sources: vec!["a".into()],
            dest: "/b".into(),
            owner: None,
            mode: None,
            unpack: false,
        };
        let core: cua_spaces::build::BuildStep = via_json(&s).unwrap();
        assert!(matches!(core, cua_spaces::build::BuildStep::Copy { .. }));
        let run: cua_spaces::build::BuildStep = via_json(&BuildStep::Run {
            command: "make".into(),
        })
        .unwrap();
        assert_eq!(run.describe(), "RUN make");
    }

    #[test]
    fn options_need_a_dockerfile_or_a_base() {
        assert!(BuildOptions::default().validate().is_err());
        let both = BuildOptions {
            dockerfile: Some("FROM linux".into()),
            base: Some("linux".into()),
            ..Default::default()
        };
        assert!(both.validate().is_err());
        let ok = BuildOptions {
            base: Some("ubuntu:24.04".into()),
            steps: vec![BuildStep::Run {
                command: "true".into(),
            }],
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
        let core = ok.to_core(None).unwrap();
        assert_eq!(core.steps.len(), 1);
        assert_eq!(core.kind, cua_sandbox_core::placement::Kind::Auto);
    }

    #[test]
    fn the_request_carries_everything_and_an_absolute_context() {
        let o = BuildOptions {
            dockerfile: Some("FROM linux".into()),
            context_dir: Some(".".into()),
            steps: vec![BuildStep::Env {
                vars: [("A".to_string(), "1".to_string())].into(),
            }],
            tag: Some("app".into()),
            kind: Some("vm".into()),
            cpus: Some(4),
            no_cache: true,
            step_timeout_ms: Some(1500),
            ..Default::default()
        };
        let r = o.to_request().unwrap();
        assert!(std::path::Path::new(&r.context_dir).is_absolute());
        assert!(r.steps_json.contains("\"op\":\"env\""), "{}", r.steps_json);
        assert_eq!((r.tag.as_str(), r.kind.as_str(), r.cpus), ("app", "vm", 4));
        assert!(r.no_cache);
        assert_eq!(r.step_timeout.unwrap().nanos, 500_000_000);
        let bad = BuildOptions {
            base: Some("linux".into()),
            kind: Some("spaceship".into()),
            ..Default::default()
        };
        assert!(matches!(
            bad.to_request(),
            Err(CuaError::InvalidPlacement(_))
        ));
    }

    #[test]
    fn results_convert_from_the_core_and_the_wire() {
        let core = cua_spaces::build::BuildResult {
            checkpoint: "build-abc".into(),
            tag: Some("app".into()),
            key: "k".repeat(64),
            cached: true,
            steps: vec![cua_spaces::build::StepResult {
                index: 0,
                text: "FROM linux".into(),
                cached: true,
                seconds: 0.0,
                checkpoint: Some("build-abc".into()),
            }],
            config: cua_spaces::build::ImageConfig {
                env: [("APP".to_string(), "/srv/app".to_string())].into(),
                workdir: Some("/srv/app".into()),
                ..Default::default()
            },
            seconds: 0.1,
        };
        let sdk: BuildResult = via_json(&core).unwrap();
        assert_eq!(sdk.config.env["APP"], "/srv/app");
        assert_eq!(sdk.config.workdir.as_deref(), Some("/srv/app"));
        assert_eq!(sdk.steps[0].checkpoint.as_deref(), Some("build-abc"));
        let wire = dpb::BuildResult {
            checkpoint: "build-abc".into(),
            tag: String::new(),
            key: "k".into(),
            cached: false,
            steps: vec![],
            config_json: serde_json::to_string(&core.config).unwrap(),
            seconds: 1.0,
        };
        let from_wire = result_from_pb(wire);
        assert_eq!(from_wire.tag, None);
        assert_eq!(from_wire.config, sdk.config);
    }

    #[test]
    fn progress_events_convert() {
        use cua_spaces::build::BuildEvent as E;
        let p: BuildProgress = (&E::Step {
            index: 2,
            total: 3,
            text: "RUN make".into(),
            cached: true,
        })
            .into();
        assert_eq!(
            (p.event.as_str(), p.index, p.total, p.cached),
            ("step", 2, 3, true)
        );
        let p: BuildProgress = (&E::Output {
            index: 2,
            line: "hi".into(),
        })
            .into();
        assert_eq!((p.event.as_str(), p.text.as_str()), ("output", "hi"));
    }
}
