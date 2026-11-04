//! Building a Dockerfile (`buildInfo` of a sandbox create or a snapshot)
//! into Cua image layers, and the build log clients read.
//!
//! The Dockerfile is parsed by [`super::dockerfile`]; its steps become an
//! `ImageBuild` (Cua's own image layers: the core builds them, caches them
//! by content and boots the result), `COPY` sources are taken from the
//! contexts clients uploaded to the gateway's object store
//! ([`super::objectstore`]).

use super::Daytona;
use super::dockerfile::{self, Step, shell_quote};
use cua_sdk::{BuildFile, ImageBuild, ImageLayer};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, watch};

/// A Dockerfile resolved against Cua.
#[derive(Debug, Clone)]
pub(crate) struct Built {
    pub from: String,
    /// Layers to build on `from` (none for a bare `FROM`).
    pub build: Option<ImageBuild>,
    /// ENTRYPOINT + CMD as the sandbox's command.
    pub command: Option<Vec<String>>,
    pub env: BTreeMap<String, String>,
    pub workdir: Option<String>,
    pub user: Option<String>,
    /// `Sandbox.buildInfo` / `Snapshot.buildInfo`.
    pub info: Value,
}

fn rfc3339_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Parses `dockerfile` and turns it into [`Built`]. `hashes` name the
/// uploaded contexts `COPY` reads from.
pub(crate) async fn resolve(
    d: &Daytona,
    dockerfile_text: &str,
    hashes: &[String],
) -> Result<Built, String> {
    let df = dockerfile::parse(dockerfile_text)?;
    let plan = dockerfile::plan(&df, &BTreeMap::new())?;
    if plan.from.is_empty() {
        return Err("a Dockerfile must start with FROM".into());
    }
    let mut layers = Vec::new();
    let mut files = Vec::new();
    let needs_ctx = plan.steps.iter().any(|s| matches!(s, Step::Copy { .. }));
    let ctx: Vec<PathBuf> = if needs_ctx {
        d.objects.context_roots(hashes).await?
    } else {
        Vec::new()
    };
    let stage = d.objects.stage_dir();
    for (i, s) in plan.steps.iter().enumerate() {
        match s {
            Step::Run(cmd) => layers.push(ImageLayer::Run {
                command: cmd.clone(),
            }),
            Step::Copy {
                source,
                dest,
                chown,
                chmod,
                unpack,
            } => {
                let (tarball, is_dir) = tar_from_contexts(&ctx, source, &stage, i)?;
                let remote = format!("/tmp/.cua-ctx-{i}.tar");
                files.push(BuildFile {
                    source: tarball.display().to_string(),
                    destination: remote.clone(),
                });
                layers.push(ImageLayer::Run {
                    command: place_script(i, &remote, dest, is_dir, *unpack, source, chown, chmod),
                });
            }
        }
    }
    let command = match (&plan.entrypoint, &plan.cmd) {
        (Some(e), Some(c)) => Some(e.iter().chain(c.iter()).cloned().collect()),
        (Some(e), None) => Some(e.clone()),
        (None, Some(c)) => Some(c.clone()),
        (None, None) => None,
    };
    let build =
        (!layers.is_empty() || !files.is_empty() || !plan.ports.is_empty()).then(|| ImageBuild {
            layers,
            env: plan.env.clone().into_iter().collect(),
            ports: plan.ports.clone(),
            files,
            timeout_ms: None,
        });
    let now = rfc3339_now();
    Ok(Built {
        from: plan.from.clone(),
        build,
        command,
        env: plan.env.clone(),
        workdir: plan.workdir.clone(),
        user: plan.user.clone(),
        info: json!({
            "dockerfileContent": dockerfile_text,
            "contextHashes": hashes,
            "createdAt": now,
            "updatedAt": now,
            "snapshotRef": format!("cua-build:{}", plan.from),
        }),
    })
}

