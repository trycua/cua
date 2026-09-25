//! Transport axis: CLI `set_config` visibility across daemon-backed shell
//! processes.
//!
//! `CliDriver` starts a fresh shell process for each call, but all calls go
//! through one test-owned daemon, so a `set_config` in one process must be
//! visible to the next. MCP in-session visibility is owned by
//! `protocol_tools_call_test::get_config_and_check_permissions`.
//!
//! Uses `max_image_dimension` as the persisted key. `capture_mode` /
//! `capture_scope` are NO LONGER settings (`capture_scope` is per-session), so
//! `max_image_dimension` is the remaining
//! disk-persisted config field and the right probe for this transport behavior.
//!
//! The test is `#[ignore]`: an anonymous CLI `set_config` mutates the real
//! on-disk config, so it saves the prior value and restores it. Run explicitly:
//!   cargo test -p cua-driver --test transport_config_persistence_test -- --ignored --nocapture

use cua_driver_testkit::{CliDriver, Driver};

const KEY: &str = "max_image_dimension";
/// A distinctive probe value unlikely to be the current setting.
const PROBE: u64 = 1234;

fn config_max_dim(structured: &serde_json::Value) -> Option<u64> {
    structured[KEY].as_u64()
}

/// CLI: a `set_config` in one shell process is observed by a separate
/// `get_config` process through their shared daemon.
#[test]
#[ignore]
fn cli_set_config_visible_across_daemon_backed_invocations() {
    let mut cli = CliDriver::new();
    if !cli.available() {
        eprintln!("[transport] driver binary not built — skipping");
        return;
    }

    // Save the current value so we can restore it.
    let original = config_max_dim(cli.call("get_config", serde_json::json!({})).structured());

    let set = cli.call(
        "set_config",
        serde_json::json!({ "key": KEY, "value": PROBE }),
    );
    assert!(!set.is_error(), "CLI set_config errored: {}", set.text());

    // A fresh shell process must see the daemon-owned value.
    let after = cli.call("get_config", serde_json::json!({}));
    assert_eq!(
        config_max_dim(after.structured()),
        Some(PROBE),
        "CLI set_config was not visible across daemon-backed invocations: {}",
        after.text()
    );

    // Restore.
    if let Some(orig) = original {
        let _ = cli.call(
            "set_config",
            serde_json::json!({ "key": KEY, "value": orig }),
        );
    }
}
