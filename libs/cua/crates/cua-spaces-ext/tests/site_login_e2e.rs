// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live "log me into this site" against a real Linux container Space.
//! Opt-in: without `CUA_SITE_LOGIN_E2E_URL` every test prints why and
//! passes. Run through `tests/e2e/run-site-login-e2e.sh`, which starts the
//! container, serves the login site inside it and exports:
//!
//! - `CUA_SITE_LOGIN_E2E_URL`, `CUA_SITE_LOGIN_E2E_TOKEN`: the spacesd;
//! - `CUA_SITE_LOGIN_E2E_USER`, `CUA_SITE_LOGIN_E2E_PASSWORD`: what the
//!   site accepts (random per run; written into the synthetic profile only);
//! - `CUA_SITE_LOGIN_E2E_EVIDENCE`: where timings and the audit tail go.
//!
//! Host safety: the Chrome profile is synthetic, in a temp home read through
//! a `FakeHost` (its Keychain is a scripted answer), the Keyvault is a
//! passphrase vault in a temp dir, and the user's approval is a fake
//! presence gate. Nothing reads the real Chrome profile, Keychain or ~/.cua.

#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_keyvault::CallerIdentity;
use cua_keyvault::broker::{FakePresence, InitRequest, PasswordImportSpec};
use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::daemon::keyvault::{DaemonBackend, Keyvault};
use cua_spaces_ext::teleport::AppSessions;
use cua_teleport::passwords::{encrypt_for_tests, write_login_data_for_tests};
use cua_teleport::{FakeHost, HostOutput, Platform};
use serde_json::{Value, json};

const SITE: &str = "http://login.example.test:8000/";
const SAFE_STORAGE: &str = "synthetic-safe-storage";

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

struct Rig {
    _reg: tempfile::TempDir,
    _vault: tempfile::TempDir,
    _home: tempfile::TempDir,
    spaces: Spaces,
    kv: Keyvault,
    mcp: McpServer,
    cua: CallerIdentity,
    space: String,
    password: String,
    user: String,
    log: Vec<Value>,
}

impl Rig {
    async fn tool(&self, name: &str, args: Value) -> (bool, Value) {
        let out = self.mcp.call(name, args).await;
        let v = out.to_result();
        assert!(
            !v.to_string().contains(&self.password),
            "{name} leaked the password"
        );
        let body = match v.get("structuredContent") {
            Some(s) => s.clone(),
            None => v["content"][0]["text"]
                .as_str()
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null),
        };
        (out.is_error, body)
    }

    /// The site's own record, read inside the Space.
    async fn site_status(&self) -> Value {
        let s = self.spaces.space(&self.space).await.unwrap();
        let out = s
            .bash(
                "curl -s http://login.example.test:8000/status",
                Duration::from_secs(30),
            )
            .await
            .unwrap();
        serde_json::from_str(out.stdout.trim()).unwrap_or(Value::Null)
    }

    async fn driver(&self, tool: &str, arguments: Value) -> Value {
        let (err, v) = self
            .tool(
                "call_tool",
                json!({"space": self.space, "tool": tool, "arguments": arguments}),
            )
            .await;
        assert!(!err, "{tool}: {v}");
        v
    }
}

