//! microVM (Firecracker) backend e2e (CUA_E2E_FIRECRACKER=1, Linux/KVM).
//!
//! Boots the Spaces image `ghcr.io/trycua/linux:24.04-slim` as a microVM,
//! waits for cua-spacesd, checks it accepts the Space's token, runs commands
//! through the guest init, then checkpoints, forks (from the checkpoint and
//! from the running VM), restores a second Space from the golden snapshot,
//! and deletes everything.
//!
//! Needs `firecracker`, `pasta`, `sqfstar`, `mkfs.ext4`, `iptables` and the
//! static guest init (`CUA_MICROVM_INIT`, or `cua-microvm-init` in
//! `~/.cua/bin`). `CUA_E2E_MICROVM_IMAGE` overrides the image.

use std::time::{Duration, Instant};

use cua_e2e_local_runtimes::{Timer, gated, init_tracing, run_id};
use cua_vmm::microvm::{MicroVmConfig, MicroVmRuntime};
use cua_vmm::{ExecRequest, ImageSource, Probe, Runtime, StartSpec, Status};

const IMAGE: &str = "ghcr.io/trycua/linux:24.04-slim";

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

fn spec(name: &str, token: &str) -> StartSpec {
    let image = std::env::var("CUA_E2E_MICROVM_IMAGE").unwrap_or_else(|_| IMAGE.into());
    StartSpec::new(name, ImageSource::oci(image))
        .cpus(2)
        .memory_mb(2048)
        .env("CUA_ENV_TOKEN", token)
        .probe(Probe::tcp(3211))
        .ready_timeout(Duration::from_secs(180))
}

/// cua-spacesd at `port` answers an authenticated call with `token`
/// (retrying while a freshly written token file is being picked up).
async fn spacesd_accepts(port: u16, token: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
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
            Ok(_) => return,
            Err(e) if Instant::now() < deadline => {
                eprintln!("spacesd not ready yet: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("cua-spacesd on {port} never accepted the token: {e}"),
        }
    }
}

/// Serves a 1 MiB HTTP response on this host's own network address (not
/// loopback, which guests cannot reach): `(ip, port)`.
async fn serve_one_mib() -> (std::net::IpAddr, u16) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    probe.connect("8.8.8.8:53").unwrap();
    let ip = probe.local_addr().unwrap().ip();
    let listener = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let body = vec![b'x'; 1 << 20];
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
            });
        }
    });
    (ip, port)
}

/// Kills and removes the test's VMs when the test fails midway (a passing
/// run has deleted them already).
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

