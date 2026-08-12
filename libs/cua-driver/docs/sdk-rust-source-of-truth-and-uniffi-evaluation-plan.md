# Cua Driver SDK Rust source-of-truth and UniFFI evaluation plan

Status: superseded by the final package-boundary decision in PR #2341

Applies to: PR #2341 and follow-up SDK/runtime work

Primary objective: eliminate manual contract/runtime bookkeeping before publishing the Python and TypeScript SDKs

Decision rationale: [Cua Driver integration surfaces: MCP/CLI and SDK bindings](why-cua-driver-uses-mcp-instead-of-uniffi.md)

> **Final decision update (2026-07-21):** the evaluation below records the
> sequence and evidence that led to the implementation, but its interim
> recommendation to retain Python and TypeScript MCP facades is no longer
> current. The language packages now expose only the Rust-backed UniFFI SDK at
> their package roots. Agents use `cua-driver mcp` through their runtime's
> existing MCP client. The thin language MCP clients and their generator were
> removed before publication.

## Executive decision

Proceed with the Rust source-of-truth work while separating the agent boundary
from the imported SDK boundary. MCP/CLI is the baseline agent architecture;
UniFFI remains a candidate for distributing a Rust SDK/server implementation.

The recommended sequence is:

1. Immediately prove that PR #2341's ten portable desktop contracts are valid
   subsets of every live platform schema.
2. Make a transport-free Rust specification crate the single owner of typed
   inputs, structured outputs, metadata, capabilities, platform presence, and
   portable projections.
3. Move live platform implementations onto those shared types incrementally,
   beginning with the four already-canonical session tools and then a genuinely
   divergent desktop slice.
4. Keep MCP and the CLI as the canonical agent integration. MCP-capable agents
   do not need a generated Cua client; prove this with Python Claude Agent SDK
   and TypeScript Codex SDK examples.
5. Keep the generated Python and TypeScript packages as typed MCP client SDKs
   for application code, without claiming that they add runtime portability.
6. Run a time-boxed, language-split UniFFI spike for consumers that want a
   shared Rust implementation, especially server-hosting or embedded SDK
   consumers. Treat Python as the mature target and TypeScript as a separate
   third-party toolchain evaluation.
7. Adopt UniFFI for a language only if it is reproducible, centralizes
   meaningful Rust behavior, and passes the packaging and runtime gates below.

Directly embedding the GUI engine is not part of the current client-generation
work. The existing runtime assumes a shared daemon, process-global state, and
OS-specific main-thread and permission behavior that a host-process library
cannot safely inherit without a separate design. A UniFFI facade over a shared
Rust daemon client and a UniFFI embedded/server SDK are distinct candidates.

## Confirmed current state

### PR #2341

- `cua-driver-contract` declares fourteen typed tools: four session lifecycle
  tools and ten portable desktop-loop tools.
- The manifest, Python argument classes/methods, and TypeScript
  interfaces/methods are generated from that Rust contract crate.
- The Python and TypeScript SDKs communicate with `cua-driver mcp` over stdio.
- Both SDKs retain generic calls and `tools/list` access for runtime-discovered
  and platform-specific tools.
- Generated files have an ownership inventory and a real `--check` drift mode.
- The language packages' public classes are `CuaDriver` and
  `AsyncCuaDriver` in Python, and `CuaDriver` in TypeScript.

### What parity exists today

The four session tools build their live `ToolDef` metadata from canonical
contracts through `ToolDef::from_contract`. Their invocation logic remains
handwritten, so this proves metadata parity, not behavioral or output parity.

The ten desktop contracts are separately authored `portable_subset`
declarations. The platform crates do not consume those input types and instead
own richer handwritten `ToolDef` schemas plus field-by-field `ArgsExt` parsing.
No test currently proves that each portable input schema is accepted by every
corresponding live platform schema.

Capabilities also have two declaration sites today: the runtime capability map
and each `ToolContract`. Existing tests compare them, but do not eliminate the
duplicate ownership.

