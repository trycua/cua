//! Opt-in: copy-on-write fork fan-out of a live QEMU memory checkpoint,
//! with the daemon's identity provider. Boots real VMs from a Linux Spaces
//! disk:
//!
//! 1. a bake VM ("world") dirties a few hundred MB of RAM (a tmpfs file)
//!    and writes a marker to its disk, then is checkpointed and pinned;
//! 2. the checkpoint is forked in waves (8, then 32, ...; `CUA_FORK_WAVES`),
//!    every fork of a wave started at once;
//! 3. every fork must hold the world's RAM and disk state, take a write of
//!    its own (RAM and disk) that no other fork sees, and share the world's
//!    pages it did not write: its private memory (macOS `footprint`, Linux
//!    `smaps_rollup`) must stay well under the world's RAM;
//! 4. a pinned checkpoint refuses deletion until unpinned.
//!
//! With `CUA_FORK_FORKABLE=0` the bake VM runs with anonymous RAM, so the
//! checkpoint takes the other path (its RAM stream is loaded into a file
//! once). Results (fork latencies, totals, memory per fork) are written as
//! JSON to `CUA_FORK_BENCH_OUT` when set: `libs/cua/bench/fork-fanout`
//! checks them against its baseline.
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 CUA_VMM_TEST_DISK=<linux containerDisk qcow2> \
//!   [CUA_FORK_WAVES=8,32,64] [CUA_FORK_MEMORY_MB=2048] [CUA_FORK_DIRTY_MB=300] \
//!   [CUA_FORK_FORKABLE=1] [CUA_FORK_ROOT=<state dir>] [CUA_FORK_BENCH_OUT=out.json] \
//!   cargo test -p cua-daemon --test qemu_fork_fanout -- --ignored --nocapture
//! ```
//!
//! At most 1 + the largest wave VMs at once; state in a temp dir (or
//! `CUA_FORK_ROOT`), every VM deleted on every path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_daemon::guest_identity::SpacesdIdentity;
use cua_spacesd_client::SpacesdClient;
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ImageSource, Instance, Probe, Runtime, StartSpec};
use serde_json::json;

const SPACESD: u16 = 3211;

fn env_or<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(d)
}

