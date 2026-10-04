//! `cua sb` with `--on`: `url:` direct sandboxes, Cua Cloud (closed), and
//! argument validation for the local backends (nothing here starts a VM or
//! container).

mod common;
use common::*;
use cua_daemon::fixtures;
use serde_json::Value;

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
    // launch --local --pool (deprecated spelling) needs no --name; pools
    // were Cua Cloud's, which has closed.
    let o = h
        .run(&["--embedded", "sb", "launch", "--local", "--pool", "p"])
        .await;
    assert_eq!(o.code, 4, "{o:?}");
    assert!(o.stderr.contains("Cua Cloud has closed"), "{o:?}");
    assert!(o.stderr.contains("`--local` is deprecated"), "{o:?}");
    // The cloud says it has closed.
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
    assert!(o.stderr.contains("Cua Cloud has closed"), "{o:?}");
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
/// cloud default says Cua Cloud has closed.
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
    let o = h
        .run(&["--embedded", "sb", "create", "ghcr.io/x/y@sha256:1"])
        .await;
    assert_eq!(o.code, 4, "{o:?}");
    assert!(o.stderr.contains("Cua Cloud has closed"), "{o:?}");
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
