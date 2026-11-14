//! A recipe resolved for execution: the state each step runs in (user,
//! working directory, environment, shell), the files each `COPY` copies, and
//! the chain of cache keys.

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::base::{Base, classify_base};
use super::context::{CopyEntry, plan_copy};
use super::recipe::{BuildRecipe, BuildStep, ImageConfig, RECIPE_VERSION};
use crate::error::{Error, Result};

/// What shape of machine the build runs in; part of the first key (a fork
/// of a checkpoint has the machine's CPUs and memory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Shape {
    pub kind: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub headless: bool,
}

/// What a layer does to the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Action {
    /// The base image (and Python for `python:` bases).
    Base,
    Run {
        argv: Vec<String>,
        env: BTreeMap<String, String>,
        workdir: String,
        /// Empty: root.
        user: String,
    },
    Copy {
        entries: Vec<CopyEntry>,
        owner: Option<String>,
        mode: Option<String>,
    },
    Mkdir {
        path: String,
    },
}

/// One layer: a checkpoint `build-<key12>`.
#[derive(Debug, Clone)]
pub(super) struct Layer {
    pub key: String,
    pub action: Action,
}

impl Layer {
    pub fn checkpoint(&self) -> String {
        checkpoint_name(&self.key)
    }
}

/// `build-<first 12 hex of the key>`.
pub fn checkpoint_name(key: &str) -> String {
    format!("build-{}", &key[..12.min(key.len())])
}

/// A recipe ready to run.
#[derive(Debug, Clone)]
pub(super) struct Plan {
    pub base: Base,
    /// The resolved digest of the base image, when known.
    pub base_digest: Option<String>,
    pub shape: Shape,
    /// `Base` first, then one per layer step.
    pub layers: Vec<Layer>,
    /// The text of every step, base first (index 0 is `FROM`).
    pub texts: Vec<String>,
    /// For each step text, the layer it made (`None`: metadata).
    pub layer_of: Vec<Option<usize>>,
    pub config: ImageConfig,
    /// The key of the whole recipe.
    pub key: String,
}

fn hash(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    hex::encode(h.finalize())
}

fn abs_path(workdir: &str, p: &str) -> String {
    let joined = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("{}/{}", workdir.trim_end_matches('/'), p)
    };
    let trailing = joined.ends_with('/') || p == "." || p.ends_with("/.");
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = format!("/{}", parts.join("/"));
    if trailing && out != "/" {
        out.push('/');
    }
    out
}

fn valid_mode(m: &str) -> bool {
    !m.is_empty()
        && (m.chars().all(|c| ('0'..='7').contains(&c))
            || m.chars().all(|c| "ugoa+-=rwxXst,".contains(c)))
}

fn valid_owner(o: &str) -> bool {
    !o.is_empty()
        && o.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.:".contains(c))
}

