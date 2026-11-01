//! Opt-in: VM density on a real host (HVF on a Mac, KVM on Linux). Boots
//! real QEMU VMs from a Linux Spaces disk:
//!
//! 1. **Shared memory counted once.** Two VMs of one configuration restore
//!    one golden snapshot; the runtime's accounting
//!    ([`QemuRuntime::density`]) sees one shared RAM file, resident once,
//!    and each VM's private memory well under its guest RAM.
//! 2. **Admission.** A vCPU overcommit that fits exactly those two refuses
//!    a third with `HostCapacity` naming the CPU limit; a memory cap just
//!    above what the two use refuses it naming the memory limit.
//! 3. **Headless.** A VM whose guest serves no desktop
//!    (`CUA_ENV_NO_DESKTOP`, the `rollout` profile) has no display or input
//!    hardware and still runs commands.
//! 4. **Free page reporting.** A cold-booted VM (anonymous RAM) with the
//!    balloon: memory the guest frees goes back to the host on Linux. On
//!    macOS QEMU's discard leaves it charged (the reason the default is off
//!    there); this asserts that, so a QEMU that starts returning it fails
//!    here and the default can change.
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 CUA_VMM_TEST_DISK=<linux containerDisk qcow2> \
//!   cargo test -p cua-daemon --test qemu_density -- --ignored --nocapture
//! ```
//!
//! At most three 1 GiB VMs at once; state in a temp dir, deleted on every
//! path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_daemon::guest_identity::SpacesdIdentity;
use cua_spacesd_client::SpacesdClient;
use cua_vmm::density::DensityConfig;
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ImageSource, Instance, Probe, Runtime, StartSpec, VmmError};

const SPACESD: u16 = 3211;

fn token() -> String {
    let mut b = [0u8; 16];
    rand::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn spec(name: &str, disk: &Path, token: &str, cpus: u32) -> StartSpec {
    StartSpec::new(
        name,
        ImageSource::Disk {
            path: disk.to_path_buf(),
        },
    )
    .cpus(cpus)
    .memory_mb(1024)
    .env("CUA_ENV_TOKEN", token)
    .port(SPACESD)
    .probe(Probe::Tcp { port: SPACESD })
    .ready_timeout(Duration::from_secs(300))
}

async fn client(inst: &Instance, token: &str) -> Result<SpacesdClient, String> {
    let url = format!(
        "http://{}:{}",
        inst.endpoints.host, inst.endpoints.ports[&SPACESD]
    );
    SpacesdClient::connect_url(&url, Some(token.to_string()))
        .await
        .map_err(|e| format!("connect {}: {e}", inst.name))
}

async fn sh(c: &SpacesdClient, cmd: &str) -> Result<String, String> {
    let o = c.run(cmd).await.map_err(|e| format!("{cmd}: {e}"))?;
    Ok(o.stdout_str())
}

fn runtime(root: &Path, density: DensityConfig, warm: bool, fpr: bool) -> QemuRuntime {
    QemuRuntime::new(QemuConfig {
        root: root.to_path_buf(),
        vnc: false,
        graceful_stop: Duration::from_secs(5),
        identity: Some(Arc::new(SpacesdIdentity)),
        warm,
        density,
        free_page_reporting: fpr,
        ..QemuConfig::default()
    })
}

fn private_mb(rt: &QemuRuntime, name: &str) -> u64 {
    rt.density()
        .usage
        .vms
        .iter()
        .find(|v| v.name == name)
        .map_or(0, |v| v.private_mb)
}

fn gate() -> Option<PathBuf> {
    if std::env::var("CUA_VMM_TEST_QEMU").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_QEMU=1");
        return None;
    }
    Some(PathBuf::from(
        std::env::var("CUA_VMM_TEST_DISK").expect("CUA_VMM_TEST_DISK"),
    ))
}

