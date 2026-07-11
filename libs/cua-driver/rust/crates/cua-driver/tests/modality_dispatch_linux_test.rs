//! Modality axis: the Linux dispatch ladder (delivery_mode parity).
//!
//! Asserts the contract the Linux parity work added for X11
//! background/foreground delivery. Uses the binary's own `describe` (which computes each
//! ToolDef schema locally) rather than a daemon round-trip, so it reflects the
//! freshly-built binary and runs in CI without a display or daemon.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

fn driver_bin() -> PathBuf {
    // tests run with CARGO_MANIFEST_DIR = crates/cua-driver; the workspace
    // target/debug is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/cua-driver")
}

/// `cua-driver describe <tool>` → the tool's advertised inputSchema as text.
/// Returns None (test skips) when the binary isn't built.
fn describe(tool: &str) -> Option<String> {
    let bin = driver_bin();
    if !bin.exists() {
        eprintln!("[dispatch-linux] {bin:?} not built — skipping");
        return None;
    }
    let out = Command::new(&bin).arg("describe").arg(tool).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Every Linux input tool advertises `delivery_mode` with the two-mode enum —
/// the per-call background/foreground rung selector (parity with macOS/Windows).
#[test]
fn linux_input_tools_advertise_delivery_mode() {
    for tool in ["click", "type_text", "press_key", "hotkey", "double_click", "right_click", "scroll"] {
        let Some(schema) = describe(tool) else { return };
        assert!(
            schema.contains("delivery_mode"),
            "{tool} schema is missing delivery_mode:\n{schema}"
        );
        assert!(
            schema.contains("foreground") && schema.contains("background"),
            "{tool} delivery_mode enum should include background + foreground:\n{schema}"
        );
    }
}
