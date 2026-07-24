# CUA Driver Binding Opportunities

## Scope and evidence

This analysis uses shallow clones created on July 20, 2026:

| Repository | Revision inspected | Role |
|---|---:|---|
| `trycua/cua` | `b3765c2` | CUA Driver source and Rust workspace |
| `trycua/cloud` | `75f4501` | Current deployment and consumer pattern |
| `mozilla/uniffi-rs` | `d1f766f` | UniFFI implementation and documentation |

The driver is in `cua/libs/cua-driver/rust`, not at the CUA repository root. Its workspace members include the executable `cua-driver`, reusable `cua-driver-core`, `cua-driver-uia`, and the three OS backends: `platform-macos`, `platform-windows`, and `platform-linux` (`cua/libs/cua-driver/rust/Cargo.toml:3`). The executable is currently a binary-only crate: its root is `crates/cua-driver/src/main.rs`, which keeps its modules private (`cua/libs/cua-driver/rust/crates/cua-driver/src/main.rs:21`). The reusable Rust implementation is `cua-driver-core`; its module root exports a broad set of implementation modules and re-exports only `RecordingSession` (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/lib.rs:35`).

The current product boundary is MCP/CLI/daemon rather than an in-process SDK. The driver describes itself as an MCP JSON-RPC daemon over stdio (`cua/libs/cua-driver/rust/crates/cua-driver/src/main.rs:3`), and Cloud packages it that way: its desktop workspace systemd service runs `cua-driver mcp` behind an MCP Streamable HTTP gateway (`cloud/desktop-workspace-fileops/files/cua-driver-mcp.service:1`). This matters: generated language bindings should complement, not replace, the already portable MCP boundary.

## ZeroMQ philosophy applied to CUA

The relevant ZeroMQ idea is not "generate every language package ourselves." It is to make the core's contract small, stable, independently buildable, and easy for language communities to wrap; then let those communities own the idiomatic packages, release cadence, and credit.

For CUA, separate the following responsibilities.

| CUA owns: stable core library and contract | Binding owners own: idiomatic distribution and ecosystem work |
|---|---|
| The Rust driver engine, OS backends, capability and approval semantics, policy enforcement, and security fixes. | Package naming, package-manager publishing, language ergonomics, platform packaging, and integration with language-native tools. |
| A narrow `cua-driver-ffi` facade with a semantic-versioned, documented object model. | Kotlin/Gradle, SwiftPM/Xcode, PyPI, RubyGems, NuGet, or Go module repositories and CI. |
| The UDL (or equivalent UniFFI metadata), ABI compatibility policy, conformance fixtures, and a generated binding smoke-test contract. | Generated-code check-ins if their ecosystem expects them, convenience APIs, docs, examples, and community support. |
| Security advisories and a minimum supported core version. | Timely downstream releases, with a documented response target for security and compatibility updates. |

Do not make the existing internal crate's public visibility the foreign-language API. `cua-driver-core` exposes implementation modules because the platform crates need them; that is different from promising those modules as a supported SDK. Bind a purpose-built facade crate, perhaps `crates/cua-driver-ffi`, whose only job is to translate a small, durable object model into the existing platform registry and policy path.

MCP remains the best default for remote control, polyglot agents, and Cloud-hosted desktops. The facade is useful when a host process needs local, in-process calls, wants typed result objects, or cannot cheaply manage a subprocess and JSON-RPC connection. Both surfaces should share the same driver semantics and conformance tests.

## UniFFI-rs fit assessment

### What UniFFI provides

UniFFI is a Rust component toolchain that compiles a Rust `cdylib` and generates bindings that load and call its C-compatible FFI layer. Its own README calls it a multi-language bindings generator and says that a Rust object model can be described with an interface definition file or proc macros (`uniffi-rs/README.md:1`, `uniffi-rs/README.md:8`).

Its first-party generators are Kotlin, Swift, Python, and Ruby (`uniffi-rs/README.md:22`). Its source tree has exactly those four built-in binding backends under `uniffi_bindgen/src/bindings/`. The same README lists third-party C# and Go generators, as well as JavaScript/React Native, Kotlin Multiplatform, Dart, Java, and Node projects (`uniffi-rs/README.md:51`). Therefore C# and Go are viable ecosystem routes, but they are not Mozilla-maintained generator targets and should not be described as first-party UniFFI support.

There are two interface-description approaches:

1. **UDL.** A `.udl` file declares the namespace, records, enums/errors, functions, and opaque interfaces independently of Rust. The UDL guide explicitly says it controls which functions, methods, and types appear in foreign bindings (`uniffi-rs/docs/manual/src/udl/index.md:3`). `build.rs` runs `uniffi::generate_scaffolding("src/<name>.udl")`, as shown in the upstream todo-list example (`uniffi-rs/examples/todolist/build.rs:5`).
2. **Proc macros.** Rust items are annotated or derived, principally with `#[uniffi::export]`, and a proc-macro-only crate invokes `uniffi::setup_scaffolding!()` (`uniffi-rs/docs/manual/src/proc_macro/index.md:24`, `uniffi-rs/docs/manual/src/proc_macro/index.md:36`). This avoids duplicating Rust signatures in a UDL file, but UniFFI documents the facility as newer and potentially changeable (`uniffi-rs/docs/manual/src/proc_macro/index.md:21`).

For CUA, use a **UDL-first facade**. It provides a language-neutral contract that external binding authors can review, track, and feed to third-party generators without parsing the Rust source. Proc macros may implement the facade internally, but the published UDL should be canonical while CUA is trying to foster independently owned bindings.

At the ABI boundary, UniFFI supports a constrained object model: scalar types, strings, byte sequences, optional values, sequences/maps, records, enums, errors, and opaque objects. Opaque objects are passed as handles backed by Rust `Arc` values; UniFFI's bindgen source describes objects as pointers to `Arc` handles (`uniffi-rs/uniffi_bindgen/src/interface/ffi.rs:123`). The foreign bindings lift and lower those values, manage object lifetimes, translate declared errors, and schedule asynchronous calls through UniFFI's future/callback machinery. This is an ABI contract, not a way to expose arbitrary Rust types.

### Can UniFFI express the current driver API?

Not directly, and it should not try. UniFFI can express the *behavior* of a useful driver facade, including an opaque `Driver` object, records/enums, declared errors, byte images, asynchronous `call` methods, and selected callbacks. It cannot faithfully or cleanly expose the current internal API as-is.

The following current public items illustrate why:

| Current item | Why it is not an FFI contract | Facade treatment |
|---|---|---|
| `Tool` is an `async_trait` trait with `async fn invoke(&self, args: serde_json::Value) -> ToolResult` (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:287`). | A Rust plugin trait, dynamic dispatch, Tokio future, and `serde_json::Value` are implementation details. Foreign callers must not implement platform tools by accident. | Keep it internal. Make `Driver::call(tool_name, request)` invoke it after validating a concrete request type. |
| `ToolRegistry` stores `HashMap<String, Box<dyn Tool>>` (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:295`) and takes `Box<dyn Tool>` in `register` (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:312`). | Trait objects and registry mutation are not an ownership model for a cross-language API. | Own the registry entirely inside an opaque `Driver` handle. Do not export registry construction or registration. |
| `ToolRegistry::iter_defs` and `tool_names` return `impl Iterator` (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:377`, `cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:389`). | `impl Trait` and borrowed iterators cannot cross a stable FFI boundary. | Return `Vec<ToolDescriptor>` instead. |
| MCP `Request`, `ToolCall`, `Response`, and `ToolResult` contain `serde_json::Value` (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/protocol.rs:10`, `cua/libs/cua-driver/rust/crates/cua-driver-core/src/protocol.rs:180`, `cua/libs/cua-driver/rust/crates/cua-driver-core/src/protocol.rs:286`). | Generic JSON values do not map to UniFFI records/enums. The API is also MCP transport-shaped, rather than host-SDK-shaped. | Translate to explicit records. As a low-friction first release, retain schemas and unmodeled tool arguments as validated JSON strings, not `Value`. |
| `StdioObserver` and `SessionObserver` are `Arc<dyn Trait>` callback hooks (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/server.rs:17`, `cua/libs/cua-driver/rust/crates/cua-driver-core/src/session.rs:133`). | They are process-global telemetry internals and callbacks have threading/reentrancy/lifetime costs. | Do not export in v1. Add one narrowly scoped UniFFI callback interface only if a consumer has a real need for progress or approval events. |
| The platform crates expose `register_tools*() -> ToolRegistry`, for example Linux (`cua/libs/cua-driver/rust/crates/platform-linux/src/lib.rs:125`), macOS (`cua/libs/cua-driver/rust/crates/platform-macos/src/lib.rs:48`), and Windows (`cua/libs/cua-driver/rust/crates/platform-windows/src/lib.rs:72`). | These functions choose OS-specific machinery, cursor overlays, and main-thread setup. They must remain Rust-owned. | A platform-selected `Driver::new(options)` calls the appropriate function internally. Document platform availability and initialization requirements. |

UniFFI supports asynchronous exported functions and methods, so CUA does **not** need to turn each future into a user-visible callback just to make it bindable. Export a facade method as asynchronous and let each generated binding use its own idiom. Callbacks are appropriate only for true reverse-direction events such as progress, host approval, or cancellation notifications. UniFFI supports callback interfaces, but a foreign callback may be invoked later and on a different thread; upstream Swift templates explicitly retain callback state for the process lifetime (`uniffi-rs/uniffi_bindgen/src/bindings/swift/templates/CallbackInterfaceImpl.swift:125`). That is a poor default for the driver's global observer hooks.

UniFFI also rejects generic exported impl blocks: its proc macro emits "generic impls are not currently supported by uniffi::export" (`uniffi-rs/uniffi_macros/src/export/item.rs:75`). Do not expose Rust generics, trait objects, `impl Trait`, borrowed references/lifetimes, raw Tokio channels, or `anyhow::Result` in the facade. Convert them to concrete records/enums, owned strings/bytes/vectors, and a small declared `DriverError` enum.

### Recommended facade

Create a new library crate rather than changing public behavior in `cua-driver-core`:

```text
cua/libs/cua-driver/rust/crates/cua-driver-ffi/
  Cargo.toml                 # cdylib + rlib; depends on core and selected platform backend
  build.rs                   # uniffi::generate_scaffolding("src/cua_driver.udl")
  src/lib.rs                 # private adapter, platform selection, error translation
  src/cua_driver.udl         # published, versioned language-neutral contract
  uniffi.toml                # namespaces/package names; optional binding configuration
```

Make `Driver` an opaque, internally synchronized handle. Its constructor owns the platform registry and any main-thread or overlay initialization sequence. Its public v1 methods should be intentionally small:

| Facade item | Shape | Purpose |
|---|---|---|
| `DriverOptions` | record | `compat_mode`, optional cursor configuration, and explicit local-platform options. Keep security mode and policy path separate, documented, and fail-closed. |
| `Driver` | opaque interface | Constructs one local driver instance and owns the `ToolRegistry`. |
| `list_tools()` | `sequence<ToolDescriptor>` | Returns names, descriptions, annotations/capability tokens, and `input_schema_json`. |
| `call(tool_name, arguments_json)` | async `CallResult` | Invokes the existing registry after JSON parsing and current authorization/policy checks. Preserve the MCP tool names initially. |
| `CallResult` | record | `is_error`, `content: sequence<ContentPart>`, and optional `structured_content_json`; use `bytes` for image payloads rather than base64 where feasible. |
| `start_session`, `get_session_state`, `end_session` | async concrete records/errors | Optional v1 convenience methods only if they map to a stable CUA session contract; otherwise expose them as named tools through `call` first. |
| `DriverError` | declared error enum/record | Invalid JSON, unknown tool, unauthorized/refused, unsupported platform, unavailable display, and internal failure with a safe message. |

The first release may use `arguments_json`, `input_schema_json`, and `structured_content_json` to avoid prematurely freezing every evolving tool schema. That is an intentional transitional boundary, not a claim that JSON is the final ergonomic SDK. Once particular tool groups are stable, add typed convenience methods additively (`screenshot`, `click`, `type_text`, `get_screen_size`) without removing generic `call`.

### Concrete refactor candidates

1. **Add a library facade instead of exporting the executable.** `cua-driver` contains only a private-module `main.rs` (`cua/libs/cua-driver/rust/crates/cua-driver/src/main.rs:21`), so it cannot become a binding artifact without conflating CLI startup with embeddable initialization. Add `cua-driver-ffi` as a `cdylib` and leave the binary untouched.
2. **Narrow the public Rust surface.** `cua-driver-core/src/lib.rs` makes many modules public (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/lib.rs:35`). Preserve that temporarily for workspace use, but establish `cua-driver-ffi` as the only compatibility promise to foreign consumers. Over time, use `pub(crate)` or an internal workspace crate for implementation-only modules where feasible.
3. **Keep the dynamic tool plugin system private.** The `Tool` trait's async `serde_json::Value` method (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:287`) and registry's `Box<dyn Tool>` storage (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:295`) are correct for Rust platform implementations but unsuitable for UniFFI. The facade should hold `Arc<ToolRegistry>` or a mutexed equivalent and export concrete methods only.
4. **Translate borrowed/generic return types to owned collections.** `iter_defs` and `tool_names` expose iterator return types (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:377`, `cua/libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs:389`). Add a private adapter that collects descriptors into `Vec<ToolDescriptor>`.
5. **Separate protocol transport from API data.** `protocol::Content` and `ToolResult` are close to useful transport data but depend on base64 and `Value` (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/protocol.rs:244`, `cua/libs/cua-driver/rust/crates/cua-driver-core/src/protocol.rs:286`). Convert them in the facade into a closed `ContentPart` enum and owned records. Do not expose `Request`/`Response`, because those are JSON-RPC transport types.
6. **Do not bind global observers in v1.** `set_stdio_observer` accepts `Arc<dyn StdioObserver>` (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/server.rs:26`), and session observers are similarly global (`cua/libs/cua-driver/rust/crates/cua-driver-core/src/session.rs:154`). They pose callback ownership and threading risks. Export polling state or a single documented event callback later, only with a concrete user.
7. **Make platform initialization explicit.** macOS cursor-overlay setup requires a caller to run work on the OS main thread (`cua/libs/cua-driver/rust/crates/platform-macos/src/lib.rs:70`). The facade must either provide one safe initialization path or return an explicit `PlatformInitializationRequired` error. Do not let every binding author rediscover this requirement.

## Concrete binding opportunities

The table deliberately separates an upstream-supported contract from independently owned packages. "Likely owner" means the community or maintainer profile to recruit; it is not a claim that a specific person or organization has agreed.

| Target language | Likely community owner | Use case unlocked | What the binding author needs from CUA |
|---|---|---|---|
| **Kotlin** (first-party UniFFI) | Android/Kotlin Multiplatform developer who already maintains an agent, device-lab, or remote-desktop client. | Android control-plane/mobile companion for a remote CUA desktop; JVM integration with local desktop automation services. CUA currently has desktop OS backends, so this is not a promise to run the driver natively on Android. | Versioned `cua_driver.udl`; reproducible Android/JVM `cdylib` build matrix; Gradle sample; prebuilt native artifacts or a documented artifact pipeline; remote-vs-local support matrix. |
| **Swift** (first-party UniFFI) | macOS/iOS developer or a SwiftPM maintainer for an agent host. | In-process macOS host integration and iOS control-plane client for a remote CUA driver. An iOS binding alone cannot grant iOS the macOS/Windows/Linux driver backend. | UDL; SwiftPM/Xcode packaging example; macOS entitlement/TCC and main-thread initialization documentation; universal-archive build script; clear iOS remote-only statement. |
| **Python** (first-party UniFFI) | ML/research lab, benchmark maintainer, or Python SDK maintainer. | Low-overhead local execution in evaluation harnesses, data collection, computer-use research, and notebooks; avoids implementing a subprocess/MCP wrapper for every experiment. | UDL; `maturin`/wheel or equivalent packaging recipe; wheel build matrix; typed Python examples; explicit GIL, async, and image-byte usage notes. UniFFI generates flat modules, while packaging remains the binding maintainer's responsibility (`uniffi-rs/docs/manual/src/python/configuration.md:30`). |
| **Ruby** (first-party UniFFI) | Ruby automation/Rails community maintainer. | Browser/desktop workflow automation and CUA orchestration in existing Ruby systems. | UDL; gem skeleton and native-library loading recipe; CI matrix; Ruby examples. UniFFI Ruby bindings require the `ffi` gem (`uniffi-rs/docs/manual/src/ruby/configuration.md:17`). |
| **C#** (third-party UniFFI generator) | .NET desktop/enterprise automation maintainer, ideally a Windows CUA contributor. | Native Windows agent-host, test harness, and enterprise desktop automation integration; C# is especially natural near the Windows backend. | Same stable UDL and ABI fixtures; a pinned third-party C# generator version; NuGet package conventions; Windows RID/native asset plan; parity tests. C# generator ownership stays with its external project and CUA's C# package owner. |
| **Go** (cbindgen adjunct; optional third-party UniFFI Go route) | Go infrastructure/agent-platform maintainer. | Go control-plane services, schedulers, and local sidecars that want in-process calls. For remote drivers, the MCP client remains simpler. | A tiny explicit C ABI facade generated with `cbindgen`, header/version checks, ownership/freeing rules, and Go `cgo` wrapper. Alternatively, adopt the third-party `uniffi-bindgen-go` against the same UDL, but do not pretend cbindgen itself generates Go bindings: it emits the C/C++ header, and the owner writes/maintains the Go layer. |

The recommended order is Python first, then Swift/Kotlin, then C# or Go when a maintainer volunteers. Python has the clearest immediate research value. Swift/Kotlin validate UniFFI's intended mobile ecosystem but should lead with remote-control and macOS-host use cases, not imply unsupported mobile OS capture/input backends.

## What we ship vs. what we do not

### Ship: the minimal binding kit

CUA should ship the contract and proof that external packages can consume it:

- `crates/cua-driver-ffi/src/cua_driver.udl`, documented as the stable foreign API and versioned with CUA Driver releases.
- `crates/cua-driver-ffi/build.rs`, `Cargo.toml` with `crate-type = ["cdylib", "rlib"]`, and a tiny private Rust adapter that generates UniFFI scaffolding and maps the facade to the existing registry.
- `uniffi.toml` for canonical namespaces and package-name defaults, plus any supported custom type configuration.
- A `make bindings` or `cargo xtask bindings` target that runs `uniffi-bindgen generate` for Kotlin, Swift, Python, and Ruby, writes only to a disposable `target/bindings/` directory, and can validate third-party C#/Go generation when configured.
- ABI/API conformance fixtures: a deterministic fake or unsupported-platform test backend, expected `list_tools` descriptors, JSON argument/error cases, image-byte checks, and each language owner's smoke-test command.
- A `CONTRIBUTING.md` section, `docs/bindings.md`, and a `BINDING_OWNERS.md` registry that state support boundaries, ownership, release expectations, compatibility policy, and credit rules.

The existing example confirms the build shape is small: a UDL build script invokes `uniffi::generate_scaffolding` (`uniffi-rs/examples/todolist/build.rs:5`), and a library must be built as a `cdylib` for foreign consumers (`uniffi-rs/examples/sprites/Cargo.toml:8`).

### Do not ship: centrally maintained generated SDKs

CUA should explicitly not:

- Publish or maintain official generated Kotlin, Swift, Python, Ruby, C#, or Go packages merely because generation is possible.
- Commit generated binding source to the CUA core repository as a long-lived product surface.
- Accept responsibility for language-native API design, package-manager support, or downstream framework integration owned by a binding project.
- Expose the raw `ToolRegistry`, `Tool` trait, platform backend modules, global observer traits, or MCP JSON-RPC structs as the foreign API.
- Promise every CUA platform backend on every target language/runtime; package support must state whether it is a local-driver build, a remote-driver client, or both.

This is not abandonment. CUA provides the ABI, docs, test vectors, coordination, and security notices. A binding project provides the language package and earns visible ownership of it.

## Governance and credit

### Registry and repository structure

Keep the Rust facade in the CUA repository. Keep each mature language binding in its own upstream repository, for example `cua-driver-python`, `cua-driver-swift`, or an owner-selected name. The CUA repository should have a registry, not a monorepo of generated packages:

```text
cua/
  libs/cua-driver/rust/crates/cua-driver-ffi/
    src/cua_driver.udl
    build.rs
  docs/bindings.md
  BINDING_OWNERS.md
  tests/binding-fixtures/
```

`BINDING_OWNERS.md` should resemble ZeroMQ's bindings directory. Each entry should list: language; repository; primary and backup maintainers; package URLs; supported operating systems/architectures; local vs. remote scope; current compatible CUA Driver range; generator/toolchain; CI badge; conformance status; and security-contact process. The CUA website can render this registry as the canonical discoverability page without claiming all entries are CUA-maintained.

A binding may initially live under `bindings/<language>/` only as an incubation area, with a named external owner and an explicit graduation plan. Do not put generated files there. Move or mirror only owner-authored packaging, tests, and documentation if that helps an early maintainer; the end state is a separately owned repository.

### Versioning and release coordination

1. Version the UDL/facade as part of CUA Driver's SemVer release process. Additive records, enum variants handled by the binding strategy, optional fields, methods, and tools are minor-compatible only when documented as such. Renames/removals and incompatible semantic changes are major.
2. Publish a machine-readable compatibility manifest alongside the core artifact: facade version, ABI version, UniFFI version, supported platform triples, and minimum binding version. Do not couple every binding package's version number to the Rust crate; use a compatibility range instead.
3. Run the core facade's Rust tests and binding conformance fixture for every core release. Binding owners run their package CI against release candidates and publish on their own schedule.
4. Announce breaking changes, security fixes, and release candidates in a binding-owners channel and issue tracker. Give owners an agreed response window for security advisories; retain embargoes where needed.
5. Treat the MCP surface and in-process facade as related but independently versioned contracts. A new MCP tool should not silently require a generated binding update unless it changes the facade's stable types or behavior.

### Credit and maintainer autonomy

Adopt these rules in `CONTRIBUTING.md` and the registry:

- The binding repository README identifies its maintainer(s) first and states "built on CUA Driver's binding kit," not "official CUA package" unless CUA itself agrees to take ownership.
- Every registry entry links to its maintainers, package page, source repository, and contributors. The CUA site credits the binding project prominently.
- Changes in CUA that materially adapt a binding author's work preserve authorship. The existing repository guidance already requires preserving contributor credit, cherry-picking with provenance, and co-author trailers when adapting work (`cua/AGENTS.md:3`); apply that rule to binding work without exception.
- CUA maintainers review contract/security compatibility, not stylistic language-level choices. Binding owners choose their idioms, package layout, and convenience layers.
- An owner can transfer or retire a binding transparently. The registry records the status rather than leaving users to infer maintenance state from stale generated code.

## Decision

Proceed with a small, UDL-first `cua-driver-ffi` facade and a binding-owner registry. Do not bind `cua-driver-core` directly and do not centrally ship generated language packages. Keep MCP as the universal remote interface; offer UniFFI as a high-quality local embedding substrate. This puts CUA in charge of the difficult, security-sensitive driver semantics while giving Kotlin, Swift, Python, Ruby, C#, and Go communities a real package they can own, evolve, and receive credit for.
