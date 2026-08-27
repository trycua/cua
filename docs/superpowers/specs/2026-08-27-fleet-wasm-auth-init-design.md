# Fleet WASM Authentication and Initialization Design

**Goal:** Make `@trycua/fleet` client-credentials authentication work in Node/WASM and make all exported synchronous UniFFI constructors safe immediately after importing the package.

## Scope

- Replace the WASM-incompatible `std::time::Instant` used by OAuth token caching.
- Initialize the package's WASM module once during ESM module evaluation.
- Preserve the existing generated Rust and TypeScript public APIs.
- Keep `uniffiInitAsync()` exported and idempotent for existing callers.
- Add local Node regression coverage without requiring live Fleet credentials.

## Token Timing

Use `web_time::Instant` with `std::time::Duration` in the Fleet SDK transport. `web-time` delegates to the standard library on native targets and provides a browser-compatible monotonic clock on `wasm32-unknown-unknown`, so the existing expiry, skew, cache, and 401 refresh logic remains structurally unchanged.

This is preferable to maintaining a custom `js_sys` clock shim because it preserves the current `Instant` API, supports checked arithmetic, and keeps target-specific JavaScript details outside the SDK.

## Package Initialization

Keep the existing memoized `uniffiInitAsync()` implementation, then await it at the top level of both generated package entry points before exports become usable. Static and dynamic ESM imports therefore complete only after the WASM module and both UniFFI binding tables are initialized.

This covers every synchronous generated FFI surface, including credentials, configuration record factories, and client constructors. It avoids patching generated classes individually and leaves explicit `await uniffiInitAsync()` calls harmless.

The accepted tradeoff is that importing `@trycua/fleet/node` or `@trycua/fleet/browser` always loads the WASM artifact and import failures surface during module evaluation.

## Regression Coverage

Add a Node test that imports the built package and immediately constructs `CyclopsCredentials` and a `CyclopsConfiguration` record without calling `uniffiInitAsync()`. A scripted `HttpClient` returns a local OAuth token response and a control-plane response, exercising token acquisition, expiry creation, caching, and authenticated request execution inside the WASM module.

The test asserts:

- synchronous constructors work directly after import;
- the token endpoint receives one client-credentials request;
- two authenticated calls reuse the valid token;
- the Authorization header contains the minted bearer token;
- no `Instant::now()` or `rustcallstatus_new` panic occurs.

Run the focused native OAuth tests as a compatibility check, then build and execute the package-level Node regression test. Browser artifact coverage remains responsible for validating browser bundling and loading.

## Non-Goals

- Changing OAuth request semantics or token refresh policy.
- Changing the static-token or access-token-provider APIs.
- Adding a second client-credentials wrapper API.
- Running a live Fleet lifecycle test with real credentials.
