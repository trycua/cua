# Experimental local Android Driver

This is the first runnable local slice of [RFC 3673](https://github.com/trycua/cua/issues/3673).
It is not a stable Android backend or a complete implementation of the RFC.

The Kotlin runtime owns one virtual display, capture, a bounded touch queue,
session expiry, and outcome evidence. A host `cua-driver --device SERIAL` route
and an on-device shell CLI use the same runtime as the Android SDK demo app.
There is no `android` command namespace. Existing desktop dispatch is unchanged
when no Android connection selector is supplied.

## Build

Requirements: JDK 21, Android SDK Platform 37.0, Build Tools 37.0.0, and an
authorized ARM64 Android 17 Google APIs emulator. Set `JAVA_HOME`, `ANDROID_HOME`,
and optionally `ANDROID_USER_HOME`/`ANDROID_AVD_HOME` for your environment.
Accept the applicable Google SDK/system-image terms yourself before installation.
The Gradle wrapper pins Gradle 9.7.1 and its SHA-256; AGP is pinned to 9.4.0.

From this directory:

```bash
./gradlew :runtime:assembleDebug :sdk:assembleDebug \
  :demo:assembleDebug :fixture-target:assembleDebug
cd ../rust
cargo build -p cua-driver --bin cua-driver
```

Use that worktree's built binary for this experiment. These changes do not
install or replace an existing desktop Driver service.

## Install in a disposable development emulator

Put this SDK's `platform-tools` on PATH. The following explicitly installs or
updates three synthetic development APKs and starts the shell runtime:

```bash
python3 scripts/deploy.py --device emulator-5554
../rust/target/debug/cua-driver --device emulator-5554 doctor
```

The installed packages are `ai.cua.driver.runtime`, `ai.cua.fixture.notes`, and
`ai.cua.android.demo`. The helper APK and phone CLI live under
`/data/local/tmp/cua-driver`. The deployment script only signals a previously
recorded runtime PID after checking its command line. Runtime startup is explicit;
there is no boot receiver or permanent host service.

## Host CLI

Replace example identifiers with the results of previous commands. The initial
CLI outputs JSON; command names and supported flags are intentionally limited.
Put `--device SERIAL` before the command to select Android. Selectors inside
desktop subcommand arguments do not change the backend.

```bash
../rust/target/debug/cua-driver --device emulator-5554 session create \
  --size 1080x1920 --density 320 --allow-app ai.cua.fixture.notes
../rust/target/debug/cua-driver --device emulator-5554 --session SESSION_ID \
  app launch --package ai.cua.fixture.notes
../rust/target/debug/cua-driver --device emulator-5554 --session SESSION_ID \
  snapshot --target TARGET_ID --image target.png
../rust/target/debug/cua-driver --device emulator-5554 --session SESSION_ID \
  tap --snapshot SNAPSHOT_ID --x 540 --y 250
../rust/target/debug/cua-driver --device emulator-5554 --session SESSION_ID session stop
```

Touch coordinates refer to the full-resolution snapshot. Obtain a fresh snapshot
before another action; mutation invalidates the old handle. A tap's native API
acceptance returns `effect: unverifiable` and `actual_delivery: unknown` until
external evidence establishes the application outcome and interference profile.
It is not a claim of universally isolated Android input. PNG export refuses to
overwrite an existing path. No arbitrary shell command is accepted by the API.

## On-device CLI and SDK

Inside the authorized Android shell:

```sh
/data/local/tmp/cua-driver/cua-driver --local doctor
/data/local/tmp/cua-driver/cua-driver --local session create \
  --allow-app ai.cua.fixture.notes --size 1080x1920 --density 320
```

The local launcher uses Android's `app_process`; it is not a desktop Linux
binary. This slice qualifies the shell caller only. An ordinary Termux app UID
is not currently admitted. Desktop `--local` and remote `--connection` requests
refuse instead of accidentally controlling the desktop.

Open the demo app on display 0 and press Start to create an app-owned session.
It displays read-only downscaled frames and renews its lease on Android without
a host controller. Stop invokes runtime cleanup. The SDK's current
`DriverClient.call` is a blocking worker-thread API with JSON results; a complete
typed coroutine SDK remains follow-up work. Construct `DriverClient(context)`
inside an app: a signing-identity-checked provider returns the shell runtime's
Binder, and responses travel through bounded pipes. Shell clients instead use a
local socket with peer UID checks. Android's SELinux policy remains enforced.
The runtime admits the demo UID only if its installed signature matches the
installed runtime APK; it checks session ownership on session operations.
A session ID alone grants no access. Pass a caller-owned `requestId` to `call`
when an operation needs a retained identity; transport exceptions expose that ID
through `UncertainRequestException`. Never blindly retry an uncertain mutation.

## Current constraints

- One session and one launch attempt per session; stop and create a new session
  after an interrupted launch. A second launch never adopts an old task.
- Caller-owned 60-second lease; renew with `session renew`. CLI exit does not
  destroy the session, but no persistent host broker renews it automatically.
  The demo renews every 10 seconds while its Activity is alive. Activity
  recreation currently stops the session; a foreground lifecycle owner is pending.
- Gestures last at most one second. Stop serializes behind an admitted gesture;
  the proposed 250 ms Stop-admission target is not implemented in this slice.
- Rotation other than zero refuses input. Snapshot handles expire after five
  seconds. Input also requires the bound producer frame to be no older than
  five seconds; an unchanged screen whose producer has stopped submitting frames
  can therefore refuse input with `frame_stale`. Preview may show older frames
  with their reported age and does not refresh the actionable snapshot's age.
- The runtime retains the last 128 mutation request IDs/outcomes, including
  uncertain results, until process death. Retention is bounded; no exactly-once
  guarantee survives eviction or restart. Do not blindly retry after either.
- Semantic text, raw keyboard input, accessibility refs, target verification
  predicates, event subscriptions, general app support, and human IME concurrency
  remain unsupported or unqualified. There is no model-driven agent loop yet.
- Frames currently use bounded base64 IPC; full-resolution capture is separate
  from downscaled preview. Streaming/Surface transport remains follow-up work.
- Runtime installation requires Android 17/API 37. Framework calls and producer
  timestamps are qualified only on the documented emulator image; physical
  phones, vendor ROMs, and other Android versions remain untested.
- This shell helper has explicit development authorization. It does not yet use
  the desktop ToolRegistry permission/approval implementation and is not enabled
  by default in any shipped installation.

## Verification

```bash
python3 scripts/smoke.py --device emulator-5554 \
  --driver ../rust/target/debug/cua-driver --evidence-dir /tmp/android-driver-evidence
./gradlew :runtime:lintDebug :sdk:lintDebug :demo:lintDebug :fixture-target:lintDebug
```

The experimental Rust request definitions live in
`../rust/crates/cua-driver-contract/src/android.rs`. Rust tests and the device
harness both reject the shared cases in `contract/invalid-requests.json`.
The extension is separate from the published desktop schema.

The evidence directory must not exist. The smoke harness mutates only the named
synthetic fixture apps. It overlaps display-0 ADB text input with virtual-display
Driver taps, checks actual receipt/counters/focus and timestamp overlap, rejects
stale snapshots, verifies task destruction on Stop and lease expiry, and
exercises phone-local geometry, swipe, PNG export, duplicate mutation IDs, caller
denial, and an SDK-owned preview interval with no host requests. Corrupted event
copies check that its receipt oracle rejects wrong-display and missing-event
evidence; these are oracle tests, not deliberate misrouting by the runtime.
The synthetic user route is not hardware keyboard,
software-keyboard composition, physical phone, or complete disconnection proof.
The fixture providers expose synthetic state for the independent test oracle;
never put personal data into these apps or ship these providers in production.

Native Android evidence supplements rather than replaces the repository's
canonical desktop E2E gates when shared desktop behavior is changed. Keep the
implementation PR draft until the intended scope and remaining qualification
gates are reviewed.