/// The first context that has `source`.
fn tar_from_contexts(
    roots: &[PathBuf],
    source: &str,
    stage: &Path,
    i: usize,
) -> Result<(PathBuf, bool), String> {
    let mut last = format!("COPY source {source:?} was not found in the build context");
    for r in roots {
        match tar_source(r, source, stage, i) {
            Ok(x) => return Ok(x),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Tars the `source` of a COPY (a file or a directory's contents) from the
/// build context. Refuses paths outside the context.
fn tar_source(
    root: &Path,
    source: &str,
    stage: &Path,
    i: usize,
) -> Result<(PathBuf, bool), String> {
    let rel = source.trim_start_matches("./").trim_start_matches('/');
    if rel.split('/').any(|c| c == "..") {
        return Err(format!(
            "COPY source {source:?} is outside the build context"
        ));
    }
    let full = root.join(rel);
    let real = full
        .canonicalize()
        .map_err(|_| format!("COPY source {source:?} was not found in the build context"))?;
    let real_root = root.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(&real_root) {
        return Err(format!(
            "COPY source {source:?} is outside the build context"
        ));
    }
    std::fs::create_dir_all(stage).map_err(|e| e.to_string())?;
    let out = stage.join(format!("ctx-{}-{i}.tar", std::process::id()));
    let file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut tb = tar::Builder::new(file);
    tb.follow_symlinks(false);
    let is_dir = real.is_dir();
    if is_dir {
        tb.append_dir_all(".", &real).map_err(|e| e.to_string())?;
    } else {
        tb.append_path_with_name(&real, "f")
            .map_err(|e| e.to_string())?;
    }
    tb.finish().map_err(|e| e.to_string())?;
    Ok((out, is_dir))
}

#[allow(clippy::too_many_arguments)]
fn place_script(
    i: usize,
    tar: &str,
    dest: &str,
    is_dir: bool,
    unpack: bool,
    source: &str,
    chown: &Option<String>,
    chmod: &Option<String>,
) -> String {
    let s = format!("/tmp/.cua-ctx-{i}");
    let d = shell_quote(dest);
    let mut sc = format!("set -e; mkdir -p {s}; tar -xf {tar} -C {s}; ");
    if is_dir {
        sc.push_str(&format!("mkdir -p {d}; cp -a {s}/. {d}/; "));
    } else {
        let base = shell_quote(source.rsplit('/').next().unwrap_or("f"));
        let is_archive = unpack
            && [".tar", ".tar.gz", ".tgz", ".tar.xz", ".tar.bz2"]
                .iter()
                .any(|e| source.ends_with(e));
        if is_archive {
            sc.push_str(&format!("mkdir -p {d}; tar -xf {s}/f -C {d}; "));
        } else {
            sc.push_str(&format!(
                "case {d} in */) mkdir -p {d}; cp -a {s}/f {d}{base};; *) if [ -d {d} ]; then cp -a {s}/f {d}/{base}; else mkdir -p \"$(dirname {d})\"; cp -a {s}/f {d}; fi;; esac; "
            ));
        }
    }
    if let Some(c) = chown {
        sc.push_str(&format!("chown -R {} {d}; ", shell_quote(c)));
    }
    if let Some(m) = chmod {
        sc.push_str(&format!("chmod -R {} {d}; ", shell_quote(m)));
    }
    sc.push_str(&format!("rm -rf {s} {tar}"));
    sc
}

/// A build log clients follow (`GET .../build-logs?follow=true`).
pub struct LogBuf {
    text: Mutex<String>,
    done: watch::Sender<bool>,
}

impl LogBuf {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            text: Mutex::new(String::new()),
            done: watch::channel(false).0,
        })
    }

    pub async fn line(&self, l: &str) {
        let mut t = self.text.lock().await;
        t.push_str(l);
        t.push('\n');
    }

    pub async fn finish(&self) {
        let _ = self.done.send(true);
    }

    pub async fn snapshot(&self) -> (String, bool) {
        (self.text.lock().await.clone(), *self.done.borrow())
    }

    pub fn watch(&self) -> watch::Receiver<bool> {
        self.done.subscribe()
    }
}

/// The log of build `id` (a sandbox or a snapshot id).
pub(crate) async fn log_for(d: &Daytona, id: &str) -> Arc<LogBuf> {
    let mut m = d.build_logs.lock().await;
    m.entry(id.to_string()).or_insert_with(LogBuf::new).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_scripts_place_files_and_directories() {
        let f = place_script(
            0,
            "/tmp/.cua-ctx-0.tar",
            "/w/f.txt",
            false,
            false,
            "a/f.txt",
            &None,
            &None,
        );
        assert!(f.contains("cp -a /tmp/.cua-ctx-0/f /w/f.txt"), "{f}");
        let d = place_script(
            1,
            "/tmp/.cua-ctx-1.tar",
            "/w",
            true,
            false,
            "a",
            &Some("u:g".into()),
            &Some("755".into()),
        );
        assert!(
            d.contains("cp -a /tmp/.cua-ctx-1/. /w/")
                && d.contains("chown -R u:g /w")
                && d.contains("chmod -R 755 /w"),
            "{d}"
        );
        let a = place_script(2, "/t.tar", "/x", false, true, "p.tar.gz", &None, &None);
        assert!(a.contains("tar -xf /tmp/.cua-ctx-2/f -C /x"), "{a}");
    }

    #[test]
    fn tar_source_stays_inside_the_context() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ctx");
        std::fs::create_dir_all(root.join("home/u")).unwrap();
        std::fs::write(root.join("home/u/f.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("secret"), b"s").unwrap();
        let stage = dir.path().join("stage");
        assert!(!tar_source(&root, "home/u/f.txt", &stage, 0).unwrap().1);
        assert!(tar_source(&root, "home/u", &stage, 1).unwrap().1);
        assert!(tar_source(&root, "../secret", &stage, 2).is_err());
        assert!(tar_source(&root, "nope", &stage, 3).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("secret"), root.join("link")).unwrap();
            assert!(tar_source(&root, "link", &stage, 4).is_err());
        }
    }
}