### Platform roster and runtime shape

Registration-based counts at this revision are approximately:

- macOS: 48 tools;
- Windows: 49 tools; and
- Linux: 52 tools.

These counts must be verified against live `tools/list` on each OS before being
used as release evidence. They must not become fixed global assertions because
the rosters intentionally differ and evolve. For example, Windows registers
`debug_window_info`, while Linux registers four additional pointer primitives.

The daemon serves concurrent connections through one shared registry and tool
state. Core session hooks, activity, element tokens, and other facilities use
process-global singletons. On macOS, AppKit surfaces require the main thread and
permission identity is tied to the executable. These constraints are central to
the transport decision.

### What Fleet proves about UniFFI

Fleet provides a strong precedent for official UniFFI targets such as Python,
Kotlin, Swift, and Ruby: its checked-in generator uses pinned Rust dependencies
and supports regeneration checking.

Fleet also contains generated TypeScript examples:

- a Node N-API path using `uniffi-bindgen-react-native`, `@ubjs/core`, and
  `@ubjs/node` to load a native `cdylib`; and
- a browser/WASM path.

Those TypeScript artifacts prove feasibility, not a production-ready generation
pipeline. In the inspected tree, TypeScript is absent from Fleet's main
generation/check script, the React Native generator is not pinned in the Rust
workspace, the example `@ubjs/*` dependencies use `latest`, and the browser
WASM glue depends on configuration and generation that are not checked in.

Therefore:

- do not describe Fleet's TypeScript path as a reproducible template;
- use Cua Driver's existing deterministic `--check` flow as the standard the
  TypeScript UniFFI spike must meet; and
- evaluate Python and TypeScript UniFFI adoption independently.

## Required invariants

1. Rust remains the only owner of GUI execution, permissions, policy, session
   state, and platform integration.
2. Python and TypeScript do not reimplement OS automation behavior.
3. Every published typed method maps to Rust input and result types consumed by
   the live implementation.
4. MCP remains available regardless of the binding decision.
5. Experimental and runtime-only tools remain reachable through a generic
   call until intentionally promoted.
6. A transport never resends a state-changing tool call automatically.
   Bounded retries inside one platform implementation remain allowed when they
   are part of that single invocation and preserve its semantics.
7. Generated files are deterministic, ownership-scoped, atomically replaced,
   and checked for drift in CI.
8. Platform availability is explicit at the tool, variant, and field levels.
9. Capabilities and risk metadata have exactly one authoritative declaration.
10. Mixed MCP content remains a transport envelope: typed structured results do
    not erase text, images, refusals, degraded results, or diagnostics.

## Target architecture and public surfaces

```text
Transport-free Rust ToolSpec + typed inputs/results
                  |
                  +--> live ToolDef and tools/list metadata
                  +--> runtime typed input parsing
                  +--> structured-result validation
                  +--> portable and platform-rich projections
                  +--> contract manifest
                  +--> optional typed MCP-client generation
                  +--> optional UniFFI implementation exports

Agent surface:
Codex / Claude / other agent ---- MCP stdio ---- native daemon ---- OS APIs
Shell automation ---------------- CLI call ----- native daemon ---- OS APIs

Imported application SDK surface:
Python/TS typed client ----------- MCP stdio ---- native daemon ---- OS APIs
        or
Language binding ---------------- UniFFI ------- Rust daemon client ---- daemon
        or, after a separate host-runtime design
Language binding ---------------- UniFFI ------- embedded Rust engine ---- OS APIs
```

The transport-free specifications should live in the existing
`cua-driver-contract` crate or a narrowly scoped sibling `rlib`. All platform
crates must be able to depend on the types without importing MCP framing or a
public SDK runtime.

The typed layer must distinguish:

- the typed command input;
- the typed successful `structuredContent` payload, when present;
- stable typed refusals/errors; and
- the MCP content envelope that can contain text, images, and diagnostics.

A blanket adapter must not pretend every tool result is one serializable success
object. Agent examples must use the existing agent SDK's MCP client directly;
otherwise they would obscure the distinction this plan depends on.

