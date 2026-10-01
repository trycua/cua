// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua volume` against the embedded runtime's local drive in a temporary
//! HOME. Approvals and grants run with the debug build's presence escape
//! hatch (`CUA_ENV_SKIP_SENSITIVE_AUTH=1`), so no Touch ID prompt appears.

mod common;
use common::*;

fn home() -> Home {
    let mut h = Home::new();
    h.set("CUA_DAEMON_NO_RELAY", "1")
        .set("CUA_NO_DAEMON_AUTOSTART", "1")
        .set("CUA_ENV_SKIP_SENSITIVE_AUTH", "1")
        .set("CUA_ENV_TEST_SANDBOX", "1");
    h
}

async fn put(h: &Home, path: &str, body: &str, extra: &[&str]) -> Out {
    let src = h.dir.path().join(format!("src-{}", rand_name()));
    std::fs::write(&src, body).unwrap();
    let mut args = vec![
        "--embedded",
        "--json",
        "drive",
        "put",
        path,
        src.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    h.run(&args).await
}

fn rand_name() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[tokio::test]
async fn files_versions_and_history() {
    let h = home();
    let w = put(&h, "public/rules.md", "be kind", &[]).await.ok().json();
    assert_eq!(w["key"], "public/rules.md");
    put(&h, "public/rules.md", "be very kind", &[]).await.ok();
    let stale = put(
        &h,
        "public/rules.md",
        "x",
        &["--if-etag", w["etag"].as_str().unwrap()],
    )
    .await;
    assert_ne!(stale.code, 0, "{stale:?}");
    let cat = h
        .run(&["--embedded", "drive", "cat", "public/rules.md"])
        .await;
    assert_eq!(cat.ok().stdout, "be very kind");
    let ls = h
        .run(&["--embedded", "--json", "drive", "ls", "public/"])
        .await
        .ok()
        .json();
    assert_eq!(ls["entries"][0]["path"], "public/rules.md");
    let hist = h
        .run(&[
            "--embedded",
            "--json",
            "drive",
            "history",
            "public/rules.md",
        ])
        .await
        .ok()
        .json();
    let versions = hist["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    let first = versions[1]["version"].as_str().unwrap();
    h.run(&["--embedded", "drive", "restore", "public/rules.md", first])
        .await
        .ok();
    let cat = h
        .run(&["--embedded", "drive", "cat", "public/rules.md"])
        .await;
    assert_eq!(cat.ok().stdout, "be kind");
    h.run(&["--embedded", "drive", "rm", "public/rules.md"])
        .await
        .ok();
    let gone = h
        .run(&["--embedded", "drive", "cat", "public/rules.md"])
        .await;
    assert_ne!(gone.code, 0);
}

#[tokio::test]
async fn an_agent_needs_a_grant_for_another_agents_folder() {
    let h = home();
    put(&h, "agents/writer/outputs/draft.md", "draft", &[])
        .await
        .ok();
    let as_rs = ["--as", "agent:researcher", "--space", "local:lab"];
    let mut read = vec![
        "--embedded",
        "drive",
        "cat",
        "agents/writer/outputs/draft.md",
    ];
    read.extend_from_slice(&as_rs);
    let refused = h.run(&read).await;
    assert_ne!(refused.code, 0);
    assert!(refused.stderr.contains("may not read"), "{refused:?}");
    // The agent's own home and its Space's folder are writable.
    put(&h, "agents/researcher/notes.md", "mine", &as_rs)
        .await
        .ok();
    put(&h, "spaces/local-lab/out.txt", "out", &as_rs)
        .await
        .ok();
    // Secrets never land in an agent home.
    let leak = format!("key {}{}", "AKIA", "ABCDEFGHIJKLMNOP");
    let blocked = put(&h, "agents/researcher/leak.md", &leak, &as_rs).await;
    assert_ne!(blocked.code, 0);
    assert!(blocked.stderr.contains("secret detected"), "{blocked:?}");
    assert!(!blocked.stderr.contains("ABCDEFGHIJKLMNOP"));
    // Grant, read, revoke, refused again.
    let g = h
        .run(&[
            "--embedded",
            "--json",
            "drive",
            "grant",
            "agent:researcher",
            "agents/writer/outputs/",
        ])
        .await
        .ok()
        .json();
    let id = g["grants"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(h.run(&read).await.ok().stdout, "draft");
    let grants = h
        .run(&["--embedded", "--json", "drive", "grants"])
        .await
        .ok()
        .json();
    assert_eq!(grants["grants"].as_array().unwrap().len(), 1);
    h.run(&["--embedded", "drive", "revoke", &id]).await.ok();
    assert_ne!(h.run(&read).await.code, 0);
    let audit = h
        .run(&["--embedded", "--json", "drive", "audit", "--limit", "100"])
        .await
        .ok()
        .json();
    assert_eq!(audit["verified"], true);
    let actions: Vec<&str> = audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["action"].as_str())
        .collect();
    for want in ["denied", "grant", "read", "revoke", "secret_blocked"] {
        assert!(actions.contains(&want), "{want} in {actions:?}");
    }
    let requests = h.run(&["--embedded", "drive", "requests"]).await;
    assert!(requests.ok().stdout.contains("no requests waiting"));
}

#[tokio::test]
async fn config_never_holds_keys() {
    let h = home();
    let show = h
        .run(&["--embedded", "--json", "drive", "config", "show"])
        .await
        .ok()
        .json();
    assert_eq!(show["backend"], "fs");
    let bad = h
        .run(&["--embedded", "drive", "config", "set", "--backend", "s3"])
        .await;
    assert_ne!(bad.code, 0, "s3 needs a bucket");
    h.run(&[
        "--embedded",
        "drive",
        "config",
        "set",
        "--backend",
        "s3",
        "--endpoint",
        "http://127.0.0.1:9",
        "--bucket",
        "b",
        "--path-style",
    ])
    .await
    .ok();
    let mut child = h.spawn(&["--embedded", "drive", "config", "set-keys"]);
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(b"AKTESTKEYID\nSECRETVALUE123\n")
            .await
            .unwrap();
    }
    let o = child.wait_with_output().await.unwrap();
    assert!(o.status.success(), "{o:?}");
    let show = h
        .run(&["--embedded", "--json", "drive", "config", "show"])
        .await
        .ok()
        .json();
    assert_eq!(show["backend"], "s3");
    assert_eq!(show["s3_keys_saved"], true);
    let file = std::fs::read_to_string(h.cua_home().join("volume/config.json")).unwrap();
    assert!(
        !file.contains("SECRETVALUE123") && !file.contains("AKTESTKEYID"),
        "{file}"
    );
    assert!(!show.to_string().contains("SECRETVALUE123"));
}
