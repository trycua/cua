//! Opt-in: the built-in Linux runtime VM keeps its host file descriptors
//! bounded and does not outlive its daemon.
//!
//! 1. A file-heavy job (`uv venv && uv pip install numpy pandas scipy`,
//!    then 100,000 small files written and read, all on the shared job
//!    directories) must not leave the VM's Apple Virtualization process
//!    holding the host's file table: its descriptor count stays under
//!    [`PEAK_LIMIT`] during the job (without the guest's cache trimmer it
//!    grows to about one per file, 101,000) and returns to within
//!    [`SETTLE_MARGIN`] of its idle baseline once the job is done.
//! 2. `stop_linux_runtime_now` (what `cua daemon stop` calls) keeps the VM
//!    while a Linux Space runs and stops it once none does.
//!
//! ```text
//! CUA_VMM_TEST_BUILTIN_LINUX=1 CUA_HOME=<a home with the built-in runtime> \
//!   cargo test -p cua-daemon --test runtime_vm_fds -- --ignored --nocapture
//! ```
//!
//! Boots the runtime VM (downloads it the first time), pulls the uv image
//! and installs numpy, pandas and scipy from PyPI; the VM is stopped at
//! the end either way.

use std::time::{Duration, Instant};

use cua_vmm::container::ContainerConfig;
use cua_vmm::filetable;
use cua_vmm::managed::{self, Choice, LinuxSource, RuntimeStop};
use cua_vmm::runtime::Runtime;
use cua_vmm::types::{ImageSource, Mount, StartSpec, Status};

/// How far above its idle baseline the VM's descriptor count may sit once
/// the job is done.
const SETTLE_MARGIN: u64 = 5_000;

/// The most descriptors the VM may hold at any moment of the job.
const PEAK_LIMIT: u64 = 40_000;

const UV_IMAGE: &str = "ghcr.io/astral-sh/uv:python3.12-bookworm-slim";

/// Pids of Apple Virtualization VM processes (one per VZ VM on the Mac).
fn vz_pids() -> Vec<u32> {
    let out = std::process::Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    out.lines()
        .filter(|l| l.contains("com.apple.Virtualization.VirtualMachine"))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect()
}

/// The runtime VM's process: the one VM process that was not there before.
fn runtime_pid(others: &[u32]) -> Option<u32> {
    let mut mine: Vec<u32> = vz_pids()
        .into_iter()
        .filter(|p| !others.contains(p))
        .collect();
    mine.sort_by_key(|p| std::cmp::Reverse(filetable::process_fds(*p).unwrap_or(0)));
    mine.first().copied()
}

fn fds(pid: u32) -> u64 {
    filetable::process_fds(pid).unwrap_or(0)
}

fn table_open() -> u64 {
    filetable::read().map(|t| t.open).unwrap_or(0)
}

