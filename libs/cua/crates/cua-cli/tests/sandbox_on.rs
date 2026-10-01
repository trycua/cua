//! `cua sb` with `--on`: managed Fleet pools (fake Fleet API plus a mock
//! spacesd behind its gateway), `url:` direct sandboxes, the daemon that
//! keeps Fleet claims alive, and argument validation for the local
//! backends (nothing here starts a VM or container).

mod common;
use common::*;
use cua_daemon::fixtures;
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::{CreateOptions, ProviderKind};
use serde_json::Value;

/// A digest-pinned image: no registry lookups.
const IMAGE: &str = "registry.example.com/cua-e2e-cli@sha256:0123";

/// The managed pool the CLI will pick for `IMAGE` on gVisor with the
/// default shape plus `extra` services.
async fn managed_pool(extra: &[(&str, u16)]) -> String {
    cua_fleet::testing::install_image_fixtures();
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE);
    o.fleet.runtime = Some(cua_fleet::RuntimeKind::Gvisor);
    for (n, p) in extra {
        o.services.insert(n.to_string(), *p);
    }
    let tenant = cua_fleet::autopool::tenant_from_token(fixtures::FAKE_FLEET_TOKEN);
    cua_fleet::autopool::candidate_names(&tenant, &o.fleet_pool_key().await.unwrap().spec_hash())[0]
        .clone()
}

struct FleetWorld {
    fake: FakeFleet,
    _fleet: fixtures::FleetHttpFixture,
    _gw: fixtures::SpacesdFixture,
    pool: String,
    h: Home,
}

async fn fleet_world(claim: &str, extra: &[(&str, u16)]) -> FleetWorld {
    let pool = managed_pool(extra).await;
    let gw = fixtures::start_env(None, Some(fixtures::fake_gateway(&pool, claim))).await;
    let fake = FakeFleet::new();
    let fleet = fixtures::start_fleet_http_with_gateway(fake.clone(), Some(gw.url.clone())).await;
    let mut h = Home::new();
    h.set("CUA_FLEET_BASE_URL", &fleet.base_url)
        .set("FLEETS_TOKEN", fixtures::FAKE_FLEET_TOKEN)
        .set("CUA_FLEET_POOL_IDLE_GC", "off");
    FleetWorld {
        fake,
        _fleet: fleet,
        _gw: gw,
        pool,
        h,
    }
}

/// A loopback echo server; returns its port.
async fn echo_server() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

/// The `local_addr` a running `sb port-forward --json` prints (bounded).
async fn forward_addr(child: &mut tokio::process::Child) -> String {
    use tokio::io::AsyncBufReadExt;
    let stdout = child.stdout.take().unwrap();
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    for _ in 0..20 {
        let line = tokio::time::timeout(std::time::Duration::from_secs(30), lines.next_line())
            .await
            .expect("port-forward printed nothing")
            .unwrap()
            .expect("port-forward exited");
        if let Ok(v) = serde_json::from_str::<Value>(&line)
            && let Some(a) = v["local_addr"].as_str()
        {
            return a.to_string();
        }
    }
    panic!("no local_addr from port-forward");
}

/// Sends 4 KiB of `a + i` through `addr` and reads the echo (bounded).
async fn echo_through(addr: &str, i: u8) -> Vec<u8> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
    c.write_all(&[b'a' + i; 4096]).await.unwrap();
    let mut got = vec![0u8; 4096];
    tokio::time::timeout(std::time::Duration::from_secs(10), c.read_exact(&mut got))
        .await
        .expect("echo timed out")
        .unwrap();
    got
}

