//! One connection status, three surfaces (CUA-2268): `cua connect status
//! --json`, the `connect_status` MCP tool and the SDK call the app makes
//! (`AgentSetup.connection_status_json`) answer the same rows in every
//! state, against fake harness homes: a temporary HOME, fake agent
//! binaries, a fake `cua` that answers the MCP handshake, and a test
//! keychain for Keyvault. Nothing real is read, run or written.
#![cfg(unix)]

mod common;
use common::*;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const FAKE_CUA: &str = r#"#!/bin/sh
[ "$1" = "mcp" ] || exit 2
read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","serverInfo":{"name":"cua","version":"0"},"capabilities":{}}}'
"#;

fn script(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn agent(version: &str) -> String {
    format!("#!/bin/sh\n[ \"$1\" = --version ] && echo \"{version}\"\nexit 0\n")
}

/// The rows, without the time they were checked.
fn rows(v: &Value) -> Value {
    let mut v = v.clone();
    v.as_object_mut().unwrap().remove("checked_at_ms");
    for r in v["agents"].as_array_mut().unwrap() {
        r.as_object_mut().unwrap().remove("checked_at_ms");
    }
    v["agents"].clone()
}

/// `cli`'s rows with only the fields `like`'s rows have.
fn report_fields(cli: &Value, like: &Value) -> Value {
    let like = like.as_array().unwrap();
    Value::Array(
        cli.as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(
                |(i, row)| match (row.as_object(), like.get(i).and_then(Value::as_object)) {
                    (Some(row), Some(want)) => Value::Object(
                        row.iter()
                            .filter(|(k, _)| want.contains_key(*k))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    ),
                    _ => row.clone(),
                },
            )
            .collect(),
    )
}

async fn mcp_status(h: &Home) -> Value {
    let mut child = h.spawn(&["--embedded", "mcp"]);
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let stdin = child.stdin.as_mut().unwrap();
    for (id, method, params) in [
        (
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
        ),
        (
            2,
            "tools/call",
            json!({"name": "connect_status", "arguments": {}}),
        ),
    ] {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        stdin
            .write_all(format!("{req}\n").as_bytes())
            .await
            .unwrap();
        stdin.flush().await.unwrap();
        loop {
            let line = tokio::time::timeout(Duration::from_secs(60), lines.next_line())
                .await
                .expect("mcp reply timed out")
                .unwrap()
                .expect("mcp closed stdout");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == id {
                if id == 2 {
                    drop(child.stdin.take());
                    let _ = child.kill().await;
                    assert_ne!(v["result"]["isError"], true, "{v}");
                    let text = v["result"]["content"][0]["text"].as_str().unwrap();
                    return serde_json::from_str(text).unwrap();
                }
                break;
            }
        }
    }
    unreachable!()
}

fn sdk_status(h: &Home, bin: &Path) -> Value {
    let home = h.dir.path();
    let mut env = cua_agent_setup::HostEnv::isolated(home);
    env.path = std::env::split_paths(&h.env["PATH"]).collect();
    env.vars
        .insert("CUA_HOME".into(), h.env["CUA_HOME"].clone());
    env.vars
        .insert("XDG_CONFIG_HOME".into(), h.env["XDG_CONFIG_HOME"].clone());
    env.run_agent_clis = true;
    let s = cua_sdk::AgentSetup::with_env(env);
    let expected = cua_sdk::AgentMcpServer {
        name: String::new(),
        command: bin.join("cua").display().to_string(),
        args: vec!["mcp".into()],
        env: Default::default(),
    };
    serde_json::from_str(&s.connection_status_json(Some(expected), true)).unwrap()
}

async fn all_three(h: &Home, bin: &Path) -> Value {
    let cli = h.run(&["connect", "status", "--json"]).await;
    cli.ok();
    let mcp = rows(&mcp_status(h).await);
    // `--json` also keeps each agent's earlier status fields beside the
    // row (agents::status_json, for scripts); the rows themselves must agree.
    let cli = report_fields(&rows(&cli.json()), &mcp);
    let sdk = rows(&sdk_status(h, bin));
    assert_eq!(cli, mcp, "CLI and MCP disagree");
    assert_eq!(cli, sdk, "CLI and the app's SDK call disagree");
    cli
}

fn h_config(home: &Path) -> String {
    home.join(".config").display().to_string()
}

fn row<'a>(rows: &'a Value, id: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_mcp_and_app_report_the_same_connection_in_every_state() {
    let mut h = Home::new();
    let h_dir = h.dir.path().to_path_buf();
    let bin = h.dir.path().join("fakebin");
    let vault = h.dir.path().join("keychain");
    std::fs::create_dir_all(&vault).unwrap();
    script(&bin, "cua", FAKE_CUA);
    script(&bin, "codex", &agent("codex-cli 0.61.0"));
    script(&bin, "hermes", &agent("Hermes Agent v0.22.5"));
    script(&bin, "cursor-agent", &agent("2026.10.21"));
    h.set("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .set("CUA_AGENT_BIN_DIRS", "")
        .set("CUA_AGENT_APP_DIRS", "")
        .set("XDG_CONFIG_HOME", h_config(&h_dir))
        .set(
            "CUA_CREDENTIAL_STORE",
            format!("test-keychain:{}", vault.display()),
        );
    // The SDK side (this process) reads the same test keychain.
    // SAFETY: this test binary runs this one test; nothing else reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("CUA_TEST", "1");
        std::env::set_var(
            "CUA_CREDENTIAL_STORE",
            format!("test-keychain:{}", vault.display()),
        );
    }
    let home = h.dir.path();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".codex/auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"id_token":"x","access_token":"y","refresh_token":"rt"}}"#,
    )
    .unwrap();

    // Nothing connected yet.
    let r = all_three(&h, &bin).await;
    assert_eq!(row(&r, "codex")["state"], "not_connected");
    assert_eq!(row(&r, "claude-code")["state"], "not_installed");

    // Connect Codex with its sign-in, Hermes (never signed in) and Cursor.
    let out = h
        .run(&[
            "connect",
            "setup",
            "--agents",
            "codex,hermes,cursor",
            "--sign-in",
            "-y",
            "--json",
        ])
        .await;
    out.ok();
    let res = out.json();
    let codex = res["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"] == "codex")
        .unwrap();
    assert_eq!(
        codex["sign_in"]["imported"]["label"], "Codex · ChatGPT sign-in",
        "{res}"
    );
    let hermes = res["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"] == "hermes")
        .unwrap();
    assert_eq!(hermes["sign_in"]["refused"], "not_signed_in", "{res}");
    let r = all_three(&h, &bin).await;
    assert_eq!(row(&r, "codex")["state"], "connected", "{r}");
    assert_eq!(row(&r, "codex")["signin_import"], "ok");
    assert_eq!(row(&r, "hermes")["state"], "attention");
    assert_eq!(row(&r, "hermes")["problem"]["action"], "none");
    assert_eq!(row(&r, "cursor")["state"], "connected");
    assert_eq!(row(&r, "cursor")["signed_in"], "unknown");

    // Codex's config edited outside Cua to a cua that isn't there.
    let cfg = home.join(".codex/config.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(
        &cfg,
        text.replace(&bin.join("cua").display().to_string(), "/nowhere/cua"),
    )
    .unwrap();
    let r = all_three(&h, &bin).await;
    assert_eq!(row(&r, "codex")["mcp"], "broken");
    assert_eq!(row(&r, "codex")["problem"]["action"], "reconnect");

    // Reconnect, then Cursor's entry removed outside Cua.
    h.run(&[
        "connect",
        "setup",
        "--agents",
        "codex",
        "--sign-in",
        "-y",
        "--json",
    ])
    .await
    .ok();
    std::fs::write(home.join(".cursor/mcp.json"), "{}").unwrap();
    let r = all_three(&h, &bin).await;
    assert_eq!(row(&r, "codex")["state"], "connected");
    assert_eq!(row(&r, "cursor")["state"], "attention");

    // Codex uninstalled after connecting.
    std::fs::remove_file(bin.join("codex")).unwrap();
    let r = all_three(&h, &bin).await;
    assert_eq!(row(&r, "codex")["problem"]["action"], "disconnect");

    // Disconnect with --sign-in undoes both.
    let out = h
        .run(&[
            "connect",
            "remove",
            "--agents",
            "codex",
            "--sign-in",
            "-y",
            "--json",
        ])
        .await;
    out.ok();
    assert_eq!(out.json()["removed_sign_ins"], json!(["CODEX_AUTH_JSON"]));
    let r = all_three(&h, &bin).await;
    assert_eq!(row(&r, "codex")["state"], "not_installed");
    assert_eq!(row(&r, "codex")["signin_imported"], false);

    // The human table says what to do in one line per problem.
    let out = h.run(&["connect", "status"]).await;
    out.ok();
    assert!(out.stdout.contains("AGENT"), "{}", out.stdout);
    assert!(
        out.stdout
            .contains("Cursor: Cua was taken out of ~/.cursor/mcp.json. Reconnect adds it back."),
        "{}",
        out.stdout
    );
}
