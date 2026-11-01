//! The build recipe: a base image and ordered steps, from a Dockerfile or
//! built directly, and the options and results of [`crate::Spaces::build`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use cua_image::dockerfile::{self, Exec, Instr};
use cua_sandbox_core::placement::{Kind, On};
use serde::{Deserialize, Serialize};

use super::engine::BuildSink;
use crate::error::{Error, Result};

/// Bump to invalidate every cached layer when the build logic changes.
pub const RECIPE_VERSION: u32 = 1;

/// The `PATH` an image starts with (what Docker's Debian and Ubuntu images
/// set), the value `$PATH` has in a Dockerfile's `ENV`.
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// A base image and the steps applied to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRecipe {
    /// The `FROM` reference (see [`super::classify_base`] for what maps).
    pub base: String,
    /// Steps, in order.
    pub steps: Vec<BuildStep>,
}

/// One build step. Values are literal (a Dockerfile's `$VAR`s are already
/// expanded). `user`, `workdir` and `env` of a `Run` default to what the
/// steps before it set (`User`, `Workdir`, `Env`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BuildStep {
    /// A shell command line (run by the current `Shell`, `/bin/sh -c`).
    Run {
        /// The command line.
        command: String,
        /// Run as this user (default: the current `User`, else root).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        /// Run in this directory (default: the current `Workdir`, else `/`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
        /// Extra environment, over the current `Env`.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
    },
    /// A command and arguments, run without a shell.
    RunExec {
        /// Program and arguments.
        argv: Vec<String>,
        /// As for `Run`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        /// As for `Run`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
        /// As for `Run`.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
    },
    /// Files of the build context copied in (`COPY` / `ADD`).
    Copy {
        /// Paths in the context directory (globs allowed).
        sources: Vec<String>,
        /// Destination in the image; relative to the current `Workdir`.
        dest: String,
        /// `user[:group]` (names or ids) that owns what lands (default root).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        /// A `chmod` mode applied to what lands (default: the source's).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        /// `ADD`: a local tar archive is unpacked into `dest`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unpack: bool,
    },
    /// Environment for later steps and for commands run in the image.
    Env {
        /// Variables.
        vars: BTreeMap<String, String>,
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
    /// Ports the image listens on (`80`, `443/tcp`).
    Expose {
        /// Ports.
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
        labels: BTreeMap<String, String>,
    },
    /// Volume mount points.
    Volume {
        /// Paths.
        paths: Vec<String>,
    },
    /// The stop signal.
    StopSignal {
        /// Signal name or number.
        signal: String,
    },
    /// The health check, as written.
    Healthcheck {
        /// The `HEALTHCHECK` arguments.
        text: String,
    },
}

impl BuildStep {
    /// Whether the step changes the image's filesystem (makes a layer).
    pub fn is_layer(&self) -> bool {
        matches!(
            self,
            BuildStep::Run { .. }
                | BuildStep::RunExec { .. }
                | BuildStep::Copy { .. }
                | BuildStep::Workdir { .. }
        )
    }

    /// The step as one line for progress and errors (`RUN make`).
    pub fn describe(&self) -> String {
        match self {
            BuildStep::Run { command, .. } => format!("RUN {command}"),
            BuildStep::RunExec { argv, .. } => {
                format!("RUN {}", serde_json::to_string(argv).unwrap_or_default())
            }
            BuildStep::Copy {
                sources,
                dest,
                unpack,
                ..
            } => format!(
                "{} {} {dest}",
                if *unpack { "ADD" } else { "COPY" },
                sources.join(" ")
            ),
            BuildStep::Env { vars } => format!(
                "ENV {}",
                vars.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            BuildStep::Workdir { path } => format!("WORKDIR {path}"),
            BuildStep::User { user } => format!("USER {user}"),
            BuildStep::Expose { ports } => format!("EXPOSE {}", ports.join(" ")),
            BuildStep::Cmd { argv } => {
                format!("CMD {}", serde_json::to_string(argv).unwrap_or_default())
            }
            BuildStep::Entrypoint { argv } => {
                format!(
                    "ENTRYPOINT {}",
                    serde_json::to_string(argv).unwrap_or_default()
                )
            }
            BuildStep::Shell { argv } => {
                format!("SHELL {}", serde_json::to_string(argv).unwrap_or_default())
            }
            BuildStep::Label { labels } => format!(
                "LABEL {}",
                labels
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            BuildStep::Volume { paths } => format!("VOLUME {}", paths.join(" ")),
            BuildStep::StopSignal { signal } => format!("STOPSIGNAL {signal}"),
            BuildStep::Healthcheck { text } => format!("HEALTHCHECK {text}"),
        }
    }
}

/// What an image configures for commands run in it, collected from the
/// recipe's metadata steps. A Space made from the build applies `env`,
/// `workdir` and `user` to the commands run through Cua.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageConfig {
    /// The `FROM` reference.
    #[serde(default)]
    pub base: String,
    /// Environment set by `ENV` steps.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory (absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// User (name or id, optionally `:group`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// The user cua-spacesd runs as in the image (commands for another
    /// user are switched to it through `sudo`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub driver_user: String,
    /// Default command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<Vec<String>>,
    /// Entrypoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<Vec<String>>,
    /// The shell `RUN` used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<Vec<String>>,
    /// Exposed ports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposed: Vec<String>,
    /// Labels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Volumes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
    /// Stop signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_signal: Option<String>,
    /// Health check, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healthcheck: Option<String>,
}

impl ImageConfig {
    /// Whether the image configures nothing that changes how a command runs.
    pub fn is_default(&self) -> bool {
        self.env.is_empty() && self.workdir.is_none() && self.user.is_none()
    }

