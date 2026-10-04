// The Kotlin streaming example (examples/streaming/SCENARIO.md).
//
// Compiles the generated UniFFI binding (libs/cua/kotlin, package
// `ai.cua.sdk`) together with the example; the binding needs JNA and
// kotlinx-coroutines, and the `cua_sdk` native library at run time
// (`-Djna.library.path=<dir>` or
// `-Duniffi.component.cua_sdk.libraryOverride=<file>`).
plugins {
    kotlin("jvm") version "2.0.21"
    application
}

repositories { mavenCentral() }

dependencies {
    implementation("net.java.dev.jna:jna:5.15.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
}

kotlin { jvmToolchain(17) }

// The generated binding does not compile as checked in: `MediaSession`,
// `PortForward` and `SpaceStreamSession` export a Rust `close()` that UniFFI
// emits as `suspend fun close()`, which conflicts with the
// `AutoCloseable.close()` every UniFFI object gets ("Conflicting overloads").
// Until the SDK renames those methods, compile a copy with the async one
// renamed to `closeAsync()` (same FFI symbol, same behaviour).
val bindingSrc = rootDir.resolve("../../../libs/cua/kotlin/src/main/kotlin")
val patchedBinding = layout.buildDirectory.dir("generated/cua-binding")
val patchBinding by tasks.registering(Copy::class) {
    from(bindingSrc)
    into(patchedBinding)
    filter { line: String -> line.replace("suspend fun `close`()", "suspend fun `closeAsync`()") }
}
sourceSets {
    main {
        kotlin.srcDir(patchedBinding)
    }
}
tasks.named("compileKotlin") { dependsOn(patchBinding) }

application {
    mainClass.set("ai.cua.examples.streaming.MainKt")
    // Where the host cua-sdk library lives (cargo build --release -p cua-sdk).
    val libDir = System.getenv("CUA_SDK_LIB_DIR")
        ?: rootDir.resolve("../../../libs/cua/target/release").canonicalPath
    applicationDefaultJvmArgs = listOf("-Djna.library.path=$libDir")
}

tasks.named<JavaExec>("run") {
    // Pass the scenario environment through unchanged.
    environment(System.getenv())
}
