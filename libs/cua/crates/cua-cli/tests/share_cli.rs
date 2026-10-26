//! `cua share` and the `share`, `share_list` and `share_stop` MCP tools
//! against a real daemon that is a signed-in relay host.
//!
//! The relay and the host's cua-spacesd are `share_relay.py` (libs/cua/tests/e2e/
//! cua-sdk/fixtures): just enough of both to put a request, with the
//! account the relay would verify, on the daemon's share gateway socket.
//! Everything on the daemon side is real. Unix only (the gateway is a Unix
//! socket); needs python3.
#![cfg(unix)]

mod common;

use common::Home;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const MACHINE: &str = "0123456789abcdef0123456789abcdef";

struct Rig {
    h: Home,
    relay: Child,
    relay_url: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.relay.kill();
        let _ = self.relay.wait();
    }
}

async fn rig() -> Option<Rig> {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipping: python3 is not installed");
        return None;
    }
    let mut h = Home::new();
    let home = h.cua_home();
    std::fs::create_dir_all(home.join("host")).unwrap();
    std::fs::write(
        home.join("credentials.json"),
        r#"{"access_token":"owner","refresh_token":null,"expires_at":"2099-01-01T00:00:00Z","token_type":"Bearer"}"#,
    )
    .unwrap();
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../tests/e2e/cua-sdk/fixtures/share_relay.py"
    );
    let mut relay = Command::new("python3")
        .arg(script)
        .arg("--gateway")
        .arg(home.join("share/gateway.sock"))
        .args(["--machine-id", MACHINE])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn share_relay.py");
    let mut line = String::new();
    BufReader::new(relay.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let relay_url = serde_json::from_str::<Value>(&line).unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(
        home.join("host/config.json"),
        json!({"mode": "relay", "relay_url": relay_url, "name": "share-test",
            "machine_id": MACHINE, "runner": "process", "driver_bin": "/bin/true",
            "share_desktop": false})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        home.join("host/host.json"),
        r#"{"owner":"acct-owner","owner_email":"owner@example.com"}"#,
    )
    .unwrap();
    h.set("CUA_RELAY_URL", relay_url.clone());
    h.run(&["daemon", "start", "--loopback", "off"]).await.ok();
    Some(Rig {
        h,
        relay,
        relay_url,
    })
}

/// GET with an optional bearer: (status, body).
fn get(url: &str, bearer: Option<&str>) -> (u16, String) {
    let rest = url.strip_prefix("http://").unwrap();
    let (addr, path) = rest.split_at(rest.find('/').unwrap());
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let auth = bearer
        .map(|b| format!("authorization: Bearer {b}\r\n"))
        .unwrap_or_default();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{auth}\r\n"
    )
    .unwrap();
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    let code = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (
        code,
        text.split_once("\r\n\r\n")
            .map(|x| x.1)
            .unwrap_or("")
            .to_string(),
    )
}

/// A tiny app: answers `app says <path>`.
fn app() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body = format!("app says {path}");
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cua_share_makes_lists_checks_and_stops_links() {
    let Some(r) = rig().await else { return };
    let port = app();
    let dir = r.h.dir.path().join("site");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("report.html"), "<h1>report</h1>").unwrap();

    // A file, relative to the working directory, private by default.
    let o = r.h.run(&["share", "site/report.html", "--json"]).await;
    o.ok();
    let f = o.json();
    let url = f["url"].as_str().unwrap().to_string();
    assert_eq!(f["access"], "private");
    assert_eq!(f["target"]["kind"], "file");
    assert!(
        url.starts_with(&format!("{}/m/{MACHINE}/share/", r.relay_url)),
        "{url}"
    );
    // Path mode (the stand-in relay): --json carries the notice.
    assert!(
        f["notice"]
            .as_str()
            .unwrap()
            .contains("Browser viewing of private links is coming"),
        "{f}"
    );
    assert_eq!(f["notices"].as_array().unwrap().len(), 1, "{f}");
    assert_eq!(get(&url, Some("owner")), (200, "<h1>report</h1>".into()));
    assert_eq!(get(&url, None).0, 401);
    assert_eq!(get(&url, Some("other")).0, 403);

    // The plain output is the link and nothing else.
    let o =
        r.h.run(&["share", &format!("localhost:{port}"), "--name", "dash"])
            .await;
    o.ok();
    let purl = o.stdout.trim().to_string();
    assert_eq!(purl.lines().count(), 1, "{o:?}");
    // The notice goes to stderr, never stdout.
    assert!(
        o.stderr.contains("Browser viewing of private links is coming; this link opens for tools and signed-in devices."),
        "{o:?}"
    );
    assert_eq!(
        get(&format!("{purl}x"), Some("owner")),
        (200, "app says /x".into())
    );

    let o = r.h.run(&["share", "ls", "--json"]).await;
    o.ok();
    assert_eq!(o.json().as_array().unwrap().len(), 2);
    let o = r.h.run(&["share", "ls"]).await;
    o.ok();
    assert!(
        o.stdout.contains("dash") && o.stdout.contains("private"),
        "{o:?}"
    );

    let id = f["id"].as_str().unwrap();
    let o = r.h.run(&["share", "check", id, "--json"]).await;
    o.ok();
    assert_eq!(o.json()["ok"], true);

    r.h.run(&["share", "stop", "dash"]).await.ok();
    assert_eq!(get(&purl, Some("owner")).0, 404);

    // Public: a folder, with a TTL; the TTL is bounded.
    let o =
        r.h.run(&["share", "site", "--public", "--ttl", "10m", "--json"])
            .await;
    o.ok();
    let p = o.json();
    assert!(p["url"].as_str().unwrap().contains("/share/pub/"));
    assert!(p["expires_at"].is_number());
    let (code, body) = get(p["url"].as_str().unwrap(), None);
    assert!(code == 200 && body.contains("report.html"), "{body}");
    let o = r.h.run(&["share", "site", "--public", "--ttl", "5s"]).await;
    assert_eq!(o.code, 2, "{o:?}");

    // Bad input is a usage error with a fix; --team and --public conflict.
    assert_eq!(r.h.run(&["share", "nowhere/at/all"]).await.code, 2);
    assert_eq!(
        r.h.run(&["share", "3000", "--team", "--public"]).await.code,
        2
    );
    assert_eq!(r.h.run(&["share", "stop", "share-nope"]).await.code, 3);

    let o = r.h.run(&["share", "stop", "all", "--json"]).await;
    o.ok();
    assert_eq!(o.json()["stopped"].as_array().unwrap().len(), 2);
    assert_eq!(get(p["url"].as_str().unwrap(), None).0, 404);

    // A public token is 160 random bits, and every create, open and stop is
    // in the audit log, which never holds the token itself.
    let token = p["url"]
        .as_str()
        .unwrap()
        .rsplit("/share/pub/")
        .next()
        .unwrap()
        .trim_end_matches('/')
        .to_string();
    assert_eq!(token.len(), 40, "{token}");
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()), "{token}");
    let audit = std::fs::read_to_string(r.h.cua_home().join("share/audit.jsonl")).unwrap();
    for action in ["create", "open", "stop"] {
        assert!(
            audit.contains(&format!("\"action\":\"{action}\"")),
            "no {action} line in the audit log:\n{audit}"
        );
    }
    assert!(
        !audit.contains(&token),
        "the audit log holds a public token"
    );
    r.h.run(&["daemon", "stop"]).await.ok();
}

