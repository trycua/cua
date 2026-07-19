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

## Formatting

The supported Rust toolchain and `rustfmt` component are pinned in
`rust-toolchain.toml`. Install [rustup](https://rustup.rs/) and run the
canonical formatting check from this directory:

```bash
cargo fmt --all -- --check
```

To apply formatting, omit `--check`:

```bash
cargo fmt --all
```

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

## Permission Policies

Set `CUA_DRIVER_POLICY_FILE` on the daemon to enforce a deny-by-default policy
on all tool calls from MCP and CLI clients. If the variable is unset, or points to a path that does
not exist, the driver keeps its backward-compatible behavior with no policy
enforcement. Supported paths are:

- `.yaml` / `.yml`: a single YAML policy file.
- `.rego`: a single Rego policy file.
- A directory: all top-level `.rego` files are loaded in sorted order.

YAML policies allow unconstrained tools through `allow.tools`, constrained
calls through `allow.rules`, and explicit overrides through `deny.tools`:

```yaml
allow:
  tools: [screenshot, scroll, wait]
  rules:
    - tool: click
      constraints:
        x: { min: 0, max: 1920 }
        y: { min: 0, max: 1080 }
    - tool: type_text
      constraints:
        text: { max_length: 1000, pattern: "^[a-zA-Z0-9\\s]+$" }
    - tool: launch_app
      constraints:
        app:
          allowed: [Calculator, TextEdit, Safari]
deny:
  tools: [shell_execute, file_delete]
```

Rego policies must expose a boolean rule at `data.cua.policy.allow`. The input
shape is `{ "server": "cua-driver", "tool": <name>, "arguments": <object> }`.
Regorus evaluates policies inside the daemon; no OPA service is required.