## Workstream A0: close PR #2341's immediate parity gap

Add a mechanical subset-and-existence test for the ten desktop contracts before
the larger refactor.

For every declared platform and portable desktop tool, the test must prove:

1. the live registry contains the tool;
2. every portable property exists in the live input schema;
3. portable required fields remain required live;
4. portable types, enum/const values, bounds, and closed-object behavior are no
   broader than the live schema allows;
5. portable annotations and capability tokens are compatible with the live
   tool; and
6. the test fails on a deliberately introduced incompatible field change.

Implement a small schema-subset checker for the schema constructs used by the
ten contracts. Do not claim general JSON Schema implication. Fail explicitly on
an unsupported keyword so the test cannot silently accept an unproven case.

### A0 acceptance gate

- All ten portable contracts pass against macOS, Windows, and Linux registries.
- The workflow runs when a contract, platform `ToolDef`, registry, or capability
  map changes.
- PR #2341's description accurately says this proves portable input/metadata
  compatibility, not full runtime behavior.

## Workstream A: make Rust the true source of truth

### A1. Establish the transport-free specification model

Define a `ToolSpec` model containing:

- canonical name and description;
- typed input;
- typed structured success payload, if applicable;
- stable refusal/error shapes;
- annotations and risk metadata;
- the sole capability declaration;
- per-platform tool presence;
- portable versus rich exposure metadata; and
- schema and contract versions.

Add the new dependency from the macOS, Windows, and Linux platform crates to
the specification crate. Keep dependency direction acyclic and transport-free.

Do not commit to `schemars` up front. First run a schema-dialect spike on one
session input and one complex desktop input. It must reproduce the existing
required/optional, closed-object, enum/const, nullability, default, and bounds
semantics with a bounded override surface. If it cannot, keep the existing
schema builders while still making typed Rust inputs authoritative.

### A2. Add typed runtime adaptation

Introduce an adapter that:

1. deserializes MCP arguments into the shared typed input;
2. calls the platform implementation with that input;
3. validates/serializes stable structured output when declared;
4. preserves the existing MCP text/image/diagnostic envelope; and
5. normalizes stable errors without erasing platform diagnostics.

For a migrated tool, the live `ToolDef` must be derived from its spec and the
platform implementation must not retain a second schema literal or field-by-
field `ArgsExt` parser.

Start with the four session tools because their live metadata already comes
from canonical contracts. This proves the adapter and output model with the
lowest platform risk.

### A3. Model platform presence and rich variants once

The model must cover both field-level richness and tool-set divergence:

- portable desktop coordinates versus window/element targeting;
- platform-only fields and enum variants;
- Windows-only tools such as `debug_window_info`; and
- Linux-only low-level pointer primitives.

Portable SDK inputs must be derived projections of richer typed declarations,
not parallel handwritten schemas. If one public union would be misleading,
define separate typed commands such as `DesktopClickInput` and
`WindowClickInput`, but route them to the same implementation family.

### A4. Prove a divergent vertical slice

Migrate:

- `start_session` and `end_session` for lifecycle and structured output;
- `get_desktop_state` for image plus structured metadata;
- `click` for portable versus rich targeting and destructive annotations;
- `get_window_state` for window-focused richness; and
- at least one tool absent from one or more platforms to prove presence
  modeling.

The slice is complete only when:

- all participating platform implementations consume shared inputs;
- live `tools/list` metadata is spec-derived;
- representative structured results validate against their declared schemas;
- the MCP envelope preserves text and images; and
- generated Python and TypeScript methods pass executable fixture tests.

### A5. Migrate the current fourteen-tool SDK surface

After the slice stabilizes, migrate all fourteen methods currently generated by
PR #2341. Remove duplicated schemas and ad hoc parsing for those tools. Keep
compatibility aliases only in dispatch and hidden from `tools/list` and SDKs.

### A6. Expand by tool family

Migrate stable tools in independently reviewable families:

