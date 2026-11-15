//! Opt-in: memory checkpoints, forks and warm starts of gVisor Spaces on an
//! engine with cua's runsc wrapper (`cua_vmm::container::snapshot`), and the
//! identity of every clone.
//!
//! Needs a Docker engine with `cua-runsc` registered (the built-in Linux
//! runtime registers it; elsewhere add
//! `"cua-runsc": {"path": "/usr/local/lib/cua/cua-runsc"}` to daemon.json's
//! `runtimes`), whose kernel exposes no arm64 pointer authentication, and a
//! Spaces image with cua-spacesd (default `ghcr.io/trycua/linux:24.04-slim`,
//! or `CUA_TEST_IMAGE`). Use a throwaway `CUA_HOME`:
//!
//! ```text
//! CUA_HOME=$(mktemp -d) CUA_TEST_ENGINE=unix:///path/docker.sock \
//!   cargo test -p cua-vmm --test gvisor_snapshot_live -- --ignored --nocapture
//! ```
#![cfg(feature = "container")]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use cua_vmm::container::snapshot::{self, SPACESD_PORT};
use cua_vmm::container::{ContainerConfig, ContainerRuntime};
use cua_vmm::exec::ExecRequest;
use cua_vmm::{ImageSource, Runtime, StartSpec};

fn token() -> String {
    snapshot::random_hex(16)
}

fn spec(name: &str, image: ImageSource, token: &str) -> StartSpec {
    let mut s = StartSpec::new(name, image)
        .cpus(2)
        .memory_mb(2048)
        .port(SPACESD_PORT)
        .ready_timeout(Duration::from_secs(180));
    s.env = BTreeMap::from([("CUA_ENV_TOKEN".to_string(), token.to_string())]);
    s.container_runtime = Some("gvisor".into());
    s
}

