# Experimental cua-driver SDK contract

This directory contains the checked-in, generated contract for the first
portable cua-driver SDK slice. The Rust crate at
`rust/crates/cua-driver-contract` is the source of truth. It generates this
manifest and exports the request/result records consumed by the live daemon and
the UniFFI SDK. Python and TypeScript bindings are generated from the compiled
Rust library by `scripts/generate-uniffi-bindings.mjs`.

The prototype intentionally keeps execution, platform integration, policy, and
permission handling in the native Cua Driver process. Imported SDKs call the
daemon through the shared Rust socket client; they do not route through MCP or
embed platform code. Agents independently use the public `cua-driver mcp`
surface through their runtime's existing MCP client.

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
- `clipboard_read` for available types and opt-in plain-text readback
- `clipboard_write` for text, image, and file-URL clipboard content

The canonical session-owned cursor slice is shared exactly by MCP and both
generated SDKs:

- `set_agent_cursor_enabled`
- `set_agent_cursor_motion`
- `set_agent_cursor_theme`
- `get_agent_cursor_state`

Cursor artwork is selected by installed theme ID. Theme installation and
dotLottie compilation are intentionally local CLI operations, not SDK or MCP
tool calls.

The checked-observation slice is also shared by MCP and both generated SDKs:

- `verify_state` evaluates one to eight ANDed predicates against one exact
  `(pid, window_id)`.
- Window existence/bounds and semantic accessibility element
  existence/value/enabled/selected predicates return `satisfied`,
  `unsatisfied`, or `unknown`.
- A bounded poll can require consecutive stable samples. `unknown` never
  implies success.
- `include_screenshot=true` adds final image content for a multimodal agent
  harness to interpret. Cua Driver does not OCR or assign task meaning to it.

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
schemas. The live MCP `tools/list` response advertises these successful-result
schemas as `outputSchema`; all action tools share the closed `ActionResult`
schema even when their richer runtime input is not part of the portable SDK
manifest.

See [Action results and postcondition verification](../docs/action-result-contract.md)
for the wire shape and 0.14 migration guidance.

Compatibility is tracked separately at each boundary:

| Field | Current | Meaning |
| --- | --- | --- |
| `contract_version` | `0.5.0` | Generated manifest and typed SDK shape |
| `tools_list_schema_version` | `1` | cua-driver `tools/list` extension shape |
| `capability_version` | `1` | Additive capability-token vocabulary |
| `mcp_protocol_version` | `2025-06-18` | MCP initialization protocol served to agent runtimes |

This implementation does not use WASM. UniFFI distributes one Rust
daemon-client implementation to Python and Node while preserving the daemon's
permission identity and runtime ownership. The language packages do not
generate or maintain separate MCP transports.

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

SDK loader tests live in `python/tests/test_uniffi_loader.py` and
`typescript/test/native-loader.test.mjs`. CI checks the manifest generator,
deterministically regenerates both UniFFI binding sets, verifies parity against
the live tool registry, and crosses the real Python and Node FFI loaders into a
deterministic daemon-socket fixture.
