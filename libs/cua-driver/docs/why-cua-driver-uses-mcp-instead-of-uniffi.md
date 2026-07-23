# Cua Driver integration surfaces: MCP/CLI and SDK bindings

Status: superseded in part by RFC 2447

Decision date: 2026-07-21

> This document preserves the rationale for the 0.11.0 daemon-client SDK.
> [RFC 2447](../../../rfcs/2447-cua-driver-native-core-and-mcp-adapter.md)
> keeps MCP as the agent boundary but replaces the imported SDK topology with a
> same-process runtime and makes MCP a downstream SDK consumer. Where this
> document says the SDK is only a daemon client, RFC 2447 is authoritative.

This document separates two Cua products that were previously discussed as if
they were one: Cua as a tool used by an agent, and Cua as an API imported by an
application. MCP and UniFFI solve different boundaries in those products and
are not competing protocol choices.

## The product distinction

| Surface                    | Consumer                                                  | Public shape                                        | What provides runtime portability                                                                                              |
| -------------------------- | --------------------------------------------------------- | --------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| Cua as an agent MCP or CLI | Codex, Claude Code, another agent, or a shell             | `cua-driver mcp` and `cua-driver call`              | MCP and the executable protocol already work from any capable runtime.                                                         |
| Cua as an imported SDK     | Python, TypeScript, Swift, Kotlin, or another application | `cua_driver` or `@trycua/cua-driver`                | Python and Node call a shared Rust daemon-client implementation through experimental UniFFI bindings.                          |

The [Codex and Claude Agent SDK examples](../examples/agent-sdks/README.md)
make the first surface concrete. Each agent SDK receives the same stdio MCP
server declaration and discovers the driver tools itself. Neither example
imports a generated Cua client.

## Decision

1. Keep MCP and the CLI as the canonical agent boundary. Do not require a Cua
   language package merely to connect an MCP-capable agent.
2. Make `cua-driver` and `@trycua/cua-driver` application SDK packages backed
   by UniFFI. Remove their language-native MCP facades because those duplicate
   a runtime-neutral protocol client that agent runtimes already provide. This
   deliberately makes the package API breaking before publication.
3. Export the shared Rust request records for all 14 typed tools and the shared
   typed session results. Desktop structured results retain platform extension
   fields, so the native SDK preserves them in `ToolResult.structured_json`
   while the live registry validates them with the canonical Rust result types.
4. Use UniFFI as an SDK/server-composition architecture, not as a replacement
   for the agent MCP boundary. The first slice exports a shared Rust daemon
   client that application-owned servers can compose.
5. Do not claim that direct engine embedding is ready today. Daemon-owned
   state, permission identity, and OS event-loop behavior still require an
   explicit embedding design.

This corrects an earlier framing in which a thin MCP client and UniFFI were
treated as alternatives for the same job.

## Cua as an agent MCP or CLI

```text
Codex / Claude / another agent
  -> that agent runtime's existing MCP client
  -> cua-driver mcp
  -> native driver daemon
  -> OS APIs

shell or automation
  -> cua-driver call
  -> native driver daemon
  -> OS APIs
```

MCP already defines tool discovery, invocation, results, and errors across
language runtimes. A Python agent and a TypeScript agent can consume the same
server without Cua generating either agent's protocol client. Adding generated
Python and TypeScript wrappers does not make this surface more runtime-neutral.

Application code that wants autocomplete and typed direct calls uses the
Rust-backed packages instead. The CLI remains useful for one-shot shell
automation and deterministic lifecycle calls around an agent run.

## Cua as an imported SDK

The removed thin MCP facade used this architecture:

```text
application
  -> generated Python or TypeScript method
  -> small language-native MCP transport
  -> cua-driver mcp
  -> native driver daemon
```

The imported SDK implemented by this change uses:

```text
application
  -> generated UniFFI binding
  -> libcua_driver_sdk
       - exported typed interfaces
       - result normalization and error mapping
       - shared daemon framing, timeouts, and observation metadata
  -> native driver daemon
  -> OS APIs
```