1. remaining perception and desktop input;
2. window and app lifecycle;
3. browser;
4. recording and replay;
5. cursor, configuration, and diagnostics; and
6. remaining platform-specific stable tools.

Each runtime roster entry must be classified as stable typed, experimental
generic-only, compatibility alias, or internal/hidden. Do not encode one global
tool count.

## Workstream B: separate agent integration from SDK distribution

### Agent integration: direct MCP/CLI — baseline recommendation

Configure each agent runtime's existing MCP client with `cua-driver mcp`, or
use `cua-driver call` from shell-oriented agents. Do not require generated Cua
language clients in this path. Maintain runnable Python Claude Agent SDK and
TypeScript Codex SDK examples as proof.

This is the O(1) agent-protocol surface: one MCP server is consumable from N
MCP-capable runtimes. Generated clients add ergonomics for applications, not
interoperability for these agents.

### SDK option 1: thin generated MCP clients

Continue generating Python and TypeScript types/methods while small native
transports speak MCP stdio directly.

This preserves:

- the daemon's permission identity and main-thread ownership;
- process and crash isolation;
- the current generic `tools/list` / `tools/call` escape hatch;
- small language packages without an additional client `cdylib`; and
- Cua Driver's already-reproducible TypeScript generation/check pipeline.

The remaining handwritten language code is a few small transport, result, and
facade modules. This is a typed remote-client SDK, not a native implementation
binding. Measure it against the actual application SDK requirement.

### SDK option 2: UniFFI facade over a Rust daemon client — gated spike

Build a platform-neutral Rust client that owns MCP framing and result
normalization while still launching or connecting to the native driver daemon.
Export only the vertical-slice API through UniFFI.

Run two distinct experiments:

- Python through official UniFFI support; and
- Node TypeScript through `uniffi-bindgen-react-native` and `@ubjs/node`.

The TypeScript experiment starts from a green-field reproducibility assumption,
not from Fleet's manual artifacts. It must add:

- exact pinned generator and runtime versions;
- an in-repository generation command;
- deterministic owned outputs and `--check` mode;
- a lockfile and publishable package definition;
- host `cdylib` resolution from Cargo compiler-artifact output; and
- isolated package tests for every supported platform/architecture.

Do not implement browser/WASM packaging for local desktop automation in this
spike.

The purpose of this option is to implement application client behavior once in
Rust and distribute that implementation to N runtimes. Merely moving JSON-RPC
framing into Rust is insufficient benefit; the spike must identify the shared
lifecycle, normalization, policy, or server-composition behavior it owns.

### SDK option 3: UniFFI embedded/server implementation — separate proposal

This is the path for application developers who want to create or embed a Cua
server. Do not treat it as a fallback transport change. A separate proposal
would first need to eliminate or explicitly host:

- process-global session and element-token state;
- one-registry assumptions and concurrent-session cleanup hazards;
- macOS AppKit main-thread ownership and permission identity;
- Windows interactive-session, UIAccess, foreground, and overlay behavior;
- Linux compositor, X11/Wayland, and session-bus integration; and
- host crash, runtime, and cancellation coupling.

### B1. Measure the SDK candidates

For SDK option 1 and each language-specific SDK option 2 spike, record:

- generated and handwritten maintained source lines;
- wheel/npm artifact size by platform and architecture;
- cold start, first call, and steady-state read-only call latency;
- sync and async Python behavior and async TypeScript behavior;
- error, refusal, image, and structured-result fidelity;
- generic runtime-only tool access;
- install and native-loader failure modes;
- release-matrix and CI duration changes; and
- current timeout/deadline behavior without implying tool cancellation.

### B2. Language-split SDK decision gate

Decide Python and TypeScript independently. UniFFI may pass for Python and fail
for TypeScript.

Adopt SDK option 2 for a language only if:

- it materially reduces maintained SDK behavior;
- generated outputs are reproducible and drift-checked in-repository;
- all generator/runtime dependencies are pinned;
- installation, startup, generic access, and error fidelity do not regress;
- its full supported artifact matrix passes; and
- MCP remains available as a public boundary.

