//! macOS microVM (cua-vz, Virtualization.framework) e2e: checkpoint,
//! restore and fork (CUA_E2E_VZ=1, an Apple silicon Mac on macOS 14+).
//!
//! Boots the Spaces image `ghcr.io/trycua/linux:24.04-slim` as a microVM,
//! leaves state in its memory (a file in tmpfs, a running process) and on
//! its disk, checkpoints it, restores a Space from the checkpoint, then
//! forks it [`FORKS`] times at once and checks every fork is its own
//! machine: its memory and disk start as the checkpoint's and then diverge
//! on their own, and each has its own token, hostname, machine id,
//! entropy, clock and network. Prints timings and memory and, with
//! `CUA_E2E_VZ_REPORT=<file>`, writes them as JSON (the perf guard,
//! libs/cua/bench/microvm-fork, compares them with its baseline).
//!
//! Needs `cua-vz` (`CUA_VZ`, built by libs/cua/vz/build.sh) and the static
//! aarch64 guest init (`CUA_MICROVM_INIT`); the kernel and gvproxy
//! download on first use. `CUA_E2E_MICROVM_ROOT` keeps the state out of
//! `~/.cua`; `CUA_E2E_MICROVM_IMAGE` overrides the image.
//!
//! ```text
//! CUA_E2E_VZ=1 CUA_VZ=... CUA_MICROVM_INIT=... CUA_E2E_MICROVM_ROOT=/tmp/x \
//!   cargo test --test microvm_vz -- --nocapture --test-threads=1
//! ```

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use cua_e2e_local_runtimes::{Timer, gated, init_tracing, run_id};
use cua_vmm::microvm::{MicroVmConfig, MicroVmRuntime};
use cua_vmm::{ExecRequest, ImageSource, Probe, Runtime, StartSpec, Status};

const IMAGE: &str = "ghcr.io/trycua/linux:24.04-slim";
/// Forks of one checkpoint started at once.
const FORKS: usize = 8;
/// What the source leaves in its memory (tmpfs) before the checkpoint.
const SHM_MIB: u32 = 256;

fn runtime() -> MicroVmRuntime {
    let mut cfg = MicroVmConfig {
        resolver: Some(cua_image::RootfsLayerResolver::shared()),
        ..Default::default()
    };
    if let Ok(root) = std::env::var("CUA_E2E_MICROVM_ROOT") {
        cfg.root = root.into();
    }
    MicroVmRuntime::new(cfg)
}

fn token(n: &str) -> String {
    format!("tok-{n}-0123456789abcdef")
}

fn spec(name: &str) -> StartSpec {
    let image = std::env::var("CUA_E2E_MICROVM_IMAGE").unwrap_or_else(|_| IMAGE.into());
    StartSpec::new(name, ImageSource::oci(image))
        .cpus(2)
        .memory_mb(2048)
        .env("CUA_ENV_TOKEN", token(name))
        .probe(Probe::tcp(3211))
        .ready_timeout(Duration::from_secs(300))
}

fn existing(name: &str) -> StartSpec {
    let mut s = StartSpec::new(name, ImageSource::Existing).env("CUA_ENV_TOKEN", token(name));
    s.ready_timeout = Duration::from_secs(120);
    s
}

/// cua-spacesd at `port` answers an authenticated call with `token`.
async fn spacesd_accepts(port: u16, token: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let r = async {
            let c = cua_spacesd_client::SpacesdClient::connect_url(
                &format!("http://127.0.0.1:{port}"),
                Some(token.to_string()),
            )
            .await?;
            c.health().await?;
            c.metrics().await
        }
        .await;
        match r {
            Ok(_) => return true,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            Err(e) => {
                eprintln!("cua-spacesd on {port} refused {token}: {e}");
                return false;
            }
        }
    }
}

/// Whether cua-spacesd at `port` refuses `token` (a sibling's).
async fn spacesd_refuses(port: u16, token: &str) -> bool {
    let r = async {
        let c = cua_spacesd_client::SpacesdClient::connect_url(
            &format!("http://127.0.0.1:{port}"),
            Some(token.to_string()),
        )
        .await?;
        c.metrics().await
    }
    .await;
    r.is_err()
}

