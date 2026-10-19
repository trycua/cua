//! `cua sandbox` and `cua do` against the mock spacesd (direct
//! sandboxes), and relay Spaces.

mod common;
use common::*;
use cua_daemon::fixtures;
use cua_host::testing::FakeRelay;
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
    let mut h = Home::new();
    // No local containers: this machine's own Docker Spaces must not show up
    // in a "local" listing (the test runs next to real Spaces on dev Macs).
    h.set("DOCKER_HOST", "unix:///nonexistent/cua-test-no-docker.sock");
    direct(&h, &env.url, "dev").await;

    // `ls` lists everything with its location (no warning); a direct URL is not local.
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
    let o = h.run(&["--embedded", "sb", "launch", "--name", "x"]).await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("specify IMAGE"), "{o:?}");
    let o = h
        .run(&[
            "--embedded",
            "sb",
            "launch",
            "linux",
            "--on",
            "local",
            "--memory",
            "lots",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    // The options of the ended Cua Cloud still parse (hidden) and fail with
    // the one message.
    for args in [
        &["--pool", "p", "--name", "x"][..],
        &["linux", "--pool", "p"],
        &["linux", "--warm"],
        &["linux", "--claim-ttl", "15m"],
    ] {
        let o = h
            .run(&[&["--embedded", "sb", "launch"][..], args].concat())
            .await;
        assert_eq!(o.code, 4, "{args:?}: {o:?}");
        assert!(
            o.stderr.contains(cua_sandbox_core::CLOUD_ENDED),
            "{args:?}: {o:?}"
        );
    }
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

fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn select_closed(h: &Home) -> String {
    let url = format!("127.0.0.1:{}", closed_port());
    h.run(&[
        "--embedded",
        "do",
        "switch",
        "url",
        &url,
        "--token",
        "t",
        "--as",
        "gone",
    ])
    .await
    .ok();
    url
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn do_unselects_a_direct_target_that_stops_answering() {
    let h = Home::new();
    let url = select_closed(&h).await;
    let o = h.run(&["--embedded", "do", "screenshot"]).await;
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stderr.contains(&format!("gone (http://{url})")), "{o:?}");
    assert!(o.stderr.contains("cua do switch gone"), "{o:?}");
    assert!(o.stderr.contains("cua sandbox rm gone"), "{o:?}");
    assert!(o.stderr.contains("List Spaces: cua spaces ls"), "{o:?}");
    assert!(!o.stderr.contains("retry in a few seconds"), "{o:?}");
    assert!(!o.stdout.contains('💻'), "{o:?}");
    let o = h.run(&["--embedded", "do", "status"]).await;
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stderr.contains("No target selected"), "{o:?}");
    let listed = h.run(&["--embedded", "do", "ls", "direct"]).await;
    listed.ok();
    assert!(
        listed.stdout.contains("  gone  [") && listed.stdout.contains("direct"),
        "{listed:?}"
    );
    assert_eq!(
        read_json(&h.cua_home().join("env-tokens.json"))["gone"],
        "t"
    );
    h.run(&["--embedded", "do", "switch", "gone"]).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn do_names_a_registered_relay_space_without_touching_the_registry() {
    let h = Home::new();
    std::fs::create_dir_all(h.cua_home()).unwrap();
    let spaces = h.cua_home().join("spaces.json");
    let body = r#"[{"id":"direct:10.9.8.7:3211","name":"other","features":[]},{"id":"relay:a30ad1ef1e4fea09","name":"desk","features":[]}]"#;
    std::fs::write(&spaces, body).unwrap();
    select_closed(&h).await;
    let before = std::fs::read(&spaces).unwrap();
    let o = h.run(&["--embedded", "do", "screenshot"]).await;
    assert_eq!(o.code, 1, "{o:?}");
    assert!(
        o.stderr.contains("cua do switch relay:a30ad1ef1e4fea09"),
        "{o:?}"
    );
    assert!(!o.stderr.contains("direct:10.9.8.7:3211"), "{o:?}");
    assert!(!o.stderr.contains("List Spaces:"), "{o:?}");
    assert_eq!(std::fs::read(&spaces).unwrap(), before);
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

const SPACE_TOKEN: &str = "registry-only-5073-tok";

async fn spacesd_with_token() -> (cua_spacesd_client::testing::MockServer, String) {
    use cua_spacesd_client::testing::{MockAuth, MockServer};
    let server = MockServer::start(MockAuth {
        token: Some(SPACE_TOKEN.into()),
        ..Default::default()
    })
    .await;
    let id = format!("direct:{}", server.url().trim_start_matches("http://"));
    (server, id)
}

fn add_space(h: &Home, id: &str, credential: cua_spaces::registry::Credential) {
    let space = cua_proto::daemon::v1::Space {
        id: id.into(),
        ..Default::default()
    };
    cua_spaces::registry::Registry::new(h.cua_home())
        .upsert(space, credential)
        .unwrap();
}

async fn view(h: &Home, id: &str) -> Out {
    h.run(&["--embedded", "--json", "sb", "view", id, "--no-open"])
        .await
}

fn files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .flat_map(|e| {
            let p = e.unwrap().path();
            if p.is_dir() { files(&p) } else { vec![p] }
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_view_sends_the_spacesd_token_of_a_space_added_by_address() {
    use cua_spaces::registry::Credential;
    let (server, id) = spacesd_with_token().await;
    let mut h = Home::new();
    add_space(
        &h,
        &id,
        Credential {
            url: Some(server.url()),
            token: Some(SPACE_TOKEN.into()),
            ..Default::default()
        },
    );
    let o = view(&h, &id).await;
    o.ok();
    assert!(
        o.json()["url"]
            .as_str()
            .unwrap()
            .contains("#ticket=mock-viewer"),
        "{o:?}"
    );
    h.set("CUA_ENV_TOKEN", "wrong");
    view(&h, &id).await.ok();

    let creds = h.cua_home().join("spaces-credentials.json");
    for f in files(&h.cua_home()) {
        if f != creds {
            let body = String::from_utf8_lossy(&std::fs::read(&f).unwrap()).into_owned();
            assert!(!body.contains(SPACE_TOKEN), "{}", f.display());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&creds).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    h.env.remove("CUA_ENV_TOKEN");

    let (server, _) = spacesd_with_token().await;
    let url = server.url().trim_start_matches("http://").to_string();
    direct_with(&h, &url, "own-token", SPACE_TOKEN).await;
    add_space(
        &h,
        &format!("direct:{url}"),
        Credential {
            token: Some("wrong".into()),
            ..Default::default()
        },
    );
    view(&h, "own-token").await.ok();

    let (mcp_server, mcp_id) = spacesd_with_token().await;
    add_space(
        &h,
        &mcp_id,
        Credential {
            token: Some(SPACE_TOKEN.into()),
            service_urls: [("mcp".into(), format!("{}/mcp", mcp_server.url()))].into(),
            ..Default::default()
        },
    );
    assert_ne!(view(&h, &mcp_id).await.code, 0);

    let (_unregistered, unregistered_id) = spacesd_with_token().await;
    assert_ne!(view(&h, &unregistered_id).await.code, 0);
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

/// `sb ls` lists what this machine knows: it never reads the signed-in
/// session from the OS credential vault (a stand-in vault here; it counts its
/// reads).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sb_ls_never_reads_the_credential_vault() {
    let vault = tempfile::tempdir().unwrap();
    let mut h = Home::new();
    h.set(
        "CUA_CREDENTIAL_STORE",
        format!("test-keychain:{}", vault.path().display()),
    );
    // A signed-in session in the (stand-in) vault.
    let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    std::fs::write(
        vault.path().join("vault.json"),
        json!({"access_token": "a-session-token", "expires_at": expires}).to_string(),
    )
    .unwrap();
    let o = h.run(&["--embedded", "--json", "sb", "ls"]).await;
    o.ok();
    assert_eq!(o.json(), json!([]), "{o:?}");
    let reads = std::fs::read_to_string(vault.path().join("reads"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    assert_eq!(reads, 0, "the vault was read by `sb ls`");
}

/// `cua do` drives a relay Space: switch, screenshot, click, type and shell
/// go through the same `env_of` path as a direct sandbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn do_drives_a_relay_space() {
    const MACHINE: &str = "a30ad1ef1e4fea09";
    let relay = FakeRelay::start().await;
    relay.add_account("relay-token", "user-1", Some("ada@example.com"));
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "relay-token",
            &cua_host::relay::RegisterRequest {
                id: MACHINE.into(),
                name: "Relay desk".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let env = fixtures::start_env(None, None).await;
    relay.tunnel(MACHINE, &env.url);
    relay.set_online(MACHINE, true, "0.0.0-mock");

    let mut h = Home::new();
    h.set("CUA_RELAY_URL", &relay.url);
    let id = format!("relay:{MACHINE}");

    // A bad machine id stays a ref error and is not stored as a target.
    let o = h.run(&["--embedded", "do", "switch", "relay:short"]).await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(
        o.stderr.contains(
            "invalid argument: not a sandbox ref: \"relay:short\" (relay computer ids are [a-z0-9-]{8,64})"
        ),
        "{o:?}"
    );

    // Signed out, the directory refresh asks for the session. Previously
    // every relay id was refused as unsupported before that.
    let o = h.run(&["--embedded", "do", "switch", &id]).await;
    assert_eq!(o.code, 6, "{o:?}");
    assert!(
        o.stderr.contains(
            "unauthenticated: Relay authentication failed. Sign in again to refresh your computers."
        ),
        "{o:?}"
    );
    assert!(!o.stderr.contains("unsupported"), "{o:?}");

    std::fs::create_dir_all(h.cua_home()).unwrap();
    std::fs::write(
        h.cua_home().join("credentials.json"),
        json!({
            "access_token": "relay-token",
            "expires_at": "2999-01-01T00:00:00Z",
            "token_type": "Bearer"
        })
        .to_string(),
    )
    .unwrap();

    let o = h
        .run(&["--embedded", "do", "switch", "relay:0123abcd4567ef89"])
        .await;
    assert_eq!(o.code, 3, "{o:?}");
    assert!(
        o.stderr.contains("not found: Space relay:0123abcd4567ef89"),
        "{o:?}"
    );

    let o = h.run(&["--embedded", "do", "switch", &id]).await;
    o.ok();
    assert!(o.stdout.contains(&format!("✅ Switched to {id}")), "{o:?}");
    let state = read_json(&h.cua_home().join("do_target.json"));
    assert_eq!(state["provider"], "relay");
    assert_eq!(state["name"], id);

    // The legacy spelling stores the canonical id.
    let legacy = format!("space://relay/{MACHINE}");
    let o = h.run(&["--embedded", "do", "switch", &legacy]).await;
    o.ok();
    assert!(o.stdout.contains(&format!("✅ Switched to {id}")), "{o:?}");

    let o = h.run(&["--embedded", "do", "status"]).await;
    o.ok();
    assert!(
        o.stdout.contains(&format!("Current target: relay/{id}")),
        "{o:?}"
    );

    let shot = h.dir.path().join("relay.png");
    let o = h
        .run(&[
            "--embedded",
            "do",
            "screenshot",
            "--save",
            shot.to_str().unwrap(),
        ])
        .await;
    o.ok();
    assert!(o.stdout.contains("screenshot saved to"), "{o:?}");
    assert!(o.stdout.contains(&format!("💻 {id}")), "{o:?}");
    assert!(std::fs::read(&shot).unwrap().starts_with(b"\x89PNG"));
    let state = read_json(&h.cua_home().join("do_target.json"));
    assert!((state["zoom_scale"].as_f64().unwrap() - 1200.0 / 1280.0).abs() < 1e-9);

    // Image (600, 300) → screen (640, 320), the same mapping as a direct sandbox.
    let o = h.run(&["--embedded", "do", "click", "600", "300"]).await;
    o.ok();
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
    assert!(ptr.contains("x: 640.0, y: 320.0"), "{ptr}");

    h.run(&["--embedded", "do", "type", "relay-ok"]).await.ok();
    let kb = env.mock.state.observed.keyboard.lock().unwrap().clone();
    assert!(kb.iter().any(|k| k.contains("relay-ok")), "{kb:?}");

    let o = h
        .run(&["--embedded", "do", "shell", "echo", "relay-ok"])
        .await;
    o.ok();
    assert!(o.stdout.starts_with("✅ relay-ok"), "{o:?}");

    let target = std::fs::read(h.cua_home().join("do_target.json")).unwrap();
    relay.tunnel(MACHINE, &format!("http://127.0.0.1:{}", closed_port()));
    let o = h.run(&["--embedded", "do", "screenshot"]).await;
    assert_eq!(o.code, 1, "{o:?}");
    assert!(
        o.stderr.contains("has no cua-spacesd") && o.stderr.contains("502 Bad Gateway"),
        "{o:?}"
    );
    assert!(!o.stderr.contains("Unselected"), "{o:?}");
    assert!(o.stdout.contains("💻 relay:"), "{o:?}");
    assert_eq!(
        std::fs::read(h.cua_home().join("do_target.json")).unwrap(),
        target
    );

    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .stop_sharing("relay-token", MACHINE)
        .await
        .unwrap();
    let o = h.run(&["--embedded", "do", "switch", &id]).await;
    assert_eq!(o.code, 1, "{o:?}");
    assert!(
        o.stderr.contains(
            "env: Relay desk stopped sharing: ask its owner to Resume sharing (or run `cua host start` there)"
        ),
        "{o:?}"
    );
    // A failed switch leaves the selected target in place.
    let state = read_json(&h.cua_home().join("do_target.json"));
    assert_eq!(state["name"], id);
}