For TypeScript, failure to provide pinned deterministic generation and CI
checking is an automatic rejection regardless of runtime benchmark results.

Otherwise retain SDK option 1. The source-of-truth work is complete and
valuable regardless of this decision. Neither outcome changes direct MCP/CLI
as the recommended agent integration.

## Workstream D: define cancellation and timeout semantics

Cancellation is not currently a runtime capability: dispatch awaits a tool
directly, serve-layer deadlines do not cancel a running action, and individual
tools contain their own bounded retry/polling behavior.

Do not block A0 or the initial typed migration on inventing cancellation.
Before claiming cancellation parity or adopting a UniFFI transport, make one
explicit decision:

1. implement caller cancellation by threading a token/deadline through the
   `Tool` trait and typed adapter, defining safe interruption points and action
   completion semantics; or
2. document best-effort client deadlines that stop waiting but do not cancel an
   in-flight tool.

In either case, transports must never automatically resend an ambiguous
state-changing request. Tests must distinguish transport resend, caller
cancellation, and bounded retries within one tool invocation.

## Verification plan

### Contract and source tests

- A0 subset/existence parity for all ten portable desktop contracts.
- Every generated stable method maps to exactly one Rust spec.
- Every migrated platform tool consumes its shared input type.
- Migrated tools contain no second `ToolDef` schema or `ArgsExt` parser.
- Capabilities have one declaration site.
- Per-platform presence and portable projections are mechanically derived.
- Generated manifest and SDK outputs are deterministic and drift-free.

### Runtime behavior tests

- Minimal valid inputs use the same typed adapter as MCP.
- Invalid types, missing fields, unknown fields, bounds, and enums produce
  stable errors.
- Representative structured results validate against success schemas.
- Text, images, refusals, verification metadata, degraded results, and platform
  diagnostics survive unchanged.
- Compatibility aliases invoke canonical implementations but stay hidden.
- Deadline/cancellation tests assert only the semantics actually implemented by
  Workstream D.

### Platform tests

- macOS, Windows, and Linux compile and run registry/schema parity tests.
- Live roster evidence is captured from the exact source revision.
- Fixture-backed calls cover session lifecycle, desktop perception, pointer,
  keyboard, and window state without uncontrolled CI actions.
- Platform-only tools and fields appear only where declared.
- Session capture scopes (`auto`, `window`, and `desktop`) enforce identical
  policy behavior through generic calls and every generated binding option.

### Package tests

- Build isolated Python wheels and npm tarballs.
- Install into empty consumers with no repository-relative imports.
- Verify exact native artifact provenance and load paths.
- Run one common MCP fixture suite through every candidate.
- Assert public class names and reject stale `Client` exports.
- Assert package contents contain no stale generated/platform artifacts.

## CI and release wiring

The contract/SDK workflow must trigger on changes to:

- the specification crate and generated manifests;
- platform `ToolDef`, registry, and migrated implementation code;
- shared result/content envelopes and capability declarations;
- generators and ownership inventories; and
- Python, TypeScript, native artifact, and packaging code.

Required jobs:

- Rust formatting and contract/unit tests;
- A0 subset/existence checks for all three platform registries;
- typed runtime and structured-output tests for migrated tools;
- deterministic generation check mode;
- Python sync/async tests and isolated wheel consumption;
- TypeScript typecheck/tests and isolated npm consumption; and
- UniFFI generation/artifact jobs only while a spike or adopted binding exists.

Do not publish either binding option during the comparison.

## Delivery sequence

Keep commits independently reviewable:

