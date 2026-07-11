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

The compatibility layer blocks the driver/host, Codex and ChatGPT, macOS
authentication services, System Settings, and common terminal apps. Before an
app can launch or expose accessibility and screenshot state, the daemon issues
an authenticated MCP `elicitation/create` request. Plain accept is scoped to the
MCP session. A response with `_meta.persist: "always"` writes a permanent
approval keyed by the bundle ID and canonical app path. The daemon verifies
that the broker is matching live CuaDriver code launched by signed OpenAI Codex,
then requires its daemon-minted token on every tool call. Raw and in-process
calls fail closed because they do not own that broker session.

Permanent approvals have a local management surface that does not expand the
ten-tool MCP catalog:

```bash
cua-driver approvals list
cua-driver approvals revoke com.example.Editor
cua-driver approvals clear
```

Each command accepts `--json`. The accessibility tree uses cua-driver-generated
indices, which are not numerically identical to Codex's proprietary indices.
Compatibility mode preserves and indexes meaningful containers and static text.
The native tree remains action-only.

Unsigned clients are rejected. Isolated tests and local driver development may
set `CUA_DRIVER_CODEX_ALLOW_UNVERIFIED_CLIENT=1` to skip only the signed Codex
parent check. The live CuaDriver code-identity and broker-token checks still run.

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