#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1 and CUA_VMM_TEST_DISK, run with --ignored"]
async fn shared_memory_counts_once_and_admission_refuses_past_the_limits() {
    let Some(disk) = gate() else { return };
    let root = tempfile::tempdir().unwrap();
    let cpus = cua_vmm::density::host_cpus();
    // Two VMs of half the host's CPUs each fill a 1x vCPU limit.
    let half = (cpus / 2).max(1);
    let open = DensityConfig::default();
    let rt = runtime(root.path(), open.clone(), true, false);
    let names = ["dn-a", "dn-b", "dn-c", "dn-h"];
    let result = async {
        let (ta, tb) = (token(), token());
        let a = rt
            .start(&spec("dn-a", &disk, &ta, half))
            .await
            .map_err(|e| format!("a: {e}"))?;
        let b = rt
            .start(&spec("dn-b", &disk, &tb, half))
            .await
            .map_err(|e| format!("b: {e}"))?;
        for (inst, t) in [(&a, &ta), (&b, &tb)] {
            let c = client(inst, t).await?;
            sh(&c, "true").await?;
        }
        // Let both settle past the admission's pending window.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let d = rt.density();
        eprintln!("usage with two warm VMs: {:#?}", d.usage);
        if d.usage.vms.len() != 2 {
            return Err(format!("measured {} VMs, not 2", d.usage.vms.len()));
        }
        // Both restored the one golden snapshot: one shared file, counted
        // once, and each VM's own memory is a fraction of its 1 GiB.
        if d.usage.shared.len() != 1 || d.usage.shared_mb() == 0 {
            return Err(format!("shared RAM files: {:?}", d.usage.shared));
        }
        for v in &d.usage.vms {
            if v.shared_file.is_none() || v.private_mb == 0 || v.private_mb >= 1024 {
                return Err(format!("{}: {v:?}", v.name));
            }
        }
        let summed: u64 =
            d.usage.vms.iter().map(|v| v.private_mb).sum::<u64>() + d.usage.shared_mb();
        if d.usage.vm_rss_mb() != summed {
            return Err("vm_rss_mb is not private + shared once".into());
        }
        if d.headroom_mb.is_none() || d.typical_vm_mb.is_none() {
            return Err(format!("no headroom or typical VM: {d:?}"));
        }

        // A 1x vCPU limit: the two use every CPU, a third is refused.
        let cpu_limited = runtime(
            root.path(),
            DensityConfig {
                vcpu_overcommit: f64::from(cpus) / f64::from(cpus.max(1)),
                ..open.clone()
            },
            true,
            false,
        );
        let tc = token();
        match cpu_limited
            .start(&spec("dn-c", &disk, &tc, cpus - 2 * half + 1))
            .await
        {
            Err(VmmError::HostCapacity(m)) if m.contains("VM CPU limit") => {
                eprintln!("refused as expected: {m}")
            }
            other => return Err(format!("expected a CPU HostCapacity, got {other:?}")),
        }
        if cpu_limited
            .load("dn-c")
            .map(|s| s.pid.is_some())
            .unwrap_or(false)
        {
            return Err("a refused VM was launched".into());
        }
        let _ = cpu_limited.delete("dn-c").await;

        // A memory cap just above what the two use: a third is refused.
        let host = d.usage.host.expect("host memory");
        let fraction = (d.usage.vm_rss_mb() + 16) as f64 / host.total_mb as f64;
        let mem_limited = runtime(
            root.path(),
            DensityConfig {
                max_rss_fraction: fraction,
                ..open.clone()
            },
            true,
            false,
        );
        match mem_limited.start(&spec("dn-c", &disk, &tc, 1)).await {
            Err(VmmError::HostCapacity(m)) if m.contains("VM memory limit") => {
                eprintln!("refused as expected: {m}")
            }
            other => return Err(format!("expected a memory HostCapacity, got {other:?}")),
        }
        let _ = mem_limited.delete("dn-c").await;

        // Headless: no display or input hardware, commands still run.
        let th = token();
        let h = rt
            .start(&spec("dn-h", &disk, &th, 1).env("CUA_ENV_NO_DESKTOP", "true"))
            .await
            .map_err(|e| format!("h: {e}"))?;
        let st = rt.load("dn-h").map_err(|e| e.to_string())?;
        let cmdline = std::fs::read_to_string(root.path().join("dn-h").join("cmdline"))
            .map_err(|e| e.to_string())?;
        if !st.headless
            || ["virtio-gpu", "usb-tablet", "qemu-xhci"]
                .iter()
                .any(|d| cmdline.contains(d))
        {
            return Err(format!("headless VM has display hardware: {cmdline}"));
        }
        let c = client(&h, &th).await?;
        if sh(&c, "echo ok").await?.trim() != "ok" {
            return Err("headless VM did not run a command".into());
        }
        eprintln!(
            "private memory: desktop VM {} MiB, headless VM {} MiB",
            private_mb(&rt, "dn-a"),
            private_mb(&rt, "dn-h")
        );
        Ok::<(), String>(())
    }
    .await;
    for n in names {
        let _ = rt.delete(n).await;
    }
    if let Err(e) = result {
        panic!("{e}");
    }
}