fn token() -> String {
    let mut b = [0u8; 16];
    rand::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn spec(name: &str, image: ImageSource, token: &str, memory_mb: u64) -> StartSpec {
    StartSpec::new(name, image)
        .cpus(2)
        .memory_mb(memory_mb)
        .env("CUA_ENV_TOKEN", token)
        .port(SPACESD)
        .probe(Probe::Tcp { port: SPACESD })
        .ready_timeout(Duration::from_secs(300))
}

fn url(inst: &Instance) -> String {
    format!(
        "http://{}:{}",
        inst.endpoints.host, inst.endpoints.ports[&SPACESD]
    )
}

async fn sh(inst: &Instance, token: &str, cmd: &str) -> Result<String, String> {
    // A cold-booted guest's cua-spacesd restarts once when cloud-init
    // finishes: retry what never reached the guest.
    let mut last = String::new();
    for _ in 0..60 {
        let c = match SpacesdClient::connect_url(&url(inst), Some(token.to_string())).await {
            Ok(c) => c,
            Err(e) => {
                last = format!("connect {}: {e}", inst.name);
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        match c.run(cmd).await {
            Ok(o) if o.success() => return Ok(o.stdout_str().trim().to_string()),
            Ok(o) => {
                return Err(format!(
                    "{} `{cmd}` failed ({:?}): {}",
                    inst.name,
                    o.status,
                    o.stderr_str()
                ));
            }
            Err(e) if e.to_string().contains("transport") => {
                last = format!("run on {}: {e}", inst.name);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => return Err(format!("run on {}: {e}", inst.name)),
        }
    }
    Err(last)
}

/// Private (unique) and shared resident bytes of process `pid`.
#[derive(Clone, Copy, Debug, Default)]
struct Mem {
    private: u64,
    shared: u64,
}

/// Host memory, bytes: private (anonymous, wired and compressed pages: what
/// a fork's copy-on-write copies and its QEMU process cost) and the file
/// cache (shared pages of the checkpoint's RAM file, counted once).
#[derive(Clone, Copy, Debug, Default)]
struct HostMem {
    private: i64,
    file: i64,
}

fn host_mem() -> HostMem {
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("vm_stat")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let page: i64 = out
            .split("page size of ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(16384);
        let get = |k: &str| -> i64 {
            out.lines()
                .find(|l| l.starts_with(k))
                .and_then(|l| l.rsplit(':').next())
                .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
                .unwrap_or(0)
        };
        HostMem {
            private: (get("Anonymous pages")
                + get("Pages wired down")
                + get("Pages occupied by compressor"))
                * page,
            file: get("File-backed pages") * page,
        }
    } else {
        let s = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let kb = |k: &str| -> i64 {
            s.lines()
                .find(|l| l.starts_with(k))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
                * 1024
        };
        HostMem {
            private: kb("AnonPages:") + kb("Shmem:") + kb("KernelStack:") + kb("PageTables:"),
            file: kb("Cached:") - kb("Shmem:"),
        }
    }
}

/// Lets the host's memory counters settle, then samples them.
async fn settled_host_mem() -> HostMem {
    tokio::time::sleep(Duration::from_secs(3)).await;
    host_mem()
}

fn mem_of(pids: &[u32]) -> std::collections::BTreeMap<u32, Mem> {
    let mut out = std::collections::BTreeMap::new();
    if cfg!(target_os = "macos") {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("fp.json");
        let mut cmd = std::process::Command::new("footprint");
        cmd.args(["-f", "bytes", "-j"]).arg(&json);
        for p in pids {
            cmd.args(["-p", &p.to_string()]);
        }
        let _ = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let v: serde_json::Value = std::fs::read(&json)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for p in v["processes"].as_array().into_iter().flatten() {
            let pid = p["pid"].as_u64().unwrap_or(0) as u32;
            let shared = p["categories"]["mapped file"]["clean"]
                .as_u64()
                .unwrap_or(0);
            out.insert(
                pid,
                Mem {
                    private: p["footprint"].as_u64().unwrap_or(0),
                    shared,
                },
            );
        }
    } else {
        for p in pids {
            let s = std::fs::read_to_string(format!("/proc/{p}/smaps_rollup")).unwrap_or_default();
            let kb = |k: &str| -> u64 {
                s.lines()
                    .find(|l| l.starts_with(k))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
                    * 1024
            };
            out.insert(
                *p,
                Mem {
                    private: kb("Private_Dirty:") + kb("Private_Clean:"),
                    shared: kb("Shared_Clean:") + kb("Shared_Dirty:"),
                },
            );
        }
    }
    out
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return 0.0;
    }
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1 and CUA_VMM_TEST_DISK, run with --ignored"]
async fn forks_of_a_live_checkpoint_share_its_memory() {
    if std::env::var("CUA_VMM_TEST_QEMU").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_QEMU=1");
        return;
    }
    // `RUST_LOG=cua_vmm=info` shows each restore's phases.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    let disk = PathBuf::from(std::env::var("CUA_VMM_TEST_DISK").expect("CUA_VMM_TEST_DISK"));
    let waves: Vec<usize> = std::env::var("CUA_FORK_WAVES")
        .unwrap_or_else(|_| "8".into())
        .split(',')
        .filter_map(|w| w.trim().parse().ok())
        .collect();
    let memory_mb: u64 = env_or("CUA_FORK_MEMORY_MB", 2048);
    let dirty_mb: u64 = env_or("CUA_FORK_DIRTY_MB", 300);
    let forkable = std::env::var("CUA_FORK_FORKABLE").as_deref() != Ok("0");
    let tmp;
    let root: PathBuf = match std::env::var("CUA_FORK_ROOT") {
        Ok(r) => {
            std::fs::create_dir_all(&r).unwrap();
            PathBuf::from(r)
        }
        Err(_) => {
            tmp = tempfile::tempdir().unwrap();
            tmp.path().to_path_buf()
        }
    };
    let rt = QemuRuntime::new(QemuConfig {
        root: root.clone(),
        vnc: false,
        graceful_stop: Duration::from_secs(5),
        identity: Some(Arc::new(SpacesdIdentity)),
        // A bake VM restored from a golden snapshot (an image whose
        // cua-spacesd can become a new machine) clones its RAM file.
        warm: std::env::var("CUA_FORK_WARM").as_deref() != Ok("0"),
        ..QemuConfig::default()
    });
    let mut report = json!({
        "host": std::env::consts::OS,
        "forkable": forkable,
        "memory_mb": memory_mb,
        "dirty_mb": dirty_mb,
        "waves": [],
    });
    let mut made: Vec<String> = vec![];
    let result = run(
        &rt,
        &root,
        &disk,
        &waves,
        memory_mb,
        dirty_mb,
        forkable,
        &mut report,
        &mut made,
    )
    .await;
    // Tear down: forks, the bake VM, then the checkpoint.
    for n in made.iter().rev() {
        let _ = rt.pin_checkpoint(n, false).await;
        let _ = rt.delete(n).await;
    }
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    if let Ok(out) = std::env::var("CUA_FORK_BENCH_OUT") {
        std::fs::write(out, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    if let Err(e) = result {
        panic!("{e}");
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    rt: &QemuRuntime,
    root: &Path,
    disk: &Path,
    waves: &[usize],
    memory_mb: u64,
    dirty_mb: u64,
    forkable: bool,
    report: &mut serde_json::Value,
    made: &mut Vec<String>,
) -> Result<(), String> {
    // 1. The bake VM and its world.
    let tw = token();
    let mut ws = spec(
        "ff-world",
        ImageSource::Disk {
            path: disk.to_path_buf(),
        },
        &tw,
        memory_mb,
    );
    ws.forkable = forkable;
    made.push("ff-world".into());
    let t = Instant::now();
    let world = rt.start(&ws).await.map_err(|e| format!("world: {e}"))?;
    let world_start = t.elapsed();
    let world_st = rt.load("ff-world").map_err(|e| e.to_string())?;
    eprintln!(
        "world up in {world_start:?} ({:?}, shared memory: {})",
        world_st.last_start, world_st.shared_memory
    );
    // An image whose cua-spacesd can become a new machine restores the bake
    // VM from its golden snapshot (into a clone of the golden RAM file).
    if std::env::var("CUA_FORK_EXPECT_WARM").as_deref() == Ok("1")
        && world_st.last_start.as_deref() != Some("warm")
    {
        return Err(format!(
            "the world started {:?}, not warm",
            world_st.last_start
        ));
    }
    if forkable && !(world_st.shared_memory && root.join("ff-world").join("mem.ram").is_file()) {
        return Err("the forkable world's RAM is not its own shared file".into());
    }
    // Settle first: cloud-init restarts cua-spacesd (and its children) once.
    let settle = Instant::now();
    while sh(
        &world,
        &tw,
        "cloud-init status --wait >/dev/null 2>&1; true",
    )
    .await
    .is_err()
    {
        if settle.elapsed() > Duration::from_secs(180) {
            return Err("the world never settled".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let world_hash = sh(
        &world,
        &tw,
        &format!(
            "dd if=/dev/urandom of=/dev/shm/world bs=1M count={dirty_mb} 2>/dev/null && \
             echo world-disk >/var/tmp/world-marker && sync && \
             sha256sum /dev/shm/world | cut -d' ' -f1"
        ),
    )
    .await?;
    eprintln!("world dirtied {dirty_mb} MB of RAM: {world_hash}");

    // 2. Checkpoint (pinned).
    made.push("ff-ck".into());
    let t = Instant::now();
    let info = rt
        .checkpoint("ff-world", "ff-ck")
        .await
        .map_err(|e| format!("checkpoint: {e}"))?;
    let ck_ms = t.elapsed().as_secs_f64() * 1000.0;
    if forkable && !info.shared_memory {
        return Err("the checkpoint's forks would not share its memory".into());
    }
    rt.pin_checkpoint("ff-ck", true)
        .await
        .map_err(|e| format!("pin: {e}"))?;
    let ck_dir = root.join("ff-ck");
    // A forkable world's RAM is its own file, cloned into the checkpoint; any
    // other world's RAM is inside the stream, which a fork loads into a file
    // once (the per-fork memory check below proves the sharing).
    let files: &[&str] = if forkable {
        &["mem.ram", "state.mig"]
    } else {
        &["state.mig"]
    };
    for f in files.iter().copied() {
        if !ck_dir.join(f).is_file() {
            return Err(format!("the checkpoint has no {f}"));
        }
    }
    let state_mb = std::fs::metadata(ck_dir.join("state.mig"))
        .map(|m| m.len() as f64 / 1048576.0)
        .unwrap_or(0.0);
    eprintln!("checkpoint in {ck_ms:.0} ms; device state {state_mb:.1} MB");
    report["checkpoint_ms"] = json!(ck_ms);
    report["state_mb"] = json!(state_mb);
    // The world keeps running after its checkpoint.
    sh(&world, &tw, "true").await?;

    // A pinned checkpoint is kept.
    match rt.delete_checkpoint("ff-ck").await {
        Err(e) if e.to_string().contains("pinned") => {}
        other => return Err(format!("a pinned checkpoint was not kept: {other:?}")),
    }

    // 3. Waves of forks.
    let mut prev: Vec<String> = vec![];
    for (w, &n) in waves.iter().enumerate() {
        for name in prev.drain(..) {
            rt.delete(&name)
                .await
                .map_err(|e| format!("delete {name}: {e}"))?;
        }
        let names: Vec<String> = (0..n).map(|i| format!("ff-{w}-{i}")).collect();
        made.extend(names.iter().cloned());
        let tokens: Vec<String> = names.iter().map(|_| token()).collect();
        let before = settled_host_mem().await;
        let wave_t = Instant::now();
        let forks = futures_util::future::join_all(names.iter().zip(&tokens).map(|(name, tk)| {
            let mut s = spec(name, ImageSource::Existing, tk, memory_mb);
            s.image = ImageSource::Existing;
            async move {
                let t = Instant::now();
                rt.fork("ff-ck", name)
                    .await
                    .map_err(|e| format!("fork {name}: {e}"))?;
                let inst = rt
                    .start(&s)
                    .await
                    .map_err(|e| format!("start {name}: {e}"))?;
                Ok::<_, String>((inst, t.elapsed()))
            }
        }))
        .await;
        let wave_s = wave_t.elapsed().as_secs_f64();
        let mut insts = vec![];
        let mut lat = vec![];
        for f in forks {
            let (i, d) = f?;
            lat.push(d.as_secs_f64() * 1000.0);
            insts.push(i);
        }
        let pids: Vec<u32> = names
            .iter()
            .filter_map(|n| rt.load(n).ok().and_then(|s| s.pid))
            .collect();
        let idle_host = settled_host_mem().await;
        let idle = mem_of(&pids);
        let mut idle_private: Vec<f64> = idle
            .values()
            .map(|m| m.private as f64 / 1048576.0)
            .collect();
        let idle_shared: f64 = idle
            .values()
            .map(|m| m.shared as f64 / 1048576.0)
            .sum::<f64>()
            / idle.len().max(1) as f64;
        eprintln!(
            "wave {w}: {n} forks up in {wave_s:.2}s; idle private p50 {:.0} MB, shared avg {idle_shared:.0} MB",
            pct(&mut idle_private, 0.5)
        );
        // Each fork: the world's state, then a write of its own.
        let checks = futures_util::future::join_all(insts.iter().zip(&tokens).map(|(i, tk)| {
            let world_hash = world_hash.clone();
            async move {
                let got = sh(
                    i,
                    tk,
                    "sha256sum /dev/shm/world | cut -d' ' -f1; cat /var/tmp/world-marker",
                )
                .await?;
                if got != format!("{world_hash}\nworld-disk") {
                    return Err(format!("{} lost the world's state: {got:?}", i.name));
                }
                sh(
                    i,
                    tk,
                    &format!(
                        "echo {n} >/dev/shm/me && echo {n} >/var/tmp/me && \
                         dd if=/dev/urandom of=/dev/shm/world bs=1M count=8 conv=notrunc 2>/dev/null",
                        n = i.name
                    ),
                )
                .await
            }
        }))
        .await;
        for c in checks {
            c?;
        }
        // Read back: every fork sees only its own writes.
        for (i, tk) in insts.iter().zip(&tokens) {
            let got = sh(i, tk, "cat /dev/shm/me /var/tmp/me").await?;
            if got != format!("{0}\n{0}", i.name) {
                return Err(format!("{} is not independent: read {got:?}", i.name));
            }
        }
        let world_now = sh(
            &world,
            &tw,
            "sha256sum /dev/shm/world | cut -d' ' -f1; cat /dev/shm/me 2>/dev/null || echo none",
        )
        .await?;
        if world_now != format!("{world_hash}\nnone") {
            return Err(format!("a fork's write reached the world: {world_now:?}"));
        }
        // Memory per fork, after reading the whole world and writing.
        let after = settled_host_mem().await;
        let mem = mem_of(&pids);
        let mb = |b: i64| b as f64 / 1048576.0;
        let per_fork = mb(after.private - before.private) / n as f64;
        let mut private: Vec<f64> = mem.values().map(|m| m.private as f64 / 1048576.0).collect();
        let shared: Vec<f64> = mem.values().map(|m| m.shared as f64 / 1048576.0).collect();
        let total_private: f64 = private.iter().sum();
        let shared_avg = shared.iter().sum::<f64>() / shared.len().max(1) as f64;
        let mut l2 = lat.clone();
        let wave = json!({
            "forks": n,
            "wall_s": wave_s,
            "fork_ms_p50": pct(&mut l2.clone(), 0.5),
            "fork_ms_p95": pct(&mut l2.clone(), 0.95),
            "fork_ms_max": pct(&mut l2, 1.0),
            // Host-wide: what the wave added, per fork (private) and once
            // (the shared file cache).
            "private_mb_per_fork": per_fork,
            "idle_private_mb_per_fork": mb(idle_host.private - before.private) / n as f64,
            "shared_file_mb": mb(after.file - before.file),
            // Per QEMU process (macOS `footprint` does not see guest pages
            // under HVF; Linux smaps does).
            "process_private_mb_p50": pct(&mut private.clone(), 0.5),
            "process_private_mb_max": pct(&mut private, 1.0),
            "process_private_mb_total": total_private,
            "process_shared_mb_avg": shared_avg,
            "process_idle_private_mb_p50": pct(&mut idle_private, 0.5),
            "process_idle_shared_mb_avg": idle_shared,
            "measured": mem.len(),
        });
        eprintln!("wave {w}: {wave}");
        // Keep the wave up for a look from outside (vmmap, footprint).
        let hold: u64 = env_or("CUA_FORK_HOLD_S", 0);
        if hold > 0 {
            eprintln!("holding wave {w} for {hold}s: pids {pids:?}");
            tokio::time::sleep(Duration::from_secs(hold)).await;
        }
        if mem.len() != n {
            return Err(format!("measured {} of {n} forks' memory", mem.len()));
        }
        // Sharing: a fork must not hold a copy of the world's dirtied RAM
        // (it read all of it and wrote 8 MB of it).
        if per_fork > dirty_mb as f64 {
            return Err(format!(
                "forks do not share the world's memory: {per_fork:.0} MB private per fork > \
                 the world's {dirty_mb} MB"
            ));
        }
        report["waves"].as_array_mut().unwrap().push(wave);
        prev = names;
    }
    for name in prev {
        rt.delete(&name)
            .await
            .map_err(|e| format!("delete {name}: {e}"))?;
    }

    // 4. Unpinned, the checkpoint deletes; the world still runs.
    rt.pin_checkpoint("ff-ck", false)
        .await
        .map_err(|e| format!("unpin: {e}"))?;
    rt.delete_checkpoint("ff-ck")
        .await
        .map_err(|e| format!("delete checkpoint: {e}"))?;
    sh(&world, &tw, "true").await?;
    Ok(())
}
