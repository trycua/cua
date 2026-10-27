//! Opt-in: QEMU memory snapshots end to end, with the daemon's identity
//! provider. Boots real VMs from a Linux Spaces disk:
//!
//! 1. two VMs of one configuration: the first records the golden snapshot,
//!    both restore it (`last_start` = `warm`);
//! 2. a memory checkpoint of the first and a fork of it, and a fork of the
//!    running first;
//! 3. a claim (rename) of the second, then a fork of its disk once stopped.
//!
//! Every pair of those guests must be distinct machines: their own token
//! (the snapshot's and each other's are refused), `/etc/machine-id`,
//! `/dev/urandom` output and SSH host key, and a clock within a second of
//! the host's. With `CUA_VMM_TEST_NEW_MACHINE=1` (a disk whose cua-spacesd
//! implements `Init.new_machine`) their machine-seal keys must differ too.
//!
//! ```text
//! CUA_VMM_TEST_QEMU=1 CUA_VMM_TEST_DISK=<linux containerDisk qcow2> \
//!   cargo test -p cua-daemon --test qemu_memory_identity -- --ignored --nocapture
//! ```
//!
//! Three 4 GiB VMs at most; state in a temp dir, deleted on every path.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use cua_daemon::guest_identity::SpacesdIdentity;
use cua_spacesd_client::SpacesdClient;
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ClaimIdentity, ImageSource, Instance, Probe, Runtime, StartSpec};

const SPACESD: u16 = 3211;