#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1 and CUA_VMM_TEST_DISK, run with --ignored"]
async fn free_page_reporting_returns_freed_guest_memory_where_qemu_can() {
    let Some(disk) = gate() else { return };
    let root = tempfile::tempdir().unwrap();
    // Cold boots (warm off): anonymous guest RAM, the only RAM the balloon
    // reports on. Forced on, also on macOS.
    let rt = runtime(root.path(), DensityConfig::default(), false, true);
    let result = async {
        let t = token();
        let inst = rt
            .start(&spec("fpr-a", &disk, &t, 1))
            .await
            .map_err(|e| format!("start: {e}"))?;
        let st = rt.load("fpr-a").map_err(|e| e.to_string())?;
        let cmdline = std::fs::read_to_string(root.path().join("fpr-a").join("cmdline"))
            .map_err(|e| e.to_string())?;
        if !st.free_page_reporting || !cmdline.contains("free-page-reporting=on") {
            return Err(format!("no balloon: {cmdline}"));
        }
        let c = client(&inst, &t).await?;
        // The guest bound its balloon driver (page reporting registers
        // with it).
        let drv = sh(&c, "ls /sys/bus/virtio/drivers/ 2>/dev/null").await?;
        eprintln!(
            "guest virtio drivers: {}",
            drv.split_whitespace().collect::<Vec<_>>().join(" ")
        );
        if !drv.contains("virtio_balloon") {
            return Err(format!("the guest has no virtio_balloon driver: {drv}"));
        }
        let order = sh(
            &c,
            "cat /sys/module/page_reporting/parameters/page_reporting_order",
        )
        .await?;
        if order.trim() != cua_vmm::qemu::PAGE_REPORTING_ORDER.to_string() {
            return Err(format!("the guest reports from order {order:?}"));
        }
        // Boot leaves free-but-backed pages the guest reports back over the
        // next seconds; wait until the VM's private memory settles.
        let mut before = private_mb(&rt, "fpr-a");
        let mut still = 0;
        let settle = Instant::now() + Duration::from_secs(60);
        while still < 3 && Instant::now() < settle {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let now = private_mb(&rt, "fpr-a");
            still = if now + 8 >= before && before + 8 >= now {
                still + 1
            } else {
                0
            };
            before = now;
        }
        eprintln!(
            "guest memory at rest:\n{}",
            sh(
                &c,
                "grep -E 'MemTotal|MemFree|MemAvailable|Cached' /proc/meminfo"
            )
            .await?
        );
        // Fill the guest's tmpfs (half its RAM; grown to 800 MiB when the
        // guest user may), then free it. `written` is what landed.
        let written: u64 = sh(
            &c,
            "sudo -n mount -o remount,size=800m /dev/shm 2>/dev/null; \
             head -c 734003200 /dev/zero | tr '\\0' 'x' > /dev/shm/cua-fpr 2>/dev/null; sync; \
             du -m /dev/shm/cua-fpr | cut -f1",
        )
        .await?
        .trim()
        .parse()
        .unwrap_or(0);
        eprintln!("the guest wrote {written} MiB to its tmpfs");
        tokio::time::sleep(Duration::from_secs(3)).await;
        let touched = private_mb(&rt, "fpr-a");
        let diag = "id -un; cat /sys/module/page_reporting/parameters/page_reporting_order 2>&1; \
                    grep -E 'Normal|DMA32' /proc/buddyinfo";
        eprintln!("guest before the free:\n{}", sh(&c, diag).await?);
        sh(&c, "rm -f /dev/shm/cua-fpr").await?;
        tokio::time::sleep(Duration::from_secs(5)).await;
        eprintln!(
            "guest after the free:\n{}\nprivate {} MiB",
            sh(&c, diag).await?,
            private_mb(&rt, "fpr-a")
        );
        // Reports go out after ~2 s; give the guest time to batch them.
        let deadline = Instant::now() + Duration::from_secs(40);
        let grown = touched.saturating_sub(before);
        let mut after = private_mb(&rt, "fpr-a");
        while Instant::now() < deadline && after + grown / 2 > touched {
            tokio::time::sleep(Duration::from_secs(2)).await;
            after = private_mb(&rt, "fpr-a");
        }
        eprintln!(
            "private memory: booted {before} MiB, after {written} MiB written {touched} MiB, \
             after it was freed {after} MiB ({})",
            std::env::consts::OS
        );
        if written < 300 || grown < written / 2 {
            return Err(format!(
                "writing {written} MiB grew the VM only {before} -> {touched} MiB"
            ));
        }
        let returned = touched.saturating_sub(after);
        if cfg!(target_os = "linux") {
            if returned < grown / 2 {
                return Err(format!(
                    "the host got back only {returned} MiB of the {grown} MiB the guest freed"
                ));
            }
        } else if returned >= grown / 2 {
            return Err(format!(
                "QEMU now returns freed guest memory on {} ({returned} MiB): turn free page \
                 reporting on by default there (cua_vmm::qemu::free_page_reporting_default)",
                std::env::consts::OS
            ));
        }
        Ok::<(), String>(())
    }
    .await;
    let _ = rt.delete("fpr-a").await;
    if let Err(e) = result {
        panic!("{e}");
    }
}