async fn sh(rt: &MicroVmRuntime, name: &str, script: &str) -> String {
    let exec = rt.guest_exec(name).await.unwrap().expect("exec");
    exec.exec(ExecRequest::sh(script))
        .await
        .unwrap()
        .check(script)
        .unwrap()
        .stdout_str()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn microvm_space_lifecycle_checkpoint_fork_and_warm_start() {
    if !gated("CUA_E2E_FIRECRACKER") {
        return;
    }
    init_tracing();
    let rt = runtime();
    let id = run_id();
    let (a, b, c, d) = (
        format!("e2e-mvm-a-{id}"),
        format!("e2e-mvm-b-{id}"),
        format!("e2e-mvm-c-{id}"),
        format!("e2e-mvm-d-{id}"),
    );
    let ckpt = format!("e2e-mvm-ckpt-{id}");
    let _cleanup = Cleanup {
        root: rt.config().root.clone(),
        names: vec![a.clone(), b.clone(), c.clone(), d.clone()],
        checkpoint: ckpt.clone(),
    };

    // 1. Create (cold boot, or a warm restore when a golden snapshot of this
    //    configuration exists from an earlier run).
    let t = Timer::start();
    let inst = rt
        .start(&spec(&a, "token-a-0123456789abcdef"))
        .await
        .expect("start a");
    t.lap("microVM a: start to cua-spacesd listening");
    assert_eq!(inst.status, Status::Running);
    let port = inst.endpoints.host_port(3211).expect("3211 published");
    spacesd_accepts(port, "token-a-0123456789abcdef").await;
    t.lap("microVM a: cua-spacesd accepts its token");

    // 2. Commands through the guest init.
    let uname = sh(&rt, &a, "uname -r; hostname; cat /run/cua/env-token").await;
    eprintln!("guest: {uname}");
    assert!(uname.contains("6.1"), "a Firecracker guest kernel: {uname}");
    assert!(uname.contains(&a), "hostname is the Space name: {uname}");
    assert!(uname.contains("token-a-0123456789abcdef"));
    let exec = rt.guest_exec(&a).await.unwrap().unwrap();
    let r = exec
        .exec(ExecRequest::sh("echo out; echo err >&2; exit 3"))
        .await
        .unwrap();
    assert_eq!(
        (r.exit_code, r.stdout_str(), r.stderr_str()),
        (3, "out\n".into(), "err\n".into())
    );
    exec.put_file("/root/marker.txt", b"before-checkpoint", 0o644)
        .await
        .unwrap();
    // Egress and DNS through pasta: real downloads, bigger than one TCP
    // segment, from the internet and from this host's own network address
    // (a VPC/LAN service).
    let (host_ip, host_port) = serve_one_mib().await;
    let net = exec
        .exec(ExecRequest::sh(format!(
            "getent hosts github.com >/dev/null && echo dns-ok; \
             curl -s -o /dev/null -m 20 -w 'ext=%{{http_code}}\\n' https://example.com; \
             curl -s -o /dev/null -m 20 -w 'lan=%{{http_code}}:%{{size_download}}\\n' \
             http://{host_ip}:{host_port}/"
        )))
        .await
        .unwrap();
    eprintln!("egress: {}", net.stdout_str());
    assert!(net.stdout_str().contains("dns-ok"), "{net:?}");
    assert!(net.stdout_str().contains("ext=200"), "{net:?}");
    assert!(net.stdout_str().contains("lan=200:1048576"), "{net:?}");

    // 3. Checkpoint (memory + disk) while running.
    let t = Timer::start();
    rt.checkpoint(&a, &ckpt).await.expect("checkpoint");
    t.lap("checkpoint a (pause, snapshot, disk copy, resume)");
    assert_eq!(rt.status(&a).await.unwrap(), Status::Running);
    spacesd_accepts(port, "token-a-0123456789abcdef").await;

    // 4. Fork from the checkpoint: a new VM with its own identity.
    let t = Timer::start();
    rt.fork(&ckpt, &b).await.expect("fork from checkpoint");
    let mut sb =
        StartSpec::new(&b, ImageSource::Existing).env("CUA_ENV_TOKEN", "token-b-0123456789abcdef");
    sb.ready_timeout = Duration::from_secs(60);
    let ib = rt.start(&sb).await.expect("start fork b");
    t.lap("fork b from checkpoint: restore to cua-spacesd listening");
    let pb = ib.endpoints.host_port(3211).expect("b publishes 3211");
    assert_ne!(pb, port);
    spacesd_accepts(pb, "token-b-0123456789abcdef").await;
    t.lap("fork b: accepts its own token");
    let got = sh(
        &rt,
        &b,
        "cat /root/marker.txt; echo; hostname; cat /run/cua/env-token",
    )
    .await;
    assert!(got.contains("before-checkpoint"), "{got}");
    assert!(
        got.contains(&b) && got.contains("token-b-0123456789abcdef"),
        "{got}"
    );

    // 5. Fork the running VM directly.
    let t = Timer::start();
    rt.fork(&a, &c).await.expect("fork running a");
    t.lap("fork c from running a: snapshot");
    let mut sc =
        StartSpec::new(&c, ImageSource::Existing).env("CUA_ENV_TOKEN", "token-c-0123456789abcdef");
    sc.ready_timeout = Duration::from_secs(60);
    let ic = rt.start(&sc).await.expect("start fork c");
    t.lap("fork c: restore");
    spacesd_accepts(
        ic.endpoints.host_port(3211).unwrap(),
        "token-c-0123456789abcdef",
    )
    .await;
    assert!(
        sh(&rt, &c, "cat /root/marker.txt")
            .await
            .contains("before-checkpoint")
    );

    // 6. A second Space of the same configuration starts warm (golden
    //    snapshot recorded by the first cold boot).
    let t = Timer::start();
    let id_ = rt
        .start(&spec(&d, "token-d-0123456789abcdef"))
        .await
        .expect("start d");
    let warm = t.lap("Space d (same config): start to cua-spacesd listening");
    spacesd_accepts(
        id_.endpoints.host_port(3211).unwrap(),
        "token-d-0123456789abcdef",
    )
    .await;
    t.lap("Space d: accepts its token");
    let state = rt.load(&d).unwrap();
    eprintln!("d started {:?} in {warm:.2}s", state.last_start);
    if rt.config().warm {
        assert_eq!(state.last_start.as_deref(), Some("warm"));
        assert!(
            !sh(&rt, &d, "ls /root").await.contains("marker"),
            "a warm start is a fresh Space, not a copy of a"
        );
    }

    // 7. Suspend / resume, stop / start, delete.
    rt.suspend(&d).await.unwrap();
    assert_eq!(rt.status(&d).await.unwrap(), Status::Paused);
    rt.resume(&d).await.unwrap();
    assert_eq!(rt.status(&d).await.unwrap(), Status::Running);
    let t = Timer::start();
    rt.stop(&b).await.unwrap();
    t.lap("stop b (guest power-off)");
    assert_eq!(rt.status(&b).await.unwrap(), Status::Stopped);
    let ib2 = rt
        .start(&sb)
        .await
        .expect("start b again (cold boot of its disk)");
    t.lap("b started again");
    spacesd_accepts(
        ib2.endpoints.host_port(3211).unwrap(),
        "token-b-0123456789abcdef",
    )
    .await;
    assert!(
        sh(&rt, &b, "cat /root/marker.txt")
            .await
            .contains("before-checkpoint")
    );

    let mut pids = Vec::new();
    for n in [&a, &b, &c, &d] {
        let st = rt.load(n).unwrap();
        pids.extend(st.fc_pid.into_iter().chain(st.pasta_pid));
        rt.delete(n).await.unwrap();
        assert!(matches!(
            rt.status(n).await,
            Err(cua_vmm::VmmError::NotFound(_))
        ));
    }
    rt.delete_checkpoint(&ckpt).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    for p in pids {
        assert!(!cua_vmm::host::pid_alive(p), "process {p} leaked");
    }
}

/// The product path: `Sandboxes` (what the daemon, the CLI and the SDKs
/// use) creates a microVM Space with `kind = microvm`, checkpoints it and
/// forks a second Space from the checkpoint, which answers with its own
/// token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn microvm_space_through_the_sandboxes_api() {
    if !gated("CUA_E2E_FIRECRACKER") {
        return;
    }
    init_tracing();
    use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes, placement::Kind};
    let local = std::sync::Arc::new(cua_daemon::local::VmmLocal::default());
    let state = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder()
        .local(local.clone())
        .state_dir(state.path())
        .build();
    let id = run_id();
    let (name, fork, ckpt) = (
        format!("e2e-mvm-api-{id}"),
        format!("e2e-mvm-apifork-{id}"),
        format!("e2e-mvm-apickpt-{id}"),
    );
    let _cleanup = Cleanup {
        root: local.microvm().config().root.clone(),
        names: vec![name.clone(), fork.clone()],
        checkpoint: ckpt.clone(),
    };
    let mut o = CreateOptions::new(
        ProviderKind::Local,
        std::env::var("CUA_E2E_MICROVM_IMAGE").unwrap_or_else(|_| IMAGE.into()),
    )
    .name(&name);
    o.kind = Kind::MicroVm;
    o.cpus = 2;
    o.memory_mb = 2048;
    let t = Timer::start();
    let sb = sbx.create(o).await.expect("create a microVM Space");
    t.lap("Sandboxes::create (kind microvm) to cua-spacesd answering");
    let info = sbx.get(&name).await.unwrap();
    assert_eq!(info.runtime_type, "firecracker");
    let out = sb
        .spacesd()
        .await
        .unwrap()
        .run("echo hello-from-$(hostname)")
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(&format!("hello-from-{name}")),
        "{out:?}"
    );

    let t = Timer::start();
    let rec = sbx.checkpoint(&name, &ckpt).await.expect("checkpoint");
    t.lap("Sandboxes::checkpoint");
    assert_eq!(rec.runtime_type, "firecracker");
    assert_eq!(sbx.checkpoints().len(), 1);

    let t = Timer::start();
    let f = sbx
        .fork(&ckpt, &fork)
        .await
        .expect("fork from the checkpoint");
    t.lap("Sandboxes::fork from the checkpoint to cua-spacesd answering");
    assert_ne!(
        f.env_token(),
        sb.env_token(),
        "a microVM fork has its own token"
    );
    let out = f.spacesd().await.unwrap().run("hostname").await.unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(&fork),
        "{out:?}"
    );

    sbx.delete(&fork).await.unwrap();
    sbx.delete(&name).await.unwrap();
    sbx.delete_checkpoint(&ckpt).await.unwrap();
    assert!(sbx.checkpoints().is_empty());
}
