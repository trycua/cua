# Cyclops UniFFI SDK bindings

This directory contains the checked-in generated sources for several Cyclops
SDK targets. Rust is the native `cyclops-sdk` API and owns the canonical
implementation. The authoritative deterministic generation and drift pipeline
in `generate-sdk-bindings.sh` owns **Python, Kotlin, Swift, and Ruby**.
The OpenAPI JavaScript client and native-library packaging are outside that
four-language binding surface; separate UniFFI snapshots are described below.

### Separately generated targets

`go-uniffi` and `ts-uniffi` (Node.js) are checked-in compatibility snapshots
produced by `uniffi-bindgen-go` and `uniffi-bindgen-react-native`, not outputs
of `generate-sdk-bindings.sh`. `generate-compat-sdk-bindings.sh` owns their
narrow repository compatibility normalization: every snapshot is derived from
fresh raw generator output, then deterministic removal excludes only generated
builder ABI. It retains all non-builder generated content, including record
fields and serialization such as TTL fields, so these snapshots continue to
expose direct record constructors only. The normalizer never uses a checked-in
snapshot as transformation input.

`ts-uniffi-browser` (Browser/WASM) regenerates from Rust metadata during its
build, commits its TypeScript record modules, and verifies that generated
surface with an executable WASM builder artifact contract. Browser/WASM is an
advertised generated builder target alongside Python, Kotlin, Swift, and Ruby.

Go and Node.js remain separate compatibility snapshots. Adding builders to
their public contracts requires separately regenerating and validating each
third-party generator, cross-component converter layer, packaging path,
checked-in scope, and runtime API.

## Source of truth and compatibility

`cyclops-cs/sdk-schema` is the canonical schema source. The raw Kubernetes
CRD bundle at `clusters/base/osgym/crd.yaml` is derived from that schema with
`generate-crds`; it is not hand-maintained and must not be post-processed.
Short-term compatibility breaks in this evolving API are intentional. Update
the schema and raw CRD together rather than adding compatibility shims or
rewriters. The only binding-specific exception is the repository-owned Go/Node
compatibility normalization above: it deterministically removes generated
builder ABI from fresh raw output and does not alter the schema or CRD source.

Generated binding source is committed so review and drift checks are
reproducible. Native libraries, Cargo target output, Gradle caches, and staged
language runtime directories are not committed.

## Native HTTP transport

Native bindings can call `CyclopsClient.connect_with_native_http_client(configuration)` instead of implementing a foreign `HttpClient`. Static-token and token-provider flows use `connect_with_access_token_and_native_http_client(configuration, access_token)` and `connect_with_access_token_provider_and_native_http_client(configuration, provider)`. Existing `http_client` constructors remain the advanced path for proxying, custom CA trust, TLS policy, and tests. The native transport uses `reqwest` with Rustls/platform trust roots, a 30-second whole-request timeout, direct connections without proxy environment variables, and no automatic redirects, so bearer credentials are not replayed to redirect hosts. Duplicate headers and status bodies are preserved. Browser/WASM continues to use `connect_browser_with_access_token`.

## Prerequisites

- Rust `1.97.0` with `rustfmt` and `clippy` (`cyclops-cs/rust-toolchain.toml`).
- Python `3.11` for the Python contract fixture and source checker.
- A supported Ruby `3.3` runtime for the Ruby fixture.
- JDK `21` and Gradle `8.10.2` for Kotlin (or an equivalent checked-out
  wrapper that resolves Gradle `8.10.2`).
- A host C compiler and linker (`build-essential`/`pkg-config` on Ubuntu).
- macOS Swift requires Xcode's `swiftc`; the native library is named
  `libcyclops_sdk.dylib`. Linux uses `libcyclops_sdk.so`.

Every command below is cwd-independent. Set `REPO_ROOT` to the absolute path
of this checkout; CI uses `REPO_ROOT="$GITHUB_WORKSPACE"`.

```sh
REPO_ROOT=/absolute/path/to/cloud
export REPO_ROOT
```

## Generate and check

Generate the raw kube-derived CRDs after changing the schema:

```sh
cargo run --locked --manifest-path "$REPO_ROOT/cyclops-cs/Cargo.toml" \
  --package cyclops-sdk-schema --bin generate-crds -- \
  --output "$REPO_ROOT/clusters/base/osgym/crd.yaml"
```

Verify the committed CRD bundle without rewriting it:

```sh
cargo run --locked --manifest-path "$REPO_ROOT/cyclops-cs/Cargo.toml" \
  --package cyclops-sdk-schema --bin generate-crds -- --check \
  --output "$REPO_ROOT/clusters/base/osgym/crd.yaml"
```

Generate or check all four UniFFI language roots with the pinned workspace
wrapper around UniFFI `0.31.0`:

```sh
"$REPO_ROOT/cyclops-cs/scripts/generate-sdk-bindings.sh"
"$REPO_ROOT/cyclops-cs/scripts/generate-sdk-bindings.sh" --check
"$REPO_ROOT/cyclops-cs/scripts/generate-compat-sdk-bindings.sh"
"$REPO_ROOT/cyclops-cs/scripts/generate-compat-sdk-bindings.sh" --check
"$REPO_ROOT/cyclops-cs/scripts/test-generate-sdk-bindings.sh"
```

