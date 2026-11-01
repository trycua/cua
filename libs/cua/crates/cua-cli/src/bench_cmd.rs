//! `cua bench`: the Spaces performance benchmarks of `libs/cua/bench`.
//!
//! The binary embeds the bench scripts, their checkers and baselines (the
//! files `bench/cuabench.py manifest` lists), so a released `cua` runs the
//! scripts of its own version without a checkout. On use they are unpacked
//! once to `~/.cache/cua/bench/<version>-<hash>/` and `cuabench.py` runs from
//! there with `python3`, with `CUA` set to this binary (or to
//! `CUA_BENCH_CUA`, another build to measure). `CUA_BENCH_DIR` runs another
//! copy of the scripts (a source checkout's `libs/cua/bench`) instead.

use crate::util::internal;
use clap::Subcommand;
use cua_sdk::CuaError;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// `cua bench` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum BenchCmd {
    /// Every benchmark: what it measures, the host it needs, what `--n` sets
    /// and whether it has a perf guard.
    #[command(after_help = "Examples:
  cua bench list
  cua bench list --json")]
    List,
    /// Run one benchmark in Spaces of its own and write the run as
    /// bench-result v1: result.json, samples.csv and the bench's own output
    /// in raw/.
    #[command(
        after_help = "Every run uses a Spaces home of its own (<out>/cua-home, or CUA_HOME when
set) and a daemon of this binary, and deletes the Spaces it creates. A bench
that needs a program a release does not ship (CHANNEL_BENCH, ROLLOUT_BENCH,
DISK, a source checkout) stops and says how to get it (`cua bench list`).

Examples:
  # 5 samples per measurement, results in ./mf
  cua bench run microvm-fork --n 5 --out ./mf
  # With CHANNEL_BENCH set to the SDK's channel_bench example
  cua bench run channel
  # The run as JSON on stdout (progress stays on stderr)
  cua bench run microvm-fork --json"
    )]
    Run {
        /// The benchmark (`cua bench list`).
        name: String,
        /// Directory for result.json, samples.csv and raw/ (default
        /// ./bench-results/<name>/<UTC time>).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Samples (or the size the bench scales by; `cua bench list` says
        /// which). Default: the bench's own.
        #[arg(long)]
        n: Option<u32>,
        /// Source checkout (libs/cua) for benches that build with cargo
        /// (default: CUA_SRC, or the checkout the current directory is in).
        #[arg(long)]
        src: Option<PathBuf>,
    },
    /// Apply a benchmark's perf guard to a run against a committed
    /// baseline; exits 1 on a regression.
    #[command(
        after_help = "A baseline path that does not exist as given is looked up among the
baselines this binary ships (relative to the bench directory).

Examples:
  cua bench check ./mf/result.json --baseline microvm-fork/baseline.json
  cua bench check ./ff/result.json --baseline fork-fanout/baseline.json --host macos-hvf"
    )]
    Check {
        /// A result.json `cua bench run` wrote (its raw/ directory next to it).
        result: PathBuf,
        /// The baseline file.
        #[arg(long)]
        baseline: PathBuf,
        /// Host class or profile in a baseline that holds several
        /// (fork-fanout, fork-batch).
        #[arg(long)]
        host: Option<String>,
    },
}

macro_rules! embed {
    ($($p:literal),* $(,)?) => {
        &[$(($p, include_str!(concat!("../../../bench/", $p)))),*]
    };
}

