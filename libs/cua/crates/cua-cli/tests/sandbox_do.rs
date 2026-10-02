#![allow(deprecated)] // also exercises the deprecated `apply_pool` wrapper
//! `cua sandbox` and `cua do` against the mock spacesd (direct
//! sandboxes) and the fake Fleet API.

mod common;
use common::*;
use cua_daemon::fixtures;
use cua_fleet::testing::FakeFleet;
use serde_json::json;

async fn direct(h: &Home, url: &str, name: &str) {
    direct_with(h, url, name, "t").await;
}

async fn direct_with(h: &Home, url: &str, name: &str, token: &str) {
    h.run(&[
        "--embedded",
        "sb",
        "create",
        "--on",
        &format!("direct:{url}"),
        "--token",
        token,
        "--name",
        name,
    ])
    .await
    .ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_lifecycle_exec_and_compat_aliases() {
    let env = fixtures::start_env(Some("t"), None).await;
    let h = Home::new();
    direct(&h, &env.url, "dev").await;

    // `ls` lists everything with its location (no Fleet credentials here:
    // no cloud rows and no warning); a direct URL is not local.
    let o = h.run(&["--embedded", "--json", "sb", "ls"]).await;
    o.ok();
    assert_eq!(o.json()[0]["name"], "dev");
    assert_eq!(o.json()[0]["location"], "direct");
    // `ls` prints refs, and every NAME argument takes them.
    let authority = env.url.trim_start_matches("http://").to_string();
    let id = format!("direct:{authority}");
    assert_eq!(o.json()[0]["id"], id.as_str());
    assert!(!o.stderr.contains("warning"), "{o:?}");
    let o = h
        .run(&["--embedded", "--json", "sb", "ls", "--local"])
        .await;
    o.ok();
    assert_eq!(o.json(), serde_json::json!([]));
    // --cloud without credentials: empty, with the reason, exit 0.
    let o = h
        .run(&["--embedded", "--json", "sb", "ls", "--cloud"])
        .await;
    o.ok();
    assert_eq!(o.json(), serde_json::json!([]));
    assert!(
        o.stderr.contains("warning: cloud sandboxes not listed:"),
        "{o:?}"
    );
    // `list` alias and `get` alias.
    let o = h.run(&["--embedded", "sandbox", "list"]).await;
    o.ok();
    assert!(o.stdout.contains("ID") && o.stdout.contains(&id), "{o:?}");
    let o = h.run(&["--embedded", "sb", "get", "dev"]).await;
    o.ok();
    assert!(o.stdout.contains("Location: direct"), "{o:?}");
    assert!(o.stdout.contains(&format!("Id:       {id}")), "{o:?}");
    let o = h
        .run(&[
            "--embedded",
            "--json",
            "sb",
            "get",
            &format!("url:{authority}"),
        ])
        .await;
    o.ok();
    assert_eq!(o.json()["name"], "dev", "legacy url: spelling");
    // --local narrows the lookup: no local sandbox is named dev.
    let o = h.run(&["--embedded", "sb", "get", "--local", "dev"]).await;
    assert_eq!(o.code, 3, "{o:?}");
    let o = h
        .run(&["--embedded", "sb", "exec", &id, "echo", "by-ref"])
        .await;
    o.ok();
    assert_eq!(o.stdout, "by-ref\n");

    // exec joins the command into one shell line (Python CLI parity).
    let o = h
        .run(&["--embedded", "sb", "exec", "dev", "echo", "hello", "world"])
        .await;
    o.ok();
    assert_eq!(o.stdout, "hello world\n");
    let o = h
        .run(&["--embedded", "sb", "exec", "dev", "--", "fail", "4"])
        .await;
    assert_eq!(o.code, 4);
    let o = h
        .run(&["--embedded", "--json", "sb", "exec", "dev", "echo hi"])
        .await;
    o.ok();
    let v = o.json();
    assert_eq!(
        (v["stdout"].as_str(), v["returncode"].as_i64()),
        (Some("hi\n"), Some(0))
    );
    // shell with a command and no TTY runs it once.
    let o = h
        .run(&["--embedded", "sb", "shell", "dev", "echo", "pty"])
        .await;
    o.ok();
    assert!(o.stdout.contains("pty"), "{o:?}");

    let shot = h.dir.path().join("s.png");
    h.run(&[
        "--embedded",
        "sb",
        "screenshot",
        "dev",
        "-o",
        shot.to_str().unwrap(),
    ])
    .await
    .ok();
    assert!(std::fs::read(&shot).unwrap().starts_with(b"\x89PNG"));

    // Direct sandboxes cannot be suspended.
    let o = h.run(&["--embedded", "sb", "suspend", "dev"]).await;
    assert_eq!(o.code, 4, "{o:?}");

    // delete alias. Without a terminal, deleting needs --force (the
    // decision itself is unit-tested in sandbox.rs).
    h.run(&["--embedded", "sb", "delete", "--force", "dev"])
        .await
        .ok();
    let o = h.run(&["--embedded", "sb", "rm", "dev", "--force"]).await;
    assert_eq!(o.code, 3, "{o:?}");
    let o = h.run(&["--embedded", "sb", "ls"]).await;
    assert!(o.stdout.contains("No sandboxes found."), "{o:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_validates_arguments() {
    let h = Home::new();
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "launch",
            "linux",
            "--pool",
            "p",
            "--name",
            "x",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    let o = h.run(&["--embedded", "sb", "launch", "--pool", "p"]).await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("--name is required"), "{o:?}");
    let o = h
        .run(&["--embedded", "sb", "launch", "linux", "--memory", "lots"])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    // Fleet without credentials.
    let o = h
        .run(&["--embedded", "sb", "launch", "--pool", "p", "--name", "x"])
        .await;
    assert_eq!(o.code, 4, "{o:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fleet_pool_launch_list_suspend_and_release() {
    let fake = FakeFleet::new();
    let fleet = fixtures::start_fleet_http(fake.clone()).await;
    fake.client()
        .apply_pool(&cua_fleet::PoolSpec::new(
            "smoke-pool",
            "ghcr.io/trycua/cua-desktop-linux:test",
        ))
        .await
        .unwrap();
    let mut h = Home::new();
    h.set("CUA_FLEET_BASE_URL", &fleet.base_url)
        .set("FLEETS_TOKEN", "fake-fleet-token");

    let o = h
        .run(&[
            "--embedded",
            "--json",
            "sb",
            "launch",
            "--pool",
            "smoke-pool",
            "--name",
            "claim-1",
        ])
        .await;
    o.ok();
    assert_eq!(o.json(), json!({"name": "claim-1", "status": "ready"}));
    assert!(fake.exists("claim", "smoke-pool", "claim-1"));

    // Live Fleet listing (the Python CLI could not list Fleet sandboxes).
    let o = h
        .run(&["--embedded", "--json", "sb", "ls", "--cloud"])
        .await;
    o.ok();
    let rows = o.json();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "claim-1" && r["location"] == "cloud"),
        "{rows}"
    );

    // `sb suspend <pool>` (used by the scheduled smoke) scales the pool.
    h.run(&["--embedded", "sb", "suspend", "smoke-pool"])
        .await
        .ok();
    let pool = fake.object("pool", "smoke-pool", "smoke-pool").unwrap();
    assert_eq!(pool["spec"]["replicas"], 0);
    h.run(&["--embedded", "sb", "resume", "smoke-pool"])
        .await
        .ok();
    assert_eq!(
        fake.object("pool", "smoke-pool", "smoke-pool").unwrap()["spec"]["replicas"],
        1
    );
    let o = h.run(&["--embedded", "sb", "info", "smoke-pool"]).await;
    o.ok();
    assert!(o.stdout.contains("(dedicated cloud capacity)"), "{o:?}");

    h.run(&["--embedded", "sb", "delete", "claim-1", "--force"])
        .await
        .ok();
    assert!(!fake.exists("claim", "smoke-pool", "claim-1"));
    let o = h.run(&["--embedded", "sb", "suspend", "nope"]).await;
    assert_eq!(o.code, 3, "{o:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn do_drives_a_sandbox_with_zoom_windows_a11y_and_trajectories() {
    let env = fixtures::start_env(Some("t"), None).await;
    let h = Home::new();
    direct(&h, &env.url, "dev").await;
    let d = |args: &'static [&'static str]| {
        let mut v = vec!["--embedded", "do"];
        v.extend_from_slice(args);
        v
    };

    let o = h.run(&d(&["status"])).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("No target selected"), "{o:?}");
    let o = h.run(&d(&["switch", "missing"])).await;
    assert_eq!(o.code, 3, "{o:?}");
    let o = h.run(&d(&["switch", "docker", "dev"])).await;
    o.ok();
    assert!(o.stdout.starts_with("✅ Switched to dev"), "{o:?}");
    let o = h.run(&d(&["status"])).await;
    assert!(o.stdout.contains("Current target: docker/dev"), "{o:?}");
    let o = h.run(&d(&["ls"])).await;
    assert!(o.stdout.contains("  dev  [running]  direct"), "{o:?}");

    // Full-screen screenshot: mock screen is 1280x800 → capped at 1200.
    let shot = h.dir.path().join("full.png");
    let o = h
        .run(
            &d(&["screenshot", "--save"])
                .into_iter()
                .chain([shot.to_str().unwrap()])
                .collect::<Vec<_>>(),
        )
        .await;
    o.ok();
    assert!(o.stdout.contains("screenshot saved to"), "{o:?}");
    assert!(o.stdout.contains("💻 dev\t🔍 zoom: off"), "{o:?}");
    let state = read_json(&h.cua_home().join("do_target.json"));
    assert!((state["zoom_scale"].as_f64().unwrap() - 1200.0 / 1280.0).abs() < 1e-9);
    // Image (600, 300) → screen (640, 320).
    h.run(&d(&["click", "600", "300"])).await.ok();
    assert!(
        env.mock
            .state
            .observed
            .pointer
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .contains("x: 640.0, y: 320.0")
    );

    // Zoom to a window: screenshots come from that window, coordinates map
    // into it (origin 100,50; 800x600 is under the cap → scale 1).
    let o = h.run(&d(&["zoom", "term"])).await;
    o.ok();
    assert!(o.stdout.contains("Zoomed to 'Terminal' (w1)"), "{o:?}");
    let o = h.run(&d(&["zoom", "e"])).await; // matches both windows
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("Multiple windows matched"), "{o:?}");
    h.run(&d(&["screenshot"])).await.ok();
    let last = env
        .mock
        .state
        .observed
        .screenshots
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert!(format!("{:?}", last.source).contains("\"w1\""), "{last:?}");
    let o = h.run(&d(&["click", "10", "20", "right"])).await;
    o.ok();
    assert!(o.stdout.contains("clicked (10, 20) [right]"), "{o:?}");
    assert!(o.stdout.contains("zoom: term (w1)"), "{o:?}");
    let ptr = env
        .mock
        .state
        .observed
        .pointer
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert!(
        ptr.contains("x: 110.0, y: 70.0") && ptr.contains("button: Right"),
        "{ptr}"
    );
    assert!(
        env.mock
            .state
            .observed
            .windows
            .lock()
            .unwrap()
            .iter()
            .any(|w| w == "activate w1")
    );
    h.run(&d(&["dclick", "1", "1"])).await.ok();
    h.run(&d(&["move", "5", "5"])).await.ok();
    h.run(&d(&["drag", "0", "0", "10", "10"])).await.ok();
    h.run(&d(&["type", "hello"])).await.ok();
    h.run(&d(&["key", "enter"])).await.ok();
    let o = h.run(&d(&["hotkey", "ctrl+shift+t"])).await;
    o.ok();
    assert!(o.stdout.contains("hotkey: ctrl+shift+t"));
    let o = h.run(&d(&["key", "notakey"])).await;
    assert_eq!(o.code, 1, "{o:?}");
    h.run(&d(&["scroll", "down", "2"])).await.ok();
    let kb = env.mock.state.observed.keyboard.lock().unwrap().clone();
    assert!(
        kb.iter().any(|k| k.contains("hello")) && kb.iter().any(|k| k.contains("Hotkey")),
        "{kb:?}"
    );
    let o = h.run(&d(&["unzoom"])).await;
    assert!(o.stdout.contains("Unzoomed"), "{o:?}");

    // Windows service.
    let o = h.run(&d(&["window", "ls"])).await;
    o.ok();
    // Each row names the owning app (cua-driver's window info).
    assert!(
        o.stdout
            .contains("w1  Terminal  Terminal  [100,50,800,600]"),
        "{o:?}"
    );
    assert!(o.stdout.contains("w2  Editor  notes.txt - Editor"), "{o:?}");
    h.run(&d(&["window", "focus", "w2"])).await.ok();
    h.run(&d(&["window", "resize", "w2", "300", "200"]))
        .await
        .ok();
    h.run(&d(&["window", "move", "w2", "5", "6"])).await.ok();
    let o = h.run(&d(&["window", "info", "w2"])).await;
    o.ok();
    let info: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(info["position"], json!([5.0, 6.0]));
    assert_eq!(info["size"], json!([300.0, 200.0]));
    assert_eq!(info["focused"], true);
    h.run(&d(&["window", "minimize", "w2"])).await.ok();
    h.run(&d(&["window", "close", "w2"])).await.ok();
    let o = h.run(&d(&["window", "close", "w2"])).await;
    assert_eq!(o.code, 1);
    h.run(&d(&["window", "unfocus"])).await.ok();
    h.run(&d(&["open", "https://example.com"])).await.ok();
    h.run(&d(&["launch", "gedit", "--new-window"])).await.ok();
    let w = env.mock.state.observed.windows.lock().unwrap().clone();
    for want in [
        "activate w2",
        "bounds w2 5,6,300,200",
        "minimize w2",
        "close w2",
        "open https://example.com",
        "launch gedit --new-window",
    ] {
        assert!(w.iter().any(|x| x == want), "{want} not in {w:?}");
    }

    // Accessibility service.
    let o = h.run(&d(&["a11y", "tree"])).await;
    o.ok();
    let tree: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(tree["nodes"].as_array().unwrap().len(), 3);
    let o = h.run(&d(&["a11y", "find", "ok"])).await;
    let found: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(found["nodes"][0]["id"], "e1");
    h.run(&d(&["a11y", "act", "e2", "set_value", "query"]))
        .await
        .ok();
    assert_eq!(
        env.mock
            .state
            .observed
            .accessibility
            .lock()
            .unwrap()
            .last()
            .unwrap(),
        "act e2 ACCESSIBILITY_ACTION_SET_VALUE query"
    );

    // Clipboard, cursor, shell.
    h.run(&d(&["clipboard", "set", "copied"])).await.ok();
    let o = h.run(&d(&["clipboard", "get"])).await;
    assert_eq!(o.stdout, "copied\n");
    let o = h.run(&d(&["cursor"])).await;
    assert!(o.stdout.contains("cursor at"), "{o:?}");
    let o = h.run(&d(&["shell", "echo", "hi"])).await;
    o.ok();
    assert!(o.stdout.starts_with("✅ hi"), "{o:?}");
    let o = h.run(&d(&["shell", "fail", "3"])).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("❌ exit 3"), "{o:?}");
    let o = h.run(&d(&["shell"])).await;
    assert!(o.stderr.contains("No command provided"), "{o:?}");

    // Snapshot without a key fails cleanly.
    let o = h.run(&d(&["snapshot"])).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("ANTHROPIC_API_KEY"), "{o:?}");

    // Trajectory: every action recorded; --no-record skips.
    let o = h.run(&["--json", "trajectory", "ls", "dev"]).await;
    o.ok();
    let sessions = o.json();
    let turns = sessions[0]["turns"].as_u64().unwrap();
    assert!(turns >= 10, "{sessions}");
    h.run(&d(&["--no-record", "key", "a"])).await.ok();
    let o = h.run(&["--json", "traj", "ls"]).await;
    assert_eq!(o.json()[0]["turns"].as_u64().unwrap(), turns);
    let session = std::path::PathBuf::from(sessions[0]["path"].as_str().unwrap());
    let click = read_json(&session.join("turn_002/turn_002_agent_response.json"));
    assert_eq!(click["response"]["output"][0]["action"]["type"], "click");
    assert!(session.join("turn_002/screenshot.png").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_target_needs_consent() {
    let env = fixtures::start_env(Some("hosttok"), None).await;
    let mut h = Home::new();
    h.set("CUA_HOST_ENV_URL", &env.url)
        .set("CUA_HOST_ENV_TOKEN", "hosttok");
    let o = h.run(&["--embedded", "do", "switch", "host"]).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("cua do-host-consent"), "{o:?}");
    let o = h.run(&["--embedded", "do-host-consent"]).await;
    o.ok();
    assert!(o.stdout.contains("Switched to host"), "{o:?}");
    assert!(h.cua_home().join("host_consented").exists());
    let o = h.run(&["--embedded", "do", "type", "x"]).await;
    o.ok();
    assert!(o.stdout.contains("💻 host"), "{o:?}");
    // `do switch url` registers a direct target.
    let o = h
        .run(&[
            "--embedded",
            "do",
            "switch",
            "url",
            &env.url,
            "--token",
            "hosttok",
            "--as",
            "box",
        ])
        .await;
    o.ok();
    h.run(&["--embedded", "do", "key", "tab"]).await.ok();
}

