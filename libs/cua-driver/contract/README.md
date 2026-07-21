# Experimental cua-driver SDK contract

This directory contains the checked-in, generated contract for the first
portable cua-driver SDK slice. The Rust crate at
`rust/crates/cua-driver-contract` is the source of truth. It generates this
manifest, the Python SDK surface under `python/`, and the TypeScript SDK under
`typescript/`.

The prototype intentionally keeps execution, platform integration, policy, and
permission handling in the native cua-driver process. Clients communicate with
the public `cua-driver mcp` stdio surface; they do not call a private daemon
socket or embed platform code.

## Scope and compatibility

The typed slice covers the cross-platform session lifecycle tools:

- `start_session`
- `escalate_session`
- `get_session_state`
- `end_session`

It also covers the portable whole-desktop loop:

- `get_desktop_state`
- `get_screen_size`
- `get_cursor_position`
- `move_cursor` with the required `scope="desktop"`
- `click` with the required `scope="desktop"`
- `drag` and `scroll` in native desktop coordinates
- `type_text`, `press_key`, and `hotkey` against the foreground application

Session contracts are marked `canonical_runtime`: the same typed Rust input,
output, and metadata declaration builds the live MCP tool. Desktop contracts
are marked `portable_subset`: their typed Rust inputs are a deliberately
narrower projection of the richer macOS, Linux, and Windows runtime schemas.
Each platform's desktop branch deserializes that projection before acting,
while window/element-only fields remain in the richer live schema. Successful
SDK-path structured payloads are validated against the shared Rust output
types in the live registry.

The platform schemas remain richer by design; they are not an independent SDK
manifest. A cross-platform CI matrix proves every portable schema is accepted
by each live registry, and the published tools resolve their capability tokens
from the contract rather than a second runtime map.

Both SDKs retain a generic tool call so runtime-discovered and
platform-specific tools remain usable. The generated manifest records tool
platforms, capabilities, annotations, input schemas, and experimental success
schemas. Success schemas are not advertised as live MCP `outputSchema` values
until every transport path has passed parity tests.

Compatibility is tracked separately at each boundary:

| Field | Current | Meaning |
| --- | --- | --- |
| `contract_version` | `0.2.0` | Generated manifest and typed SDK shape |
| `tools_list_schema_version` | `1` | cua-driver `tools/list` extension shape |
| `capability_version` | `1` | Additive capability-token vocabulary |
| `mcp_protocol_version` | `2025-06-18` | MCP initialization protocol requested by SDKs |

This experiment does not use WASM. Fleet benefits from a portable core that
owns HTTP behavior and secrets; cua-driver already has a native process that
owns GUI execution and exposes MCP. Generated declarations plus thin MCP
SDKs give harnesses one contract without introducing a second runtime. WASM
can be reconsidered if future in-process integration surfaces need shared
executable policy rather than shared types.

## Generate and verify

From `libs/cua-driver/rust`:

```bash
cargo run -p cua-driver-contract --bin cua-contract-gen -- all
cargo run -p cua-driver-contract --bin cua-contract-gen -- all --check
cargo test -p cua-driver-contract
cargo test -p cua-driver-core --test contract_parity
cargo test -p cua-driver --test schema_consistency_test \
  portable_desktop_contracts_are_accepted_by_active_backend
```

SDK tests live in `python/tests/test_driver.py` and `typescript`. CI
runs the generator in check mode so hand-edited or stale generated files fail
the pull request. Each SDK package also contains a generated ownership
manifest. The generator renders and validates the complete plan before atomic
replacement, rejects unsafe or symlinked paths, detects stale owned files in
check mode, and prunes only previously declared generated files in write mode.
