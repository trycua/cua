//! `cua mcp`, `cua skills`, and `cua trajectory`.

mod common;
use common::*;
use cua_daemon::fixtures;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Mcp {
    child: tokio::process::Child,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
}

impl Mcp {
    fn start(h: &Home, args: &[&str]) -> Self {
        let mut child = h.spawn(args);
        let stdout = child.stdout.take().unwrap();
        Self {
            child,
            lines: BufReader::new(stdout).lines(),
            id: 0,
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let req = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin
            .write_all(format!("{req}\n").as_bytes())
            .await
            .unwrap();
        stdin.flush().await.unwrap();
        let line = tokio::time::timeout(Duration::from_secs(30), self.lines.next_line())
            .await
            .expect("mcp reply timed out")
            .unwrap()
            .expect("mcp closed stdout");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], self.id);
        v
    }

    async fn tool(&mut self, name: &str, args: Value) -> Value {
        self.call("tools/call", json!({"name": name, "arguments": args}))
            .await["result"]
            .clone()
    }

    async fn notify(&mut self, method: &str) {
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin
            .write_all(format!("{}\n", json!({"jsonrpc": "2.0", "method": method})).as_bytes())
            .await
            .unwrap();
    }

    async fn close(mut self) {
        drop(self.child.stdin.take());
        let s = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("mcp did not exit on EOF")
            .unwrap();
        assert!(s.success());
    }
}