    /// `line` as a shell line that runs with this image's environment,
    /// working directory and user: `export`s, then `cd`, then (when the
    /// image's user is not the one cua-spacesd runs as) `sudo -u`. The
    /// image's defaults for a command run through Cua; a caller that sets its
    /// own environment or directory can still override inside `line`.
    pub fn wrap_shell(&self, line: &str) -> String {
        use super::engine::sh_quote;
        if self.is_default() {
            return line.to_string();
        }
        let mut inner = String::new();
        if let Some(w) = &self.workdir {
            inner.push_str(&format!("cd -- {} && ", sh_quote(w)));
        }
        inner.push_str(line);
        let user = self
            .user
            .as_deref()
            .map(|u| if u == "0" { "root" } else { u });
        let switch = match user {
            Some(u) if !u.is_empty() && u != self.driver_user && !self.driver_user.is_empty() => {
                Some(u)
            }
            _ => None,
        };
        let mut out = String::new();
        for (k, v) in &self.env {
            out.push_str(&format!("export {k}={}; ", sh_quote(v)));
        }
        match switch {
            Some(u) => {
                let (name, group) = match u.split_once(':') {
                    Some((n, g)) => (n, Some(g)),
                    None => (u, None),
                };
                let target = |s: &str| {
                    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
                        format!("#{s}")
                    } else {
                        s.to_string()
                    }
                };
                out.push_str(&format!(
                    "exec sudo -n -H --preserve-env -u {}",
                    sh_quote(&target(if name.is_empty() { "root" } else { name }))
                ));
                if let Some(g) = group.filter(|g| !g.is_empty()) {
                    out.push_str(&format!(" -g {}", sh_quote(&target(g))));
                }
                out.push_str(&format!(" -- /bin/sh -c {}", sh_quote(&inner)));
            }
            None => out.push_str(&inner),
        }
        out
    }
}

/// Options for [`crate::Spaces::build`]. Give the recipe one way: `recipe`,
/// `dockerfile` (text; `context_dir` is where its `COPY` sources are), or
/// `base`; `steps` are applied after it.
#[derive(Clone, Default)]
pub struct BuildOptions {
    /// A recipe.
    pub recipe: Option<BuildRecipe>,
    /// Dockerfile text.
    pub dockerfile: Option<String>,
    /// The build context: `COPY` sources are relative to it. Default: none
    /// (a `COPY` then fails).
    pub context_dir: Option<PathBuf>,
    /// A base image, for a build made of `steps` alone.
    pub base: Option<String>,
    /// Steps, after the recipe's or the Dockerfile's.
    pub steps: Vec<BuildStep>,
    /// Build arguments (`ARG` values).
    pub build_args: BTreeMap<String, String>,
    /// Alias for the result: `Spaces::fork`, `build:<tag>` images and
    /// `Spaces.create(build)` find the build by it.
    pub tag: Option<String>,
    /// What kind of machine builds and runs it (default `vm`).
    pub kind: Kind,
    /// Where (default local: checkpoints live on the machine that took
    /// them).
    pub on: Option<On>,
    /// vCPUs of the machine (and so of Spaces made from the build).
    pub cpus: Option<u32>,
    /// Memory in MiB.
    pub memory_mb: Option<u64>,
    /// No desktop (default true): only cua-spacesd runs.
    pub headless: Option<bool>,
    /// Build every layer again instead of using cached ones.
    pub no_cache: bool,
    /// Budget for each step (default 1 hour).
    pub step_timeout: Option<Duration>,
    /// Receives what the build is doing.
    pub progress: Option<BuildSink>,
}

