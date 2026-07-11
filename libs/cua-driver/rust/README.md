# cua-driver Rust workspace

This directory is the standalone Cargo workspace for the driver daemon, platform
implementations, testkit, and helper crates.

## Crates

| Crate | Purpose |
| --- | --- |
| `cua-driver` | Main CLI/MCP daemon and integration tests |
| `cua-driver-core` | Shared protocol, tool models, config, and CDP helpers |
| `platform-macos` | macOS AX, capture, input, browser, and daemon support |
| `platform-windows` | Windows UIA, capture, input, overlay, and diagnostics |
| `platform-linux` | Linux AT-SPI, X11/Wayland, capture, and input support |
| `cua-driver-testkit` | Test-only helpers for spawning the daemon and parsing responses |
| `cua-driver-uia` | Windows UIAccess worker |
| `cursor-overlay` | Cursor overlay support |
| `pip-preview` | Packaging preview helper |

Platform crates are selected with `cfg(target_os)` from the main driver crate.
Keep platform-specific behavior inside the matching `platform-<os>` crate.

## Build

```bash
cargo build -p cua-driver
cargo build -p cua-driver --release
```

The Python wrapper and install scripts expect the built binary at
`target/<profile>/cua-driver` (or `cua-driver.exe` on Windows), unless
`CUA_DRIVER_BINARY` is set.

## Codex Computer Use compatibility (macOS)

`cua-driver mcp --codex-computer-use-compat` replaces the native MCP catalog
with the ten app-oriented tools exposed by Codex Computer Use v829. Each action
requires a current `get_app_state(app)` snapshot and returns a refreshed concise
AX tree plus an 85%-quality JPEG normalized to logical window points. The mode
uses the built-in `sky` cursor unless `--cursor-shape` or `--cursor-icon` is
explicit. Without the flag, the full native tool catalog and teardrop default
are unchanged. Compat MCP sessions use a dedicated daemon socket and PID file,
so a concurrently running native daemon cannot supply the wrong tool catalog or
cursor configuration.

The compatibility layer blocks the driver/host, macOS authentication services,
System Settings, Terminal, and Ghostty. It does not implement an approval UI or
MCP elicitation; the embedding host remains responsible for confirmation policy
before risky UI actions. Its tree uses cua-driver-generated indices, which are
not numerically identical to Codex's proprietary indices. Compatibility mode
preserves and indexes meaningful containers and static text. The native tree
remains action-only.

## Tests

Default tests should be headless and safe for CI:

```bash
cargo test -p cua-driver-core
cargo test -p cua-driver --test protocol_mcp_test
```

GUI and VM-backed tests are marked `#[ignore]` and require harness apps from
`../tests/fixtures/build/` plus a real interactive desktop session:

```bash
cargo test -p cua-driver --test harness_appkit_test -- --ignored --nocapture
cargo test -p cua-driver --test harness_wpf_test -- --ignored --nocapture
```

See `crates/cua-driver/tests/README.md` for the test matrix.

## Generated Outputs

- `target/`: Cargo output, ignored.
- `test-apps/harness-*/`: staged harness apps, ignored.
- `test-apps/README.md`: explains how staged harness outputs are produced.
