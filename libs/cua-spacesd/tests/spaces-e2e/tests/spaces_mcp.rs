// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! MCP conformance: the Spaces server over stdio framing and streamable
//! HTTP, against a real in-process spacesd.

mod common;

use common::{TOKEN, driver, spaces};
use cua_spaces::mcp::McpServer;
use cua_spaces_e2e::contract_tool_names;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// A stdio client over an in-memory pipe.
struct Stdio {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    read: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    next_id: u64,
}

impl Stdio {
    fn start(server: McpServer) -> Self {
        let (client, srv) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(srv);
        tokio::spawn(cua_spaces::mcp::stdio::serve(server, sr, sw));
        let (cr, cw) = tokio::io::split(client);
        Stdio {
            write: cw,
            read: BufReader::new(cr),
            next_id: 1,
        }
    }

    async fn send_raw(&mut self, bytes: &[u8]) {
        self.write.write_all(bytes).await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(60), self.read.read_line(&mut line))
            .await
            .expect("a response within 60 s")
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_raw(format!("{msg}\n").as_bytes()).await;
        let resp = self.recv().await;
        assert_eq!(resp["id"], id, "{resp}");
        resp
    }

    async fn call(&mut self, tool: &str, args: Value) -> Value {
        let r = self
            .request("tools/call", json!({"name": tool, "arguments": args}))
            .await;
        r["result"].clone()
    }
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn body(result: &Value) -> Value {
    serde_json::from_str(&text(result)).unwrap_or(Value::Null)
}

#[tokio::test]
async fn stdio_speaks_mcp_and_publishes_the_contract() {
    let reg = tempfile::tempdir().unwrap();
    let mut c = Stdio::start(McpServer::new(spaces(reg.path())));

    let init = c
        .request("initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}))
        .await;
    assert_eq!(
        init["result"]["protocolVersion"], "2024-11-05",
        "a supported revision is echoed"
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "cua-spaces");
    let init = c
        .request("initialize", json!({"protocolVersion": "1999-01-01"}))
        .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    // A notification gets no response; the next request's reply is next.
    c.send_raw(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await;
    assert_eq!(c.request("ping", json!({})).await["result"], json!({}));

    // tools/list is the manifest, schema for schema, in order.
    let list = c.request("tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let manifest = cua_spaces::contract::tools();
    assert_eq!(tools.len(), manifest.len());
    for (got, want) in tools.iter().zip(&manifest) {
        assert_eq!(got["name"], want.name);
        assert_eq!(got["inputSchema"], want.input_schema, "{}", want.name);
        assert_eq!(
            got["annotations"]["readOnlyHint"],
            want.annotations.read_only
        );
    }
    let checked_in: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../cua/spaces-contract/manifest.json"
        ))
        .unwrap(),
    )
    .unwrap();
    // The checked-in manifest is the contract, name for name.
    assert_eq!(checked_in["tool_count"], manifest.len());
    let checked_in_names: Vec<&str> = checked_in["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(checked_in_names, contract_tool_names());

    // Protocol errors.
    let unknown = c
        .request("tools/call", json!({"name": "rm_rf", "arguments": {}}))
        .await;
    assert_eq!(unknown["error"]["code"], -32601);
    let nope = c.request("resources/list", json!({})).await;
    assert_eq!(nope["error"]["code"], -32601);
    c.send_raw(b"{not json\n").await;
    assert_eq!(c.recv().await["error"]["code"], -32700);

    // A message split across writes is reassembled (the framer bug).
    let msg = json!({"jsonrpc": "2.0", "id": 99, "method": "ping"}).to_string() + "\n";
    let (a, b) = msg.as_bytes().split_at(10);
    c.send_raw(a).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    c.send_raw(b).await;
    assert_eq!(c.recv().await["id"], 99);

    // Tool errors are results with isError, carrying a machine kind.
    let bad = c
        .call("space_bash", json!({"space": "space://direct/127.0.0.1:1"}))
        .await;
    assert_eq!(bad["isError"], true);
    assert_eq!(
        bad["structuredContent"]["error"]["kind"],
        "invalid_argument"
    );
    let missing = c
        .call("space_bash", json!({"space": "nobody", "command": "true"}))
        .await;
    assert_eq!(missing["structuredContent"]["error"]["kind"], "not_found");

    // Host-only tools work without a Space.
    let caps = body(&c.call("agent_capabilities", json!({})).await);
    assert_eq!(caps["statuses"].as_array().unwrap().len(), 5);
    let hs = body(&c.call("hotspot_status", json!({})).await);
    assert_eq!(hs["hotspots"], json!([]));
    let teleport = c.call("teleport_manifest", json!({"app": "firefox"})).await;
    assert_eq!(
        teleport["structuredContent"]["error"]["kind"],
        "host_capability_missing"
    );
    let fleet = c.call("create_space", json!({"on": "cloud"})).await;
    assert_eq!(
        fleet["structuredContent"]["error"]["kind"],
        "host_capability_missing"
    );
}

#[tokio::test]
async fn stdio_tools_drive_a_real_space() {
    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let mut c = Stdio::start(McpServer::new(s.clone()));

    let added = body(
        &c.call(
            "add_space",
            json!({"url": d.url, "token": TOKEN, "name": "inproc"}),
        )
        .await,
    );
    let id = added["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("direct:"));
    let space = s.space(&id).await.unwrap();
    d.confine(&space).await;

    let listed = body(&c.call("list_spaces", json!({})).await);
    assert_eq!(listed[0]["id"], id);
    assert_eq!(listed[0]["name"], "inproc");
    // One phase vocabulary across providers, and the OS from the handshake.
    assert_eq!(added["phase"], "ready");
    assert_eq!(listed[0]["phase"], "ready");
    assert!(
        ["linux", "macos", "windows"].contains(&listed[0]["os"].as_str().unwrap_or("")),
        "{listed}"
    );

    // The name works wherever an id does.
    let out = c
        .call(
            "space_bash",
            json!({"space": "inproc", "command": "echo mcp-$((6*7))"}),
        )
        .await;
    assert_eq!(text(&out), "mcp-42\n[exit 0]");

    let path = d.home.join("w/hello.txt");
    let w = c
        .call(
            "space_write",
            json!({"space": id, "path": path, "content": "hi $USER"}),
        )
        .await;
    assert!(text(&w).starts_with("wrote "), "{w}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi $USER");

    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("doc.txt"), "sent").unwrap();
    let sent = body(
        &c.call(
            "send_file",
            json!({"space": id, "path": src.path().join("doc.txt"), "target_directory": "~/Downloads/mcp"}),
        )
        .await,
    );
    assert_eq!(sent["verified"], true);
    assert_eq!(
        std::fs::read_to_string(d.downloads.join("mcp/doc.txt")).unwrap(),
        "sent"
    );
    let outside = c
        .call(
            "send_file",
            json!({"space": id, "path": src.path().join("doc.txt"), "target_directory": "/etc"}),
        )
        .await;
    assert_eq!(
        outside["structuredContent"]["error"]["kind"],
        "invalid_argument"
    );

    let tools = body(&c.call("list_tools", json!({"space": id})).await);
    assert_eq!(tools["tools"][0]["description"], "Fake screen size.");
    let full = body(
        &c.call("list_tools", json!({"space": id, "name": "screen"}))
            .await,
    );
    assert!(full["tools"][0]["inputSchema"].is_object());
    let called = c
        .call("call_tool", json!({"space": id, "tool": "get_screen_size"}))
        .await;
    assert_eq!(called["isError"], false);
    assert_eq!(called["structuredContent"]["width"], 1280);

    // Capability-gated: this driver has no desktop provider.
    for (tool, args) in [
        ("stream_endpoint", json!({"space": id})),
        ("list_space_windows", json!({"space": id})),
        ("show_space_pip", json!({"space": id})),
    ] {
        let r = c.call(tool, args).await;
        assert_eq!(
            r["structuredContent"]["error"]["kind"], "capability_missing",
            "{tool}: {r}"
        );
    }

    let removed = body(&c.call("remove_space", json!({"space": id})).await);
    assert_eq!(removed["removed"]["id"], id);
    assert_eq!(body(&c.call("list_spaces", json!({})).await), json!([]));
}

/// Minimal HTTP/1.1 client (one request per connection).
async fn http(
    port: u16,
    method: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let mut req = format!(
        "{method} /mcp HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-type: application/json\r\n\
         accept: application/json, text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(30),
        s.take(1 << 20).read_to_end(&mut out),
    )
    .await
    .unwrap()
    .unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head.to_lowercase(), rest.to_string())
}

