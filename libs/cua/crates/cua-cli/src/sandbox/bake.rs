//! `cua sb bake`: the CLI face of `Sandboxes.bake` (cua-sdk), which saves a
//! running Docker Compose stack as a checkpoint that forks start from with
//! their containers already running.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;
use cua_sdk::{BakeOptions, BakeSummary, Cua, CuaError};

use super::{parse_memory, print, resolve_image};

#[derive(Args, Debug)]
pub struct BakeArgs {
    /// The Docker Compose project directory (holds compose.yaml or
    /// docker-compose.yml, and whatever it builds or mounts).
    pub project: PathBuf,
    /// The checkpoint to save (`cua sb fork CHECKPOINT NAME` starts from it).
    #[arg(long)]
    pub checkpoint: String,
    /// The image: an alias, a reference or a local disk path. Default: the
    /// docker tier, `linux --tier docker` (a Linux image with Docker).
    #[arg(long)]
    pub image: Option<String>,
    /// Tier for an alias IMAGE (`docker` for a Docker-ready Linux VM).
    #[arg(long)]
    pub tier: Option<String>,
    /// The bake Space's name (default `bake-<checkpoint>`).
    #[arg(long)]
    pub name: Option<String>,
    /// vCPUs of the bake Space (forks get the same).
    #[arg(long, visible_alias = "cpus")]
    pub cpu: Option<u32>,
    /// Memory of the bake Space, e.g. 4GB (forks get the same).
    #[arg(long)]
    pub memory: Option<String>,
    /// Compose files, relative to the project (repeatable; default: compose's
    /// own lookup, compose.yaml / docker-compose.yml).
    #[arg(long = "file", short = 'f', value_name = "FILE")]
    pub files: Vec<String>,
    /// Compose project name (default: the directory name).
    #[arg(long)]
    pub project_name: Option<String>,
    /// Where the project lands in the guest (default /opt/cua-bake/<project>).
    #[arg(long, value_name = "GUEST_DIR")]
    pub dest: Option<String>,
    /// `docker save` tarballs to `docker load` before `compose up`, for
    /// images the guest cannot pull (repeatable).
    #[arg(long = "load", value_name = "TAR")]
    pub loads: Vec<PathBuf>,
    /// A guest shell command, run as root in the project directory, that
    /// must exit 0 before the checkpoint (retried until --health-timeout;
    /// repeatable). `compose up --wait` already waits for healthchecks.
    #[arg(long = "health", value_name = "CMD")]
    pub health: Vec<String>,
    /// Budget for `compose up --wait` and each --health command, seconds.
    #[arg(long, default_value_t = 600)]
    pub health_timeout: u32,
    /// A guest shell command, run as root in the project directory after
    /// the stack is healthy and before the checkpoint (repeatable): state
    /// forks should start with, e.g. iptables rules or a warmed cache.
    #[arg(long = "run", value_name = "CMD")]
    pub run: Vec<String>,
    /// Fork the checkpoint once, check the stack is live in the fork (every
    /// service running or healthy, none restarted, the --health commands
    /// pass), then delete the fork.
    #[arg(long)]
    pub verify_fork: bool,
    /// Keep the bake Space after the checkpoint (default: delete it).
    #[arg(long)]
    pub keep: bool,
    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,
}

impl BakeArgs {
    /// The SDK options: an alias IMAGE resolved with its tier, memory in MiB.
    pub fn options(&self) -> Result<BakeOptions, CuaError> {
        let mut o = BakeOptions::new(self.project.display().to_string(), self.checkpoint.clone());
        o.image = match (&self.image, &self.tier) {
            (Some(i), t) => Some(resolve_image(i, t.as_deref())?.0),
            (None, Some(t)) => Some(resolve_image("linux", Some(t))?.0),
            (None, None) => None,
        };
        o.name = self.name.clone();
        o.cpus = self.cpu;
        o.memory_mb = self.memory.as_deref().map(parse_memory).transpose()?;
        o.compose_files = self.files.clone();
        o.project_name = self.project_name.clone();
        o.dest = self.dest.clone();
        o.image_tarballs = self.loads.iter().map(|p| p.display().to_string()).collect();
        o.health = self.health.clone();
        o.health_timeout_s = self.health_timeout;
        o.run = self.run.clone();
        o.verify_fork = self.verify_fork;
        o.keep = self.keep;
        Ok(o)
    }
}

