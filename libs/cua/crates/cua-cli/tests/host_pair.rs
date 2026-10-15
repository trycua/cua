//! `cua host setup --pair`: the command the Cua Spaces app's "Add a machine"
//! (and cua.ai/add/<code>) gives out must be accepted by the released CLI
//! (CUA-1748), and a bad code stops before anything is contacted.
mod common;
use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_is_accepted_and_checked_before_anything_else() {
    let relay = FakeHttp::start(|_| (500, json!({"error": "must not be called"}))).await;
    let mut h = Home::new();
    h.set("CUA_RELAY_URL", &relay.url);

    let help = h.run(&["host", "setup", "--help"]).await;
    help.ok();
    assert!(help.stdout.contains("--pair <CODE>"), "{help:?}");

    let o = h.run(&["host", "setup", "--pair", "not-a-code"]).await;
    assert_ne!(o.code, 0, "{o:?}");
    assert!(o.stderr.contains("is not a pairing code"), "{o:?}");
    assert!(!o.stderr.contains("unexpected argument"), "{o:?}");
    assert!(relay.requests().is_empty(), "nothing contacted for a bad code");
    assert!(!h.cua_home().join("host/machine-token").exists());

    let o = h
        .run(&[
            "host",
            "setup",
            "--pair",
            "https://cua.ai/add/K7QM-4XPB-2RHD",
            "--direct",
            "127.0.0.1:3211",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("cannot be used with"), "{o:?}");
    assert!(relay.requests().is_empty());
}
