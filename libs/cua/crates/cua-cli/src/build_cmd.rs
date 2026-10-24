// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua build`: an image from a Dockerfile, built in a Space and kept as
//! pinned checkpoints. Works in any directory (the context is the argument,
//! not the repository); layers already built are reused, so a rebuild runs
//! only what changed.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use cua_sdk::{BuildListener, BuildOptions, BuildProgress, Cua, CuaError};

use crate::util::line;

/// `cua build`.
#[derive(Args, Debug, Clone)]
#[command(after_help = "Examples:
  # Build ./Dockerfile with ./ as the context
  cua build -t app
  # Another directory and Dockerfile; build arguments
  cua build -f docker/Dockerfile.dev --build-arg PY=3.12 -t app-dev ./services/api
  # A Dockerfile piped in on stdin, a bigger machine, every layer again
  cua build -f - --cpus 4 --memory 8192 --no-cache -t app
  # Run it
  cua spaces create build:app
  cua sb fork build-<key> mybox")]
pub struct BuildArgs {
    /// The build context: `COPY` and `ADD` sources are relative to it, and
    /// its `.dockerignore` applies.
    #[arg(default_value = ".")]
    pub context: PathBuf,
    /// The Dockerfile (`-`: standard input). Default: `Dockerfile` in the
    /// context.
    #[arg(short = 'f', long = "file")]
    pub file: Option<String>,
    /// Name the result: `build:<tag>` is an image for `cua spaces create`,
    /// and `cua sb fork` finds it.
    #[arg(short = 't', long = "tag")]
    pub tag: Option<String>,
    /// `NAME=VALUE` for an `ARG` (repeatable).
    #[arg(long = "build-arg")]
    pub build_arg: Vec<String>,
    /// `vm` (default), `container` or `microvm`.
    #[arg(long)]
    pub kind: Option<String>,
    /// Where: `local` (default). A build's layers are checkpoints, which
    /// stay on the machine that took them.
    #[arg(long)]
    pub on: Option<String>,
    /// vCPUs of the machine (and of Spaces made from the build; default 2).
    #[arg(long)]
    pub cpus: Option<u32>,
    /// Memory in MiB (default 4096).
    #[arg(long = "memory")]
    pub memory_mb: Option<u64>,
    /// Keep the desktop (the build's machine is headless by default: only
    /// cua-spacesd runs).
    #[arg(long)]
    pub desktop: bool,
    /// Build every layer again.
    #[arg(long)]
    pub no_cache: bool,
    /// Budget for each step.
    #[arg(long, default_value = "1h")]
    pub step_timeout: humantime::Duration,
}

struct Printer;

impl BuildListener for Printer {
    fn on_progress(&self, p: BuildProgress) {
        let mut err = std::io::stderr().lock();
        match p.event.as_str() {
            "note" => {
                let _ = writeln!(err, "note: {}", p.text);
            }
            "step" => {
                let _ = writeln!(
                    err,
                    "Step {}/{} : {}{}",
                    p.index + 1,
                    p.total + 1,
                    p.text,
                    if p.cached { " (cached)" } else { "" }
                );
            }
            "output" => {
                let _ = writeln!(err, "    {}", p.text);
            }
            "step_done" => {
                let _ = writeln!(err, " ---> {} ({:.1}s)", p.checkpoint, p.seconds);
            }
            _ => {}
        }
    }
}

fn parse_args(pairs: &[String]) -> Result<std::collections::HashMap<String, String>, CuaError> {
    pairs
        .iter()
        .map(|p| match p.split_once('=') {
            Some((k, v)) if !k.is_empty() => Ok((k.to_string(), v.to_string())),
            _ => Err(CuaError::InvalidArgument(format!(
                "--build-arg {p:?}: expected NAME=VALUE"
            ))),
        })
        .collect()
}

/// The Dockerfile's text.
fn read_dockerfile(args: &BuildArgs) -> Result<String, CuaError> {
    match args.file.as_deref() {
        Some("-") => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| CuaError::InvalidArgument(format!("reading the Dockerfile: {e}")))?;
            Ok(s)
        }
        other => {
            let path = match other {
                Some(f) => PathBuf::from(f),
                None => args.context.join("Dockerfile"),
            };
            std::fs::read_to_string(&path).map_err(|e| {
                CuaError::NotFound(format!(
                    "{}: {e}{}",
                    path.display(),
                    if other.is_none() {
                        " (pass the Dockerfile with -f, or the directory that holds it)"
                    } else {
                        ""
                    }
                ))
            })
        }
    }
}

