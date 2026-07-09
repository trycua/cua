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
| `focus-monitor-win` | Windows focus sentinel used by UX guard tests |
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