The retained architecture is where the O(1)-implementation-to-N-runtimes argument
applies. Product behavior implemented once in Rust can be exported to N
language runtimes without maintaining N behavioral clients. There is still
per-platform and per-architecture native artifact, loader, signing, and CI
work, so packaging is not literally constant effort. The important benefit is
that behavior and interfaces do not get independently reimplemented in every
language.

The implementation calls the daemon's direct socket protocol, not MCP. The CLI,
MCP proxy, and UniFFI library now import the same Rust request/response and
socket implementation from `cua-driver-core`, so timeout or framing changes
cannot drift by language. Application code can compose that object behind its
own server without reimplementing the Cua protocol. Direct engine embedding
remains a separate future architecture.

## Public entrypoint vocabulary

The package name is the SDK entrypoint; no transport suffix is required:

| Runtime | Rust-backed application SDK | Agent integration |
| --- | --- | --- |
| Python | `cua_driver` | Configure `cua-driver mcp` in the agent runtime |
| TypeScript | `@trycua/cua-driver` | Configure `cua-driver mcp` in the agent runtime |

There is no public `/sdk`, `/mcp`, or `/native` entrypoint. “Native” describes
the private generated loader and platform library, not the product API. The
Python `_native` modules and TypeScript `dist/native` folder remain private
implementation paths because they colocate UniFFI output with its platform
library.

## MCP and UniFFI are orthogonal

MCP answers: "How does an agent or process call a tool across a service
boundary?"

UniFFI answers: "How does application code in several languages call one Rust
library implementation?"

A product can support both. MCP remains the language-neutral agent boundary,
while UniFFI bindings can let application developers host or compose the Rust
server/client implementation. Removing MCP is neither required nor desirable
for adopting UniFFI.

## Why Fleet is useful precedent

Fleet places substantial portable client behavior in a Rust library loaded by
the host application:

```text
application
  -> generated UniFFI binding
  -> libcyclops_sdk
       - authentication and token refresh
       - resource lifecycle and polling
       - route construction and validation
       - concurrency control
  -> host-provided HttpClient
  -> remote Fleet API
```

Fleet's Rust SDK contains behavior that would otherwise be repeated in every
language. For example, [`client.rs`](../../fleet/sdk/src/client.rs) owns client
configuration and namespace lifecycle locks. [`transport.rs`](../../fleet/sdk/src/transport.rs)
owns OAuth token caching and refresh. The pool, claim, and service modules own
polling, request construction, validation, and lifecycle rules.

The host language supplies an [`HttpClient`](../../fleet/sdk/src/transport.rs)
callback. This lets Python, Swift, Kotlin, Ruby, or TypeScript use a suitable
HTTP implementation while Rust keeps the product behavior. UniFFI therefore
replaces substantial duplicated implementations, and the shared library is the
component the language API is meant to call.

Fleet also has a mature checked-in generation path for Python, Kotlin, Swift,
and Ruby. Its [`generate-sdk-bindings.sh`](../../fleet/scripts/generate-sdk-bindings.sh)
script pins UniFFI through the Rust workspace, generates into temporary roots,
tracks owned files, and checks drift.

Fleet's Node and browser artifacts also establish an important fact: TypeScript
UniFFI is not limited to WASM. The checked-in Node path uses
`uniffi-bindgen-react-native`, `@ubjs/core`, `@ubjs/node`, and native platform
packages. That path proved feasibility and informed Cua's generated loader and
optional npm platform-package matrix.

Fleet therefore demonstrates the relevant architecture: bindings are for
consumers that want the shared implementation, including consumers that may
create a server. MCP clients do not need those bindings merely to call an
already-running MCP server.

## What the SDK adds

The UniFFI surface adds:

- one same-process Rust desktop runtime, lifecycle implementation, error mapping,
  and result normalization for Python and Node;
- generated bindings for every one of the 14 published typed request records;
- canonical typed session results generated from the same Rust records returned
  by the live handlers; and
- an open-ended `call_tool` JSON escape hatch for runtime and server adapters.

It does not add:

- agent/runtime interoperability, because MCP already provides it;
- automatic publication of every native OS/architecture artifact.