/// cua-spacesd's `/mcp` requires the Space token: `sb mcp` against a
/// direct sandbox sends the one registered with `--token` (it used to send
/// none and got 401).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_mcp_sends_the_direct_token() {
    use cua_sandbox_core::testing::McpTestServer;
    let server = McpTestServer::start_with_token("t").await.unwrap();
    let h = Home::new();
    direct(&h, &server.url, "auth-box").await;
    let o = h
        .run(&[
            "--embedded",
            "--json",
            "sb",
            "mcp",
            "auth-box",
            "env",
            "tools",
        ])
        .await;
    o.ok();
    assert!(o.stdout.contains("\"add\""), "{o:?}");
    // A sandbox registered with the wrong token is refused.
    let other = McpTestServer::start_with_token("t").await.unwrap();
    direct_with(&h, &other.url, "wrong-box", "nope").await;
    let o = h
        .run(&["--embedded", "sb", "mcp", "wrong-box", "env", "tools"])
        .await;
    assert_ne!(o.code, 0, "{o:?}");
    assert!(o.stderr.contains("401"), "{o:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_mcp_tools_call_and_content() {
    use cua_sandbox_core::testing::{McpTestServer, expected_result, png_bytes};
    let server = McpTestServer::start("").await.unwrap();
    let h = Home::new();
    // A direct sandbox at the MCP server: its origin is the `env` service;
    // the client (rmcp) does not need cua-spacesd.
    direct(&h, &server.url, "mcp-box").await;
    let run = |args: Vec<&'static str>| {
        let mut a = vec!["--embedded"];
        a.extend(args);
        a
    };
    let o = h
        .run(&run(vec!["--json", "sb", "mcp", "mcp-box", "env", "tools"]))
        .await;
    o.ok();
    let names: Vec<String> = o
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        ["add", "image", "audio", "resources", "fail"],
        "{o:?}"
    );

    let o = h
        .run(&run(vec![
            "sb",
            "mcp",
            "mcp-box",
            "env",
            "call",
            "add",
            r#"{"a":2,"b":3}"#,
        ]))
        .await;
    o.ok();
    assert_eq!(o.stdout.trim(), "5");
    let o = h
        .run(&run(vec!["sb", "mcp", "mcp-box", "env", "call", "fail"]))
        .await;
    assert_eq!(o.code, 1, "a tool error exits 1: {o:?}");

    // Binary content: summarized as text, raw with --json, saved with --out.
    let o = h
        .run(&run(vec![
            "sb",
            "mcp",
            "mcp-box",
            "env",
            "call",
            "image",
            r#"{"bytes":70000}"#,
        ]))
        .await;
    o.ok();
    assert!(o.stdout.contains("70000 byte PNG"), "{o:?}");
    assert!(o.stdout.contains("[image image/png, 70000 bytes]"), "{o:?}");
    let o = h
        .run(&run(vec![
            "--json",
            "sb",
            "mcp",
            "mcp-box",
            "env",
            "call",
            "image",
            r#"{"bytes":70000}"#,
        ]))
        .await;
    o.ok();
    assert_eq!(
        o.json()["content"],
        expected_result("image", json!({"bytes": 70000}))["content"]
    );
    let dir = h.dir.path().join("out");
    let dir_s = dir.display().to_string();
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "mcp",
            "mcp-box",
            "env",
            "call",
            "image",
            r#"{"bytes":70000}"#,
            "--out",
            &dir_s,
        ])
        .await;
    o.ok();
    assert_eq!(
        std::fs::read(dir.join("image-1.png")).unwrap(),
        png_bytes(70000)
    );
    let o = h
        .run(&run(vec![
            "sb",
            "mcp",
            "mcp-box",
            "env",
            "call",
            "resources",
        ]))
        .await;
    o.ok();
    assert!(
        o.stdout
            .contains("[link file:///srv/report.pdf (application/pdf)]"),
        "{o:?}"
    );
    assert!(
        o.stdout
            .contains("[blob application/octet-stream, 4 bytes]"),
        "{o:?}"
    );
    assert!(o.stdout.contains("hello"), "{o:?}");

    let o = h
        .run(&run(vec!["sb", "mcp", "mcp-box", "env", "config"]))
        .await;
    o.ok();
    let v: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(v["url"], format!("{}/mcp", server.url));
    let o = h
        .run(&run(vec!["--json", "sb", "mcp", "mcp-box", "env", "info"]))
        .await;
    o.ok();
    assert!(o.json()["protocolVersion"].as_str().is_some(), "{o:?}");
}

