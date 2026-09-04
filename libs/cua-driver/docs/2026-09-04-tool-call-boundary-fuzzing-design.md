# Tool call boundary fuzzing

Status: design accepted 2026-09-04, implemented in the same change.

## Problem

Every MCP client, the CLI, and the same-process SDK reach platform behaviour
through one chain in `cua-driver-core`:

```text
JSON-RPC bytes
  -> protocol::Request (serde)
  -> Request::tool_call()                       name + arguments
  -> server::handle_request_inner               reserved-arg strip, policy pre-check,
                                                transport session stamping
  -> ToolRegistry::invoke_authorized            strip again, alias, delivery-mode and
                                                target normalisation, hard invariants,
                                                risk classification, session namespace
  -> tool_args::parse_typed_input / projection  serde into the contract input types
```

Everything above the platform adapters is pure Rust over `serde_json::Value`
and runs on every platform. It is also where untrusted input is turned into
trusted arguments, so a panic, an unexpected `expect`, or a parser that accepts
something the published schema does not are all boundary bugs. The chain has
good example-based tests but nothing that explores the input space.

## Goals

- Fuzz the chain above with coverage-guided fuzzing (`cargo fuzz`) without
  touching any platform adapter, display, or OS input queue.
- Run the same target bodies as bounded smoke tests in ordinary `cargo test`
  so they compile on the pinned stable toolchain, run in existing CI and in the
  Nix unit check, and catch regressions without anyone remembering to fuzz.
- Keep `arbitrary` and `libfuzzer-sys` out of the shipped dependency graph.

Non-goals: fuzzing platform crates, the HTTP transport framing, or the
recording and video pipeline. Those are separate boundaries.

## Approaches considered

1. **`cargo-fuzz` crate only.** Standard layout, best exploration, but the
   targets need nightly for sanitizers, never run in CI, and rot silently.
2. **Property tests only (`proptest`).** Runs in CI, but hand-written
   strategies explore far less than libFuzzer and a second generator has to be
   maintained beside the fuzz targets.
3. **Shared target bodies, two drivers.** (chosen) Target bodies live in the
   dev-only `cua-driver-testkit` crate as `fn(&[u8])` functions built on
   `arbitrary::Unstructured`. A detached `fuzz/` crate wraps them in
   `fuzz_target!`. Smoke tests in the testkit feed the same bodies a checked-in
   seed corpus plus a deterministic pseudo-random byte stream.

Approach 3 costs one new dev-only dependency edge (`testkit -> core`) and gives
both continuous coverage and real fuzzing from one definition of each target.

## Design

### Where the code lives

```text
libs/cua-driver/rust/
  crates/cua-driver-testkit/src/boundary_fuzz.rs   target bodies + JSON generator
  crates/cua-driver-testkit/tests/tool_boundary_fuzz_smoke.rs
  fuzz/Cargo.toml                                  detached workspace, libfuzzer-sys
  fuzz/fuzz_targets/{mcp_request,tool_arguments,typed_input_json,registry_invoke}.rs
  fuzz/corpus/<target>/                            checked-in seeds only
  fuzz/.gitignore                                  target/, artifacts/, coverage/
```

The testkit is already described as dev-dependency only and never shipped, so
`cua-driver-core`, `cua-driver-contract`, `arbitrary`, and `tokio` can join
its dependency list without changing what `cua-driver` distributes.

### Targets and invariants

Each body returns normally on every input. A panic anywhere is the finding.

**`mcp_request(bytes)`** parses the bytes as a JSON-RPC `Request`. On success
it drives `server::handle_request` and `handle_request_with_transport_session`
against a recording `ToolProvider` stub. Invariants:

- the `Response` serialises and carries `jsonrpc: "2.0"`;
- for `tools/call`, the provider only ever receives underscore-prefixed
  arguments that the transport itself stamps (`_session_id`,
  `_transport_session_id`, the browser-download host approval flag), so a
  forged reserved argument never survives ingress;