`CuaDriver.create()` executes desktop behavior in the importing application and
does not require a daemon. The socket-backed `connect()` constructor remains for
external clients and permission identities that intentionally use the installed
host. An application can build an MCP or HTTP server around either SDK mode;
the transport remains downstream of the same public SDK contract.

## Compatibility and migration

Promoting UniFFI to the package roots is a deliberate breaking change to the
pre-release language APIs. It removes Python `AsyncCuaDriver`,
`CuaDriver.stdio()`, `*Args`, and transport classes, and removes the equivalent
TypeScript async facade and stdio transport. The replacement `CuaDriver` is the
synchronous Rust-backed SDK object created with `CuaDriver.connect(...)`.

Application consumers migrate their typed calls to the generated UniFFI input
records. Agent integrations remove the Cua language-package client entirely
and configure `cua-driver mcp` through the agent SDK's existing MCP support.

Package and runtime compatibility are harder to preserve. Native bindings add
failure modes that JavaScript and Python code can encounter before the SDK
connects to the driver:

- The package must contain a compatible library for each supported OS and
  architecture.
- The loader must find that library and satisfy platform signing and security
  policy.
- Native artifacts must work in each supported host, including Node-based
  desktop runtimes.
- Import-time native failures need a defined fallback path.

Consider an Electron application on Windows ARM64. The current TypeScript SDK
can load as JavaScript and launch the installed driver. If UniFFI becomes the
only transport, the same import may require a matching native artifact and
loader. A missing build, incompatible host runtime, signing rejection, or
library lookup failure can stop the application before it makes a driver call.
The TypeScript method signatures could remain unchanged while installation and
startup behavior break for that consumer.

The SDK must remain experimental until the complete platform and host matrix
loads successfully. Its package root intentionally requires the matching
native artifact; loader failures are therefore a documented package/runtime
compatibility risk rather than an implicit fallback to the removed MCP facade.

An embedded/server SDK is a new product surface and should be versioned as
such. Moving existing consumers from an out-of-process daemon to an in-process
engine can be behaviorally breaking even when method signatures match, because
permission identity, state lifetime, crash isolation, concurrency, and startup
requirements change.

Removing the executable MCP boundary or changing those behaviors would be a
separate breaking change. The package cleanup removes only duplicated language
MCP clients; `cua-driver mcp` remains supported. Adding a separate embedded SDK
can be additive, while silently changing this daemon client to embed the engine
would change process identity, permission ownership, state lifetime, and
failure isolation.

## Why direct engine embedding still needs design work

UniFFI could remove MCP only if Cua Driver moved the GUI engine into the SDK
consumer. The current runtime is not designed for that arrangement.

The public CLI routes tool execution through the required daemon so policy,
session state, and OS integration have one owner. On macOS, permission prompts
and grants attach to the running driver identity, and UI work depends on the
daemon's run loop. Windows also has daemon and UIAccess process concerns. Some
authorization and runtime configuration is fixed for the daemon lifetime. The
relevant startup and proxy rules are in [`cli.rs`](../rust/crates/cua-driver/src/cli.rs).

Embedding would therefore require decisions about host identity, permission
attribution, global state, event-loop ownership, concurrency, crash recovery,
and coexistence between multiple host applications. Binding the current Rust
functions would not answer those questions. We should treat an embedded driver
as its own product architecture, with OS-specific proofs, rather than as a
client-generation change.

## Costs and limitations

The MCP choice has costs that should remain visible:

- Requests and results cross a JSON and process boundary.
- Python and TypeScript each maintain a small transport implementation.
- The SDK must manage proxy startup, shutdown, timeouts, and protocol-version
  negotiation.
- Stable typed methods and the open-ended MCP envelope need separate modeling.
- Large screenshots pass through the protocol instead of an in-process buffer.

The implemented UniFFI SDK removes the language-native MCP hop but retains the
daemon process and JSON socket boundary. Its benefit is implementation and
interface distribution, not a claimed screenshot-throughput improvement. An
embedded/server SDK could remove the process hop only after the runtime issues
above are solved. No performance claim is part of this decision without a
comparable implementation and benchmark.