/// Resolves `recipe` for execution. The first key covers the base image's
/// reference and its resolved digest (`base_digest`): a tag that moved
/// rebuilds every layer.
pub(super) fn plan(
    recipe: &BuildRecipe,
    context: Option<&Path>,
    shape: Shape,
    base_digest: Option<&str>,
) -> Result<Plan> {
    let base = classify_base(&recipe.base)?;
    let mut key = hash(&[
        "cua-build",
        &RECIPE_VERSION.to_string(),
        &base.image,
        base_digest.unwrap_or(""),
        base.python.as_deref().unwrap_or(""),
        &shape.kind,
        &shape.cpus.to_string(),
        &shape.memory_mb.to_string(),
        &shape.headless.to_string(),
    ]);
    let mut layers = vec![Layer {
        key: key.clone(),
        action: Action::Base,
    }];
    let mut texts = vec![format!("FROM {}", recipe.base)];
    let mut layer_of: Vec<Option<usize>> = vec![Some(0)];
    let mut config = ImageConfig {
        base: recipe.base.clone(),
        ..Default::default()
    };
    // The state a step runs in.
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let mut workdir = "/".to_string();
    let mut user = String::new();
    let mut shell: Vec<String> = vec!["/bin/sh".into(), "-c".into()];
    for (i, step) in recipe.steps.iter().enumerate() {
        let text = step.describe();
        let n = i + 1;
        let fail = |m: String| Error::invalid(format!("step {n} ({text}): {m}"));
        let mut action: Option<Action> = None;
        match step {
            BuildStep::Run {
                command,
                user: u,
                workdir: w,
                env: e,
            } => {
                if command.trim().is_empty() {
                    return Err(fail("the command is empty".into()));
                }
                let mut argv = shell.clone();
                argv.push(command.clone());
                action = Some(run_action(argv, &env, e, &workdir, w, &user, u, &fail)?);
            }
            BuildStep::RunExec {
                argv,
                user: u,
                workdir: w,
                env: e,
            } => {
                if argv.is_empty() {
                    return Err(fail("the command is empty".into()));
                }
                action = Some(run_action(
                    argv.clone(),
                    &env,
                    e,
                    &workdir,
                    w,
                    &user,
                    u,
                    &fail,
                )?);
            }
            BuildStep::Copy {
                sources,
                dest,
                owner,
                mode,
                unpack,
            } => {
                if sources.is_empty() || dest.is_empty() {
                    return Err(fail("COPY needs sources and a destination".into()));
                }
                if let Some(o) = owner
                    && !valid_owner(o)
                {
                    return Err(fail(format!("{o:?} is not a user[:group]")));
                }
                if let Some(m) = mode
                    && !valid_mode(m)
                {
                    return Err(fail(format!("{m:?} is not a chmod mode")));
                }
                let dest_abs = abs_path(&workdir, dest);
                let entries = plan_copy(context, sources, &dest_abs, *unpack).map_err(|e| {
                    fail(
                        e.to_string()
                            .trim_start_matches("invalid argument: ")
                            .to_string(),
                    )
                })?;
                action = Some(Action::Copy {
                    entries,
                    owner: owner.clone(),
                    mode: mode.clone(),
                });
            }
            BuildStep::Workdir { path } => {
                if path.trim().is_empty() {
                    return Err(fail("the path is empty".into()));
                }
                workdir = abs_path(&workdir, path).trim_end_matches('/').to_string();
                if workdir.is_empty() {
                    workdir = "/".into();
                }
                config.workdir = Some(workdir.clone());
                action = Some(Action::Mkdir {
                    path: workdir.clone(),
                });
            }
            BuildStep::User { user: u } => {
                if u.trim().is_empty() {
                    return Err(fail("the user is empty".into()));
                }
                user = if u == "root" || u == "0" {
                    String::new()
                } else {
                    u.clone()
                };
                config.user = Some(u.clone());
            }
            BuildStep::Env { vars } => {
                for (k, v) in vars {
                    if k.is_empty() || k.contains('=') {
                        return Err(fail(format!("{k:?} is not a variable name")));
                    }
                    env.insert(k.clone(), v.clone());
                    config.env.insert(k.clone(), v.clone());
                }
            }
            BuildStep::Expose { ports } => config.exposed.extend(ports.iter().cloned()),
            BuildStep::Cmd { argv } => config.cmd = Some(argv.clone()),
            BuildStep::Entrypoint { argv } => {
                config.entrypoint = Some(argv.clone());
                // A new ENTRYPOINT resets the CMD, as in Docker.
                config.cmd = None;
            }
            BuildStep::Shell { argv } => {
                if argv.is_empty() {
                    return Err(fail("the shell is empty".into()));
                }
                shell = argv.clone();
                config.shell = Some(argv.clone());
            }
            BuildStep::Label { labels } => config.labels.extend(labels.clone()),
            BuildStep::Volume { paths } => config.volumes.extend(paths.iter().cloned()),
            BuildStep::StopSignal { signal } => config.stop_signal = Some(signal.clone()),
            BuildStep::Healthcheck { text } => config.healthcheck = Some(text.clone()),
        }
        // The key chains over the step as resolved (a layer's action, or the
        // metadata step itself).
        let resolved = match &action {
            Some(Action::Run {
                argv,
                env,
                workdir,
                user,
            }) => serde_json::json!({"run": argv, "env": env, "cwd": workdir, "user": user}),
            Some(Action::Copy {
                entries,
                owner,
                mode,
            }) => serde_json::json!({
                "copy": entries.iter().map(|e| e.key_text()).collect::<Vec<_>>(),
                "owner": owner,
                "mode": mode,
            }),
            Some(Action::Mkdir { path }) => serde_json::json!({"mkdir": path}),
            Some(Action::Base) => unreachable!("steps never make a base layer"),
            None => serde_json::to_value(step)?,
        };
        key = hash(&[&key, &resolved.to_string()]);
        texts.push(text);
        match action {
            Some(action) => {
                layers.push(Layer {
                    key: key.clone(),
                    action,
                });
                layer_of.push(Some(layers.len() - 1));
            }
            None => layer_of.push(None),
        }
    }
    Ok(Plan {
        base,
        base_digest: base_digest.map(str::to_string),
        shape,
        layers,
        texts,
        layer_of,
        config,
        key,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_action(
    argv: Vec<String>,
    env: &BTreeMap<String, String>,
    step_env: &BTreeMap<String, String>,
    workdir: &str,
    step_workdir: &Option<String>,
    user: &str,
    step_user: &Option<String>,
    fail: &dyn Fn(String) -> Error,
) -> Result<Action> {
    let mut e = env.clone();
    e.extend(step_env.clone());
    let cwd = match step_workdir {
        Some(w) if !w.is_empty() => abs_path(workdir, w).trim_end_matches('/').to_string(),
        _ => workdir.to_string(),
    };
    let cwd = if cwd.is_empty() { "/".into() } else { cwd };
    let u = match step_user {
        Some(u) if u == "root" || u == "0" => String::new(),
        Some(u) if u.trim().is_empty() => return Err(fail("the user is empty".into())),
        Some(u) => u.clone(),
        None => user.to_string(),
    };
    Ok(Action::Run {
        argv,
        env: e,
        workdir: cwd,
        user: u,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> Shape {
        Shape {
            kind: "vm".into(),
            cpus: 2,
            memory_mb: 4096,
            headless: true,
        }
    }

    fn run(c: &str) -> BuildStep {
        BuildStep::Run {
            command: c.into(),
            user: None,
            workdir: None,
            env: Default::default(),
        }
    }

    fn rec(steps: Vec<BuildStep>) -> BuildRecipe {
        BuildRecipe {
            base: "ubuntu:24.04".into(),
            steps,
        }
    }

    #[test]
    fn layers_are_the_filesystem_steps_and_metadata_does_not_make_one() {
        let p = plan(
            &rec(vec![
                BuildStep::Env {
                    vars: [("A".to_string(), "1".to_string())].into(),
                },
                run("a"),
                BuildStep::Cmd {
                    argv: vec!["x".into()],
                },
                BuildStep::Workdir { path: "/w".into() },
            ]),
            None,
            shape(),
            None,
        )
        .unwrap();
        assert_eq!(p.layers.len(), 3);
        assert_eq!(p.texts.len(), 5);
        assert_eq!(p.layer_of, [Some(0), None, Some(1), None, Some(2)]);
        assert_eq!(p.config.cmd, Some(vec!["x".into()]));
        assert_eq!(p.config.workdir.as_deref(), Some("/w"));
    }

    #[test]
    fn steps_inherit_user_workdir_env_and_shell() {
        let p = plan(
            &rec(vec![
                BuildStep::Env {
                    vars: [("A".to_string(), "1".to_string())].into(),
                },
                BuildStep::Workdir {
                    path: "/srv/app".into(),
                },
                BuildStep::User { user: "cua".into() },
                BuildStep::Shell {
                    argv: vec!["/bin/bash".into(), "-c".into()],
                },
                run("echo $A"),
            ]),
            None,
            shape(),
            None,
        )
        .unwrap();
        let Action::Run {
            argv,
            env,
            workdir,
            user,
        } = &p.layers.last().unwrap().action
        else {
            panic!()
        };
        assert_eq!(argv, &["/bin/bash", "-c", "echo $A"]);
        assert_eq!(env["A"], "1");
        assert_eq!(workdir, "/srv/app");
        assert_eq!(user, "cua");
        assert_eq!(p.config.user.as_deref(), Some("cua"));
    }

    #[test]
    fn root_user_steps_run_as_root() {
        let p = plan(
            &rec(vec![
                BuildStep::User { user: "cua".into() },
                BuildStep::User {
                    user: "root".into(),
                },
                run("x"),
            ]),
            None,
            shape(),
            None,
        )
        .unwrap();
        let Action::Run { user, .. } = &p.layers[1].action else {
            panic!()
        };
        assert_eq!(user, "");
    }

    #[test]
    fn keys_depend_on_everything_that_changes_the_image() {
        let base = plan(&rec(vec![run("a"), run("b")]), None, shape(), None).unwrap();
        let same = plan(&rec(vec![run("a"), run("b")]), None, shape(), None).unwrap();
        assert_eq!(base.key, same.key);
        assert_eq!(base.layers[2].key, same.layers[2].key);
        // A changed second step keeps the first layer's key.
        let changed = plan(&rec(vec![run("a"), run("c")]), None, shape(), None).unwrap();
        assert_eq!(base.layers[1].key, changed.layers[1].key);
        assert_ne!(base.layers[2].key, changed.layers[2].key);
        // A changed first step changes every later key (the chain).
        let first = plan(&rec(vec![run("z"), run("b")]), None, shape(), None).unwrap();
        assert_ne!(base.layers[1].key, first.layers[1].key);
        assert_ne!(base.layers[2].key, first.layers[2].key);
        // The base image, the shape and the recipe version are in the first key.
        let other = plan(
            &BuildRecipe {
                base: "ubuntu:22.04".into(),
                steps: vec![run("a"), run("b")],
            },
            None,
            shape(),
            None,
        )
        .unwrap();
        assert_ne!(base.layers[0].key, other.layers[0].key);
        let mut s = shape();
        s.memory_mb = 8192;
        let big = plan(&rec(vec![run("a"), run("b")]), None, s, None).unwrap();
        assert_ne!(base.layers[0].key, big.layers[0].key);
        // A moved tag (another digest) changes the first key and so every one.
        let moved = plan(
            &rec(vec![run("a"), run("b")]),
            None,
            shape(),
            Some("sha256:two"),
        )
        .unwrap();
        let moved_again = plan(
            &rec(vec![run("a"), run("b")]),
            None,
            shape(),
            Some("sha256:two"),
        )
        .unwrap();
        assert_ne!(base.layers[2].key, moved.layers[2].key);
        assert_eq!(moved.key, moved_again.key);
        // Per-step env, user and workdir are in the step.
        let env = plan(
            &rec(vec![BuildStep::Run {
                command: "a".into(),
                user: None,
                workdir: None,
                env: [("K".to_string(), "v".to_string())].into(),
            }]),
            None,
            shape(),
            None,
        )
        .unwrap();
        assert_ne!(base.layers[1].key, env.layers[1].key);
        // Trailing metadata changes the recipe key but not the last layer's.
        let tail = plan(
            &rec(vec![
                run("a"),
                run("b"),
                BuildStep::Cmd {
                    argv: vec!["x".into()],
                },
            ]),
            None,
            shape(),
            None,
        )
        .unwrap();
        assert_eq!(tail.layers[2].key, base.layers[2].key);
        assert_ne!(tail.key, base.key);
    }

    #[test]
    fn copy_keys_follow_file_contents() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("hello.txt"), "one").unwrap();
        let copy = BuildStep::Copy {
            sources: vec!["hello.txt".into()],
            dest: "./".into(),
            owner: None,
            mode: None,
            unpack: false,
        };
        let r = rec(vec![
            BuildStep::Workdir {
                path: "/srv/app".into(),
            },
            copy.clone(),
            run("after"),
        ]);
        let a = plan(&r, Some(d.path()), shape(), None).unwrap();
        let b = plan(&r, Some(d.path()), shape(), None).unwrap();
        assert_eq!(a.key, b.key);
        let Action::Copy { entries, .. } = &a.layers[2].action else {
            panic!()
        };
        assert_eq!(entries[0].dest, "/srv/app/hello.txt");
        std::fs::write(d.path().join("hello.txt"), "two").unwrap();
        let c = plan(&r, Some(d.path()), shape(), None).unwrap();
        // The workdir layer is unchanged; the COPY and everything after change.
        assert_eq!(a.layers[1].key, c.layers[1].key);
        assert_ne!(a.layers[2].key, c.layers[2].key);
        assert_ne!(a.layers[3].key, c.layers[3].key);
    }

    #[test]
    fn bad_steps_name_the_step() {
        let e = plan(&rec(vec![run("a"), run(" ")]), None, shape(), None).unwrap_err();
        assert!(e.to_string().contains("step 2"), "{e}");
        let e = plan(
            &rec(vec![BuildStep::Copy {
                sources: vec!["a".into()],
                dest: "/d".into(),
                owner: Some("a b".into()),
                mode: None,
                unpack: false,
            }]),
            None,
            shape(),
            None,
        )
        .unwrap_err();
        assert!(e.to_string().contains("user[:group]"), "{e}");
        let e = plan(
            &BuildRecipe {
                base: "alpine:3".into(),
                steps: vec![],
            },
            None,
            shape(),
            None,
        )
        .unwrap_err();
        assert!(e.to_string().contains("alpine:3"), "{e}");
    }

    #[test]
    fn paths_are_normalized() {
        assert_eq!(abs_path("/srv/app", "./"), "/srv/app/");
        assert_eq!(abs_path("/srv/app", "x"), "/srv/app/x");
        assert_eq!(abs_path("/srv/app", "../x"), "/srv/x");
        assert_eq!(abs_path("/", "x/"), "/x/");
        assert_eq!(abs_path("/a", "/b/./c"), "/b/c");
    }
}