/// The bench files the binary carries (`bench/cuabench.py manifest`; a test
/// keeps the two lists equal).
pub(crate) const FILES: &[(&str, &str)] = embed![
    "cuabench.py",
    "bench_result.py",
    "README.md",
    "spaces-startup/bench.sh",
    "spaces-startup/sample_host.py",
    "spaces-startup/README.md",
    "spaces-startup/baseline/macos-m2max-96g.json",
    "spaces-startup/baseline/linux-kvm-gha-ubuntu-24.04.json",
    "microvm-fork/bench.sh",
    "microvm-fork/check.py",
    "microvm-fork/baseline.json",
    "fork-fanout/run.sh",
    "fork-fanout/check.py",
    "fork-fanout/baseline.json",
    "fork-batch/bench.py",
    "fork-batch/check.py",
    "fork-batch/baseline.json",
    "repo-clone/bench.py",
    "repo-clone/README.md",
    "channel/run.sh",
    "channel/check.py",
    "channel/baselines/macos-m2max-qemu.json",
    "channel/baselines/ci-ubuntu-24.04-kvm-lane.json",
    "channel/baselines/aws-lan-mesh-c7g.json",
    "rollouts/guard.sh",
    "rollouts/check.py",
    "rollouts/baseline/macos-m2max-96g.json",
    "rollouts/baseline/linux-arm64-ci.json",
    "rollouts/baseline/linux-arm64-c7g.json",
    "pool-sessions/run.sh",
    "pool-sessions/session.sh",
    "pool-sessions/make-site.py",
    "pool-sessions/analyze.py",
    "pool-sessions/window.py",
    "pool-sessions/sample.sh",
    "pool-sessions/node-stats.sh",
    "pool-sessions/README.md",
];

/// A short digest of the embedded files: a new build with other scripts
/// unpacks to a new directory.
fn digest() -> String {
    let mut h = Sha256::new();
    for (p, s) in FILES {
        h.update(p.as_bytes());
        h.update([0]);
        h.update(s.as_bytes());
        h.update([0]);
    }
    h.finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn cache_root() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(x).join("cua").join("bench");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".cache").join("cua").join("bench")
}

/// Unpacks the embedded files under `root` (once per build) and returns
/// their directory.
pub(crate) fn unpack(root: &Path) -> Result<PathBuf, CuaError> {
    let dir = root.join(format!("{}-{}", cua_sdk::VERSION, digest()));
    if dir.join(".complete").exists() {
        return Ok(dir);
    }
    std::fs::create_dir_all(root).map_err(internal)?;
    let tmp = root.join(format!(".tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    for (rel, text) in FILES {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap_or(&tmp)).map_err(internal)?;
        std::fs::write(&p, text).map_err(internal)?;
        #[cfg(unix)]
        if rel.ends_with(".sh") || rel.ends_with(".py") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
                .map_err(internal)?;
        }
    }
    std::fs::write(tmp.join(".complete"), b"").map_err(internal)?;
    // A directory an interrupted unpack left behind.
    if dir.exists() && !dir.join(".complete").exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    match std::fs::rename(&tmp, &dir) {
        Ok(()) => Ok(dir),
        // Another `cua bench` unpacked the same files first.
        Err(_) if dir.join(".complete").exists() => {
            let _ = std::fs::remove_dir_all(&tmp);
            Ok(dir)
        }
        Err(e) => Err(internal(format!(
            "unpack the bench scripts to {}: {e}",
            dir.display()
        ))),
    }
}

/// The directory the bench scripts run from.
fn bench_dir() -> Result<PathBuf, CuaError> {
    if let Some(d) = std::env::var_os("CUA_BENCH_DIR").filter(|v| !v.is_empty()) {
        let d = PathBuf::from(d);
        if !d.join("cuabench.py").is_file() {
            return Err(CuaError::InvalidArgument(format!(
                "CUA_BENCH_DIR={} has no cuabench.py",
                d.display()
            )));
        }
        return Ok(d);
    }
    unpack(&cache_root())
}

/// The arguments `cuabench.py` gets for a subcommand.
fn script_args(cmd: &BenchCmd, json: bool) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    match cmd {
        BenchCmd::List => a.push("list".into()),
        BenchCmd::Run { name, out, n, src } => {
            a.extend(["run".into(), name.clone()]);
            if let Some(o) = out {
                a.extend(["--out".into(), o.display().to_string()]);
            }
            if let Some(n) = n {
                a.extend(["--n".into(), n.to_string()]);
            }
            if let Some(s) = src {
                a.extend(["--src".into(), s.display().to_string()]);
            }
        }
        BenchCmd::Check {
            result,
            baseline,
            host,
        } => {
            a.extend([
                "check".into(),
                result.display().to_string(),
                "--baseline".into(),
                baseline.display().to_string(),
            ]);
            if let Some(h) = host {
                a.extend(["--host".into(), h.clone()]);
            }
        }
    }
    if json {
        a.push("--json".into());
    }
    a
}

