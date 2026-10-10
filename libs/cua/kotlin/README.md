# cua SDK for Kotlin (generated, optional)

`src/main/kotlin/ai/cua/sdk/cua_sdk.kt` is the UniFFI Kotlin binding of
`cua-sdk` (package `ai.cua.sdk`), generated and drift-checked by
`libs/cua/scripts/generate-uniffi-bindings.mjs`. It needs JNA and the
`cua_sdk` native library on the JNA library path. Per the plan the Kotlin
surface is env + fleet; the binding exposes the whole API, and no Gradle
package or release artifact is produced yet.

Compile check (CI job `kotlin` in `ci-cua-sdk.yml`, container
`gradle:8.10.2-jdk17`): `gradle -p libs/cua/kotlin compileKotlin`.
Exported methods named `close` are renamed for Kotlin in
`crates/cua-sdk/uniffi.toml` (`closeSession`, `closeForward`,
`closeStream`, `closeClient` for `McpClient`), since `close()` is `AutoCloseable.close()`, which frees the
native handle.