/// Real memory of the QEMU processes the runtime has started (MiB): the sum
/// of their `Pss` and the sum of their `Rss` from `smaps_rollup`, plus the
/// host's `MemAvailable`. The ground truth the accounting is held to.
#[cfg(target_os = "linux")]
fn real_memory_mb(rt: &QemuRuntime) -> (u64, u64, u64, u64) {
    let kb = |t: &str, k: &str| -> u64 {
        t.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let (mut pss, mut rss, mut private, mut n) = (0, 0, 0, 0);
    for v in &rt.density().usage.vms {
        let Ok(st) = rt.load(&v.name) else { continue };
        let Some(pid) = st.pid else { continue };
        let Ok(t) = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")) else {
            continue;
        };
        pss += kb(&t, "Pss:");
        rss += kb(&t, "Rss:");
        private += kb(&t, "Private_Clean:") + kb(&t, "Private_Dirty:");
        n += 1;
    }
    let _ = n;
    let mem = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    (
        pss >> 10,
        rss >> 10,
        private >> 10,
        kb(&mem, "MemAvailable:") >> 10,
    )
}

/// A world VM (admission off) that dirties some RAM and is checkpointed as
/// `bu-ck`, to fork. Returns the names to delete at the end.
#[cfg(target_os = "linux")]
async fn bake_world(root: &Path, disk: &Path, forkable: bool) -> Vec<String> {
    let open = DensityConfig {
        enabled: false,
        ..DensityConfig::default()
    };
    let rt0 = runtime(root, open, true, true);
    let mut made: Vec<String> = vec![];
    {
        let t = token();
        let mut ws = spec("bu-world", disk, &t, 2).memory_mb(2048);
        ws.forkable = forkable;
        made.push("bu-world".into());
        let world = rt0.start(&ws).await.expect("world");
        let c = client(&world, &t).await.expect("world client");
        let _ = sh(&c, "cloud-init status --wait >/dev/null 2>&1; true").await;
        // The world touches `CUA_VMM_TEST_WORLD_ZERO_MB` of RAM it then
        // frees, and keeps 300 MiB.
        let zero: u64 = std::env::var("CUA_VMM_TEST_WORLD_ZERO_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        sh(
            &c,
            &format!(
                "dd if=/dev/zero of=/dev/shm/z bs=1M count={zero} 2>/dev/null; rm -f /dev/shm/z; \
                 dd if=/dev/urandom of=/dev/shm/world bs=1M count=300 2>/dev/null; sync"
            ),
        )
        .await
        .expect("dirty");
        made.push("bu-ck".into());
        rt0.checkpoint("bu-world", "bu-ck")
            .await
            .expect("checkpoint");
        drop(c);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    made
}

/// `CUA_VMM_TEST_BURST` VMs of 2 vCPU / 2 GiB (what the E2B gateway asks
/// for) are created at once under a memory cap of `CUA_VMM_TEST_CAP_GIB`
/// (default 24.6, the cap of the 32 GiB host of the failing run, whatever
/// this host's size). `CUA_VMM_TEST_MODE`: `cold` boots each from the disk;
/// `fork` (the gateway's path) checkpoints one world VM and forks it,
/// `CUA_VMM_TEST_FORKABLE=1` for a world with a shared RAM file.
///
/// While it runs, every 3 s: what the accounting counts ("its VMs use")
/// next to the kernel's `Pss`, `Rss`, private memory and `MemAvailable`.
#[cfg(target_os = "linux")]
async fn burst(mode: &str) {
    let Some(disk) = gate() else { return };
    let n: usize = std::env::var("CUA_VMM_TEST_BURST")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);
    let cap_gib: f64 = std::env::var("CUA_VMM_TEST_CAP_GIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24.6);
    let forkable = std::env::var("CUA_VMM_TEST_FORKABLE").as_deref() == Ok("1");
    let root = tempfile::tempdir().unwrap();
    let total_mb = cua_vmm::density::host_memory()
        .expect("host memory")
        .total_mb;
    let capped = DensityConfig {
        max_rss_fraction: (cap_gib * 1024.0 / total_mb as f64).min(1.0),
        ..DensityConfig::default()
    };
    let mut made: Vec<String> = vec![];
    if mode == "fork" {
        made = bake_world(root.path(), &disk, forkable).await;
    }
    // The measured runtime shares the state root (and so the world's VM
    // processes) but has the cap.
    let rt = Arc::new(runtime(root.path(), capped, true, true));
    let names: Vec<String> = (0..n).map(|i| format!("bu-{i:02}")).collect();
    let sampler = {
        let rt = rt.clone();
        tokio::spawn(async move {
            let t0 = Instant::now();
            loop {
                let d = rt.density();
                let pending: u64 = d.pending.iter().map(|p| p.expected_mb).sum();
                let (pss, rss, private, avail) = real_memory_mb(&rt);
                eprintln!(
                    "t={:>3}s vms={:>2} counted: private+shared {:>6} MiB, pending {:>6} MiB | \
                     real: pss {pss:>6} rss {rss:>6} private {private:>6} MiB, \
                     MemAvailable {avail} MiB",
                    t0.elapsed().as_secs(),
                    d.usage.vms.len(),
                    d.usage.vm_rss_mb(),
                    pending
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        })
    };
    let stagger: u64 = std::env::var("CUA_VMM_TEST_STAGGER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut set = tokio::task::JoinSet::new();
    for name in &names {
        if stagger > 0 {
            tokio::time::sleep(Duration::from_millis(stagger)).await;
        }
        let t = token();
        let (rt, name, disk, mode) = (rt.clone(), name.clone(), disk.clone(), mode.to_string());
        set.spawn(async move {
            let r = if mode == "fork" {
                let s = spec(&name, &disk, &t, 2)
                    .memory_mb(2048)
                    .ready_timeout(Duration::from_secs(600));
                let mut s = s;
                s.image = ImageSource::Existing;
                match rt.fork("bu-ck", &name).await {
                    Ok(()) => rt.start(&s).await.map(|_| ()),
                    Err(e) => Err(e),
                }
            } else {
                let s = spec(&name, &disk, &t, 2)
                    .memory_mb(2048)
                    .ready_timeout(Duration::from_secs(600));
                rt.start(&s).await.map(|_| ())
            };
            (name, r)
        });
    }
    let (mut up, mut refused, mut failed) = (0, vec![], vec![]);
    while let Some(Ok((name, r))) = set.join_next().await {
        match r {
            Ok(()) => up += 1,
            Err(VmmError::HostCapacity(m)) => refused.push((name, m)),
            Err(e) => failed.push((name, e.to_string())),
        }
    }
    // Let everything settle past the admission window, then compare.
    tokio::time::sleep(Duration::from_secs(50)).await;
    let d = rt.density();
    let (pss, rss, private, avail) = real_memory_mb(&rt);
    eprintln!(
        "RESULT {mode} {n}: {up} up, {} refused, {} failed. Settled: counted {} MiB (private {} \
         + shared {}), real pss {pss} rss {rss} private {private} MiB, MemAvailable {avail} MiB",
        refused.len(),
        failed.len(),
        d.usage.vm_rss_mb(),
        d.usage.vms.iter().map(|v| v.private_mb).sum::<u64>(),
        d.usage.shared_mb()
    );
    for (name, m) in refused.iter().take(3) {
        eprintln!("refused {name}: {m}");
    }
    sampler.abort();
    let counted = d.usage.vm_rss_mb();
    for name in names.iter().chain(made.iter().rev()) {
        let _ = rt.pin_checkpoint(name, false).await;
        let _ = rt.delete(name).await;
    }
    assert!(failed.is_empty(), "creates failed: {failed:?}");
    assert!(
        refused.is_empty(),
        "{} of {n} refused while the VMs really use {pss} MiB (limit {} MiB)",
        refused.len(),
        (cap_gib * 1024.0) as u64
    );
    // Settled VMs are counted within a few percent of the kernel's figure
    // (real = private memory + the shared file once = about Pss).
    let diff = counted.abs_diff(pss);
    assert!(
        diff * 100 <= pss * 8 + 128 * 100,
        "counted {counted} MiB, real Pss {pss} MiB"
    );
}

#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1, CUA_VMM_TEST_DISK and CUA_VMM_TEST_BURST, run with --ignored"]
#[cfg(target_os = "linux")]
async fn a_burst_of_cold_creates_is_admitted_while_real_memory_has_room() {
    burst("cold").await;
}

#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1, CUA_VMM_TEST_DISK and CUA_VMM_TEST_BURST, run with --ignored"]
#[cfg(target_os = "linux")]
async fn a_burst_of_forks_is_admitted_while_real_memory_has_room() {
    burst("fork").await;
}

/// Test programs create a sandbox, use it for a moment and delete it, again
/// and again. `CUA_VMM_TEST_BURST` workers each cold-create, delete and
/// repeat `CUA_VMM_TEST_ROUNDS` times (default 3), so at most that many VMs
/// exist at once and their real memory stays far under the cap. Deleted VMs
/// must stop counting at once, and a VM that is up counts what it uses, not
/// its guest size.
#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1, CUA_VMM_TEST_DISK and CUA_VMM_TEST_BURST, run with --ignored"]
#[cfg(target_os = "linux")]
async fn creating_and_deleting_vms_over_and_over_is_not_refused() {
    let Some(disk) = gate() else { return };
    let workers: usize = std::env::var("CUA_VMM_TEST_BURST")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let rounds: usize = std::env::var("CUA_VMM_TEST_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let cap_gib: f64 = std::env::var("CUA_VMM_TEST_CAP_GIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24.6);
    let fork = std::env::var("CUA_VMM_TEST_MODE").as_deref() == Ok("fork");
    let root = tempfile::tempdir().unwrap();
    let made = if fork {
        bake_world(root.path(), &disk, false).await
    } else {
        vec![]
    };
    let total_mb = cua_vmm::density::host_memory()
        .expect("host memory")
        .total_mb;
    let rt = Arc::new(runtime(
        root.path(),
        DensityConfig {
            max_rss_fraction: (cap_gib * 1024.0 / total_mb as f64).min(1.0),
            ..DensityConfig::default()
        },
        false,
        true,
    ));
    let sampler = {
        let rt = rt.clone();
        tokio::spawn(async move {
            let t0 = Instant::now();
            loop {
                let d = rt.density();
                let (pss, rss, private, avail) = real_memory_mb(&rt);
                eprintln!(
                    "t={:>3}s vms={:>2} pending={:>2} counted {:>6} MiB (private+shared) + \
                     pending {:>6} MiB | real: pss {pss:>6} rss {rss:>6} private {private:>6} \
                     MiB, MemAvailable {avail} MiB",
                    t0.elapsed().as_secs(),
                    d.usage.vms.len(),
                    d.pending.len(),
                    d.usage.vm_rss_mb(),
                    d.pending.iter().map(|p| p.expected_mb).sum::<u64>()
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        })
    };
    let mut set = tokio::task::JoinSet::new();
    for w in 0..workers {
        let (rt, disk) = (rt.clone(), disk.clone());
        set.spawn(async move {
            let mut out = vec![];
            for r in 0..rounds {
                let name = format!("ch-{w:02}-{r}");
                let t = token();
                let mut s = spec(&name, &disk, &t, 2)
                    .memory_mb(2048)
                    .ready_timeout(Duration::from_secs(600));
                let forked = if fork {
                    s.image = ImageSource::Existing;
                    rt.fork("bu-ck", &name).await
                } else {
                    Ok(())
                };
                out.push(
                    match async {
                        forked?;
                        rt.start(&s).await
                    }
                    .await
                    {
                        Ok(_) => Ok(()),
                        Err(VmmError::HostCapacity(m)) => Err(m),
                        Err(e) => Err(format!("failed: {e}")),
                    },
                );
                let _ = rt.delete(&name).await;
            }
            out
        });
    }
    let (mut ok, mut bad) = (0, vec![]);
    while let Some(Ok(res)) = set.join_next().await {
        for r in res {
            match r {
                Ok(()) => ok += 1,
                Err(m) => bad.push(m),
            }
        }
    }
    sampler.abort();
    for n in made.iter().rev() {
        let _ = rt.pin_checkpoint(n, false).await;
        let _ = rt.delete(n).await;
    }
    eprintln!(
        "RESULT churn {workers}x{rounds}: {ok} created, {} refused or failed",
        bad.len()
    );
    for m in bad.iter().take(3) {
        eprintln!("refused: {m}");
    }
    assert!(
        bad.is_empty(),
        "{} of {} refused: {:?}",
        bad.len(),
        workers * rounds,
        bad.first()
    );
}