struct Mcp {
    child: tokio::process::Child,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    id: u64,
}

impl Mcp {
    async fn start(h: &Home) -> Self {
        let mut child = h.spawn(&["mcp"]);
        let stdout = child.stdout.take().unwrap();
        let mut m = Self {
            child,
            lines: tokio::io::BufReader::new(stdout).lines(),
            id: 0,
        };
        m.call(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
        )
        .await;
        let n = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        m.child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{n}\n").as_bytes())
            .await
            .unwrap();
        m
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
        let line = tokio::time::timeout(Duration::from_secs(60), self.lines.next_line())
            .await
            .expect("mcp reply timed out")
            .unwrap()
            .expect("mcp closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    async fn tool(&mut self, name: &str, args: Value) -> Value {
        let r = self
            .call("tools/call", json!({"name": name, "arguments": args}))
            .await["result"]
            .clone();
        if r["isError"] == true {
            return r;
        }
        serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap_or(r)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_share_checks_the_link_before_returning_it() {
    let Some(r) = rig().await else { return };
    let port = app();
    let mut m = Mcp::start(&r.h).await;
    let tools = m.call("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for want in ["share", "share_list", "share_stop"] {
        assert!(names.contains(&want), "{want} missing: {names:?}");
    }
    let share = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "share")
        .unwrap();
    assert!(
        share["description"]
            .as_str()
            .unwrap()
            .contains("never hand the user localhost")
    );

    // A working app: the link, and a check that opened it.
    let s = m.tool("share", json!({"target": port.to_string()})).await;
    assert!(s["url"].as_str().unwrap().contains("/share/"), "{s}");
    assert_eq!(s["access"], "private");
    assert_eq!(s["check"]["ok"], true, "{s}");
    assert_eq!(s["check"]["status"], 200);
    assert_eq!(s["check"]["via"], "link");
    assert!(s.get("warning").is_none(), "{s}");
    assert!(
        s["notices"][0]
            .as_str()
            .unwrap()
            .contains("Browser viewing"),
        "{s}"
    );
    assert_eq!(get(s["url"].as_str().unwrap(), Some("owner")).0, 200);

    // Nothing listening: the check says so, in the result, with a warning.
    let bad = m.tool("share", json!({"target": "1"})).await;
    assert_eq!(bad["check"]["ok"], false, "{bad}");
    assert!(
        bad["warning"].as_str().unwrap().contains("Do not send"),
        "{bad}"
    );

    // A file with a TTL, public.
    let f = r.h.dir.path().join("note.txt");
    std::fs::write(&f, "hello").unwrap();
    let p = m
        .tool(
            "share",
            json!({"target": f.display().to_string(), "access": "public", "ttl": "10m"}),
        )
        .await;
    assert_eq!(p["check"]["ok"], true, "{p}");
    assert!(p["expires_at"].is_number());
    assert_eq!(get(p["url"].as_str().unwrap(), None), (200, "hello".into()));

    let l = m.tool("share_list", json!({})).await;
    assert_eq!(l["shares"].as_array().unwrap().len(), 3);
    let e = m.tool("share", json!({"target": "./does-not-exist"})).await;
    assert_eq!(e["isError"], true, "{e}");
    let e = m
        .tool("share", json!({"target": "3000", "access": "everyone"}))
        .await;
    assert_eq!(e["isError"], true, "{e}");

    let id = p["id"].as_str().unwrap();
    assert_eq!(
        m.tool("share_stop", json!({"id": id})).await["stopped"],
        json!([id])
    );
    assert_eq!(get(p["url"].as_str().unwrap(), None).0, 404);
    assert_eq!(
        m.tool("share_stop", json!({"id": "all"})).await["stopped"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(m.tool("share_list", json!({})).await["shares"], json!([]));
    drop(m);
    r.h.run(&["daemon", "stop"]).await.ok();
}