The regression harness verifies transaction rollback, inventory pruning,
generated-file modes, symlink rejection, and structural cdylib discovery.
It is intentionally a generator test, not compatibility post-processing.

Check the hand-written contract fixtures, then run the checker self-test that
proves comments alone do not satisfy source requirements:

```sh
"$REPO_ROOT/cyclops-cs/scripts/check-sdk-binding-contract-sources.sh"
"$REPO_ROOT/cyclops-cs/scripts/check-sdk-binding-contract-sources.sh" --self-test
```

## Build once and run bindings

The launchers stage a freshly built host cdylib in a temporary language runtime
and never copy it into generated source. To share one native build across
commands, use this target directory:

```sh
export CYCLOPS_SDK_NATIVE_TARGET_DIR="$REPO_ROOT/cyclops-cs/target/sdk-bindings-native"
"$REPO_ROOT/cyclops-cs/scripts/build-sdk-bindings-native.sh"
```

Run the Python contract and deterministic lifecycle example:

```sh
"$REPO_ROOT/cyclops-cs/scripts/run-python-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/python/tests/test_builders.py" -v
"$REPO_ROOT/cyclops-cs/scripts/run-python-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/python/tests/test_async_client.py" -v
"$REPO_ROOT/cyclops-cs/scripts/run-python-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/examples/python/app_controlled.py"
```

Run the Kotlin contract and example after staging the Linux cdylib:

```sh
export CYCLOPS_SDK_NATIVE_DIR="$(dirname "$CYCLOPS_SDK_NATIVE_TARGET_DIR/debug/libcyclops_sdk.so")"
gradle -p "$REPO_ROOT/cyclops-cs/sdk-bindings/kotlin" builderContract
gradle -p "$REPO_ROOT/cyclops-cs/sdk-bindings/kotlin" contract
gradle -p "$REPO_ROOT/cyclops-cs/sdk-bindings/kotlin" example
```

Run the Ruby contract and example:

```sh
"$REPO_ROOT/cyclops-cs/scripts/run-ruby-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/ruby/tests/test_builders.rb"
"$REPO_ROOT/cyclops-cs/scripts/run-ruby-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/ruby/tests/test_async_client.rb"
"$REPO_ROOT/cyclops-cs/scripts/run-ruby-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/examples/ruby/app_controlled.rb"
```

On macOS, run the Swift contract and full example. The documented runner
compiles `CyclopsSdk.swift` and `CyclopsSdkSchema.swift` together into one
`CyclopsSdk` module, includes both checked-in FFI module maps, and links with
an rpath to the host `libcyclops_sdk.dylib`.

```sh
"$REPO_ROOT/cyclops-cs/scripts/run-swift-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/swift/tests/TestBuilders.swift"
"$REPO_ROOT/cyclops-cs/scripts/run-swift-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/swift/tests/TestAsyncClient.swift"
"$REPO_ROOT/cyclops-cs/scripts/run-swift-sdk-binding.sh" \
  "$REPO_ROOT/cyclops-cs/sdk-bindings/examples/swift/AppControlled.swift"
```

## Typed lifecycle shape

### Generated record builders

The seven sandbox-pool records `VmTemplate`, `SandboxService`,
`OSGymSandboxTemplateSpec`, `CreateTemplateRequest`, `SandboxTemplateRef`,
`OSGymSandboxWarmPoolSpec`, and `CreatePoolRequest` expose generated builders.
Each setter returns a new immutable builder object; it does not mutate the
receiver. Keep the returned value, either by chaining calls or assigning it.
This `&self -> Arc<Self>` Rust receiver shape is portable across the four
official UniFFI targets and avoids foreign-language interior mutability.

Python:

```python
vm = (fleet_sdk.VmTemplateBuilder().container_disk_image(image)
      .image_pull_secret(secret).cpu_cores(4).memory("8Gi")
      .services([service]).build())
```

Kotlin:

```kotlin
val vm: VmTemplate = VmTemplateBuilder().containerDiskImage(image)
    .imagePullSecret(secret).cpuCores(4u).memory("8Gi")
    .services(listOf(service)).build()
```

Swift:

```swift
let vm: VmTemplate = try VmTemplateBuilder().containerDiskImage(value: image)
    .imagePullSecret(value: secret).cpuCores(value: 4).memory(value: "8Gi")
    .services(value: [service]).build()
```

Ruby:

```ruby
vm = FleetSdk::VmTemplateBuilder.new.container_disk_image(image)
  .image_pull_secret(secret).cpu_cores(4).memory('8Gi')
  .services([service]).build
```

Optional setters may be omitted; their record fields remain `None`, `null`, or
`nil` as appropriate. `build()` returns the exact existing record type and
reports an omitted required field through stable `SchemaBuildError` or
`SdkBuildError` variants (generated as `SchemaBuildException` and
`SdkBuildException` in Kotlin). Existing direct record constructors remain
available and unchanged. At the syntax-only derive boundary, optional record
fields must be spelled `Option<T>`, `std::option::Option<T>`, or
`core::option::Option<T>`; type aliases are not inferred.