#[tokio::test]
async fn streamable_http_with_sessions_and_a_bearer() {
    let reg = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(cua_spaces::mcp::http::serve(
        McpServer::new(spaces(reg.path())),
        listener,
        Some("daemon-secret".into()),
    ));
    let auth = ("authorization", "Bearer daemon-secret");
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;

    assert_eq!(http(port, "POST", &[], init).await.0, 401);
    assert_eq!(
        http(port, "POST", &[("authorization", "Bearer nope")], init)
            .await
            .0,
        401
    );

    let (status, head, body) = http(port, "POST", &[auth], init).await;
    assert_eq!(status, 200);
    let session = head
        .lines()
        .find_map(|l| l.strip_prefix("mcp-session-id: "))
        .expect("a session id")
        .trim()
        .to_string();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");

    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    let (status, _, body) = http(port, "POST", &[auth, ("mcp-session-id", &session)], list).await;
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    let names: Vec<&str> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, contract_tool_names());

    let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    assert_eq!(
        http(port, "POST", &[auth, ("mcp-session-id", &session)], note)
            .await
            .0,
        202
    );
    assert_eq!(
        http(port, "POST", &[auth, ("mcp-session-id", "stale")], list)
            .await
            .0,
        404
    );
    assert_eq!(http(port, "GET", &[auth], "").await.0, 405);
    assert_eq!(
        http(port, "DELETE", &[auth, ("mcp-session-id", &session)], "")
            .await
            .0,
        204
    );
    assert_eq!(
        http(port, "POST", &[auth, ("mcp-session-id", &session)], list)
            .await
            .0,
        404
    );
}
