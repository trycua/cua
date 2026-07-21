# UniFFI imported-SDK implementation journal

Status: package-root SDK cleanup validated locally; pull-request exact-head CI pending

## Definition of done

Deliver an experimental imported-SDK vertical slice in pull request #2341 where:

- Rust request types used by the live daemon are also the types exported through UniFFI.
- A Rust `CuaDriver` object invokes the existing daemon protocol without routing through MCP.
- Python and Node/TypeScript bindings are generated deterministically from the compiled Rust library.
- Host-native loader tests exercise calls across both FFI boundaries.
- CI checks Rust parity, generated-file drift, packaging inputs, and both loaders.
- MCP/CLI remains the supported agent integration boundary; UniFFI is for applications importing Cua as an SDK.

## Frozen baseline

- Branch: `codex/experimental-cua-driver-contract-sdk`
- Starting head: `07cec13ec1ecf2336d842b156993a83d4f713bfe`
- Upstream main at start: `d5a5e33d6d0b5dc4998c4985bc0704d6df2b0a1d`
- UniFFI: `0.31.0`, matching the repository's Fleet SDK precedent.
- Node generator/runtime: `uniffi-bindgen-react-native` / `@ubjs/*` `0.31.0-3`.

## Architecture decision

The FFI library is a daemon client, not an in-process copy of the GUI engine. This preserves the daemon's process-wide session state, permission ownership, platform event-loop constraints, policy enforcement, and timeout behavior. The shared Rust contract remains transport-free and is consumed by both live tool handlers and the exported UniFFI methods.

The public integration split remains:

- Agent MCP/CLI: language-neutral protocol boundary, no generated client required.
- Imported SDK: generated Python and Node/TypeScript bindings backed by one Rust implementation.

The product boundary is reflected directly in packaging:

- Python client applications import the Rust-backed SDK from `cua_driver`.
- TypeScript client applications import it from `@trycua/cua-driver`.
- Agents configure `cua-driver mcp` through their runtime's existing MCP client
  and do not import either language package.
- The language-native MCP facades and their contract generator were removed.
- No public `.sdk`, `/sdk`, `.mcp`, `/mcp`, `.native`, or `/native` alias exists;
  `native` remains only an internal loader/artifact term.

## Initial audit

- The live session handlers and desktop backends already deserialize `cua-driver-contract` request types.
- Successful structured responses are validated against output types from that same crate.
- The daemon socket protocol is currently defined inside the binary crate; it must be moved into shared Rust code so the binary, proxy, CLI, and UniFFI library use the same wire implementation.
- Python UniFFI generation and colocated library loading are supported upstream.
- Node N-API generation is available through `ubrn generate napi bindings`; it loads the same `cdylib` through `@ubjs/node` and currently requires a separately orchestrated Rust build.

## Verification log

### Rust and contract

- `cargo check --locked --workspace --all-targets` passed. Existing
  cross-platform example warnings remain outside this change.
- `cargo test --locked -p cua-driver` passed: 115 unit tests and all enabled
  integration suites passed; GUI/environment-dependent cases remained ignored.
- `cargo test --locked -p cua-driver-core` passed: 329 unit tests, 2 contract
  parity tests, and 3 session lifecycle tests.
- `cargo test --locked -p cua-driver-sdk` passed: 4 tests covering published
  method parity, typed desktop calls, typed session output, and tool discovery.
- `cargo run --locked -p cua-driver-contract --bin cua-contract-gen -- all --check`
  passed.
- `node scripts/generate-uniffi-bindings.mjs --check` rebuilt the release
  `cdylib` and reproduced every checked-in Python and TypeScript binding.

### Language loaders and packages

- Python generated-loader test: 1 passed across `ctypes` -> UniFFI -> Rust ->
  Unix socket, and asserted all 14 typed methods are exported.
- Python release-helper tests: 3 passed, including native-library membership
  for every release archive target.
- TypeScript typecheck and the actual N-API loader call passed, including all
  14 methods and proof that the removed package subpaths are not exported.
- `npm audit --audit-level=high`: 0 vulnerabilities.
- A built macOS arm64 wheel was installed into a clean Python 3.12 environment;
  `cua_driver.CuaDriver.connect(None)` loaded the packaged library and
  returned the canonical daemon socket path.
- An actual 382.3 kB npm tarball (1.2 MB unpacked) was installed into a clean
  project; the root imported without native code and
  `@trycua/cua-driver` loaded the packaged dylib successfully.
- The macOS dylib install ID is the relocatable
  `@rpath/libcua_driver_sdk.dylib`, and release CI asserts it before signing.
- After promoting the SDK to each package root, a fresh macOS arm64 wheel and
  npm tarball were installed into clean Python 3.12 and Node projects. Both
  roots loaded the Rust library and returned the canonical socket path; Python
  contained no `sdk` or `native` module, Node exported no `/sdk`, `/native`, or
  `/mcp` subpath, and neither artifact contained the removed MCP facade files.
- Package-root cleanup validation passed: Python real-loader test (1), Python
  packaging/wrapper tests (13 passed, 3 binary-dependent skips), TypeScript
  typecheck, Node real-loader test (1), and `npm audit` (0 vulnerabilities).

### CI and release boundary

- The PR workflow parses successfully and checks deterministic generation,
  Rust parity on Linux/macOS/Windows, both Linux FFI loaders, installed wheel
  and npm-tarball smoke tests, and package contents.
- Rust release artifacts now carry the SDK library on Linux, macOS, and
  Windows. The Python release matrix installs every built wheel and imports the
  generated binding before PyPI publication.
- Full multi-OS/architecture assembly for one npm publication remains a stated
  release gate. This pull request must not publish a host-local npm tarball.
- MCP/CLI remains unchanged as the agent boundary; the language package roots
  are the Rust-backed application SDK.