Use the generated constructors and schema records rather than ad-hoc JSON.
The exact optional fields and language naming are generated, so consult the
checked-in language source for the complete constructor signatures. The Python
shape is representative:

```python
credentials = CyclopsCredentials("client-id", "client-secret")
client = CyclopsClient.connect(
    CyclopsConfiguration(
        base_url="https://api.example.test",
        token_url="https://issuer.example.test/oauth/token",
        credentials=credentials,
        pool_poll_interval_ms=1_000,
        pool_poll_limit=60,
        claim_poll_interval_ms=1_000,
        claim_poll_limit=60,
    ),
    transport,
)

pool_spec = PoolSpec(replicas=1, template=template, autoscaling=None, services=[])
pool = await client.create_pool(CreatePoolRequest(namespace="default", spec=pool_spec))

claim_spec = ClaimSpec(
    sandbox_template_ref=SandboxTemplateRef(name=pool.metadata.name),
    warmpool=None,
    bind_deadline=None,  # SDK defaults omitted deadlines to 900 seconds
    lifecycle=None,
)
claim = await client.create_claim(CreateClaimRequest(pool=pool, spec=claim_spec))
sandbox = await client.wait_claim(claim)
response = await client.service_request(sandbox, "mcp", "/health", request)
await client.delete_claim(claim)
await client.delete_pool(pool)
```

The Kotlin package is `ai.cua.cyclops.sdk`; Swift compiles as `CyclopsSdk`;
Python exposes records through its public `fleet_sdk` facade; Ruby exposes its generated `FleetSdk` module. The
checked-in contract tests and examples above are the executable, per-language
forms of this lifecycle.

### Foreign HTTP client boundary

`HttpClient` is asynchronous and foreign-language implemented. It is a
security boundary: the callback sees OAuth client credentials in token-form
requests, bearer tokens on authenticated requests, and resolved control-plane and
service URLs. Treat it as trusted transport code: do not log or export
those values, validate destination policy there, and preserve request/response
body semantics. The SDK strips caller-supplied service `Authorization` and
hop-by-hop headers before applying its own authentication.

## CI mapping

`.github/workflows/ci-cyclops-cs.yml` runs Linux Rust formatting, locked
clippy/tests, raw CRD drift, binding drift, the generator harness, source
checker/self-test, and Python/Kotlin/Ruby contracts plus examples. It pins
Rust `1.97.0`, Python `3.11`, Ruby `3.3`, JDK `21`, and Gradle `8.10.2`.

`.github/workflows/cyclops-sdk-swift.yml` runs on macOS, verifies generated
bindings and the portable source checker, builds the host cdylib, checks the
Mach-O architecture and library name, then runs both Swift fixtures through
the one-module/rpath runner.

## Troubleshooting

- `cargo must be available on PATH`: install Rust `1.97.0` and retry with the
  workspace toolchain active.
- CRD or binding drift: run the matching generation command above and commit
  only the raw generated source; do not patch it afterward.
- `libcyclops_sdk` cannot load: run `build-sdk-bindings-native.sh`, export the
  documented native target variables, and use the host-appropriate `.so` or
  `.dylib` filename.
- Kotlin cannot find JNA or the cdylib: use JDK 21/Gradle 8.10.2 and set
  `CYCLOPS_SDK_NATIVE_DIR` to the staged library directory.
- Swift cannot import FFI modules: run on macOS with Xcode Swift, keep both
  FFI module maps beside the generated Swift files, and use the supplied
  runner rather than compiling either generated file in isolation.

## Live examples

The `live_app_controlled` examples use the generated typed SDK records against a real Cyclops deployment. They create billable live resources. The regular `app_controlled` examples remain deterministic and offline.

Set these variables before running a live example: `CUA_BASE_URL`, `CUA_TOKEN_URL`, `CUA_CLIENT_ID`, `CUA_CLIENT_SECRET`, `CYCLOPS_NAMESPACE`, `CUA_IMAGE`, and `CUA_IMAGE_PULL_SECRET`.

```sh
cyclops-cs/scripts/run-python-sdk-binding.sh cyclops-cs/sdk-bindings/examples/python/live_app_controlled.py
CYCLOPS_SDK_NATIVE_DIR="$CYCLOPS_SDK_NATIVE_TARGET_DIR/debug" gradle -p cyclops-cs/sdk-bindings/kotlin liveExample
cyclops-cs/scripts/run-ruby-sdk-binding.sh cyclops-cs/sdk-bindings/examples/ruby/live_app_controlled.rb
cyclops-cs/scripts/run-swift-sdk-binding.sh cyclops-cs/sdk-bindings/examples/swift/LiveAppControlled.swift
```

The pull-request workflow obtains `CUA_CLIENT_ID`, `CUA_CLIENT_SECRET`, and `CUA_TOKEN_URL` from AWS Secrets Manager through GitHub OIDC. Forked pull requests are guarded from requesting those credentials, and every live job runs the language-independent fallback cleanup script under `if: always()`.