fn token() -> String {
    let mut b = [0u8; 16];
    rand::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn spec(name: &str, disk: &Path, token: &str) -> StartSpec {
    StartSpec::new(
        name,
        ImageSource::Disk {
            path: disk.to_path_buf(),
        },
    )
    .cpus(2)
    .memory_mb(4096)
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

/// What makes a guest a machine of its own.
#[derive(Debug)]
struct Facts {
    machine_id: String,
    random: String,
    ssh_host_key: String,
    hostname: String,
    clock_skew_s: f64,
    seal: Option<[u8; 32]>,
}

async fn facts(inst: &Instance, token: &str) -> Result<Facts, String> {
    let c = SpacesdClient::connect_url(&url(inst), Some(token.to_string()))
        .await
        .map_err(|e| format!("connect {}: {e}", inst.name))?;
    // SSH host keys are regenerated in the background after a restore.
    let mut out = String::new();
    for _ in 0..50 {
        let o = c
            .run(
                "cat /etc/machine-id; head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \\n'; echo; \
                 cat /etc/ssh/ssh_host_ed25519_key.pub 2>/dev/null | cut -d' ' -f2; hostname; date +%s.%N",
            )
            .await
            .map_err(|e| format!("run on {}: {e}", inst.name))?;
        out = o.stdout_str();
        if out.lines().count() >= 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let host_now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let l: Vec<&str> = out.lines().collect();
    if l.len() < 5 {
        return Err(format!("{}: unexpected output {out:?}", inst.name));
    }
    let guest_now: f64 = l[4].trim().parse().map_err(|e| format!("{e}: {}", l[4]))?;
    Ok(Facts {
        machine_id: l[0].trim().into(),
        random: l[1].trim().into(),
        ssh_host_key: l[2].trim().into(),
        hostname: l[3].trim().into(),
        clock_skew_s: guest_now - host_now,
        seal: c.machine_seal_public_key().await.ok().flatten(),
    })
}

/// A token the guest must refuse.
async fn refused(inst: &Instance, token: &str) -> Result<(), String> {
    match SpacesdClient::connect_url(&url(inst), Some(token.to_string())).await {
        Err(_) => Ok(()),
        Ok(c) => match c.run("true").await {
            Err(_) => Ok(()),
            Ok(_) => Err(format!("{} accepted a token it must refuse", inst.name)),
        },
    }
}

fn distinct(all: &[(&str, &Facts)]) -> Result<(), String> {
    for (what, get) in [
        (
            "machine-id",
            (|f: &Facts| f.machine_id.clone()) as fn(&Facts) -> String,
        ),
        ("urandom", |f| f.random.clone()),
        ("ssh host key", |f| f.ssh_host_key.clone()),
    ] {
        let set: BTreeSet<String> = all.iter().map(|(_, f)| get(f)).collect();
        if set.len() != all.len() {
            return Err(format!("{what} shared: {all:?}"));
        }
    }
    for (n, f) in all {
        if f.clock_skew_s.abs() > 1.0 {
            return Err(format!("{n}: clock off by {:.3}s", f.clock_skew_s));
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "boots QEMU VMs; set CUA_VMM_TEST_QEMU=1 and CUA_VMM_TEST_DISK, run with --ignored"]
async fn restored_and_forked_vms_are_distinct_machines() {
    if std::env::var("CUA_VMM_TEST_QEMU").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_VMM_TEST_QEMU=1");
        return;
    }
    let disk = PathBuf::from(std::env::var("CUA_VMM_TEST_DISK").expect("CUA_VMM_TEST_DISK"));
    let root = tempfile::tempdir().unwrap();
    let rt = QemuRuntime::new(QemuConfig {
        root: root.path().to_path_buf(),
        vnc: false,
        graceful_stop: Duration::from_secs(5),
        identity: Some(Arc::new(SpacesdIdentity)),
        warm: true,
        ..QemuConfig::default()
    });
    let names = ["mi-a", "mi-b", "mi-c", "mi-b2", "mi-d", "mi-e"];
    let result = async {
        let (ta, tb, tc, tb2) = (token(), token(), token(), token());
        let t = Instant::now();
        let a = rt
            .start(&spec("mi-a", &disk, &ta))
            .await
            .map_err(|e| format!("a: {e}"))?;
        eprintln!("a (records the golden snapshot): {:?}", t.elapsed());
        let t = Instant::now();
        let b = rt
            .start(&spec("mi-b", &disk, &tb))
            .await
            .map_err(|e| format!("b: {e}"))?;
        eprintln!("b (warm): {:?}", t.elapsed());
        for n in ["mi-a", "mi-b"] {
            let st = rt.load(n).map_err(|e| e.to_string())?;
            if st.last_start.as_deref() != Some("warm") {
                return Err(format!("{n} started {:?}, not warm", st.last_start));
            }
        }
        let golden = std::fs::read_dir(root.path().join("_golden"))
            .map_err(|e| e.to_string())?
            .flatten()
            .next()
            .ok_or("no golden snapshot")?
            .path();
        let secret =
            std::fs::read_to_string(golden.join("guest-token")).map_err(|e| e.to_string())?;

        let t = Instant::now();
        rt.checkpoint("mi-a", "mi-ck")
            .await
            .map_err(|e| format!("checkpoint: {e}"))?;
        eprintln!("checkpoint a: {:?}", t.elapsed());
        rt.fork("mi-ck", "mi-c")
            .await
            .map_err(|e| format!("fork: {e}"))?;
        let mut cs = spec("mi-c", &disk, &tc);
        cs.image = ImageSource::Existing;
        let t = Instant::now();
        let c = rt.start(&cs).await.map_err(|e| format!("c: {e}"))?;
        eprintln!("fork c (restore): {:?}", t.elapsed());

        // A fork of the running VM (its own one-time snapshot).
        let te = token();
        let t = Instant::now();
        rt.fork("mi-a", "mi-e")
            .await
            .map_err(|e| format!("fork running: {e}"))?;
        let mut es = spec("mi-e", &disk, &te);
        es.image = ImageSource::Existing;
        let e = rt.start(&es).await.map_err(|e| format!("e: {e}"))?;
        eprintln!(
            "fork e of running a (snapshot + restore): {:?}",
            t.elapsed()
        );
        let fe = facts(&e, &te).await?;
        rt.delete("mi-e").await.map_err(|e| e.to_string())?;

        let fa = facts(&a, &ta).await?;
        let fb = facts(&b, &tb).await?;
        let fc = facts(&c, &tc).await?;
        eprintln!("a: {fa:?}\nb: {fb:?}\nc: {fc:?}");
        distinct(&[("a", &fa), ("b", &fb), ("c", &fc), ("e", &fe)])?;
        if fb.hostname != "mi-b" {
            return Err(format!("b's hostname is {:?}", fb.hostname));
        }
        for (inst, bad) in [
            (&a, secret.trim()),
            (&b, secret.trim()),
            (&b, ta.as_str()),
            (&c, ta.as_str()),
            (&a, tc.as_str()),
        ] {
            refused(inst, bad).await?;
        }
        let seals: BTreeSet<_> = [fa.seal, fb.seal, fc.seal].into_iter().collect();
        eprintln!("machine-seal keys distinct: {}", seals.len() == 3);
        // Only a cua-spacesd that implements `Init.new_machine` regenerates
        // its seal key; set this for a disk that ships one.
        if std::env::var("CUA_VMM_TEST_NEW_MACHINE").as_deref() == Ok("1") && seals.len() != 3 {
            return Err(format!("machine-seal keys shared: {seals:?}"));
        }

        // Claim: a running VM is renamed and keeps serving.
        let t = Instant::now();
        let b2 = rt
            .claim(
                "mi-b",
                "mi-b2",
                &ClaimIdentity {
                    token: Some(tb2.clone()),
                    hostname: None,
                },
            )
            .await
            .map_err(|e| format!("claim: {e}"))?;
        eprintln!("claim: {:?}", t.elapsed());
        if rt.load("mi-b").is_ok() || b2.name != "mi-b2" {
            return Err("claim did not rename".into());
        }
        // The claimed VM answers the Space's token only.
        let f2 = facts(&b2, &tb2).await?;
        refused(&b2, &tb).await?;
        if f2.hostname != "mi-b2" {
            return Err(format!("claimed hostname is {:?}", f2.hostname));
        }

        // A fork of a stopped VM boots its disk with its own token.
        rt.stop("mi-b2").await.map_err(|e| format!("stop: {e}"))?;
        rt.fork("mi-b2", "mi-d")
            .await
            .map_err(|e| format!("disk fork: {e}"))?;
        let td = token();
        let mut ds = spec("mi-d", &disk, &td);
        ds.image = ImageSource::Existing;
        let d = rt.start(&ds).await.map_err(|e| format!("d: {e}"))?;
        facts(&d, &td).await?;
        refused(&d, &tb2).await?;
        Ok::<(), String>(())
    }
    .await;
    for n in names {
        let _ = rt.delete(n).await;
    }
    let _ = rt.delete("mi-ck").await;
    if let Err(e) = result {
        panic!("{e}");
    }
}
