//! `cua do check` warns about unreachable direct targets and, with `--clean`,
//! drops them. A closed port stands in for a Tailscale peer that is gone:
//! those peers are the same `direct:` rows in the do-target cache and
//! `direct-hosts.json`.

mod common;
use common::*;
use cua_sandbox_core::{LocalState, SandboxState, StateStore};
use cua_spaces::registry::{Credential, Registry};
use serde_json::{Map, Value};

fn remember(home: &std::path::Path, name: &str, port: u16) {
    let store = StateStore::new(home.join("sandboxes"));
    let mut extra = Map::new();
    extra.insert(
        "url".into(),
        Value::String(format!("http://127.0.0.1:{port}")),
    );
    store
        .save(&SandboxState::Local(LocalState {
            name: name.into(),
            runtime_type: "direct".into(),
            host: "127.0.0.1".into(),
            api_port: port,
            status: "running".into(),
            extra,
            ..Default::default()
        }))
        .unwrap();
}

fn space(id: &str, name: &str) -> cua_proto::daemon::v1::Space {
    cua_proto::daemon::v1::Space {
        id: id.into(),
        name: name.into(),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_direct_targets_are_warned_then_pruned_and_the_relay_stays() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let live = listener.local_addr().unwrap().port();
    let h = Home::new();
    let home = h.cua_home();
    std::fs::create_dir_all(&home).unwrap();

    // Selected `cua do` target, remembered the way `switch url` writes it.
    remember(&home, "studio", 1);
    std::fs::write(
        home.join("do_target.json"),
        r#"{"provider":"direct","name":"studio"}"#,
    )
    .unwrap();
    // A direct host added with `cua spaces add --host`, and one listener
    // that still accepts connections.
    remember(&home, "livebox", live);
    let reg = Registry::new(&home);
    reg.upsert_direct_host("spare", "direct:127.0.0.1:2")
        .unwrap();
    reg.upsert(space("direct:127.0.0.1:2", "spare"), Credential::default())
        .unwrap();
    reg.upsert(
        space(&format!("direct:127.0.0.1:{live}"), "livebox"),
        Credential::default(),
    )
    .unwrap();
    reg.upsert(
        space("relay:0123abcd4567ef89", "desk"),
        Credential::default(),
    )
    .unwrap();

    let warn = h.run(&["--embedded", "do", "check"]).await;
    assert_eq!(warn.code, 1, "{warn:?}");
    assert!(
        warn.stdout
            .contains("Current target studio (direct:127.0.0.1:1) is unreachable."),
        "{warn:?}"
    );
    assert!(
        warn.stdout.contains("spare") && warn.stdout.contains("direct:127.0.0.1:2"),
        "{warn:?}"
    );
    assert!(
        warn.stdout
            .contains("Next: cua do switch relay:0123abcd4567ef89"),
        "{warn:?}"
    );
    assert!(warn.stdout.contains("cua do check --clean"), "{warn:?}");
    assert!(
        !warn.stdout.contains(&format!("direct:127.0.0.1:{live}")),
        "a listener that accepts connections is not stale: {warn:?}"
    );
    assert!(home.join("sandboxes/studio.json").exists());
    assert!(home.join("direct-hosts.json").exists());
    let hosts = std::fs::read_to_string(home.join("direct-hosts.json")).unwrap();
    assert!(hosts.contains("127.0.0.1:2"), "{hosts}");

    let cleaned = h.run(&["--embedded", "do", "check", "--clean"]).await;
    assert_eq!(cleaned.code, 0, "{cleaned:?}");
    assert!(cleaned.stdout.contains("Removed studio"), "{cleaned:?}");
    assert!(cleaned.stdout.contains("Removed spare"), "{cleaned:?}");
    assert!(
        cleaned
            .stdout
            .contains("Cleared the current cua do target."),
        "{cleaned:?}"
    );
    assert!(
        cleaned
            .stdout
            .contains("Next: cua do switch relay:0123abcd4567ef89"),
        "{cleaned:?}"
    );
    assert!(
        !home.join("sandboxes/studio.json").exists(),
        "dead do target remains"
    );
    assert!(
        home.join("sandboxes/livebox.json").exists(),
        "reachable direct target was removed"
    );
    let hosts = std::fs::read_to_string(home.join("direct-hosts.json")).unwrap_or_default();
    assert!(!hosts.contains("127.0.0.1:2"), "{hosts}");
    let spaces = std::fs::read_to_string(home.join("spaces.json")).unwrap();
    assert!(spaces.contains("relay:0123abcd4567ef89"), "{spaces}");
    assert!(
        spaces.contains(&format!("direct:127.0.0.1:{live}")),
        "{spaces}"
    );
    assert!(!spaces.contains("direct:127.0.0.1:2"), "{spaces}");
    let target: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("do_target.json")).unwrap())
            .unwrap();
    assert!(target["provider"].is_null(), "{target}");
    assert_eq!(target["name"].as_str().unwrap_or("x"), "");

    let again = h.run(&["--embedded", "do", "check"]).await;
    assert_eq!(again.code, 0, "{again:?}");
    assert!(
        again.stdout.contains("Direct targets are reachable."),
        "{again:?}"
    );
    assert!(!again.stdout.contains("unreachable"), "{again:?}");
}
