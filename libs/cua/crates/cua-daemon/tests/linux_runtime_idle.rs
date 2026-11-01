//! Opt-in: the built-in Linux runtime VM (the Mac's gVisor/runc engine)
//! gives its memory back to macOS once no Linux Space runs.
//!
//! 1. Deleting the last container Space stops the VM after its idle grace.
//! 2. A VM left running with no Space in it (a previous daemon's) is stopped
//!    by the daemon's idle check ([`VmmLocal::stop_idle_linux_runtime`]).
//!
//! Each step measures the VM process's private memory before and after.
//!
//! ```text
//! CUA_VMM_TEST_BUILTIN_LINUX=1 CUA_HOME=<a home with the built-in runtime> \
//!   cargo test -p cua-daemon --test linux_runtime_idle -- --ignored --nocapture
//! ```
//!
//! Boots the runtime VM (downloads it the first time) and one small
//! container; the VM is stopped at the end either way.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use cua_daemon::local::VmmLocal;
use cua_sandbox_core::{LocalRuntime, LocalStartSpec};
use cua_vmm::managed;

fn spec(name: &str) -> LocalStartSpec {
    LocalStartSpec {
        name: name.into(),
        image: "docker.io/library/alpine:3.20".into(),
        os: "linux".into(),
        cpus: 1,
        memory_mb: 256,
        disk_size_gb: None,
        ports: vec![],
        env: BTreeMap::new(),
        command: Some(vec!["sleep".into(), "3600".into()]),
        ready_timeout: Duration::from_secs(300),
        firmware: None,
        sidecars: vec![],
        registry_credentials: None,
        container_runtime: None,
        kind: Some("container".into()),
        runtime: None,
        restrict_network: false,
        gpu: None,
        probes: vec![],
        ephemeral: false,
        warm: None,
        mounts: vec![],
        forkable: false,
        headless: false,
        egress: Default::default(),
        // A bare alpine, as it is.
        spacesd: Some(false),
    }
}

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

/// Private memory (MiB) of the VZ VM processes that are not in `others`
/// (the VMs other programs ran before this test booted the runtime VM).
fn vm_memory_mb(others: &[u32]) -> u64 {
    vz_pids()
        .into_iter()
        .filter(|p| !others.contains(p))
        .filter_map(cua_vmm::density::process_private_mb)
        .sum()
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

#[tokio::test]
#[ignore = "boots the built-in Linux runtime VM; set CUA_VMM_TEST_BUILTIN_LINUX=1, run with --ignored"]
async fn the_idle_runtime_vm_gives_its_memory_back() {
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
    let result = async {
        // 1. The last Space deleted: the VM stops after its grace.
        let local = VmmLocal::default();
        local
            .start(&spec("idle-a"))
            .await
            .map_err(|e| format!("start: {e}"))?;
        let busy = vm_memory_mb(&others);
        eprintln!("runtime VM with one Space: {busy} MiB");
        if !managed::running() || busy == 0 {
            return Err("the runtime VM is not running".into());
        }
        local
            .delete("idle-a")
            .await
            .map_err(|e| format!("delete: {e}"))?;
        let took = wait_stopped(Duration::from_secs(90)).await?;
        let after = vm_memory_mb(&others);
        eprintln!("stopped {took:?} after the last delete; VM memory now {after} MiB");
        if after > busy / 10 {
            return Err(format!("the stopped VM still holds {after} MiB"));
        }

        // 2. A VM a previous daemon left running, with no Space: a new
        // daemon's idle check stops it.
        managed::ensure().await.map_err(|e| format!("boot: {e}"))?;
        let left = vm_memory_mb(&others);
        eprintln!("runtime VM left running with no Space: {left} MiB");
        let fresh = VmmLocal::default();
        fresh.stop_idle_linux_runtime().await;
        let took = wait_stopped(Duration::from_secs(90)).await?;
        let after = vm_memory_mb(&others);
        eprintln!("a new daemon stopped it in {took:?}; VM memory now {after} MiB");
        if after > left / 10 + 64 {
            return Err(format!("the stopped VM still holds {after} MiB"));
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
