# cua-driver Rust integration tests

Tests in this directory exercise the public driver interface without a
desktop: MCP and CLI protocol, schemas, sessions, policy, and daemon
lifecycle. Plain `cargo test -p cua-driver` runs them and must stay hermetic.
They get `CARGO_BIN_EXE_cua-driver`, so Cargo builds the driver for them.

Desktop E2E suites that need a real desktop, fixture apps, installed apps, or
OS permissions live in their own crate:
[`../../cua-driver-e2e/tests/`](../../cua-driver-e2e/tests/README.md).

The few `#[ignore]` tests left here need real input or real per-user state.
Each is either selected by a canonical runner or listed with a reason in
`libs/cua-driver/tests/manual-e2e-allowlist.txt`;
`.github/scripts/tests/test_cua_driver_e2e_inventory.py` enforces this for both
crates.

## Naming

| Prefix | Purpose |
| --- | --- |
| `protocol_*_test.rs` | MCP/CLI protocol and schema behavior |
| `session_capture_scope_test.rs` | Per-session policy isolation, escalation, lifecycle, and retired config key |
| `schema_*_test.rs` | Generated schema consistency |
| `*_cli_test.rs` | CLI verbs driven through the built binary |

## Running

```bash
cargo test -p cua-driver --tests
cargo test -p cua-driver --test protocol_handshake_test
```