/// The options these arguments describe.
pub fn options(args: &BuildArgs) -> Result<BuildOptions, CuaError> {
    if !args.context.is_dir() {
        return Err(CuaError::NotFound(format!(
            "{}: not a directory (the build context)",
            args.context.display()
        )));
    }
    Ok(BuildOptions {
        dockerfile: Some(read_dockerfile(args)?),
        context_dir: Some(args.context.display().to_string()),
        build_args: parse_args(&args.build_arg)?,
        tag: args.tag.clone(),
        kind: args.kind.clone(),
        on: args.on.clone(),
        cpus: args.cpus,
        memory_mb: args.memory_mb,
        headless: args.desktop.then_some(false),
        no_cache: args.no_cache,
        step_timeout_ms: Some(Duration::from(args.step_timeout).as_millis() as u64),
        ..Default::default()
    })
}

/// `cua build`.
pub async fn run(
    cua: &Cua,
    args: BuildArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let options = options(&args)?;
    let spaces = cua.spaces();
    let result = if json {
        spaces.build(options).await?
    } else {
        let printer = Arc::new(Printer);
        spaces.build_with_progress(options, printer).await?
    };
    if json {
        line(
            out,
            serde_json::to_string(&result).map_err(|e| CuaError::Internal(e.to_string()))?,
        );
        return Ok(0);
    }
    let cached = result.steps.iter().filter(|s| s.cached).count();
    let image = result
        .tag
        .clone()
        .unwrap_or_else(|| result.key[..12.min(result.key.len())].to_string());
    line(
        out,
        format!(
            "Built {} ({}; {} of {} steps cached; {:.1}s)",
            result.checkpoint,
            if result.cached {
                "all cached".to_string()
            } else {
                "rebuilt".to_string()
            },
            cached,
            result.steps.len(),
            result.seconds
        ),
    );
    line(out, format!("Run it: cua spaces create build:{image}"));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(dir: &std::path::Path) -> BuildArgs {
        BuildArgs {
            context: dir.to_path_buf(),
            file: None,
            tag: Some("app".into()),
            build_arg: vec!["PY=3.12".into()],
            kind: None,
            on: None,
            cpus: Some(4),
            memory_mb: None,
            desktop: false,
            no_cache: true,
            step_timeout: "90s".parse().unwrap(),
        }
    }

    #[test]
    fn options_read_the_dockerfile_of_any_directory() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Dockerfile"), "FROM ubuntu:24.04\nRUN true\n").unwrap();
        let o = options(&args(d.path())).unwrap();
        assert!(o.dockerfile.unwrap().contains("RUN true"));
        assert_eq!(o.context_dir, Some(d.path().display().to_string()));
        assert_eq!(o.build_args["PY"], "3.12");
        assert_eq!(
            (o.cpus, o.tag.as_deref(), o.no_cache),
            (Some(4), Some("app"), true)
        );
        assert_eq!(o.step_timeout_ms, Some(90_000));
        assert_eq!(o.headless, None);
    }

    #[test]
    fn a_named_dockerfile_and_desktop() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("Dockerfile.dev");
        std::fs::write(&f, "FROM linux\n").unwrap();
        let mut a = args(d.path());
        a.file = Some(f.display().to_string());
        a.desktop = true;
        let o = options(&a).unwrap();
        assert_eq!(o.dockerfile.as_deref(), Some("FROM linux\n"));
        assert_eq!(o.headless, Some(false));
    }

    #[test]
    fn missing_pieces_say_what_to_do() {
        let d = tempfile::tempdir().unwrap();
        let e = options(&args(d.path())).unwrap_err().to_string();
        assert!(e.contains("Dockerfile") && e.contains("-f"), "{e}");
        let mut a = args(&d.path().join("nope"));
        let e = options(&a).unwrap_err().to_string();
        assert!(e.contains("not a directory"), "{e}");
        a.context = d.path().to_path_buf();
        a.build_arg = vec!["NOEQUALS".into()];
        std::fs::write(d.path().join("Dockerfile"), "FROM linux").unwrap();
        let e = options(&a).unwrap_err().to_string();
        assert!(e.contains("NAME=VALUE"), "{e}");
    }
}