async fn rig() -> Option<Rig> {
    let url = env("CUA_SITE_LOGIN_E2E_URL")?;
    let token = env("CUA_SITE_LOGIN_E2E_TOKEN").unwrap_or_default();
    let user = env("CUA_SITE_LOGIN_E2E_USER").expect("CUA_SITE_LOGIN_E2E_USER");
    let password = env("CUA_SITE_LOGIN_E2E_PASSWORD").expect("CUA_SITE_LOGIN_E2E_PASSWORD");
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&url, Some(token), Some("login-e2e".into()))
        .await
        .unwrap();
    // A synthetic Chrome profile, encrypted the way Chrome on this host
    // does (macOS: the Safe Storage secret, scripted; Linux: v10).
    let home = tempfile::tempdir().unwrap();
    let platform = Platform::current();
    let profile = home
        .path()
        .join(cua_teleport::layout::chrome::user_data_dir_for(platform))
        .join("Default");
    let encrypted = match platform {
        Platform::MacOS => encrypt_for_tests(b"v10", SAFE_STORAGE.as_bytes(), 1003, &password),
        _ => encrypt_for_tests(b"v10", b"peanuts", 1, &password),
    };
    write_login_data_for_tests(&profile, &[(SITE, &user, encrypted)]).unwrap();
    let host = Arc::new(FakeHost::new().with_home(home.path()).with_responder(|c| {
        if c.args.iter().any(|a| a == "-w") {
            Ok(HostOutput::ok(format!("{SAFE_STORAGE}\n").into_bytes()))
        } else {
            Ok(HostOutput::ok(b"    \"acct\"<blob>=\"Chrome\"\n".to_vec()))
        }
    }));
    let backend = Arc::new(
        DaemonBackend::new(spaces.clone(), Arc::new(AppSessions::builtin())).with_host(host),
    );
    let vault = tempfile::tempdir().unwrap();
    let kv = Keyvault::new(
        vault.path().join("keyvault"),
        backend,
        Arc::new(FakePresence::new(true)),
        false,
    )
    .unwrap();
    spaces.set_site_login_broker(Some(kv.site_login_broker()));
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    kv.broker()
        .init(
            &cua,
            InitRequest {
                os_protector: false,
                passphrase: Some("correct horse battery".into()),
                recovery_key: false,
            },
        )
        .await
        .unwrap();
    let t = Instant::now();
    let items = kv
        .broker()
        .import_passwords(
            &cua,
            PasswordImportSpec {
                app: "chrome".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(items.saved, 1);
    let import_ms = t.elapsed().as_millis();
    let mcp = McpServer::new(spaces.clone());
    Some(Rig {
        _reg: reg,
        _vault: vault,
        _home: home,
        spaces,
        kv,
        mcp,
        cua,
        space: info.id,
        password,
        user,
        log: vec![json!({"step": "import_passwords", "ms": import_ms})],
    })
}

fn evidence(name: &str, v: &Value) {
    if let Some(dir) = env("CUA_SITE_LOGIN_E2E_EVIDENCE") {
        let _ = std::fs::write(
            std::path::Path::new(&dir).join(name),
            serde_json::to_vec_pretty(v).unwrap(),
        );
    }
}

/// One test so the three scenarios share one container and run in order.
/// It runs on a thread with a large stack: the unoptimized futures of a
/// real Space session are deep.
#[test]
fn e2e_site_login_needs_approval_and_signs_in_without_showing_the_password() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(16 << 20)
                .enable_all()
                .build()
                .unwrap()
                .block_on(scenarios())
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn scenarios() {
    let Some(mut r) = rig().await else {
        eprintln!("skipped: set CUA_SITE_LOGIN_E2E_URL (run tests/e2e/run-site-login-e2e.sh)");
        return;
    };
    let broker = r.kv.broker();
    assert_eq!(r.site_status().await["attempts"], 0);

    // 1. Declined: nothing is typed.
    let t = Instant::now();
    let (err, v) = r
        .tool(
            "request_site_login",
            json!({"space": r.space, "url": SITE, "agent": "ada"}),
        )
        .await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "pending");
    let id = v["request_id"].as_str().unwrap().to_string();
    r.log
        .push(json!({"step": "request (pending)", "ms": t.elapsed().as_millis()}));
    broker.deny(&r.cua, &id).await.unwrap();
    let (err, v) = r
        .tool(
            "request_site_login",
            json!({"space": r.space, "url": SITE, "request_id": id, "wait_secs": 5}),
        )
        .await;
    assert!(err, "{v}");
    assert_eq!(v["error"]["kind"], "login_refused", "{v}");
    let s = r.site_status().await;
    assert_eq!(s["attempts"], 0, "declined: the site saw no attempt: {s}");
    assert_eq!(s["signed_in"], Value::Null);
    println!("declined: refused, site attempts = 0");

    // 2. Approved, in the Keyvault's own browser in the Space.
    let (_, v) = r
        .tool(
            "request_site_login",
            json!({"space": r.space, "url": SITE, "agent": "ada"}),
        )
        .await;
    let id = v["request_id"].as_str().unwrap().to_string();
    let t = Instant::now();
    broker
        .approve(&r.cua, &id, Default::default())
        .await
        .unwrap();
    let (err, v) = r
        .tool(
            "request_site_login",
            json!({"space": r.space, "url": SITE, "request_id": id, "wait_secs": 30}),
        )
        .await;
    let fill_ms = t.elapsed().as_millis();
    assert!(!err, "{v}");
    assert_eq!(v["status"], "filled", "{v}");
    assert_eq!(v["submitted"], true, "{v}");
    assert_eq!(v["username_hint"], "a***@example.test");
    r.log
        .push(json!({"step": "approve -> signed in (own browser)", "ms": fill_ms}));
    let s = r.site_status().await;
    assert_eq!(s["signed_in"].as_str(), Some(r.user.as_str()), "{s}");
    assert_eq!(s["attempts"], 1, "{s}");
    println!("approved: signed in via the Keyvault's browser in {fill_ms} ms");
    // The page itself shows it, in the tab the tool returned.
    let state = r
        .driver(
            "get_browser_state",
            json!({"session": v["session"], "target_id": v["target_id"],
                   "tab_id": v["tab_id"], "snapshot_format": "semantic_v2"}),
        )
        .await;
    assert!(
        state.to_string().contains("Welcome"),
        "the tab shows the signed-in page"
    );

    // 3. The agent's own tab: open the site like an agent does, then ask.
    let session = "agent-ada";
    let prepared = r
        .driver(
            "browser_prepare",
            json!({"session": session, "allow_launch": true,
                   "profile": {"mode": "isolated_named", "name": "agent-ada"}}),
        )
        .await;
    let text = prepared.to_string();
    let pid: i64 =
        serde_json::from_str::<Value>(prepared["content"][0]["text"].as_str().unwrap_or("{}"))
            .ok()
            .and_then(|x| x["prepared_pid"].as_i64())
            .or_else(|| prepared["prepared_pid"].as_i64())
            .unwrap_or_else(|| panic!("no pid in {text}"));
    let mut window = None;
    for _ in 0..40 {
        let w = r.driver("list_windows", json!({"session": session})).await;
        window = w["windows"].as_array().and_then(|ws| {
            ws.iter()
                .find(|x| x["pid"].as_i64() == Some(pid))
                .and_then(|x| x["window_id"].as_i64())
        });
        if window.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let bound = r
        .driver(
            "get_browser_state",
            json!({"session": session, "pid": pid, "window_id": window.expect("agent window")}),
        )
        .await;
    let target = bound["target_id"].as_str().unwrap().to_string();
    let tab = bound["tabs"][0]["tab_id"].as_str().unwrap().to_string();
    r.driver(
        "browser_navigate",
        json!({"session": session, "target_id": target, "tab_id": tab, "url": SITE}),
    )
    .await;
    let (_, v) = r
        .tool(
            "request_site_login",
            json!({"space": r.space, "url": SITE, "agent": "ada",
                   "session": session, "target_id": target, "tab_id": tab}),
        )
        .await;
    let id = v["request_id"].as_str().unwrap().to_string();
    let t = Instant::now();
    broker
        .approve(&r.cua, &id, Default::default())
        .await
        .unwrap();
    let (err, v) = r
        .tool(
            "request_site_login",
            json!({"space": r.space, "url": SITE, "request_id": id, "wait_secs": 30,
                   "session": session, "target_id": target, "tab_id": tab}),
        )
        .await;
    let own_ms = t.elapsed().as_millis();
    assert!(!err, "{v}");
    assert_eq!(v["status"], "filled", "{v}");
    assert_eq!(
        v["tab_id"].as_str(),
        Some(tab.as_str()),
        "filled the agent's tab"
    );
    let s = r.site_status().await;
    assert_eq!(s["attempts"], 2, "{s}");
    r.log
        .push(json!({"step": "approve -> signed in (agent's tab)", "ms": own_ms}));
    println!("approved: signed in the agent's own tab in {own_ms} ms");

    // The audit tells the story, without the password.
    let audit = broker.audit_tail(&r.cua, 200).await.unwrap();
    let text = serde_json::to_string(&audit).unwrap();
    assert!(!text.contains(&r.password), "audit has no password");
    let fills = audit
        .iter()
        .filter(|e| e.event.kind == "login.fill" && e.event.decision == "allow")
        .count();
    assert_eq!(fills, 2);
    evidence("timings.json", &json!(r.log));
    evidence(
        "audit.json",
        &serde_json::to_value(
            audit
                .iter()
                .filter(|e| e.event.kind.starts_with("login."))
                .map(|e| {
                    json!({"kind": e.event.kind, "decision": e.event.decision,
                                "target": e.event.target, "detail": e.event.detail})
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    );
    println!("timings: {}", json!(r.log));
}