async fn sh(rt: &MicroVmRuntime, name: &str, script: &str) -> String {
    let exec = rt.guest_exec(name).await.unwrap().expect("exec");
    exec.exec(ExecRequest::sh(script).timeout(Duration::from_secs(120)))
        .await
        .unwrap()
        .check(script)
        .unwrap()
        .stdout_str()
}

/// Host memory in use (active + wired + held by the compressor), MiB.
fn host_used_mib() -> f64 {
    let out = std::process::Command::new("vm_stat").output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    let page: f64 = s
        .lines()
        .next()
        .and_then(|l| l.split("page size of ").nth(1))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(16384.0);
    let get = |k: &str| -> f64 {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
            .unwrap_or(0.0)
    };
    (get("Pages active") + get("Pages wired down") + get("Pages occupied by compressor")) * page
        / 1048576.0
}

/// The Virtualization.framework VM processes running now.
fn vm_processes() -> BTreeSet<u32> {
    let out = std::process::Command::new("pgrep")
        .args(["-f", "com.apple.Virtualization.VirtualMachine"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// `(physical footprint, resident)` of process `pid`, MiB (`footprint`
/// counts the guest's memory, compressed pages at full size; resident is
/// what is in RAM now).
fn vm_memory(pid: u32) -> (f64, f64) {
    let out = std::process::Command::new("vmmap")
        .args(["-summary", &pid.to_string()])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    let mib = |v: &str| -> f64 {
        let v = v.trim();
        let (n, unit) = v.split_at(v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len()));
        let n: f64 = n.trim().parse().unwrap_or(0.0);
        match unit {
            "K" => n / 1024.0,
            "M" => n,
            "G" => n * 1024.0,
            _ => n / 1048576.0,
        }
    };
    let footprint = s
        .lines()
        .find(|l| l.trim_start().starts_with("Physical footprint:"))
        .and_then(|l| l.split(':').nth(1))
        .map(mib)
        .unwrap_or(0.0);
    // "TOTAL  <virtual> <resident> <dirty> <swapped> ..."
    let resident = s
        .lines()
        .find(|l| l.starts_with("TOTAL") && !l.contains("minus"))
        .and_then(|l| l.split_whitespace().nth(2))
        .map(mib)
        .unwrap_or(0.0);
    (footprint, resident)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() { 0.0 } else { v[v.len() / 2] }
}

/// Kills and removes the test's VMs when it fails midway.
struct Cleanup {
    root: std::path::PathBuf,
    names: Vec<String>,
    checkpoint: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for n in &self.names {
            let dir = self.root.join("instances").join(n);
            if let Ok(raw) = std::fs::read(dir.join("state.json"))
                && let Ok(st) = serde_json::from_slice::<serde_json::Value>(&raw)
            {
                for k in ["fc_pid", "pasta_pid"] {
                    if let Some(p) = st[k].as_u64() {
                        cua_vmm::host::signal(p as u32, "KILL");
                    }
                }
            }
            let _ = std::fs::remove_dir_all(dir);
        }
        let _ = std::fs::remove_dir_all(self.root.join("checkpoints").join(&self.checkpoint));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn vz_microvm_checkpoint_restore_and_eight_independent_forks() {
    if !gated("CUA_E2E_VZ") {
        return;
    }
    init_tracing();
    let rt = std::sync::Arc::new(runtime());
    assert_eq!(rt.kind(), cua_vmm::BackendKind::Vz);
    let id = run_id();
    let src = format!("e2e-vz-src-{id}");
    let restored = format!("e2e-vz-rst-{id}");
    let forks: Vec<String> = (0..FORKS).map(|i| format!("e2e-vz-f{i}-{id}")).collect();
    let ckpt = format!("e2e-vz-ckpt-{id}");
    let mut all = vec![src.clone(), restored.clone()];
    all.extend(forks.iter().cloned());
    let _cleanup = Cleanup {
        root: rt.config().root.clone(),
        names: all.clone(),
        checkpoint: ckpt.clone(),
    };
    let mut report = serde_json::json!({"forks": FORKS, "shm_mib": SHM_MIB});

    // 1. The source Space (cold boot, or warm from an earlier run's golden
    //    snapshot).
    let t = Timer::start();
    let inst = rt.start(&spec(&src)).await.expect("start the source");
    report["source_start_s"] = t.lap("source: start to cua-spacesd listening").into();
    report["source_start_kind"] = rt.load(&src).unwrap().last_start.clone().into();
    assert_eq!(inst.status, Status::Running);
    assert_eq!(inst.backend, cua_vmm::BackendKind::Vz);
    let port = inst.endpoints.host_port(3211).expect("3211 published");
    assert!(spacesd_accepts(port, &token(&src)).await);
    let guest = sh(&rt, &src, "uname -r; hostname; cat /run/cua/env-token").await;
    assert!(guest.contains("6.1"), "the microVM kernel: {guest}");
    assert!(
        guest.contains(&src) && guest.contains(&token(&src)),
        "{guest}"
    );
    let egress = sh(
        &rt,
        &src,
        "getent hosts github.com >/dev/null && echo dns-ok; \
         curl -s -o /dev/null -m 20 -w 'ext=%{http_code}\\n' https://example.com",
    )
    .await;
    assert!(
        egress.contains("dns-ok") && egress.contains("ext=200"),
        "{egress}"
    );
    // The guest cannot reach the host's loopback through its gateway.
    let blocked = sh(
        &rt,
        &src,
        "curl -s -o /dev/null -m 3 -w '%{http_code}' http://172.16.0.6:3211/ || echo blocked",
    )
    .await;
    assert!(
        blocked.contains("blocked") || blocked.contains("000"),
        "{blocked}"
    );

    // 2. State in memory and on disk, then the checkpoint.
    let before = sh(
        &rt,
        &src,
        &format!(
            "echo on-disk > /root/disk-marker; sync; \
             dd if=/dev/urandom of=/dev/shm/mem bs=1M count={SHM_MIB} 2>/dev/null; \
             setsid sleep 100000 >/dev/null 2>&1 & echo $! > /dev/shm/sleeper; \
             md5sum /dev/shm/mem | cut -c1-32; cat /dev/shm/sleeper"
        ),
    )
    .await;
    let mut lines = before.lines();
    let (mem_md5, sleeper) = (
        lines.next().unwrap().to_string(),
        lines.next().unwrap().to_string(),
    );
    let t = Timer::start();
    rt.checkpoint(&src, &ckpt).await.expect("checkpoint");
    report["checkpoint_s"] = t
        .lap("checkpoint (pause, save memory, clone disk, resume)")
        .into();
    let ck_dir = rt.config().root.join("checkpoints").join(&ckpt);
    let state_mib = std::fs::metadata(ck_dir.join("vzstate")).unwrap().len() as f64 / 1048576.0;
    eprintln!("saved state: {state_mib:.0} MiB");
    report["state_mib"] = state_mib.into();
    assert_eq!(
        rt.status(&src).await.unwrap(),
        Status::Running,
        "the source runs on"
    );
    assert!(spacesd_accepts(port, &token(&src)).await);

    // 3. Restore: a Space from the checkpoint, its memory and disk intact.
    let t = Timer::start();
    rt.fork(&ckpt, &restored)
        .await
        .expect("fork from the checkpoint");
    let ir = rt.start(&existing(&restored)).await.expect("restore");
    report["restore_s"] = t
        .lap("restore: fork + start to cua-spacesd listening")
        .into();
    assert_eq!(
        rt.load(&restored).unwrap().last_start.as_deref(),
        Some("restore")
    );
    let rp = ir.endpoints.host_port(3211).unwrap();
    assert!(spacesd_accepts(rp, &token(&restored)).await);
    let got = sh(
        &rt,
        &restored,
        &format!(
            "cat /root/disk-marker; md5sum /dev/shm/mem | cut -c1-32; \
             kill -0 {sleeper} && echo sleeper-alive; hostname"
        ),
    )
    .await;
    assert!(got.contains("on-disk"), "{got}");
    assert!(got.contains(&mem_md5), "memory restored: {got}");
    assert!(got.contains("sleeper-alive"), "processes restored: {got}");
    assert!(got.contains(&restored), "own hostname: {got}");

    // 4. Eight forks of the checkpoint, started at once.
    let used_before = host_used_mib();
    let vms_before = vm_processes();
    let t_all = Instant::now();
    let mut tasks = Vec::new();
    for f in &forks {
        let (rt, f, ckpt) = (rt.clone(), f.clone(), ckpt.clone());
        tasks.push(tokio::spawn(async move {
            let t = Instant::now();
            rt.fork(&ckpt, &f).await.expect("fork");
            let forked = t.elapsed().as_secs_f64();
            let inst = rt.start(&existing(&f)).await.expect("start fork");
            let started = t.elapsed().as_secs_f64();
            let port = inst.endpoints.host_port(3211).unwrap();
            assert!(spacesd_accepts(port, &token(&f)).await);
            (f, port, forked, started, t.elapsed().as_secs_f64())
        }));
    }
    let mut started = Vec::new();
    for t in tasks {
        started.push(t.await.unwrap());
    }
    let wall = t_all.elapsed().as_secs_f64();
    let mut ready: Vec<f64> = started.iter().map(|s| s.4).collect();
    let mut fork_call: Vec<f64> = started.iter().map(|s| s.2).collect();
    let p50 = median(&mut ready);
    let max = ready.iter().cloned().fold(0.0, f64::max);
    eprintln!(
        "{FORKS} forks at once: wall {wall:.2}s, each fork+start+token p50 {p50:.2}s max {max:.2}s \
         (fork call p50 {:.3}s)",
        median(&mut fork_call)
    );
    report["forks_wall_s"] = wall.into();
    report["fork_ready_p50_s"] = p50.into();
    report["fork_ready_max_s"] = max.into();
    report["fork_call_p50_s"] = median(&mut fork_call).into();

    // Memory: every fork's own, and the host's.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let used_after = host_used_mib();
    let new_vms: Vec<u32> = vm_processes().difference(&vms_before).copied().collect();
    assert_eq!(new_vms.len(), FORKS, "one VM process per fork");
    let per: Vec<(f64, f64)> = new_vms.iter().map(|p| vm_memory(*p)).collect();
    let mut resident: Vec<f64> = per.iter().map(|m| m.1).collect();
    let mut footprint: Vec<f64> = per.iter().map(|m| m.0).collect();
    let guest_used = sh(&rt, &forks[0], "free -m | awk '/Mem:/ {print $3 + $5}'").await;
    eprintln!(
        "memory: host +{:.0} MiB for {FORKS} forks ({:.0} MiB each); per VM process resident \
         p50 {:.0} MiB, footprint p50 {:.0} MiB; guest uses {} MiB (incl. tmpfs)",
        used_after - used_before,
        (used_after - used_before) / FORKS as f64,
        median(&mut resident),
        median(&mut footprint),
        guest_used.trim()
    );
    report["host_used_delta_mib"] = (used_after - used_before).into();
    report["host_used_per_fork_mib"] = ((used_after - used_before) / FORKS as f64).into();
    report["vm_resident_p50_mib"] = median(&mut resident).into();
    report["vm_footprint_p50_mib"] = median(&mut footprint).into();
    report["guest_used_mib"] = guest_used.trim().parse::<f64>().unwrap_or(0.0).into();

    // 5. Every fork is its own machine.
    let mut ids = BTreeSet::new();
    let mut randoms = BTreeSet::new();
    let host_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    for (f, port, ..) in &started {
        let out = sh(
            &rt,
            f,
            &format!(
                "hostname; cat /etc/machine-id; head -c16 /dev/urandom | od -An -tx1 | tr -d ' \\n'; \
                 echo; date +%s.%N; cat /run/cua/env-token; cat /root/disk-marker; \
                 md5sum /dev/shm/mem | cut -c1-32; kill -0 {sleeper} && echo sleeper-alive; \
                 ls /root; curl -s -o /dev/null -m 20 -w 'ext=%{{http_code}}\\n' https://example.com"
            ),
        )
        .await;
        let l: Vec<&str> = out.lines().collect();
        assert_eq!(l[0], f, "hostname");
        assert!(ids.insert(l[1].to_string()), "machine id {} repeats", l[1]);
        assert!(randoms.insert(l[2].to_string()), "entropy repeats");
        let clock: f64 = l[3].parse().unwrap();
        assert!(
            (clock - host_now).abs() < 30.0,
            "clock {clock} vs host {host_now}"
        );
        assert_eq!(l[4], token(f), "its own token");
        assert_eq!(l[5], "on-disk");
        assert_eq!(l[6], mem_md5, "memory as checkpointed");
        assert!(out.contains("sleeper-alive"));
        assert!(!out.contains("fork-own"), "no sibling's file: {out}");
        assert!(out.contains("ext=200"), "its own network: {out}");
        // Its cua-spacesd takes only its own token.
        let sibling = started.iter().find(|s| &s.0 != f).unwrap();
        assert!(spacesd_refuses(*port, &token(&sibling.0)).await);
        // Diverge: a file on disk and new memory contents.
        sh(
            &rt,
            f,
            &format!("echo {f} > /root/fork-own-{f}; dd if=/dev/urandom of=/dev/shm/mem bs=1M count=8 conv=notrunc 2>/dev/null; sync"),
        )
        .await;
    }
    // Writes stayed in each fork.
    let mut sums = BTreeSet::new();
    for (f, ..) in &started {
        let out = sh(&rt, f, "ls /root; md5sum /dev/shm/mem | cut -c1-32").await;
        let own: Vec<&str> = out.lines().filter(|l| l.starts_with("fork-own-")).collect();
        assert_eq!(
            own,
            [format!("fork-own-{f}").as_str()],
            "only its own file: {out}"
        );
        assert!(sums.insert(out.lines().last().unwrap().to_string()));
    }
    let src_now = sh(&rt, &src, "ls /root; md5sum /dev/shm/mem | cut -c1-32").await;
    assert!(
        !src_now.contains("fork-own"),
        "the source never sees a fork's writes"
    );
    assert!(src_now.contains(&mem_md5));

    // 6. Suspend and resume a fork; stop one and start it again (a cold
    //    boot of its own disk).
    rt.suspend(&forks[0]).await.unwrap();
    assert_eq!(rt.status(&forks[0]).await.unwrap(), Status::Paused);
    rt.resume(&forks[0]).await.unwrap();
    assert_eq!(rt.status(&forks[0]).await.unwrap(), Status::Running);
    rt.stop(&forks[1]).await.unwrap();
    assert_eq!(rt.status(&forks[1]).await.unwrap(), Status::Stopped);
    let again = rt.start(&existing(&forks[1])).await.expect("start again");
    assert!(spacesd_accepts(again.endpoints.host_port(3211).unwrap(), &token(&forks[1])).await);
    assert!(
        sh(&rt, &forks[1], "ls /root")
            .await
            .contains(&format!("fork-own-{}", forks[1])),
        "its disk survives a stop"
    );

    // 7. Delete everything; no process is left behind.
    let mut pids = Vec::new();
    for n in &all {
        let st = rt.load(n).unwrap();
        pids.extend(st.fc_pid.into_iter().chain(st.pasta_pid));
        rt.delete(n).await.unwrap();
        assert!(matches!(
            rt.status(n).await,
            Err(cua_vmm::VmmError::NotFound(_))
        ));
    }
    rt.delete_checkpoint(&ckpt).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    for p in pids {
        assert!(!cua_vmm::host::pid_alive(p), "process {p} leaked");
    }
    eprintln!("REPORT {report}");
    if let Ok(path) = std::env::var("CUA_E2E_VZ_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