fn text(r: &Value) -> Value {
    serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_serves_sandbox_computer_and_skills_tools() {
    let env = fixtures::start_env(Some("t"), None).await;
    let mut h = Home::new();
    // No local containers: this machine's own Docker Spaces must not show up
    // in a "local" listing (the test runs next to real Spaces on dev Macs).
    h.set("DOCKER_HOST", "unix:///nonexistent/cua-test-no-docker.sock");
    h.run(&[
        "--embedded",
        "sb",
        "create",
        "--on",
        &format!("direct:{}", env.url),
        "--token",
        "t",
        "--name",
        "dev",
    ])
    .await
    .ok();
    let mut m = Mcp::start(&h, &["--embedded", "mcp", "--sandbox", "dev"]);
    let init = m.call("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}})).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "cua");
    m.notify("notifications/initialized").await;
    let tools = m.call("tools/list", json!({})).await;
    let names: Vec<String> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    for want in [
        // The Spaces contract (one server with the sandbox tools).
        "add_space",
        "space_bash",
        "send_file",
        "sandbox_list",
        "computer_screenshot",
        "computer_window_list",
        "computer_get_accessibility_tree",
        "skills_list",
    ] {
        assert!(
            names.contains(&want.to_string()),
            "{want} missing: {names:?}"
        );
    }
    assert!(tools["result"]["tools"][0]["inputSchema"]["type"] == "object");

    // Everything by default, each row with its location; `location`
    // filters.
    let r = m.tool("sandbox_list", json!({})).await;
    assert_eq!(text(&r)[0]["name"], "dev");
    assert_eq!(text(&r)[0]["location"], "direct");
    let r = m.tool("sandbox_list", json!({"location": "local"})).await;
    assert_eq!(text(&r), json!([]), "{r}");
    let r = m.tool("sandbox_list", json!({"location": "direct"})).await;
    assert_eq!(text(&r)[0]["name"], "dev");
    let r = m.tool("sandbox_list", json!({"location": "moon"})).await;
    assert_eq!(r["isError"], true, "{r}");
    let r = m.tool("computer_screenshot", json!({})).await;
    assert_eq!(r["content"][0]["type"], "image", "{r}");
    assert_eq!(r["content"][0]["mimeType"], "image/png");
    // Screenshot pixels map to points (1200/1280 scale).
    let r = m.tool("computer_click", json!({"x": 600, "y": 300})).await;
    assert_eq!(r["isError"], false, "{r}");
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
    let r = m
        .tool("computer_window_list", json!({"app": "editor"}))
        .await;
    assert_eq!(text(&r)["windows"][0]["id"], "w2");
    let r = m
        .tool("computer_window_focus", json!({"window_id": "w2"}))
        .await;
    assert_eq!(r["isError"], false);
    let r = m.tool("computer_get_current_window", json!({})).await;
    assert_eq!(text(&r)["window_id"], "w2");
    let r = m.tool("computer_get_accessibility_tree", json!({})).await;
    assert_eq!(text(&r)["nodes"].as_array().unwrap().len(), 3);
    // computer_shell / computer_file_write run the Spaces contract's
    // space_bash / space_write handlers on the sandbox.
    let r = m
        .tool("computer_shell", json!({"command": "echo mcp"}))
        .await;
    assert_eq!(r["content"][0]["text"], "mcp\n[exit 0]", "{r}");
    let r = m
        .tool(
            "computer_file_write",
            json!({"path": "/tmp/m.txt", "content": "data"}),
        )
        .await;
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("wrote /tmp/m.txt (4 bytes"),
        "{r}"
    );
    let r = m
        .tool("computer_file_read", json!({"path": "/tmp/m.txt"}))
        .await;
    assert_eq!(text(&r)["content"], "data");
    m.tool("computer_clipboard_set", json!({"text": "clip"}))
        .await;
    let r = m.tool("computer_clipboard_get", json!({})).await;
    assert_eq!(text(&r)["content"], "clip");
    let r = m
        .tool("computer_window_close", json!({"window_id": "nope"}))
        .await;
    assert_eq!(r["isError"], true);
    let r = m
        .tool(
            "computer_key",
            json!({"key": "enter", "sandbox": "missing"}),
        )
        .await;
    assert_eq!(r["isError"], true);
    let r = m.tool("skills_list", json!({})).await;
    assert_eq!(text(&r), json!([]));
    let bad = m.call("nope/method", json!({})).await;
    assert_eq!(bad["error"]["code"], -32601);
    m.close().await;

    // Permissions narrow the tool list; `serve-mcp` alias still works.
    let mut m = Mcp::start(
        &h,
        &[
            "--embedded",
            "serve-mcp",
            "--sandbox",
            "dev",
            "--permissions",
            "computer:readonly,sandbox:readonly",
        ],
    );
    let tools = m.call("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"computer_screenshot") && names.contains(&"sandbox_get"),
        "{names:?}"
    );
    assert!(
        !names.contains(&"computer_click") && !names.contains(&"sandbox_delete"),
        "{names:?}"
    );
    let denied = m
        .call(
            "tools/call",
            json!({"name": "computer_click", "arguments": {"x": 1, "y": 1}}),
        )
        .await;
    // Unknown or not-permitted tools are "method not found" (the Spaces
    // server's convention, which its typed client maps).
    assert_eq!(denied["error"]["code"], -32601);
    m.close().await;

    // `cua daemon mcp` serves the same tools.
    let mut m = Mcp::start(&h, &["--embedded", "daemon", "mcp"]);
    let tools = m.call("tools/list", json!({})).await;
    let n = tools["result"]["tools"].as_array().unwrap().len();
    assert!(n > 31 + 40, "{n}");
    m.close().await;
}

/// `sandbox_view` on a sandbox whose viewer link is plain HTTP to a host other
/// than loopback (a Lume VM's NAT address) hands out a loopback proxy to it
/// instead, a secure context (#5734), and keeps that proxy for the session:
/// past the call, and reused by the next one. `0.0.0.0` stands in for the NAT
/// address; it reaches the mock's loopback listener on Unix only.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_sandbox_view_hands_out_a_loopback_link_for_the_session() {
    let env = fixtures::start_env(Some("t"), None).await;
    let h = Home::new();
    let port = env.url.rsplit(':').next().unwrap();
    h.run(&[
        "--embedded",
        "sb",
        "create",
        "--on",
        &format!("direct:http://0.0.0.0:{port}"),
        "--token",
        "t",
        "--name",
        "nat-box",
    ])
    .await
    .ok();
    let mut m = Mcp::start(&h, &["--embedded", "mcp"]);
    let mut view = async || {
        let r = m.tool("sandbox_view", json!({"name": "nat-box"})).await;
        let url = text(&r)["viewer_url"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let link = url::Url::parse(&url).unwrap_or_else(|e| panic!("{e}: {r}"));
        assert_eq!(link.host_str(), Some("127.0.0.1"), "{url}");
        assert_eq!(link.path(), "/viewer/", "{url}");
        assert!(
            link.fragment().unwrap().starts_with("ticket=mock-viewer"),
            "{url}"
        );
        link.origin().ascii_serialization()
    };
    let first = view().await;
    // The proxy outlives the call.
    assert_viewer_reaches(&first, &env).await;
    assert_eq!(view().await, first, "a second call opened another proxy");
    m.close().await;
}

/// Stops a daemon started by a test even when an assertion fails.
struct DaemonGuard<'a>(&'a Home);