async fn wait_stopped(limit: Duration) -> Result<Duration, String> {
    let t = Instant::now();
    while managed::running() {
        if t.elapsed() > limit {
            return Err(format!("the runtime VM still runs after {limit:?}"));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(t.elapsed())
}

fn job_spec(name: &str, work: &std::path::Path, cache: &std::path::Path) -> StartSpec {
    let script = format!(
        "set -e; cd {work}; uv venv --python 3.12 venv; \
         uv pip install --python venv/bin/python numpy pandas scipy; \
         venv/bin/python -c 'import numpy, pandas, scipy; print(numpy.__version__)' > done.txt; \
         mkdir files; cd files; \
         for d in $(seq 1 100); do mkdir d$d; (cd d$d; seq 1 1000 | xargs touch); done; \
         find . -type f | xargs cat > /dev/null",
        work = work.display()
    );
    let mut spec = StartSpec::new(name, ImageSource::oci(UV_IMAGE))
        .cpus(2)
        .memory_mb(1024)
        .env("UV_CACHE_DIR", cache.display().to_string())
        .env("UV_LINK_MODE", "copy")
        .command(["sh", "-c", script.as_str()])
        .ready_timeout(Duration::from_secs(600));
    for dir in [work, cache] {
        spec.mounts.push(Mount {
            host: dir.to_path_buf(),
            guest: dir.display().to_string(),
            read_only: false,
        });
    }
    spec
}

#[tokio::test]
#[ignore = "boots the built-in Linux runtime VM; set CUA_VMM_TEST_BUILTIN_LINUX=1, run with --ignored"]
async fn the_runtime_vm_keeps_its_file_descriptors_bounded_and_stops_with_the_daemon() {
    if std::env::var("CUA_VMM_TEST_BUILTIN_LINUX").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_BUILTIN_LINUX=1");
        return;
    }
    // SAFETY: set before anything reads it, in this test's own process.
    unsafe {
        std::env::set_var("CUA_RUNTIME_LINUX_IDLE_SECS", "5");
        std::env::set_var(managed::SETTING_ENV, "builtin");
    }
    if managed::running() {
        let _ = managed::stop(&[]).await;
    }
    let others = vz_pids();
    let Ok(Choice::Builtin(rt)) =
        managed::choose(ContainerConfig::default(), LinuxSource::Builtin).await
    else {
        panic!("the built-in runtime is not the engine here");
    };
    let result = async {
        let jobs = managed::job_shares();
        let work = jobs[0].join("fd-test");
        let cache = jobs[2].join("uv");
        // A fresh work directory: nothing a previous run left counts.
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        // Boot the VM first: the baseline is the idle VM's.
        rt.engine().await.map_err(|e| format!("boot: {e}"))?;
        let pid = runtime_pid(&others).ok_or("no new Virtualization VM process")?;
        let (baseline, table0) = (fds(pid), table_open());
        eprintln!("idle VM (pid {pid}): {baseline} fds; kern.num_files {table0}");

        rt.start(&job_spec("fd-job", &work, &cache))
            .await
            .map_err(|e| format!("start job: {e}"))?;
        let started = Instant::now();
        let mut peak = baseline;
        loop {
            let n = fds(pid);
            peak = peak.max(n);
            if started.elapsed().as_secs().is_multiple_of(10) {
                eprintln!(
                    "  t={:>3}s VM fds {n} (peak {peak}); kern.num_files {}",
                    started.elapsed().as_secs(),
                    table_open()
                );
            }
            match rt.status("fd-job").await {
                Ok(Status::Running) => {}
                Ok(_) | Err(_) => break,
            }
            if started.elapsed() > Duration::from_secs(900) {
                return Err("the job did not finish in 15 minutes".to_string());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if !work.join("done.txt").is_file() {
            return Err("the job did not finish its install".into());
        }
        eprintln!("job done in {:?}: peak {peak} fds", started.elapsed());

        // The descriptor count settles near the idle baseline.
        let settle = Instant::now();
        let mut now = fds(pid);
        while now > baseline + SETTLE_MARGIN && settle.elapsed() < Duration::from_secs(120) {
            tokio::time::sleep(Duration::from_secs(2)).await;
            now = fds(pid);
        }
        eprintln!(
            "after the job: {now} fds ({:?} to settle; baseline {baseline}); kern.num_files {}",
            settle.elapsed(),
            table_open()
        );
        if peak > PEAK_LIMIT {
            return Err(format!(
                "the VM held {peak} fds during the job (limit {PEAK_LIMIT})"
            ));
        }
        if now > baseline + SETTLE_MARGIN {
            return Err(format!(
                "the VM still holds {now} fds after the job (baseline {baseline})"
            ));
        }

        // `cua daemon stop`: a Space runs, so the VM stays; none does, so
        // it stops.
        let space = StartSpec::new(
            "fd-space",
            ImageSource::oci("docker.io/library/alpine:3.20"),
        )
        .cpus(1)
        .memory_mb(128)
        .command(["sleep", "3600"])
        .ready_timeout(Duration::from_secs(300));
        rt.start(&space)
            .await
            .map_err(|e| format!("start space: {e}"))?;
        let kept = rt.stop_now().await;
        eprintln!("daemon stop with a Space running: {kept:?}");
        if kept != RuntimeStop::KeptForSpaces(1) || !managed::running() {
            return Err(format!("the VM did not stay for a running Space: {kept:?}"));
        }
        if let Some(sentence) = kept.keeps_running_sentence() {
            eprintln!("  says: {sentence}");
        }
        rt.delete("fd-space")
            .await
            .map_err(|e| format!("delete space: {e}"))?;
        let _ = rt.delete("fd-job").await;
        // Deleting the last Space schedules the idle stop; the daemon's
        // own stop does not wait for the grace.
        let stopped = rt.stop_now().await;
        eprintln!("daemon stop with no Space: {stopped:?}");
        if managed::running() {
            wait_stopped(Duration::from_secs(60)).await?;
        } else if !matches!(stopped, RuntimeStop::Stopped | RuntimeStop::NotRunning) {
            return Err(format!("unexpected stop result {stopped:?}"));
        }
        let after = table_open();
        eprintln!("runtime VM gone; kern.num_files {after} (was {table0} with it idle)");
        if vz_pids().contains(&pid) {
            return Err("the VM process is still alive".into());
        }
        Ok::<(), String>(())
    }
    .await;
    if managed::running() {
        let _ = managed::stop(&[]).await;
    }
    if let Err(e) = result {
        panic!("{e}");
    }
}