impl std::fmt::Debug for BuildOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildOptions")
            .field("recipe", &self.recipe)
            .field("dockerfile", &self.dockerfile.is_some())
            .field("context_dir", &self.context_dir)
            .field("base", &self.base)
            .field("steps", &self.steps.len())
            .field("tag", &self.tag)
            .field("kind", &self.kind)
            .field("on", &self.on)
            .field("no_cache", &self.no_cache)
            .finish()
    }
}

/// What one step of a build did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepResult {
    /// 0 for the base image, then 1 for the first step.
    pub index: u32,
    /// The step as a line (`RUN make`, `FROM ubuntu:24.04`).
    pub text: String,
    /// Served from a cached layer (nothing ran).
    pub cached: bool,
    /// Seconds it took (0 when cached).
    pub seconds: f64,
    /// The layer's checkpoint, for a step that makes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

/// The result of [`crate::Spaces::build`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildResult {
    /// The final layer's checkpoint: `Spaces::fork_local` and
    /// `cua sb fork` start Spaces from it.
    pub checkpoint: String,
    /// The tag, when one was asked for.
    pub tag: Option<String>,
    /// The cache key of the whole recipe (the checkpoint is
    /// `build-<first 12>`).
    pub key: String,
    /// Every layer was cached: nothing ran.
    pub cached: bool,
    /// One per step, base first.
    pub steps: Vec<StepResult>,
    /// What commands run in the image get.
    pub config: ImageConfig,
    /// Seconds the whole build took.
    pub seconds: f64,
}

/// Rust-side hash input of the base layer: stable text.
pub(super) fn exec_argv(e: &Exec, shell: &[String]) -> Vec<String> {
    match e {
        Exec::Argv(v) => v.clone(),
        Exec::Shell(line) => {
            let mut a = shell.to_vec();
            a.push(line.clone());
            a
        }
    }
}

fn scope(
    env: &BTreeMap<String, String>,
    args: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut s = args.clone();
    s.extend(env.clone());
    s
}

fn host_platform_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    }
}