/// Runs `cua bench`. Progress goes to stderr; the summary (or JSON) to
/// `out`. Exit code: 0 ok, 1 the bench or its guard failed, 2 a usage or
/// host problem.
pub fn run(cmd: BenchCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    if cfg!(windows) {
        return Err(CuaError::Unsupported(
            "`cua bench` runs on macOS and Linux (its scripts use bash)".into(),
        ));
    }
    let dir = bench_dir()?;
    let python = std::env::var("CUA_BENCH_PYTHON").unwrap_or_else(|_| "python3".into());
    // The `cua` the benches drive: this binary, or CUA_BENCH_CUA (another
    // build, such as a release, measured by this one's runner).
    let me = match std::env::var_os("CUA_BENCH_CUA").filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => std::env::current_exe().map_err(internal)?,
    };
    let child = Command::new(&python)
        .arg(dir.join("cuabench.py"))
        .args(script_args(&cmd, json))
        .env("CUA", &me)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| {
            CuaError::Unsupported(format!(
                "`cua bench` runs its scripts with {python} (Python 3.9 or later): {e}"
            ))
        })?;
    let output = child.wait_with_output().map_err(internal)?;
    out.write_all(&output.stdout).map_err(internal)?;
    Ok(output.status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn bench_src() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench")
    }

    /// The embedded list is exactly what the runner needs (its manifest).
    #[cfg(unix)]
    #[test]
    fn embedded_files_match_the_runner_manifest() {
        let out = Command::new("python3")
            .arg(bench_src().join("cuabench.py"))
            .arg("manifest")
            .output()
            .expect("python3");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut want: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        let mut have: Vec<String> = FILES.iter().map(|(p, _)| p.to_string()).collect();
        want.sort();
        have.sort();
        assert_eq!(
            have, want,
            "bench_cmd::FILES and `cuabench.py manifest` differ"
        );
    }

    /// The binary names only our product in what it ships.
    #[test]
    fn embedded_files_name_no_other_sandbox_product() {
        for (p, text) in FILES {
            let lower = text.to_lowercase();
            for word in ["e2b", "daytona", "vercel", "runloop", "codesandbox"] {
                assert!(!lower.contains(word), "{p} mentions {word}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn unpacks_once_and_runs_list() {
        let tmp = std::env::temp_dir().join(format!("cua-bench-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = unpack(&tmp).unwrap();
        assert!(dir.join(".complete").exists());
        assert_eq!(
            unpack(&tmp).unwrap(),
            dir,
            "a second call reuses the directory"
        );
        for (p, text) in FILES {
            assert_eq!(&std::fs::read_to_string(dir.join(p)).unwrap(), text, "{p}");
        }
        let out = Command::new("python3")
            .arg(dir.join("cuabench.py"))
            .args(["list", "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(
            rows.as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "microvm-fork")
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn arguments_reach_the_runner() {
        let a = script_args(
            &BenchCmd::Run {
                name: "channel".into(),
                out: Some("o".into()),
                n: Some(3),
                src: None,
            },
            true,
        );
        assert_eq!(a, ["run", "channel", "--out", "o", "--n", "3", "--json"]);
        let a = script_args(
            &BenchCmd::Check {
                result: "r.json".into(),
                baseline: "b.json".into(),
                host: Some("macos-hvf".into()),
            },
            false,
        );
        assert_eq!(
            a,
            [
                "check",
                "r.json",
                "--baseline",
                "b.json",
                "--host",
                "macos-hvf"
            ]
        );
    }
}