1. `test(cua-driver): prove portable desktop contracts match live schemas`
2. `refactor(cua-driver): collapse tool capability ownership`
3. `refactor(cua-driver): introduce transport-free typed tool specs`
4. `refactor(cua-driver): route session tools through typed runtime adapter`
5. `refactor(cua-driver): migrate divergent desktop vertical slice`
6. `refactor(cua-driver): migrate current portable desktop SDK tools`
7. `test(cua-driver): spike UniFFI Python Rust daemon client`
8. `test(cua-driver): evaluate reproducible UniFFI Node bindings`
9. Decision commit: remove each rejected spike or adopt its production wiring.
10. Follow-up PRs: migrate stable tool families.

Before PR #2341 merges, land A0 and update its description to state the exact
guarantee. The complete typed-runtime migration can follow without presenting
the current manifest as full runtime parity.

## Completion criteria

### PR #2341 parity gap is closed when

- every portable contract is proven to exist and be accepted by each declared
  live platform schema;
- CI reruns that proof for every relevant contract/runtime change; and
- the PR description distinguishes subset compatibility from behavior parity.

### Typed Rust source-of-truth is complete for the current SDK when

- all fourteen generated methods use Rust inputs/results consumed by their live
  implementations;
- migrated tools have no duplicate schemas or ad hoc field parsing;
- live metadata and representative structured outputs pass parity tests;
- capabilities have one owner;
- all three registries pass their declared support checks; and
- changing a Rust field causes a compile failure or deterministic generated
  diff in every consumer.

### UniFFI evaluation is complete when

- the target product is explicit: typed daemon client or embedded/server SDK;
- direct agent MCP examples remain independent of every generated client;
- thin typed clients and each language-specific UniFFI prototype pass
  equivalent behavior and artifact tests for the selected SDK target;
- measurements and platform matrices are recorded;
- Python and TypeScript decisions are documented independently;
- any adopted TypeScript pipeline is pinned, reproducible, and CI-checked; and
- all rejected spike code is removed.

### Broader typed SDK coverage is complete when

Every public runtime tool is either typed with explicit platform support or
intentionally classified as generic-only, with no unclassified `tools/list`
entry on any supported OS.

## Principal risks

| Risk                                                           | Mitigation                                                                                 |
| -------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Portable declarations drift before the refactor finishes       | Land A0 first and trigger it on platform/runtime changes.                                  |
| Derived schema dialect differs from existing contracts         | Gate `schemars` on a dialect-reproduction spike; keep bounded explicit builders if needed. |
| Platform richness creates unusable public unions               | Model separate typed commands and explicit per-platform presence from one spec family.     |
| Mixed MCP results do not fit one typed output                  | Type only structured payloads and stable errors; preserve the content envelope.            |
| Migration scope expands across large platform files            | Start with session tools, prove one divergent slice, then migrate by family.               |
| TypeScript UniFFI inherits an immature toolchain               | Treat it as green-field, pin everything, require deterministic generation and CI checking. |
| Native client libraries complicate distribution                | Measure the full matrix and retain thin MCP unless benefit is material.                    |
| Cancellation is claimed but not implemented                    | Complete Workstream D or document non-cancelling deadline semantics.                       |
| Direct embedding breaks permission or global-state assumptions | Keep it outside this plan and require a separate OS-specific proof.                        |

## Non-goals

- Reimplementing GUI actions in Python or TypeScript.
- Removing the MCP server or CLI.
- Publishing experimental packages during the comparison.
- Promising browser-local desktop automation through WASM.
- Directly embedding the GUI engine as part of the SDK-generation work.
- Migrating every runtime tool before the typed foundation and decision gate are
  stable.

## Review disposition

Claude Code Opus independently reviewed the repository, the original plan,
the three platform registries, the current SDK generators, and Fleet's UniFFI
artifacts. This revision incorporates its must-fix findings:

- Fleet TypeScript is described as feasible but not reproducibly wired;
- the live tool counts and parity claims are corrected;
- A0 closes the immediate portable-subset gap;
- capability ownership and platform-crate dependencies are explicit;
- schema derivation is gated instead of assumed;
- typed structured output is separated from mixed MCP content;
- cancellation is treated as missing runtime design work; and
- UniFFI adoption is language-split; direct MCP/CLI remains the agent baseline,
  while thin MCP and UniFFI are compared only for imported application SDKs.