/// The default `sb ls` reads the signed-in session from the OS credential
/// vault only when the session marker says one is stored (a stand-in vault
/// here; it counts its reads).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_ls_reads_a_keychain_session_only_with_the_marker() {
    let fake = FakeFleet::new();
    let fleet = fixtures::start_fleet_http(fake.clone()).await;
    fake.client()
        .apply_pool(&cua_fleet::PoolSpec::new(
            "marker-pool",
            "ghcr.io/trycua/cua-desktop-linux:test",
        ))
        .await
        .unwrap();
    // Another machine holds the claim: only a live Fleet listing shows it.
    let mut other = Home::new();
    other
        .set("CUA_FLEET_BASE_URL", &fleet.base_url)
        .set("FLEETS_TOKEN", "fake-fleet-token");
    other
        .run(&[
            "--embedded",
            "sb",
            "launch",
            "--pool",
            "marker-pool",
            "--name",
            "held-elsewhere",
        ])
        .await
        .ok();

    let vault = tempfile::tempdir().unwrap();
    let mut h = Home::new();
    h.set("CUA_FLEET_BASE_URL", &fleet.base_url).set(
        "CUA_CREDENTIAL_STORE",
        format!("test-keychain:{}", vault.path().display()),
    );
    // A signed-in session in the (stand-in) vault, stored before markers.
    let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    std::fs::write(
        vault.path().join("vault.json"),
        json!({"access_token": "fake-fleet-token", "expires_at": expires}).to_string(),
    )
    .unwrap();
    let reads = || {
        std::fs::read_to_string(vault.path().join("reads"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    let names = |o: &Out| -> Vec<String> {
        o.json()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    // No marker: the vault is never probed and no cloud rows are listed.
    let o = h.run(&["--embedded", "--json", "sb", "ls"]).await;
    o.ok();
    assert!(!names(&o).contains(&"held-elsewhere".to_string()), "{o:?}");
    assert_eq!(reads(), 0, "the vault was read without a marker");

    // An explicit cloud listing reads the session and writes the marker.
    let o = h
        .run(&["--embedded", "--json", "sb", "ls", "--cloud"])
        .await;
    o.ok();
    assert!(names(&o).contains(&"held-elsewhere".to_string()), "{o:?}");
    let marker = std::fs::read_to_string(h.cua_home().join("session.json")).unwrap();
    assert!(marker.contains("\"keychain\"") && !marker.contains("fake-fleet-token"));

    // With the marker, the default listing includes the cloud rows.
    let before = reads();
    let o = h.run(&["--embedded", "--json", "sb", "ls"]).await;
    o.ok();
    let rows = o.json();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "held-elsewhere")
        .unwrap_or_else(|| panic!("{rows}"));
    assert_eq!(row["location"], "cloud");
    assert!(reads() > before);
}