/// The summary as JSON (`--json`).
pub fn summary_json(s: &BakeSummary) -> serde_json::Value {
    serde_json::json!({
        "checkpoint": s.checkpoint,
        "runtime": s.runtime,
        "space": s.space,
        "image": s.image,
        "project": s.project,
        "dest": s.dest,
        "dockerd_ready_s": s.dockerd_ready_s,
        "create_s": s.create_s,
        "upload_bytes": s.upload_bytes,
        "upload_s": s.upload_s,
        "compose_up_s": s.compose_up_s,
        "health_s": s.health_s,
        "checkpoint_s": s.checkpoint_s,
        "total_s": s.total_s,
        "containers": s.containers,
        "verify_fork": s.verify_fork.as_ref().map(|v| serde_json::json!({
            "fork": v.fork, "fork_s": v.fork_s, "check_s": v.check_s, "live": true,
        })),
    })
}

/// Runs `cua sb bake`.
pub async fn bake(
    cua: &Arc<Cua>,
    a: BakeArgs,
    json_global: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let json = a.json || json_global;
    let o = a.options()?;
    if !json {
        eprintln!(
            "Baking {} into checkpoint '{}' (create, compose up, health, checkpoint{})...",
            a.project.display(),
            a.checkpoint,
            if a.verify_fork { ", a verify fork" } else { "" }
        );
    }
    let s = cua.sandboxes().bake(o).await?;
    print(out, summary_json(&s), json, || {
        let ready = s
            .dockerd_ready_s
            .map(|r| format!("dockerd ready {r:.1}s after boot, "))
            .unwrap_or_default();
        let fork = s
            .verify_fork
            .as_ref()
            .map(|v| format!(", a fork came up live in {:.1}s", v.fork_s))
            .unwrap_or_default();
        format!(
            "Checkpoint '{}' baked in {:.1}s: {ready}compose up {:.1}s, checkpoint {:.1}s{fork}.\n{} containers running:\n  {}\nFork it: cua sb fork {} <name>",
            s.checkpoint,
            s.total_s,
            s.compose_up_s,
            s.checkpoint_s,
            s.containers.len(),
            s.containers.join("\n  "),
            s.checkpoint
        )
    });
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        bake: BakeArgs,
    }

    #[test]
    fn flags_map_to_sdk_options() {
        let c = Cli::parse_from([
            "bake",
            "./world",
            "--checkpoint",
            "w1",
            "--image",
            "/tmp/disk.img",
            "--memory",
            "4GB",
            "--cpu",
            "2",
            "-f",
            "a.yaml",
            "-f",
            "b.yaml",
            "--load",
            "img.tar",
            "--health",
            "curl -fsS http://127.0.0.1:8000/",
            "--run",
            "iptables -L",
            "--health-timeout",
            "300",
            "--verify-fork",
        ]);
        let o = c.bake.options().unwrap();
        assert_eq!(o.project_dir, "./world");
        assert_eq!(o.checkpoint, "w1");
        assert_eq!(o.image.as_deref(), Some("/tmp/disk.img"));
        assert_eq!(o.memory_mb, Some(4096));
        assert_eq!(o.cpus, Some(2));
        assert_eq!(o.compose_files, ["a.yaml", "b.yaml"]);
        assert_eq!(o.image_tarballs, ["img.tar"]);
        assert_eq!(o.health, ["curl -fsS http://127.0.0.1:8000/"]);
        assert_eq!(o.run, ["iptables -L"]);
        assert_eq!(o.health_timeout_s, 300);
        assert!(o.verify_fork && !o.keep);
        // No --image: the SDK picks the docker tier.
        let d = Cli::parse_from(["bake", ".", "--checkpoint", "w2"]);
        assert_eq!(d.bake.options().unwrap().image, None);
        // The docker tier is Linux-only.
        let mac = Cli::parse_from([
            "bake",
            ".",
            "--checkpoint",
            "w3",
            "--image",
            "macos",
            "--tier",
            "docker",
        ]);
        assert!(mac.bake.options().is_err());
    }
}