fn row<'a>(rows: &'a Value, name: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("{name} not in {rows}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fleet_managed_pool_full_lifecycle() {
    let w = fleet_world("box", &[("web", 8080)]).await;
    let h = &w.h;
    let e = |args: &[&str]| {
        let mut v = vec!["--embedded", "--json", "sb"];
        v.extend_from_slice(args);
        v.into_iter().map(str::to_string).collect::<Vec<_>>()
    };
    let run = |args: Vec<String>| async move {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        h.run(&a).await
    };

    // create --on cloud: no --pool, so shared capacity.
    let o = run(e(&[
        "create",
        IMAGE,
        "--on",
        "cloud",
        "--name",
        "box",
        "--kind",
        "container",
        "--port",
        "web=8080",
        "--max-pool-size",
        "4",
        "--claim-ttl",
        "10m",
    ]))
    .await;
    o.ok();
    let v = o.json();
    assert_eq!(
        (v["name"].as_str(), v["location"].as_str()),
        (Some("box"), Some("cloud"))
    );
    // How `auto` resolved is part of the output.
    assert_eq!(
        (v["kind"].as_str(), v["runtime"].as_str()),
        (Some("container"), Some("gvisor"))
    );
    assert!(o.stderr.contains("cua sb keep-alive cloud:box"), "{o:?}");
    assert_eq!(v["id"], "cloud:box");
    assert!(
        o.stderr
            .contains("Starting a cloud sandbox (first start of an image can take a few minutes)"),
        "{o:?}"
    );
    assert!(
        !o.stderr.contains("claim") && !o.stderr.contains("pool"),
        "{o:?}"
    );
    assert_eq!(v["location"], "cloud");
    assert!(v.get("where").is_none() && v.get("on").is_none(), "{v}");
    assert_eq!(v["status"], "ready");
    let p = w
        .fake
        .object("pool", &w.pool, &w.pool)
        .expect("managed pool");
    assert_eq!(p["spec"]["autoscaling"]["maxPoolSize"], 4);
    assert_eq!(p["spec"]["autoscaling"]["minPoolSize"], 0);
    let c = w.fake.object("claim", &w.pool, "box").unwrap();
    assert_eq!(c["spec"]["ttlSecondsAfterCreated"], 600);

    // ls --local leaves the cloud out.
    let o = run(e(&["ls", "--local"])).await;
    o.ok();
    let local = o.json();
    assert!(
        !local.as_array().unwrap().iter().any(|r| r["name"] == "box"),
        "{local}"
    );
    // ls (everything by default): pool and TTL remaining.
    let o = run(e(&["ls"])).await;
    o.ok();
    let rows = o.json();
    let r = row(&rows, "box");
    assert_eq!(r["location"], "cloud");
    assert_eq!(r["status"], "ready");
    // Fleet internals stay out of the portable fields.
    assert!(
        r.get("pool").is_none() && r.get("namespace").is_none(),
        "{r}"
    );
    assert_eq!(r["provider_details"]["pool"], w.pool.as_str());
    assert_eq!(r["provider_details"]["managed"], true);
    let left = r["expires_in_seconds"].as_u64().unwrap();
    assert!((500..=600).contains(&left), "{left}");
    let o = run(e(&["ls", "--cloud"])).await;
    row(&o.json(), "box");

    // exec / cp through the Fleet gateway.
    let o = h
        .run(&["--embedded", "sb", "exec", "box", "echo", "managed"])
        .await;
    o.ok();
    assert_eq!(o.stdout, "managed\n");
    // The driver answered uninitialized (bootstrap mode, as Fleet images
    // start): the CLI installed a token and keeps it owner-only.
    let state = h.cua_home().join("sandboxes").join("box.json");
    let st = read_json(&state);
    assert_eq!(st["env_token"].as_str().map(str::len), Some(32), "{st}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&state).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let local = h.dir.path().join("up.txt");
    std::fs::write(&local, b"hello fleet").unwrap();
    run(e(&["cp", local.to_str().unwrap(), "box:/tmp/up.txt"]))
        .await
        .ok();
    let back = h.dir.path().join("down.txt");
    let o = run(e(&["cp", "box:/tmp/up.txt", back.to_str().unwrap()])).await;
    o.ok();
    assert_eq!(o.json()["direction"], "download");
    assert_eq!(std::fs::read(&back).unwrap(), b"hello fleet");

    // keep-alive moves shutdownTime.
    let before =
        w.fake.object("claim", &w.pool, "box").unwrap()["spec"]["lifecycle"]["shutdownTime"]
            .clone();
    let o = run(e(&["keep-alive", "box", "--for", "2h"])).await;
    o.ok();
    let after =
        w.fake.object("claim", &w.pool, "box").unwrap()["spec"]["lifecycle"]["shutdownTime"]
            .clone();
    assert_ne!(before, after);

    // `sb view --service` (an image without cua-spacesd) and `sb url`:
    // signed service URLs; port-forward without the driver's tunnel: a
    // loopback proxy through the gateway.
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "view",
            "box",
            "--service",
            "web",
            "--no-open",
        ])
        .await;
    o.ok();
    assert!(o.stdout.contains("https://signed.fleet.test/"), "{o:?}");
    let o = run(e(&["url", "box", "web"])).await;
    o.ok();
    assert!(
        o.json()["url"]
            .as_str()
            .unwrap()
            .starts_with("https://signed.fleet.test/"),
        "{o:?}"
    );
    let o = run(e(&["url", "box", "web", "--public", "--ttl", "10m"])).await;
    o.ok();
    assert!(o.json()["expires_at"].is_string(), "{o:?}");

    // `sb mcp config`: the Fleet bearer prints masked unless --show-secrets.
    let o = run(e(&["mcp", "box", "web", "config"])).await;
    o.ok();
    let v = o.json();
    assert_eq!(v["headers"]["authorization"], "Bearer ****", "{o:?}");
    assert_eq!(v["headers"]["x-cua-fleet-claim"], "box", "{o:?}");
    assert!(!o.stdout.contains(fixtures::FAKE_FLEET_TOKEN), "{o:?}");
    assert!(v["url"].as_str().unwrap().ends_with("/mcp"), "{o:?}");
    let o = run(e(&["mcp", "box", "web", "config", "--show-secrets"])).await;
    o.ok();
    assert_eq!(
        o.json()["headers"]["authorization"],
        format!("Bearer {}", fixtures::FAKE_FLEET_TOKEN),
        "{o:?}"
    );
    let o = run(e(&["port-forward", "box", "8080", "--no-wait"])).await;
    o.ok();
    assert!(
        o.json()["url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:"),
        "{o:?}"
    );
    // With the driver's tunnel: a real loopback TCP forward over the
    // spacesd's /tunnel WebSocket, through the gateway.
    w._gw
        .mock
        .state
        .advertise(&[cua_spacesd_client::tunnel::TUNNEL_FORWARD_FEATURE]);
    let echo = echo_server().await;
    let mut child = h.spawn(&[
        "--embedded",
        "--json",
        "sb",
        "port-forward",
        "box",
        &echo.to_string(),
    ]);
    let local = forward_addr(&mut child).await;
    for i in 0..3u8 {
        assert_eq!(echo_through(&local, i).await, vec![b'a' + i; 4096]);
    }
    let _ = child.kill().await;
    let attaches = w._gw.mock.state.tunnel_attaches();
    assert_eq!(attaches.len(), 3, "{attaches:?}");
    for a in &attaches {
        assert!(a.accepted, "{a:?}");
        assert_eq!(a.claim.as_deref(), Some("box"));
        assert_eq!(
            a.authorization.as_deref(),
            Some(format!("Bearer {}", fixtures::FAKE_FLEET_TOKEN).as_str())
        );
    }

    // Fleet cannot suspend one sandbox and a shared pool is never resized:
    // suspend and restart fail with a typed error and leave the claim.
    let replicas = w.fake.object("pool", &w.pool, &w.pool).unwrap()["spec"]["replicas"].clone();
    for op in ["suspend", "restart"] {
        let o = h.run(&["--embedded", "sb", op, "box"]).await;
        assert_eq!(o.code, 4, "{o:?}");
        assert!(
            o.stderr.contains("cannot suspend a single sandbox"),
            "{o:?}"
        );
    }
    assert!(w.fake.exists("claim", &w.pool, "box"));
    // resume of a running claim reattaches.
    h.run(&["--embedded", "sb", "resume", "box"]).await.ok();
    let o = h
        .run(&["--embedded", "sb", "exec", "box", "echo", "back"])
        .await;
    o.ok();
    assert_eq!(o.stdout, "back\n");
    assert_eq!(
        w.fake.object("pool", &w.pool, &w.pool).unwrap()["spec"]["replicas"],
        replicas,
        "a shared pool is never resized"
    );

    // rm releases the claim; the pool stays for reuse, then GC removes it.
    h.run(&["--embedded", "sb", "rm", "box", "--force"])
        .await
        .ok();
    assert!(!w.fake.exists("claim", &w.pool, "box"));
    let o = h.run(&["--json", "fleet", "pools", "ls"]).await;
    o.ok();
    let pools = o.json();
    assert_eq!(pools[0]["name"], w.pool.as_str());
    assert_eq!(pools[0]["claims"], 0);
    let o = h.run(&["fleet", "pools", "ls"]).await;
    o.ok();
    assert!(
        o.stdout.contains(&w.pool) && o.stdout.contains("CLAIMS"),
        "{o:?}"
    );
    let o = h
        .run(&["--json", "fleet", "pools", "gc", "--idle", "0s"])
        .await;
    o.ok();
    assert_eq!(o.json()["deleted_pools"][0], w.pool.as_str());
    assert!(!w.fake.exists("pool", &w.pool, &w.pool));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_is_a_create_alias_on_fleet_with_generated_names() {
    let w = fleet_world("unused", &[]).await;
    let h = &w.h;
    // launch defaults to the cloud; no --name generates one. (This pinned
    // test image has no readable manifest, so the kind is given.)
    let o = h
        .run(&[
            "--embedded",
            "--json",
            "sb",
            "launch",
            IMAGE,
            "--kind",
            "container",
            "--cpu",
            "2",
            "--warm",
        ])
        .await;
    o.ok();
    let v = o.json();
    assert_eq!(v["status"], "ready");
    let name = v["name"].as_str().unwrap().to_string();
    assert!(name.starts_with("sb-"), "{name}");
    assert!(
        w.fake.exists("claim", &w.pool, &name),
        "{:?}",
        w.fake.all_namespaces()
    );
    let p = w.fake.object("pool", &w.pool, &w.pool).unwrap();
    assert_eq!(p["spec"]["replicas"], 1, "--warm");
    let t = w.fake.object("template", &w.pool, &w.pool).unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["runtime"], "gvisor");
    // A second launch of the same image reuses the pool.
    let o = h
        .run(&[
            "--embedded",
            "--json",
            "sb",
            "launch",
            IMAGE,
            "--kind",
            "container",
            "--name",
            "two",
        ])
        .await;
    o.ok();
    assert!(w.fake.exists("claim", &w.pool, "two"));
    assert_eq!(
        w.fake
            .all_namespaces()
            .iter()
            .filter(|n| n.starts_with("cua-auto-"))
            .count(),
        1
    );
    for n in [name.as_str(), "two"] {
        h.run(&["--embedded", "sb", "rm", n, "--force"]).await.ok();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_is_started_and_holds_fleet_claims() {
    let w = fleet_world("held", &[]).await;
    let mut h = w.h;
    struct Stop<'a>(&'a Home);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_cua"));
            c.envs(&self.0.env).args(["daemon", "stop"]);
            let _ = c.output();
        }
    }
    h.set("CUA_DAEMON_NO_RELAY", "1");
    let _stop = Stop(&h);
    let o = h
        .run(&[
            "--json",
            "sb",
            "create",
            IMAGE,
            "--on",
            "cloud",
            "--kind",
            "container",
            "--name",
            "held",
        ])
        .await;
    o.ok();
    assert!(o.stderr.contains("cua daemon started"), "{o:?}");
    assert!(
        !o.stderr.contains("keep-alive"),
        "the daemon holds it: {o:?}"
    );
    assert!(w.fake.exists("claim", &w.pool, "held"));
    // The CLI has exited; the daemon renews the claim.
    let o = h.run(&["daemon", "status"]).await;
    o.ok();
    let o = h.run(&["sb", "exec", "held", "echo", "via-daemon"]).await;
    o.ok();
    assert_eq!(o.stdout, "via-daemon\n");
    h.run(&["sb", "rm", "held", "--force"]).await.ok();
    assert!(!w.fake.exists("claim", &w.pool, "held"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_sandboxes_and_uniform_ops() {
    let env = fixtures::start_env(Some("t"), None).await;
    let h = Home::new();
    let on = format!("direct:{}", env.url);
    let o = h
        .run(&[
            "--embedded",
            "--json",
            "sb",
            "create",
            "--on",
            &on,
            "--token",
            "t",
            "--name",
            "dev",
        ])
        .await;
    o.ok();
    assert_eq!(o.json()["location"], "direct");
    // An existing machine: kind and runtime are not ours to say.
    assert_eq!(o.json()["kind"], "");
    let o = h.run(&["--embedded", "--json", "sb", "ls"]).await;
    o.ok();
    assert_eq!(row(&o.json(), "dev")["location"], "direct");
    let o = h
        .run(&["--embedded", "--json", "sb", "ls", "--cloud"])
        .await;
    assert!(
        o.code != 0 || !o.stdout.contains("\"dev\""),
        "--cloud leaves direct sandboxes out: {o:?}"
    );
    // cp both ways.
    let f = h.dir.path().join("a.bin");
    std::fs::write(&f, [7u8; 4096]).unwrap();
    h.run(&["--embedded", "sb", "cp", f.to_str().unwrap(), "dev:/tmp/"])
        .await
        .ok();
    let got = h.dir.path().join("b.bin");
    h.run(&[
        "--embedded",
        "sb",
        "cp",
        "dev:/tmp/a.bin",
        got.to_str().unwrap(),
    ])
    .await
    .ok();
    assert_eq!(std::fs::read(&got).unwrap(), vec![7u8; 4096]);
    let o = h.run(&["--embedded", "sb", "cp", "a:/x", "b:/y"]).await;
    assert_eq!(o.code, 2);
    // port-forward rides the spacesd's /tunnel WebSocket: without the
    // driver's tunnel capability it is a clear "unsupported".
    let free = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let pf = [
        "--embedded",
        "--json",
        "sb",
        "port-forward",
        "dev",
        &format!("3211:{free}"),
        "--no-wait",
    ];
    let o = h.run(&pf).await;
    assert_eq!(o.code, 4, "{o:?}");
    assert!(o.stderr.contains("tunnel.forward"), "{o:?}");
    env.mock
        .state
        .advertise(&[cua_spacesd_client::tunnel::TUNNEL_FORWARD_FEATURE]);
    // To a chosen local port.
    let o = h.run(&pf).await;
    o.ok();
    assert_eq!(o.json()["local_addr"], format!("127.0.0.1:{free}"));
    // Bytes round trip through a running forward.
    let echo = echo_server().await;
    let mut child = h.spawn(&[
        "--embedded",
        "--json",
        "sb",
        "port-forward",
        "dev",
        &echo.to_string(),
    ]);
    let local = forward_addr(&mut child).await;
    for i in 0..3u8 {
        assert_eq!(echo_through(&local, i).await, vec![b'a' + i; 4096]);
    }
    let _ = child.kill().await;
    assert!(
        env.mock.state.tunnel_attaches().iter().all(|a| a.accepted),
        "{:?}",
        env.mock.state.tunnel_attaches()
    );
    // logs: no console on a url: sandbox, guest log via the spacesd.
    let o = h
        .run(&["--embedded", "sb", "logs", "dev", "--source", "console"])
        .await;
    assert_eq!(o.code, 4, "{o:?}");
    assert!(o.stderr.contains("no console log"), "{o:?}");
    let o = h.run(&["--embedded", "sb", "logs", "dev", "-n", "5"]).await;
    o.ok();
    // Fleet-only verbs name the backend.
    let o = h.run(&["--embedded", "sb", "keep-alive", "dev"]).await;
    assert_eq!(o.code, 4, "{o:?}");
    // The token is remembered under the name and the direct ref...
    let tokens = h.dir.path().join(".cua").join("env-tokens.json");
    let saved: Value = serde_json::from_slice(&std::fs::read(&tokens).unwrap()).unwrap();
    assert_eq!(saved["dev"], "t");
    assert!(
        saved
            .as_object()
            .unwrap()
            .iter()
            .any(|(k, v)| k.starts_with("direct:") && v == "t"),
        "{saved}"
    );
    h.run(&["--embedded", "sb", "rm", "dev", "--force"])
        .await
        .ok();
    // ...and forgotten with the sandbox.
    let saved: Value = serde_json::from_slice(&std::fs::read(&tokens).unwrap()).unwrap();
    assert_eq!(
        saved,
        serde_json::json!({}),
        "no token outlives its sandbox"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_validates_on_and_flags() {
    let h = Home::new();
    // An unknown location lists the valid ones.
    let o = h
        .run(&["--embedded", "sb", "create", "img", "--on", "incus"])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("unknown location"), "{o:?}");
    assert!(o.stderr.contains("valid on: local, cloud"), "{o:?}");
    // Engines and kinds are not locations: the error names the right flag.
    for (on, hint) in [
        ("qemu", "use `--runtime qemu`"),
        ("lume", "use `--runtime lume`"),
        ("docker", "use `--kind container`"),
        ("fleet", "the cloud location is `cloud`"),
    ] {
        let o = h
            .run(&["--embedded", "sb", "create", "img", "--on", on])
            .await;
        assert_eq!(o.code, 2, "{on}: {o:?}");
        assert!(o.stderr.contains(hint), "{on}: {o:?}");
    }
    // An impossible location x kind x runtime lists the valid runtimes.
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "create",
            "img",
            "--on",
            "cloud",
            "--kind",
            "vm",
            "--runtime",
            "gvisor",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("valid runtime: auto, kubevirt"), "{o:?}");
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "create",
            "img",
            "--on",
            "local",
            "--runtime",
            "kubevirt",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(
        o.stderr
            .contains("valid runtime: auto, gvisor, runc, qemu, lume"),
        "{o:?}"
    );
    let o = h
        .run(&["--embedded", "sb", "create", "--on", "cloud"])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(
        o.stderr.contains("IMAGE is required for --on cloud"),
        "{o:?}"
    );
    // --pool locally runs that pool's template; an existing machine refuses.
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "create",
            "--on",
            "direct:127.0.0.1:1",
            "--pool",
            "p",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "create",
            "img",
            "--on",
            "local",
            "--pool",
            "p",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("not both"), "{o:?}");
    // launch --local --pool (deprecated spelling) needs no --name (a local
    // template run); without cloud credentials it fails naming the backend.
    let o = h
        .run(&["--embedded", "sb", "launch", "--local", "--pool", "p"])
        .await;
    assert_ne!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("[local]"), "{o:?}");
    assert!(o.stderr.contains("`--local` is deprecated"), "{o:?}");
    // The cloud without credentials names the backend.
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "create",
            "ghcr.io/x/y@sha256:1",
            "--on",
            "cloud",
        ])
        .await;
    assert_eq!(o.code, 4, "{o:?}");
    assert!(o.stderr.contains("[cloud]"), "{o:?}");
    // The flags `create` never shipped are gone.
    for gone in ["--vm", "--container", "--provider", "--url", "--local"] {
        let o = h.run(&["--embedded", "sb", "create", "img", gone]).await;
        assert_eq!(o.code, 2, "{gone}: {o:?}");
    }
    let o = h.run(&["sb", "create", "--help"]).await;
    o.ok();
    for flag in [
        "--on",
        "--kind",
        "--runtime",
        "--cpu",
        "--memory",
        "--disk",
        "--port",
        "--wait",
        "--warm",
        "--max-pool-size",
        "--claim-ttl",
        "--pool",
        "--token",
        "--name",
    ] {
        assert!(
            o.stdout.contains(flag),
            "{flag} missing from sb create --help"
        );
    }
    let o = h.run(&["sb", "launch", "--help"]).await;
    o.ok();
    for flag in [
        "--warm",
        "--max-pool-size",
        "--claim-ttl",
        "--kind",
        "--runtime",
        "--cpu",
        "--memory",
    ] {
        assert!(
            o.stdout.contains(flag),
            "{flag} missing from sb launch --help"
        );
    }
}

