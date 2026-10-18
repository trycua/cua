//! `cua agent harnesses` reports the readiness the Spaces server reports
//! (`agent_capabilities`): a key saved with `cua agent keys set` makes
//! claude-code `auth: ok`. A temporary HOME and the test credential store
//! (files in a temp dir); no OS keychain, no daemon.

mod common;
use common::*;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const DUMMY: &str = "sk-ant-test-readiness-0000-zq9e";

/// A home where no provider key comes from the environment.
fn home() -> Home {
    let mut h = Home::new();
    let vault = h.dir.path().join("vault");
    h.set(
        "CUA_CREDENTIAL_STORE",
        format!("test-keychain:{}", vault.display()),
    );
    for k in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
    ] {
        // Empty counts as unset.
        h.set(k, "");
    }
    h
}

fn auth(o: &Out, id: &str) -> (String, bool) {
    let v = o.json();
    let h = v
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {v}"))
        .clone();
    (
        h["auth"].as_str().unwrap_or("").to_string(),
        h["ready"] == true,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn harnesses_count_the_saved_key_like_agent_capabilities() {
    let h = home();
    let o = h.run(&["--embedded", "--json", "agent", "harnesses"]).await;
    o.ok();
    assert_eq!(auth(&o, "claude-code"), ("missing".into(), false), "{o:?}");
    assert_eq!(auth(&o, "openai-codex"), ("missing".into(), false));

    let mut child = h.spawn(&["--embedded", "agent", "keys", "set", "anthropic"]);
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(DUMMY.as_bytes()).await.unwrap();
    drop(stdin);
    let set = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let set_out =
        String::from_utf8_lossy(&set.stdout).to_string() + &String::from_utf8_lossy(&set.stderr);
    assert!(set.status.success(), "{set_out}");
    assert!(!set_out.contains(DUMMY), "the key is never printed");

    let o = h.run(&["--embedded", "--json", "agent", "harnesses"]).await;
    o.ok();
    assert_eq!(auth(&o, "claude-code"), ("ok".into(), true), "{o:?}");
    assert_eq!(auth(&o, "openai-codex"), ("missing".into(), false));
    assert!(!o.stdout.contains(DUMMY));
    let text = h.run(&["--embedded", "agent", "harnesses"]).await;
    text.ok();
    assert!(
        text.stdout
            .lines()
            .any(|l| l.starts_with("claude-code") && l.contains("auth: ok")),
        "{text:?}"
    );
    assert!(!text.stdout.contains(DUMMY));
}
