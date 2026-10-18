//! `cua telemetry`, the first-run notice and every switch, against the
//! real binary in a temporary home. Sends go nowhere: the endpoint is a
//! closed loopback port and the no-network guard is on.

mod common;
use common::Home;

fn enabled_home() -> Home {
    let mut h = Home::new();
    h.set("CUA_TELEMETRY", "1");
    h
}

const NOTICE: &str = "Cua collects anonymous usage data";

#[tokio::test]
async fn notice_once_then_events_that_show_last_prints_without_arguments() {
    let h = enabled_home();
    // An --os value that is not this host's os_family, so finding it in the
    // events can only mean the argument leaked.
    let os_arg = if cfg!(target_os = "linux") {
        "windows"
    } else {
        "linux"
    };
    let first = h.run(&["images", "ls", "--os", os_arg]).await;
    first.ok();
    assert!(first.stderr.contains(NOTICE), "{}", first.stderr);
    assert!(first.stderr.contains("cua telemetry off"));
    let second = h.run(&["images", "ls", "--os", os_arg]).await;
    second.ok();
    assert!(!second.stderr.contains(NOTICE));

    let last = h.run(&["telemetry", "show-last", "--json"]).await;
    last.ok();
    let v = last.json();
    let events: Vec<&serde_json::Value> = v.as_array().unwrap().iter().collect();
    let cmd = events
        .iter()
        .find(|e| e["payload"]["event"] == "cua_cli_command")
        .expect("a cli command event");
    let p = &cmd["payload"]["properties"];
    assert_eq!(p["command"], "images.ls");
    assert_eq!(p["outcome"], "ok");
    assert_eq!(p["$geoip_disable"], true);
    let text = v.to_string();
    // The argument value and the temp home never appear.
    assert!(!text.contains(&format!("\"{os_arg}\"")), "{text}");
    assert!(!text.contains(h.dir.path().to_str().unwrap()), "{text}");
    // The first process sent nothing: only one cli command was recorded.
    assert_eq!(
        events
            .iter()
            .filter(|e| e["payload"]["event"] == "cua_cli_command")
            .count(),
        1
    );
    // Retention: the day is marked active once, however many commands run.
    let third = h.run(&["images", "ls", "--os", os_arg]).await;
    third.ok();
    let last = h.run(&["telemetry", "show-last", "--json"]).await;
    let v = last.json();
    assert_eq!(
        v.as_array()
            .unwrap()
            .iter()
            .filter(|e| e["payload"]["event"] == "cua_app_active")
            .count(),
        1,
        "{v}"
    );
}

#[tokio::test]
async fn every_switch_turns_it_off() {
    for (k, v) in [
        ("DO_NOT_TRACK", "1"),
        ("CUA_TELEMETRY", "0"),
        ("CI", "true"),
    ] {
        let mut h = Home::new();
        h.set("CUA_TELEMETRY", "");
        h.set(k, v);
        let s = h.run(&["telemetry", "status", "--json"]).await;
        s.ok();
        assert_eq!(s.json()["enabled"], false, "{k}={v}");
        let r = h.run(&["images", "ls"]).await;
        r.ok();
        assert!(!r.stderr.contains(NOTICE), "{k}: notice while off");
        assert!(
            !h.cua_home().join("telemetry/install_id").exists(),
            "{k}: id created"
        );
    }
    // `cua config set telemetry off` and `cua telemetry off`.
    for args in [
        &["config", "set", "telemetry", "off"][..],
        &["telemetry", "off"][..],
    ] {
        let mut h = Home::new();
        h.set("CUA_TELEMETRY", "");
        h.run(args).await.ok();
        let s = h.run(&["telemetry", "status", "--json"]).await;
        assert_eq!(s.json()["enabled"], false, "{args:?}");
        assert_eq!(s.json()["source_kind"], "config");
        let r = h.run(&["images", "ls"]).await;
        assert!(!r.stderr.contains(NOTICE));
        assert!(!h.cua_home().join("telemetry/install_id").exists());
    }
}

#[tokio::test]
async fn status_prints_the_envelope_and_has_no_data_sharing() {
    let h = enabled_home();
    let s = h.run(&["telemetry", "status", "--json"]).await;
    s.ok();
    let v = s.json();
    assert_eq!(v["envelope"]["$process_person_profile"], false);
    assert_eq!(v["envelope"]["product"], "cli");
    // Opt-in data sharing is parked: no consent state, no `share` command.
    assert!(v.get("data_sharing").is_none());
    let r = h.run(&["telemetry", "share", "status"]).await;
    assert_ne!(r.code, 0);
}