/// The default location comes from `cua config` / `CUA_DEFAULT_ON`, and a
/// cloud default without credentials says how to sign in or go back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_location_is_configurable() {
    let mut h = Home::new();
    let o = h.run(&["config", "get", "default.on"]).await;
    o.ok();
    assert_eq!(o.stdout.trim(), "local");
    h.run(&["config", "set", "default.on", "cloud"]).await.ok();
    let o = h.run(&["--json", "config", "list"]).await;
    o.ok();
    let on = o
        .json()
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "default.on")
        .cloned()
        .unwrap();
    assert_eq!(
        (on["value"].as_str(), on["source"].as_str()),
        (Some("cloud"), Some("config"))
    );
    // No credentials: the error says how to log in or switch back.
    let o = h
        .run(&["--embedded", "sb", "create", "ghcr.io/x/y@sha256:1"])
        .await;
    assert_eq!(o.code, 4, "{o:?}");
    assert!(o.stderr.contains("cua auth login"), "{o:?}");
    assert!(
        o.stderr.contains("cua config set default.on local"),
        "{o:?}"
    );
    // The environment beats the config file.
    h.set("CUA_DEFAULT_ON", "local");
    let o = h.run(&["config", "get", "default.on", "--json"]).await;
    o.ok();
    assert_eq!(o.json()["source"], "env");
    // `sb ls` lists every location whatever the default.
    let o = h.run(&["--embedded", "--json", "sb", "ls"]).await;
    o.ok();
    h.run(&["config", "unset", "default.on"]).await.ok();
}