## Implementation outcome and binding decision (2026-07-21)

The implementation keeps MCP/CLI as the public agent boundary and makes typed
Rust the source for the current fourteen-tool typed-client SDK surface:

- Schemars-derived Rust input and structured-output types deterministically
  generate the checked-in manifest and Python/TypeScript sources.
- The four session tools consume the types directly. Each OS desktop branch
  deserializes the shared portable projection before acting, while its richer
  window/element path remains available to generic MCP callers.
- Successful SDK-path structured payloads are deserialized through the shared
  Rust output types in the live registry. The MCP content envelope remains
  unchanged, including images, text, diagnostics, and errors.
- The ten portable schemas are checked as logical subsets of each live
  platform schema on Linux, macOS, and Windows. Unknown schema keywords fail
  the implication checker closed.
- Capability tokens for all fourteen tools resolve from the contract; the
  legacy runtime capability map now covers only non-SDK tools.

### Measured typed MCP-client baseline

Measurements were taken from this checkout at version `0.10.0` before package
publication:

| Candidate          | Maintained source | Generated source |                             Universal package |
| ------------------ | ----------------: | ---------------: | --------------------------------------------: |
| Python MCP SDK     |         676 lines |        331 lines |                 10,666-byte pure-Python wheel |
| TypeScript MCP SDK |         278 lines |        275 lines | 4,923-byte npm tarball; 19,506 bytes unpacked |

Both clients retain generic `call_tool` access and preserve the full MCP
content/result envelope. Their executable fixture suites cover initialization,
typed calls, errors, images, and generic runtime-only calls.

### UniFFI repository spike

The comparison used the latest checked-in Fleet precedent at
`c2ba0b5e94d0f2c06d0c7efb0913803ca0a616af` rather than creating a second
throwaway binding implementation with the same toolchain:

- Fleet proves that official UniFFI Python bindings can be generated,
  drift-checked, and exercised against a host-native cdylib.
- Fleet also proves that Node and browser TypeScript generation is technically
  possible through `uniffi-bindgen-react-native`; this is not merely WASM.
- The checked-in Fleet TypeScript roots contain about 4,777 generated lines
  (220 KiB), require `@ubjs/core`, `@ubjs/node`, and a colocated native cdylib,
  and are not part of Fleet's pinned four-language generation/check script.
  The runnable example declares those runtime/build dependencies as `latest`.
- The Fleet Python generated root contains about 5,675 lines (228 KiB) for its
  larger API. Those source sizes are toolchain evidence, not a like-for-like
  API-size benchmark.

### Current pull-request package decision

**Python: retain the typed MCP client in this pull request.** UniFFI itself
passes the feasibility and reproducibility gate, but Cua Driver does not yet
have a shared Rust daemon client with meaningful application behavior. A
facade over today's transport would add a per-platform, per-architecture cdylib
and native-loader failure mode to a 10.4 KiB universal wheel while still
speaking to the existing daemon.

**TypeScript: retain the typed MCP client in this pull request.** The current
precedent fails the plan's automatic adoption gate: generation is outside the
pinned drift-check script, the runtime packages are unpinned, and consumers
must locate a host-native library. Technical feasibility does not yet make it a
reproducible npm release pipeline.

This package decision does not reject a UniFFI embedded/server SDK and does not
claim the generated clients improve MCP interoperability. MCP-capable agents
should connect directly, as the agent examples demonstrate. A follow-up SDK
proposal must decide whether its consumer needs a shared Rust daemon client or
an embedded/server implementation, then evaluate UniFFI against that target.

No rejected UniFFI spike code is retained. Cold-start and steady-state
comparisons are intentionally not claimed: neither native candidate passed the
packaging/reproducibility pre-gates that justify producing a release-matrix
prototype. Reconsider Python when a reusable Rust daemon client or supported
embedded host exists; reconsider TypeScript after pinned generation,
deterministic CI checking, and native package loading land in the repository.