Desktop successful results also remain an extensible protocol envelope rather
than closed UniFFI records. Their canonical Rust validators intentionally carry
per-platform extension maps, which UniFFI cannot export as arbitrary
`serde_json::Value`. The SDK therefore returns typed text/image/error metadata
plus `structured_json` and `raw_json`; only the stable session results cross FFI
as dedicated records. Converting desktop results to closed records later would
require either a versioned stable core plus an extension JSON field or a
breaking removal of currently preserved platform data.

## Why Rust and the runtime still have parity

UniFFI is one way to make language bindings call Rust-defined interfaces. It is
not the only way to prevent contract drift.

The Cua Driver [`cua-driver-contract`](../rust/crates/cua-driver-contract/src/lib.rs)
crate binds each published tool to typed Rust inputs and successful structured
outputs. Those types produce the manifest and are exported through the compiled
UniFFI SDK. The live session handlers consume the same inputs directly. Each
desktop backend deserializes the shared portable projection before acting, and
the live registry validates successful structured payloads with the same Rust
output types. CI also proves that every portable schema is accepted by each
supported OS registry.

This closes the contract/runtime bookkeeping gap without generating a second
Python or TypeScript MCP contract layer. MCP remains the agent process boundary;
UniFFI distributes the Rust application SDK implementation. The contract
architecture is described in the [contract README](../contract/README.md).

## Implemented generation and packaging boundary

Mozilla UniFFI `0.31.0` generates Python from the compiled `cdylib` metadata.
Node generation uses the separately maintained
`uniffi-bindgen-react-native`/UBRN `0.31.0-3` N-API target plus pinned
`@ubjs/core` and `@ubjs/node` runtimes. The checked-in outputs are regenerated
into temporary roots, tracked by ownership manifests, and compared byte for
byte in CI.

One current UBRN limitation is explicit and tested: version `0.31.0-3` cannot
configure an external UniFFI component to load symbols from the parent SDK
`cdylib`. The generator therefore performs one asserted post-processing step so
the contract namespace and SDK namespace both load `libcua_driver_sdk`. If the
expected generated selector changes, generation fails instead of silently
patching an unrelated string.

Python wheels are platform-specific and the Rust release workflow
places the matching SDK library beside the CLI in each release runtime archive.
The wheel builder moves that library next to the generated Python modules, and
CI inspects the wheel and runs it across the FFI boundary. The public Node
package uses generated platform resolution and optional native packages for
macOS arm64/x64, Linux glibc arm64/x64, and Windows arm64/x64. Release CI builds
those packages from the already verified Cua Driver assets, smoke-tests the
installed root plus matching native package, publishes native packages first,
then publishes the root. Python, npm, and Rust versions come from one release
tag and are attached to the same GitHub release.

## UniFFI follow-up gates

Evaluate the two SDK targets separately.

Before expanding the UniFFI path to another runtime or platform:

1. Load-test the native artifact on the added release OS and architecture.
2. Add the platform package to generated resolution and the release matrix.
3. Preserve release signing/notarization evidence for the native library everywhere
   the platform requires it.
4. Run the executable MCP boundary and imported SDK through the same daemon
   conformance corpus so behavior remains equivalent across the two product
   surfaces.

The embedded SDK has a supported Rust-owned host model with documented
permission identity, endpoint ownership, generation-scoped lifecycle,
concurrency, parent liveness, and recovery. MCP remains the agent boundary;
UniFFI owns application-side lifecycle and typed calls.

Python and TypeScript should be evaluated independently because their binding,
loader, and packaging toolchains differ. MCP and the CLI remain supported for
agents regardless of either SDK decision.

## Evidence snapshot

This decision used:

- Fleet commit `c2ba0b5e94d0f2c06d0c7efb0913803ca0a616af`, which added the checked-in
  Go and TypeScript UniFFI artifacts;
- Cua Driver SDK commit `896172754074f8c7ac26745685989df661714da6` before this
  explanatory document;
- the implementation measurements and adoption gates in the
  [Rust source-of-truth and UniFFI evaluation](sdk-rust-source-of-truth-and-uniffi-evaluation-plan.md).

The Fleet source had not changed between its cited commit and the latest
`main` inspected on 2026-07-21. Recheck the generation and packaging claims if
Fleet's binding pipeline changes.
