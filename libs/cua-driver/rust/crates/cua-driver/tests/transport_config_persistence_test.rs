//! Transport axis: `set_config` persistence across the CLI vs MCP transports.
//!
//! This is the one behavior that only shows up when a test covers BOTH
//! transports, so it lives on the shared testkit `Driver` abstraction:
//!
//!   - **CLI** (`CliDriver`) is stateless — each `cua-driver call` is its own
//!     process. For a `set_config` to be visible to the *next* invocation it
//!     must persist to **disk**. #2034 made that true on Windows + Linux (macOS
//!     already did); this test guards against a regression.
//!   - **MCP** (`McpDriver`) is one long-lived connection — a `set_config` is
//!     visible to later calls on the SAME driver within the session.
//!
//! Uses `max_image_dimension` as the persisted key. `capture_mode` /
//! `capture_scope` are NO LONGER settings (`capture_scope` is per-session), so
//! `max_image_dimension` is the remaining
//! disk-persisted config field and the right probe for this transport behavior.
//!
//! Both tests are `#[ignore]`: they mutate the real on-disk config, so they
//! save the prior value and restore it. Run explicitly:
//!   cargo test -p cua-driver --test transport_config_persistence_test -- --ignored --nocapture

use cua_driver_testkit::{CliDriver, Driver, McpDriver};

const KEY: &str = "max_image_dimension";
/// A distinctive probe value unlikely to be the current setting.
const PROBE: u64 = 1234;

fn config_max_dim(structured: &serde_json::Value) -> Option<u64> {
    structured[KEY].as_u64()
}

/// CLI: a `set_config` in one process is observed by a *separate* `get_config`
/// process — i.e. it persisted to disk (#2034). The two `cli.call(...)`s below
/// are independent `cua-driver call` invocations.
#[test]
#[ignore]
fn cli_set_config_persists_to_disk_across_invocations() {
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

    // A fresh process must see the persisted value.
    let after = cli.call("get_config", serde_json::json!({}));
    assert_eq!(
        config_max_dim(after.structured()),
        Some(PROBE),
        "CLI set_config did NOT persist to disk across invocations (#2034 regression): {}",
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

/// MCP: a `set_config` is visible to a later call on the SAME long-lived driver
/// (session scope). Restores the prior value before dropping the connection.
#[test]
#[ignore]
fn mcp_set_config_visible_within_session() {
    let Some(mut driver) = McpDriver::spawn() else {
        return;
    };

    let original = config_max_dim(
        driver
            .call("get_config", serde_json::json!({}))
            .structured(),
    );

    let set = driver.call(
        "set_config",
        serde_json::json!({ "key": KEY, "value": PROBE }),
    );
    assert!(!set.is_error(), "MCP set_config errored: {}", set.text());

    let after = driver.call("get_config", serde_json::json!({}));
    assert_eq!(
        config_max_dim(after.structured()),
        Some(PROBE),
        "MCP set_config not visible within the same session: {}",
        after.text()
    );

    if let Some(orig) = original {
        let _ = driver.call(
            "set_config",
            serde_json::json!({ "key": KEY, "value": orig }),
        );
    }
}