- `_session_id` equals the caller's non-empty `session` string when present
  and otherwise the transport session.

**`tool_arguments(bytes)`** picks a tool name (a published contract name, a
runtime-only name, or an arbitrary string) and generates a bounded JSON value.
It runs `sanitize_reserved_args`, `normalize_action_target`,
`authorize_tool_call_with_context` under an unrestricted in-process context,
`classify_tool_call`, and, for names that own a contract input type,
`parse_typed_input` and `parse_typed_projection`. Invariants:

- sanitised objects contain no underscore-prefixed key;
- after a successful `normalize_action_target` no `target` key remains;
- a typed input that parses re-serialises to a value that parses again to the
  same serialisation (normalisation is idempotent).

**`typed_input_json(bytes)`** treats the bytes as JSON text and offers it to
every contract input type through `parse_typed_input`. This exercises serde
edge cases (numbers, escapes, duplicate keys, deep nesting) that a structured
generator rarely reaches. Invariant: the same idempotent re-serialisation rule,
plus the canonical re-serialised form of every accepted value validates
against `T::input_schema()`, so the driver never normalises to a shape the
published contract does not advertise. The raw input is not validated: serde
is allowed to be more lenient than the schema (an explicit `null` for an
optional field, unknown keys on types without `deny_unknown_fields`).

**`registry_invoke(bytes)`** builds a `ToolRegistry` whose tools are stubs
named after every published contract tool, then calls
`invoke_with_context` with an arbitrary name and generated arguments under an
unrestricted context. This covers alias resolution, delivery-mode and target
normalisation, hard invariants, the runtime session namespace, ended-session
refusals, and result post-processing. Invariants: the call returns a
`ToolResult`; an unregistered name yields an error result.

Session lifecycle tools are deliberately not registered so a fuzz input cannot
suspend the shared runtime scope and starve later inputs.

### Determinism

The registry, contexts, and provider are built per call from explicit
constructors (`SessionAuthorizationRegistry::with_ceiling`,
`compatibility_context`) so no environment variable or policy file is read.
The unrestricted ceiling is the same one the in-crate dispatch tests use.

### Smoke tests

`tests/tool_boundary_fuzz_smoke.rs` runs each body over the seed corpus and
over a few thousand inputs from a seeded xorshift generator with lengths from
0 to 4 KiB. The seeds are also the fuzz corpus, so anything that reproduces a
past finding stays a regression test.

### Running the fuzzers

The workspace pins a stable toolchain, and `cargo-fuzz` wants nightly only for
sanitizers. With `--sanitizer none` it builds on stable, so:

```bash
cd libs/cua-driver/rust
nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#cargo-fuzz --command \
  cargo fuzz run --fuzz-dir fuzz --sanitizer none mcp_request \
  fuzz/work/mcp_request fuzz/corpus/mcp_request -- -max_total_time=300
```

libFuzzer stores discovered inputs in the first corpus directory, so the
ignored `fuzz/work/<target>` directory keeps generated entries out of the
committed seeds. A crash file under `fuzz/artifacts/<target>/` reproduces with
`cargo fuzz run ... <target> <file>`; copy it into `fuzz/corpus/<target>/` once
fixed so the smoke test keeps it.

### Continuous fuzzing

`.github/workflows/fuzz-cua-driver.yml` builds each target with
`--sanitizer none` on the stable toolchain and runs it for two minutes on pull
requests touching the boundary crates, fifteen minutes nightly, and a chosen
budget on manual dispatch. A crash fails the target's job, writes the panic
message and reproduce command to the job summary, and uploads the crash file
and log. Discovered inputs go to an ignored `fuzz/work/<target>` directory so
the committed seed corpus stays hand-picked.

## Out of scope for this change

- Fixing what the fuzzers find. This change introduces the harness; its first
  findings are fixed in a separate change stacked on top so the failures stay
  visible in this one's CI.