impl BuildRecipe {
    /// A recipe from Dockerfile `text`: `ARG`s take their `build_args`
    /// value or default, `$VAR`s are expanded, shell-form `CMD` and
    /// `ENTRYPOINT` become the current shell's argv. Refusals (multi-stage,
    /// heredocs, BuildKit flags, ...) name the line.
    pub fn from_dockerfile(text: &str, build_args: &BTreeMap<String, String>) -> Result<Self> {
        let df = dockerfile::parse(text).map_err(|e| Error::invalid(e.to_string()))?;
        // Build arguments must name an ARG the Dockerfile declares.
        let declared: std::collections::BTreeSet<&str> = df
            .global_args
            .iter()
            .map(|a| a.name.as_str())
            .chain(df.instructions.iter().flat_map(|l| match &l.instr {
                Instr::Arg(a) => a.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
                _ => vec![],
            }))
            .collect();
        if let Some(k) = build_args.keys().find(|k| !declared.contains(k.as_str())) {
            return Err(Error::invalid(format!(
                "build argument {k} is not declared by any ARG in the Dockerfile"
            )));
        }
        let base = df.from_reference(build_args);
        if base.trim().is_empty() {
            return Err(Error::invalid(format!(
                "Dockerfile line {}: FROM expands to nothing (declare the ARG it uses with a default, or pass it as a build argument)",
                df.from_line
            )));
        }
        if let Some(p) = df.from.platform.as_deref() {
            let mut scope = BTreeMap::new();
            for a in &df.global_args {
                if let Some(v) = build_args.get(&a.name).cloned().or(a.default.clone()) {
                    scope.insert(a.name.clone(), v);
                }
            }
            let p = dockerfile::expand(p, &scope);
            let ok = p.is_empty()
                || p == "linux"
                || p == format!("linux/{}", host_platform_arch())
                || p.starts_with("linux/") && p.len() > 6 && p[6..] == *host_platform_arch();
            if !ok {
                return Err(Error::invalid(format!(
                    "Dockerfile line {}: FROM --platform={p}: Spaces run linux/{} here",
                    df.from_line,
                    host_platform_arch()
                )));
            }
        }
        let mut env: BTreeMap<String, String> =
            BTreeMap::from([("PATH".to_string(), DEFAULT_PATH.to_string())]);
        // Variables ENV set (so PATH's seed is not an image variable).
        let mut env_set: std::collections::BTreeSet<String> = Default::default();
        let mut args: BTreeMap<String, String> = BTreeMap::new();
        let mut shell: Vec<String> = vec!["/bin/sh".into(), "-c".into()];
        let mut steps = Vec::new();
        for located in &df.instructions {
            match &located.instr {
                Instr::Arg(decls) => {
                    for d in decls {
                        let v = build_args.get(&d.name).cloned().or_else(|| {
                            d.default
                                .as_ref()
                                .map(|v| dockerfile::expand(v, &scope(&env, &args)))
                        });
                        if let Some(v) = v {
                            args.insert(d.name.clone(), v);
                        }
                    }
                }
                Instr::Env(kv) => {
                    let sc = scope(&env, &args);
                    let mut vars = BTreeMap::new();
                    for (k, v) in kv {
                        vars.insert(k.clone(), dockerfile::expand(v, &sc));
                    }
                    for (k, v) in &vars {
                        env.insert(k.clone(), v.clone());
                        env_set.insert(k.clone());
                    }
                    steps.push(BuildStep::Env { vars });
                }
                Instr::Workdir(w) => steps.push(BuildStep::Workdir {
                    path: dockerfile::expand(w, &scope(&env, &args)),
                }),
                Instr::User(u) => steps.push(BuildStep::User {
                    user: dockerfile::expand(u, &scope(&env, &args)),
                }),
                Instr::Run(e) => {
                    // ARG values are visible to RUN; ENV wins over ARG.
                    let step_env: BTreeMap<String, String> = args
                        .iter()
                        .filter(|(k, _)| !env_set.contains(*k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    steps.push(match e {
                        Exec::Shell(line) => BuildStep::Run {
                            command: line.clone(),
                            user: None,
                            workdir: None,
                            env: step_env,
                        },
                        Exec::Argv(v) => BuildStep::RunExec {
                            argv: v.clone(),
                            user: None,
                            workdir: None,
                            env: step_env,
                        },
                    });
                }
                Instr::Copy(c) => {
                    let sc = scope(&env, &args);
                    steps.push(BuildStep::Copy {
                        sources: c
                            .sources
                            .iter()
                            .map(|s| dockerfile::expand(s, &sc))
                            .collect(),
                        dest: dockerfile::expand(&c.dest, &sc),
                        owner: c.chown.as_ref().map(|o| dockerfile::expand(o, &sc)),
                        mode: c.chmod.clone(),
                        unpack: c.add,
                    });
                }
                Instr::Cmd(e) => steps.push(BuildStep::Cmd {
                    argv: exec_argv(e, &shell),
                }),
                Instr::Entrypoint(e) => steps.push(BuildStep::Entrypoint {
                    argv: exec_argv(e, &shell),
                }),
                Instr::Shell(s) => {
                    shell = s.clone();
                    steps.push(BuildStep::Shell { argv: s.clone() });
                }
                Instr::Label(kv) => {
                    let sc = scope(&env, &args);
                    steps.push(BuildStep::Label {
                        labels: kv
                            .iter()
                            .map(|(k, v)| (k.clone(), dockerfile::expand(v, &sc)))
                            .collect(),
                    });
                }
                Instr::Expose(p) => {
                    let sc = scope(&env, &args);
                    steps.push(BuildStep::Expose {
                        ports: p.iter().map(|x| dockerfile::expand(x, &sc)).collect(),
                    });
                }
                Instr::Volume(v) => {
                    let sc = scope(&env, &args);
                    steps.push(BuildStep::Volume {
                        paths: v.iter().map(|x| dockerfile::expand(x, &sc)).collect(),
                    });
                }
                Instr::StopSignal(s) => steps.push(BuildStep::StopSignal {
                    signal: dockerfile::expand(s, &scope(&env, &args)),
                }),
                Instr::Healthcheck(t) => steps.push(BuildStep::Healthcheck { text: t.clone() }),
            }
        }
        Ok(BuildRecipe { base, steps })
    }
}

impl BuildOptions {
    /// The recipe these options describe: the `recipe`, or the Dockerfile's,
    /// or `base`, then `steps`.
    pub fn to_recipe(&self) -> Result<BuildRecipe> {
        let given = [
            self.recipe.is_some(),
            self.dockerfile.is_some(),
            self.base.is_some(),
        ]
        .iter()
        .filter(|g| **g)
        .count();
        if given != 1 {
            return Err(Error::invalid(
                "a build needs exactly one of a recipe, a Dockerfile or a base image",
            ));
        }
        let mut recipe = match (&self.recipe, &self.dockerfile, &self.base) {
            (Some(r), _, _) => r.clone(),
            (_, Some(text), _) => BuildRecipe::from_dockerfile(text, &self.build_args)?,
            (_, _, Some(base)) => BuildRecipe {
                base: base.clone(),
                steps: vec![],
            },
            _ => unreachable!("exactly one is given"),
        };
        recipe.steps.extend(self.steps.iter().cloned());
        if recipe.base.trim().is_empty() {
            return Err(Error::invalid("the build has no base image"));
        }
        Ok(recipe)
    }
}