impl Drop for DaemonGuard<'_> {
    fn drop(&mut self) {
        let discovery = self.0.cua_home().join("daemon.json");
        if let Some(pid) = std::fs::read(&discovery)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v["pid"].as_u64())
        {
            // Only the daemon this test's temp HOME started.
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
            #[cfg(not(unix))]
            let _ = pid;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_mcp_starts_the_daemon_and_serves_its_spaces_runtime() {
    // A MockServer spacesd in this process: processes are simulated, so
    // nothing runs on the host. (Spaces against a real driver core are
    // covered by libs/cua-spacesd/tests/spaces-e2e.)
    const TOKEN: &str = "cua-cli-test-token";
    let d = cua_spacesd_client::testing::MockServer::start(cua_spacesd_client::testing::MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    let mut h = Home::new();
    // Teleport and agents never read the real home.
    let host = h.dir.path().join("host-home").display().to_string();
    h.set("CUA_SPACES_TELEPORT_HOME", host)
        .set("CUA_SPACES_AGENT_CREDENTIALS_HOME", "none");
    let _guard = DaemonGuard(&h);
    let mut m = Mcp::start(&h, &["daemon", "mcp"]);
    let init = m.call("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}})).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "cua");
    // The daemon was started for us.
    let discovery: Value =
        serde_json::from_slice(&std::fs::read(h.cua_home().join("daemon.json")).unwrap()).unwrap();
    assert!(discovery["pid"].as_u64().is_some());

    let tools = m.call("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for t in cua_spaces::contract::tools() {
        assert!(names.contains(&t.name), "{} missing", t.name);
    }
    assert!(names.contains(&"computer_screenshot"));

    let r = m
        .tool(
            "add_space",
            json!({"url": d.url(), "token": TOKEN, "name": "stdio"}),
        )
        .await;
    assert_eq!(r["isError"], false, "{r}");
    let id = text(&r)["id"].as_str().unwrap().to_string();
    // It ran in the daemon, whose registry is under the temp CUA_HOME.
    let reg = std::fs::read_to_string(h.cua_home().join("spaces.json")).unwrap();
    assert!(reg.contains(&id));
    let r = m
        .tool(
            "space_bash",
            json!({"space": id, "command": "echo from-daemon"}),
        )
        .await;
    assert_eq!(r["content"][0]["text"], "from-daemon\n[exit 0]", "{r}");
    let r = m.tool("stream_endpoint", json!({"space": id})).await;
    assert_eq!(r["isError"], true);
    assert_eq!(
        r["structuredContent"]["error"]["kind"],
        "capability_missing"
    );
    let r = m.tool("remove_space", json!({"space": id})).await;
    assert_eq!(r["isError"], false, "{r}");
    m.close().await;
    h.run(&["daemon", "stop"]).await.ok();
}

fn write_skill(h: &Home, name: &str) {
    let d = h.cua_home().join("skills").join(name);
    std::fs::create_dir_all(d.join("trajectory")).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: Opens the settings panel\n---\n\n# {name}\n\nBody.\n"
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("trajectory/trajectory.json"),
        json!({"trajectory": [{"step_idx": 1}, {"step_idx": 2}], "metadata": {"created_at": "2026-09-01T10:00:00"}}).to_string(),
    )
    .unwrap();
    std::fs::write(d.join("trajectory/demo.mp4"), b"fake").unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skills_list_read_replay_delete_clean() {
    let h = Home::new();
    let o = h.run(&["skills", "ls"]).await;
    o.ok();
    assert!(o.stdout.contains("No skills found."));
    write_skill(&h, "settings");
    write_skill(&h, "other");
    let o = h.run(&["--json", "skills", "list"]).await;
    o.ok();
    let v = o.json();
    assert_eq!(v.as_array().unwrap().len(), 2);
    assert_eq!(v[1]["name"], "settings");
    assert_eq!(v[1]["steps"], 2);
    let o = h.run(&["skills", "list"]).await;
    assert!(
        o.stdout
            .contains("settings  Opens the settings panel  2      2026-09-01"),
        "{o:?}"
    );
    let o = h.run(&["skills", "read", "settings"]).await;
    assert!(o.stdout.starts_with("---\nname: settings"), "{o:?}");
    let o = h.run(&["skills", "read", "settings", "-f", "json"]).await;
    let v = o.json();
    assert_eq!(v["skill_prompt"], "# settings\n\nBody.");
    assert_eq!(v["trajectory"].as_array().unwrap().len(), 2);
    let o = h.run(&["skills", "replay", "settings"]).await;
    assert!(o.stdout.contains("demo.mp4"), "{o:?}");
    let o = h.run(&["skills", "read", "../etc"]).await;
    assert_eq!(o.code, 2);
    h.run(&["skills", "delete", "other"]).await.ok();
    let o = h.run(&["skills", "delete", "other"]).await;
    assert_eq!(o.code, 3);
    let o = h.run(&["skills", "clean", "-y"]).await;
    assert!(o.stdout.contains("Deleted 1 skill(s)."), "{o:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skills_record_receives_captions_and_writes_a_skill() {
    use futures_util::SinkExt;
    let llm = FakeHttp::start(|r| {
        assert_eq!(r.path, "/v1/messages");
        assert_eq!(r.headers.get("x-api-key").map(String::as_str), Some("test-key"));
        (200, json!({"content": [{"type": "text", "text": "{\"Observation\": \"a window\", \"Think\": \"open it\", \"Action\": \"click OK\", \"Expectation\": \"dialog closes\"}"}], "stop_reason": "end_turn"}))
    })
    .await;
    let mut h = Home::new();
    h.set("ANTHROPIC_BASE_URL", &llm.url)
        .set("ANTHROPIC_API_KEY", "test-key");
    // A real 1 s video when ffmpeg is present (frames + captions); a
    // placeholder otherwise (captions fall back to the event type).
    let video = h.dir.path().join("v.mp4");
    let have_ffmpeg = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=64x64:d=1",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&video)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !have_ffmpeg {
        eprintln!("ffmpeg not available; skipping skills record test");
        return;
    }
    let mut child = h.spawn(&[
        "skills",
        "record",
        "--viewer-url",
        "https://viewer.test/viewer/#ticket=abc",
        "--name",
        "demo",
        "--description",
        "Click OK",
    ]);
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let mut port = None;
    for _ in 0..10 {
        let Ok(Ok(Some(l))) =
            tokio::time::timeout(Duration::from_secs(20), lines.next_line()).await
        else {
            break;
        };
        if let Some(u) = l.strip_prefix("Viewer: ") {
            let u = url::Url::parse(u).unwrap();
            let rec = u
                .query_pairs()
                .find(|(k, _)| k == "record_url")
                .unwrap()
                .1
                .to_string();
            assert!(
                u.query_pairs()
                    .any(|(k, v)| k == "autorecord" && v == "true")
            );
            port = Some(rec.rsplit(':').next().unwrap().to_string());
            break;
        }
    }
    let port = port.expect("viewer URL printed");
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
        .await
        .unwrap();
    let rec = json!({"events": [{"type": "click", "timestamp": 300, "data": {"x": 1}}, {"type": "key", "timestamp": 600}], "metadata": {"width": 64, "height": 64, "duration": 1}});
    let js = rec.to_string().into_bytes();
    let mut data = (js.len() as u32).to_be_bytes().to_vec();
    data.extend(&js);
    data.extend(std::fs::read(&video).unwrap());
    for chunk in data.chunks(4096) {
        ws.send(tokio_tungstenite::tungstenite::Message::Binary(
            chunk.to_vec().into(),
        ))
        .await
        .unwrap();
    }
    ws.close(None).await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(60), child.wait())
        .await
        .unwrap()
        .unwrap();
    let mut rest = String::new();
    while let Ok(Ok(Some(l))) =
        tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await
    {
        rest.push_str(&l);
        rest.push('\n');
    }
    assert!(status.success(), "{rest}");
    assert!(rest.contains("Steps: 2"), "{rest}");
    let skill = h.cua_home().join("skills/demo");
    let md = std::fs::read_to_string(skill.join("SKILL.md")).unwrap();
    assert!(
        md.contains("### Step 1: click OK") && md.contains("**Context:** a window"),
        "{md}"
    );
    assert!(skill.join("trajectory/demo.mp4").exists());
    assert!(skill.join("trajectory/step_1_full.jpg").exists());
    assert_eq!(llm.requests().len(), 2);
    let o = h.run(&["--json", "skills", "ls"]).await;
    assert_eq!(o.json()[0]["steps"], 2);
}

/// `skills record --sandbox` opens the viewer of a sandbox whose link is plain
/// HTTP to a host other than loopback (a Lume VM's NAT address) through a
/// loopback proxy, a secure context (#5734), for as long as it records.
/// `0.0.0.0` stands in for the NAT address; it reaches the mock's loopback
/// listener on Unix only. The recording itself is covered above; ffmpeg is
/// only probed before the viewer opens, so a stub stands in for it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skills_record_opens_a_non_loopback_sandbox_viewer_on_loopback() {
    use std::os::unix::fs::PermissionsExt;
    let env = fixtures::start_env(Some("t"), None).await;
    let mut h = Home::new();
    let bin = h.dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(bin.join("ffmpeg"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(bin.join("ffmpeg"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    h.set("PATH", path);
    let port = env.url.rsplit(':').next().unwrap();
    h.run(&[
        "--embedded",
        "sb",
        "create",
        "--on",
        &format!("direct:http://0.0.0.0:{port}"),
        "--token",
        "t",
        "--name",
        "nat-box",
    ])
    .await
    .ok();
    let mut child = h.spawn(&[
        "--embedded",
        "skills",
        "record",
        "--sandbox",
        "nat-box",
        "--api-key",
        "test-key",
        "--name",
        "demo",
        "--description",
        "Click OK",
    ]);
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut url = None;
    while let Ok(Ok(Some(l))) =
        tokio::time::timeout(Duration::from_secs(60), lines.next_line()).await
    {
        if let Some(u) = l.strip_prefix("Viewer: ") {
            url = Some(u.to_string());
            break;
        }
    }
    let url = url.expect("no viewer link printed");
    let link = url::Url::parse(&url).unwrap();
    assert_eq!(link.host_str(), Some("127.0.0.1"), "{url}");
    assert_eq!(link.path(), "/viewer/", "{url}");
    assert!(
        link.fragment().unwrap().starts_with("ticket=mock-viewer"),
        "{url}"
    );
    assert!(
        link.query_pairs()
            .any(|(k, v)| k == "autorecord" && v == "true"),
        "{url}"
    );
    // The proxy lives while it records.
    assert_viewer_reaches(&link.origin().ascii_serialization(), &env).await;
    assert!(child.try_wait().unwrap().is_none(), "record exited: {url}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trajectory_view_serves_the_zip_with_cors_then_stops() {
    let h = Home::new();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let s = h.cua_home().join(format!("trajectories/box/{ts}/turn_001"));
    std::fs::create_dir_all(&s).unwrap();
    std::fs::write(s.join("turn_001_agent_response.json"), "{}").unwrap();
    let old = h.cua_home().join("trajectories/box/20200101-000000");
    std::fs::create_dir_all(&old).unwrap();
    let o = h.run(&["trajectory", "ls"]).await;
    assert!(
        o.stdout
            .contains(&format!("box                  {ts}        1")),
        "{o:?}"
    );
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let o = h
        .run(&["traj", "view", "box", "--port", &port.to_string()])
        .await;
    o.ok();
    assert!(
        o.stdout
            .contains("https://cua.ai/trajectory-viewer?zip=http%3A%2F%2Flocalhost%3A"),
        "{o:?}"
    );
    let url = format!("http://127.0.0.1:{port}/{ts}.zip");
    let mut resp = None;
    for _ in 0..50 {
        if let Ok(r) = reqwest::get(&url).await {
            resp = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let r = resp.expect("file server up");
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    assert!(r.bytes().await.unwrap().starts_with(b"PK\x03\x04"));
    let r = reqwest::get(format!("http://127.0.0.1:{port}/../../etc/passwd"))
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let o = h.run(&["trajectory", "stop"]).await;
    o.ok();
    assert!(o.stderr.contains("Stopped file server"), "{o:?}");
    let mut down = false;
    for _ in 0..50 {
        if reqwest::get(&url).await.is_err() {
            down = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(down, "server stopped");

    let o = h
        .run(&["trajectory", "clean", "--older-than", "30", "-y"])
        .await;
    assert!(o.stdout.contains("Deleted 1 session(s)."), "{o:?}");
    assert!(!old.exists());
    let o = h
        .run(&["trajectory", "clean", "--machine", "box", "-y"])
        .await;
    assert!(o.stdout.contains("Deleted 1 session(s)."), "{o:?}");
    let o = h.run(&["trajectory", "ls"]).await;
    assert!(o.stdout.contains("No trajectory sessions found."), "{o:?}");
}

/// A Space another account shared through the relay (CUA-1418): the guest
/// drives `relay:<id>` with the `computer_*` MCP tools and `cua do`, which
/// resolve it through the Spaces API, not the sandboxes API ("not supported
/// by the Direct provider" before).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_relay_space_is_driven_by_the_guest() {
    use cua_host::testing::FakeRelay;
    const MACHINE: &str = "5a7ed0e1a5bce111";
    let relay = FakeRelay::start().await;
    relay.add_account("owner-token", "owner-1", Some("owner@example.com"));
    relay.add_account("guest-token", "guest-1", Some("guest@example.com"));
    let owner = cua_host::RelayClient::new(&relay.url).unwrap();
    owner
        .register(
            "owner-token",
            &cua_host::relay::RegisterRequest {
                id: MACHINE.into(),
                name: "Owner desk".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    owner
        .patch(
            "owner-token",
            MACHINE,
            &cua_host::relay::MachinePatch {
                allow: Some(vec!["guest@example.com".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let env = fixtures::start_env(None, None).await;
    relay.tunnel(MACHINE, &env.url);
    relay.set_online(MACHINE, true, "0.0.0-mock");

    // Signed in as the guest.
    let mut h = Home::new();
    h.set("CUA_RELAY_URL", &relay.url);
    h.set("DOCKER_HOST", "unix:///nonexistent/cua-test-no-docker.sock");
    std::fs::create_dir_all(h.cua_home()).unwrap();
    std::fs::write(
        h.cua_home().join("credentials.json"),
        json!({
            "access_token": "guest-token",
            "expires_at": "2999-01-01T00:00:00Z",
            "token_type": "Bearer"
        })
        .to_string(),
    )
    .unwrap();
    let id = format!("relay:{MACHINE}");

    let mut m = Mcp::start(&h, &["--embedded", "mcp"]);
    m.call("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}})).await;
    m.notify("notifications/initialized").await;
    let r = m.tool("computer_screenshot", json!({"sandbox": id})).await;
    assert_eq!(r["content"][0]["type"], "image", "{r}");
    let r = m
        .tool("computer_click", json!({"sandbox": id, "x": 600, "y": 300}))
        .await;
    assert_eq!(r["isError"], false, "{r}");
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
    let r = m
        .tool(
            "computer_type",
            json!({"sandbox": id, "text": "guest-typed"}),
        )
        .await;
    assert_eq!(r["isError"], false, "{r}");
    let kb = env.mock.state.observed.keyboard.lock().unwrap().clone();
    assert!(kb.iter().any(|k| k.contains("guest-typed")), "{kb:?}");
    m.close().await;

    let o = h.run(&["--embedded", "do", "switch", &id]).await;
    o.ok();
    assert!(o.stdout.contains(&format!("✅ Switched to {id}")), "{o:?}");
    let o = h
        .run(&["--embedded", "do", "shell", "echo", "guest-ok"])
        .await;
    o.ok();
    assert!(o.stdout.starts_with("✅ guest-ok"), "{o:?}");
}
