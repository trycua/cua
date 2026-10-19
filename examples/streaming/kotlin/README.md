# Streaming example: Kotlin (headless)

The shared scenario ([`../SCENARIO.md`](../SCENARIO.md)) on the generated
UniFFI Kotlin binding of cua-sdk (`libs/cua/kotlin`, package `ai.cua.sdk`,
JNA). It does the same steps as the Swift example, headless only: stream
the desktop and the grid window (the SDK decodes to BGRA and PCM), hash the
last frame (FNV-1a 64), write `desktop.wav` / `window.wav`, click grid cell
(2, 3), and print `SUMMARY {...}`. It also runs as a bench lane
(`CUA_BENCH_JSONL`, `tc_ms` decoded from the frames). `cpu_*` in the bench
JSONL comes from the JVM (`ThreadMXBean` user time, and process CPU minus
that for sys) because the JVM has no `getrusage`.

## Layout

- `build.gradle.kts`: Kotlin/JVM + `application`. Dependencies are JNA,
  kotlinx-coroutines (used by the binding's async calls) and
  kotlinx-serialization-json (JSON parsing only, no compiler plugin).
- The binding is compiled from `libs/cua/kotlin/src/main/kotlin` after a
  one-line patch copied into `build/generated/cua-binding`. As checked in, it
  does not compile: `MediaSession`, `PortForward` and `SpaceStreamSession`
  get both `suspend fun close()` (the Rust method) and
  `AutoCloseable.close()`, which Kotlin reports as "Conflicting overloads".
  The patch renames the async one to `closeAsync()`. The FFI symbol does
  not change.

## Build

No JDK is needed on the host. Compile inside the official Gradle image:

```sh
docker run --rm --memory=4g --memory-swap=4g \
  -v "$PWD":/work -v "$HOME/.gradle-docker":/gh -e GRADLE_USER_HOME=/gh \
  -w /work/examples/streaming/kotlin gradle:8.10.2-jdk17 \
  gradle --no-daemon -q installDist        # or compileKotlin
```

(run from the repo root). With a local JDK 17+ and Gradle, `gradle installDist`
in this directory does the same.

## Run

The binding needs the cua-sdk native library for the JVM's OS and arch, on
`-Djna.library.path`. The start script and `gradle run` use
`$CUA_SDK_LIB_DIR`, or `libs/cua/target/release` by default.

- **macOS host with a JDK**:
  `cargo build --release -p cua-sdk` in `libs/cua`, then
  `CUA_SDK_LIB_DIR=libs/cua/target/release gradle run`, with the scenario
  variables (`CUA_ENV_URL`, `CUA_ENV_TOKEN`, ...) exported.
- **Docker (Linux), the tested path**: build `libcua_sdk.so` for Linux in a
  Rust image. `libs/cua` has path dependencies on `libs/cua-spacesd` and
  `libs/cua-driver`, so mount the whole repo. Then run the installed
  distribution in the Space's network namespace. From the repo root:

```sh
docker run --rm --memory=4g --memory-swap=4g -v "$PWD":/work:ro \
  -v cua-e2e-kotlin-target:/target -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 \
  -w /work/libs/cua <rust image with clang/cmake> cargo build --locked --release -p cua-sdk

eval "$(libs/cua/bench/streaming/scripts/space.sh env)"   # CUA_ENV_TOKEN
docker run --rm --memory=4g --memory-swap=4g --network container:cua-e2e-bench-space \
  -v "$PWD":/work -v cua-e2e-kotlin-target:/target:ro \
  -e CUA_ENV_URL=http://127.0.0.1:3211 -e CUA_ENV_TOKEN -e CUA_STREAM_SECONDS=5 \
  -e CUA_OUT_DIR=/tmp/out -e JAVA_OPTS=-Djna.library.path=/target/release \
  gradle:8.10.2-jdk17 /work/examples/streaming/kotlin/build/install/cua-streaming-kotlin/bin/cua-streaming-kotlin
```

  The bench lane works the same way with `-e CUA_BENCH_JSONL=... -e
  CUA_BENCH_TARGET=... -e CUA_BENCH_SECONDS=... -e CUA_BENCH_AUDIO=...`.

## Notes (the same as the Swift example)

- **Click.** It first sends a media-plane `action` on a session opened with
  `SESSION_POLICY_ALLOW_ACTIVATION`. If the fixture log shows no press, it
  falls back to `ComputerService.Pointer` with `DELIVERY_FOREGROUND` (auto
  delivery uses X11 `XSendEvent`, which GTK3 ignores).
- **`keyframes` / `bytes` are `null`.** The decoded callbacks do not carry
  them.
- **Stop the bench fixture before the scenario.** The timecode window is
  keep-above at (128, 96) and covers grid cell (2, 3), so the foreground
  click lands on it and `logged` is false.
- **Audio allocations.** `PcmAudio.samples` is a `List<Short>` in this
  binding (boxed). That is fine for an example, but it allocates heavily per
  audio frame.