async fn sh(rt: &ContainerRuntime, name: &str, script: &str) -> String {
    let ex = rt.guest_exec(name).await.unwrap().unwrap();
    let o = ex
        .exec(ExecRequest::sh(script).timeout(Duration::from_secs(20)))
        .await
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// Whether `name`'s cua-spacesd accepts `token`, waiting up to `wait`.
async fn accepts(rt: &ContainerRuntime, name: &str, token: &str, wait: Duration) -> bool {
    let port = rt
        .endpoints(name)
        .await
        .unwrap()
        .host_port(SPACESD_PORT)
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + wait;
    loop {
        if let Ok(c) =
            cua_spacesd_client::SpacesdClient::connect_url(&url, Some(token.to_string())).await
            && c.metrics().await.is_ok()
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

struct Facts {
    machine_id: String,
    random: String,
    clock_skew_s: f64,
    ip: String,
}

async fn facts(rt: &ContainerRuntime, name: &str) -> Facts {
    let out = sh(
        rt,
        name,
        "cat /etc/machine-id; head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \\n'; echo; date +%s.%N; \
         ip -4 -o addr show eth0 | awk '{print $4}'",
    )
    .await;
    let host = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let l: Vec<&str> = out.lines().collect();
    Facts {
        machine_id: l[0].to_string(),
        random: l[1].to_string(),
        clock_skew_s: (l[2].parse::<f64>().unwrap() - host).abs(),
        ip: l.get(3).unwrap_or(&"").to_string(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker engine with cua-runsc (boots gVisor desktops)"]
async fn checkpoint_fork_and_warm_start_give_every_clone_its_own_identity() {
    let home = std::env::var("CUA_HOME").expect("set CUA_HOME to a temporary directory");
    assert!(!home.ends_with("/.cua"), "never the real cua home");
    let engine = std::env::var("CUA_TEST_ENGINE").expect("CUA_TEST_ENGINE=<docker uri>");
    let image = std::env::var("CUA_TEST_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    let rt = ContainerRuntime::connect(ContainerConfig {
        endpoint: Some(engine),
        ..Default::default()
    })
    .await
    .unwrap();
    let sup = rt.support().await.clone();
    assert!(sup.ok(), "memory checkpoints unsupported: {:?}", sup.memory);
    let names = ["gvs-src", "gvs-c1", "gvs-c2", "gvs-c3", "gvs-w1", "gvs-w2"];
    for n in names {
        let _ = rt.delete(n).await;
    }
    let _ = rt.delete_checkpoint("gvs-ck").await;
    let oci = ImageSource::Oci {
        reference: image.clone(),
    };

    // Source: cold.
    let t_src = token();
    rt.start(&spec("gvs-src", oci.clone(), &t_src))
        .await
        .unwrap();
    assert!(accepts(&rt, "gvs-src", &t_src, Duration::from_secs(120)).await);
    sh(&rt, "gvs-src", "echo from-source > /tmp/marker").await;

    // Checkpoint with memory.
    let t = Instant::now();
    rt.checkpoint("gvs-src", "gvs-ck").await.unwrap();
    eprintln!("checkpoint: {} ms", t.elapsed().as_millis());
    assert!(snapshot::read_record("checkpoints", "gvs-ck").is_some());

    // Two forks of the checkpoint, one of the running source.
    let mut clones = Vec::new();
    for (name, src) in [
        ("gvs-c1", "gvs-ck"),
        ("gvs-c2", "gvs-ck"),
        ("gvs-c3", "gvs-src"),
    ] {
        let tok = token();
        let t = Instant::now();
        rt.fork(src, name).await.unwrap();
        rt.start(&spec(name, ImageSource::Existing, &tok))
            .await
            .unwrap();
        eprintln!("fork {src} -> {name}: {} ms", t.elapsed().as_millis());
        assert!(
            accepts(&rt, name, &tok, Duration::from_secs(10)).await,
            "{name} takes its token"
        );
        assert!(
            !accepts(&rt, name, &t_src, Duration::ZERO).await,
            "{name} refuses its source's token"
        );
        // Memory came along: the source's file in its gVisor overlay.
        assert_eq!(sh(&rt, name, "cat /tmp/marker").await, "from-source");
        clones.push((name, tok));
    }
    let src = facts(&rt, "gvs-src").await;
    let mut seen = vec![src.machine_id.clone()];
    let mut randoms = vec![src.random.clone()];
    let mut ips = vec![src.ip.clone()];
    for (name, _) in &clones {
        let f = facts(&rt, name).await;
        eprintln!(
            "{name}: machine-id {} ip {} skew {:.3}s",
            f.machine_id, f.ip, f.clock_skew_s
        );
        assert!(
            !seen.contains(&f.machine_id),
            "{name}: machine-id is unique"
        );
        assert!(
            !randoms.contains(&f.random),
            "{name}: /dev/urandom is its own"
        );
        assert!(!ips.contains(&f.ip), "{name}: its own address");
        assert!(f.clock_skew_s < 1.0, "{name}: clock within 1 s of the host");
        seen.push(f.machine_id);
        randoms.push(f.random);
        ips.push(f.ip);
        // Its network works.
        assert!(
            !sh(&rt, name, "getent hosts example.com || true")
                .await
                .is_empty(),
            "{name}: DNS"
        );
    }

    // Warm start: the first start records the golden checkpoint in the
    // background; a later one restores it.
    let t_w1 = token();
    rt.start(&spec("gvs-w1", oci.clone(), &t_w1)).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(240);
    while snapshot::records("golden").is_empty() {
        assert!(
            Instant::now() < deadline,
            "no golden checkpoint was recorded"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let t_w2 = token();
    let t = Instant::now();
    rt.start(&spec("gvs-w2", oci.clone(), &t_w2)).await.unwrap();
    eprintln!("warm start: {} ms", t.elapsed().as_millis());
    let restored = rt
        .docker()
        .inspect_container("gvs-w2", None)
        .await
        .unwrap()
        .host_config
        .and_then(|h| h.annotations)
        .is_some_and(|a| a.contains_key(snapshot::ANNOTATION_RESTORE));
    assert!(restored, "the second start restored the golden checkpoint");
    assert!(accepts(&rt, "gvs-w2", &t_w2, Duration::from_secs(10)).await);
    let golden_token = snapshot::records("golden")[0].token().unwrap();
    assert!(!accepts(&rt, "gvs-w2", &golden_token, Duration::ZERO).await);
    let w2 = facts(&rt, "gvs-w2").await;
    assert!(!seen.contains(&w2.machine_id));
    assert!(w2.clock_skew_s < 1.0);

    for n in names {
        rt.delete(n).await.unwrap();
    }
    rt.delete_checkpoint("gvs-ck").await.unwrap();
    for g in snapshot::records("golden") {
        snapshot::remove_record("golden", &g.name);
        rt.helper().remove(&[&g.dir]).await.unwrap();
    }
}
