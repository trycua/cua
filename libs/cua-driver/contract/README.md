# Experimental cua-driver client contract

This directory contains the checked-in, generated contract for the first
portable cua-driver client slice. The Rust crate at
`rust/crates/cua-driver-contract` is the source of truth. It generates this
manifest and the Python and TypeScript client surfaces under `clients/`.

The prototype intentionally keeps execution, platform integration, policy, and
permission handling in the native cua-driver process. Clients communicate with
the public `cua-driver mcp` stdio surface; they do not call a private daemon
socket or embed platform code.

## Scope and compatibility

The typed slice currently covers the cross-platform session lifecycle tools:

- `start_session`
- `escalate_session`
- `get_session_state`
- `end_session`

Both clients retain a generic tool call so runtime-discovered and
platform-specific tools remain usable. The generated manifest records tool
platforms, capabilities, annotations, input schemas, and experimental success
schemas. Success schemas are not advertised as live MCP `outputSchema` values
until every transport path has passed parity tests.

Compatibility is tracked separately at each boundary:

| Field | Current | Meaning |
| --- | --- | --- |
| `contract_version` | `0.1.0` | Generated manifest and typed-client shape |
| `tools_list_schema_version` | `1` | cua-driver `tools/list` extension shape |
| `capability_version` | `1` | Additive capability-token vocabulary |
| `mcp_protocol_version` | `2025-06-18` | MCP initialization protocol requested by clients |

This experiment does not use WASM. Fleet benefits from a portable core that
owns HTTP behavior and secrets; cua-driver already has a native process that
owns GUI execution and exposes MCP. Generated declarations plus thin MCP
clients give harnesses one contract without introducing a second runtime. WASM
can be reconsidered if future in-process integration surfaces need shared
executable policy rather than shared types.

## Generate and verify

From `libs/cua-driver/rust`:

```bash
cargo run -p cua-driver-contract --bin cua-contract-gen -- all
cargo run -p cua-driver-contract --bin cua-contract-gen -- all --check
cargo test -p cua-driver-contract
cargo test -p cua-driver-core --test contract_parity
```

Client tests live in `clients/python` and `clients/typescript`. CI runs the
generator in check mode so hand-edited or stale generated files fail the pull
request.
