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
Its foreground service displays a notification with a Stop action, obtains
read-only downscaled frames, and renews the lease without a host controller.
Activity recreation or backgrounding preserves the service, session, and target.
Stop drains the in-flight request before cleanup. Start during cleanup queues
one restart after confirmed release; another Stop cancels that restart.
Cleanup errors remain errors and never trigger an automatic restart. Process death loses the
controller; the runtime expires the unrenewed lease and destroys the target.
A fresh process never silently adopts that session or starts another one.

Construct `AndroidDriver(context)` for typed suspend operations and results.
The SDK dispatches blocking IPC off the UI thread. A signing-identity-checked
provider returns the shell runtime's Binder, and responses travel through
bounded pipes. Shell clients instead use a local socket with peer UID checks.
Android's SELinux policy remains enforced. `DriverClient.call` remains available
as the lower-level blocking JSON API.
The runtime admits the demo UID only if its installed signature matches the
installed runtime APK; it checks session ownership on session operations.
A session ID alone grants no access; using the SDK from another APK does not
authorize it. Pass a caller-owned `requestId` to a typed operation when it needs
a retained identity. `DriverRefusedException`, `DriverUncertainException`, and
`DriverRuntimeException` preserve that ID. `DriverRequestCancelledException`
also reports whether dispatch may have occurred: cancelling a coroutine does
not retract native input. No operation automatically retries an uncertain result.

For example, from a coroutine in the admitted demo app:

```kotlin
val driver = AndroidDriver(context)
val created = driver.createSession(SessionOptions(allowedApps = listOf("ai.cua.fixture.notes")))
val sessionId = created.data.sessionId
try {
    val target = driver.launchApp(sessionId, "ai.cua.fixture.notes").data
    val frame = driver.snapshot(sessionId, target.targetId).data
    // frame.png contains PNG bytes; use its full-resolution geometry for input.
} finally {
    withContext(NonCancellable) { driver.stopSession(sessionId) }
}
```

The typed API covers doctor/capabilities, session lifecycle, launch, snapshots,
preview, taps, and swipes. Results retain the request and runtime generation IDs
and optional action evidence. A native event's accepted transport remains an
unverifiable application effect.

## Current constraints

- One session with one owned task per allowlisted package (at most eight).
  `app launch` creates a fresh task or switches to that session's existing task.
  Every switch returns a new target ID and invalidates old snapshots. Existing
  tasks outside the session are refused; an uncertain launch requires Stop.
  Stop verifies display removal and destruction of every owned task.
- Caller-owned 60-second lease; renew with `session renew`. CLI exit does not
  destroy the session, but no persistent host broker renews it automatically.
  The demo's user-started foreground service renews every 10 seconds, including
  while its Activity is backgrounded or recreated. Grant notification permission
  to see its notification and Stop action. Force-stopping the app prevents
  renewal; the runtime lease bounds cleanup after process death.
- Gestures last at most one second. Stop serializes behind an admitted gesture;
  the proposed 250 ms Stop-admission target is not implemented in this slice.
- Rotation other than zero refuses input. Snapshot handles expire after five
  seconds. Input also requires the bound producer frame to be no older than
  five seconds. For a static app, actionable capture replaces the ImageReader
  output surface and requires a genuinely new compositor frame. It never
  rewrites a cached frame's producer timestamp. Preview may show older frames
  with their reported age and does not refresh the actionable snapshot's age.
- The runtime retains the last 128 mutation request IDs/outcomes, including
  uncertain results, until process death. Retention is bounded; no exactly-once
  guarantee survives eviction or restart. Do not blindly retry after either.
- Semantic text, raw keyboard input, accessibility refs, target verification
  predicates, event subscriptions, general app support, and human IME concurrency
  remain unsupported or unqualified. The debug demo has a bounded visual agent
  loop for the two real apps documented below; this is not general app support.
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
python3 scripts/lifecycle-smoke.py --device emulator-5554 \
  --driver ../rust/target/debug/cua-driver --evidence-dir /tmp/android-lifecycle-evidence
./gradlew :sdk:testDebugUnitTest :demo:testDebugUnitTest
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

The lifecycle harness grants notification permission only to the selected
guest's synthetic demo. It preserves one session through three real Activity
recreations, backgrounds the app for 65 seconds without host requests, verifies
renewal/preview progress, resumes, and stops through the notification action.
It also force-stops the controller,
checks expiry-driven task destruction, and requires an explicit fresh start.
The debug-only `ai.cua.android.demo.RECREATE` Activity action exists for this
test; it has no effect in a non-debuggable build. The evidence provider marks
persisted service state as stale after process death.

Deterministic controller tests hold creation, preview, and cleanup in flight to
exercise Stop/Start races, repeat Start, cancel a queued restart, reject
unconfirmed cleanup, and stop using a changed runtime. SDK tests cover typed
responses, request correlation, refusals, uncertain results, and cancellation.

### Record a scripted SDK demonstration

The optional instrumentation harness runs real SDK snapshots and taps from the
demo's UID, checking the synthetic counter after every tap. It waits up to
60 seconds for Start, continues while the Activity is on Home, and exits after
Stop. It is a scripted fixture, not a model-driven agent.

```bash
./gradlew :demo:assembleDebug :demo:assembleDebugAndroidTest -PcuaRecording=true
adb -s emulator-5554 install -r demo/build/outputs/apk/debug/demo-debug.apk
adb -s emulator-5554 install -r demo/build/outputs/apk/androidTest/debug/demo-debug-androidTest.apk
adb -s emulator-5554 shell am instrument -w \
  ai.cua.android.demo.test/ai.cua.android.demo.RecordingInstrumentation
```

Instrumentation restarts the demo process; stop any existing session first.
While the command runs, open the demo, press Start, and let at least five taps
complete before pressing Stop. The test retains the stopped UI for 10 seconds
so a screen recorder can capture it before instrumentation exits. Debug builds
declare visibility of the synthetic fixture provider for this test's oracle;
release builds do not include that declaration. Record the main display with
Android's `screenrecord` and label synthetic ADB text as synthetic input.

Native Android evidence supplements rather than replaces the repository's
canonical desktop E2E gates when shared desktop behavior is changed. Keep the
implementation PR draft until the intended scope and remaining qualification
gates are reviewed.

### Run a visual model agent in two real apps

The debug demo's **Run agent** button starts an Android foreground-service loop:
snapshot, model decision, fresh-frame verification, then SDK action. It renews
the lease independently of inference and updates the read-only preview. Stop
cancels inference, drains admitted runtime requests, and releases the workspace.
The loop is bounded to 40 decisions and 10 minutes. An uncertain input stops the
run; it is never blindly retried. `blocked` is distinct from successful `done`.

Inference is relayed through the host's authenticated Claude Code CLI. The
relay has no shell, file, browser, or Driver tools available to the model. Its
structured-response tool only formats a decision. Android owns the session,
control loop, app switching, input, and cleanup. This prototype therefore needs
the relay connection; it is not an offline language model running on the phone.
The runtime is an explicitly authorized shell-UID helper, not a system-UID app.

The selected APKs are unmodified upstream releases:

| App | Package | Version | Source |
| --- | --- | --- | --- |
| OpenCalc | `com.darkempire78.opencalculator` | 3.2.1 | [GPL-3.0 source and APK](https://github.com/clementwzk/OpenCalc/releases/tag/v3.2.1) |
| Tasks.org | `org.tasks` | 15.10, publisher's F-Droid variant | [GPL-3.0 source and APK](https://github.com/tasks/tasks/releases/tag/15.10) |

Verify APK digests and signing certificates before installing in the disposable
emulator. In Tasks.org, continue without sync and create synthetic tasks through
its normal UI, for example **Order 42 snack packs** and **Book the venue**. These
are demo setup data; no private database mutation or app modification is needed.
Close both app tasks before starting a workspace. The runtime refuses to adopt
an already running task from display 0, including a `singleTask` activity.

Run the inference relay with an external evidence directory and a private token
file containing 32–512 URL-safe random characters:

```bash
python3 scripts/agent-relay.py --token-file /tmp/android-agent/relay.token \
  --evidence-dir /tmp/android-agent/inference --model opus
```

The server binds only `127.0.0.1:8788`, authenticates every request, validates
PNG dimensions and bounded JSON, and admits one inference at a time. Raw
screenshots, task/history data, model identity, and decisions stay in the chosen
local evidence directory; keep them out of Git and public reports. Model API
credentials are not copied into Android. The private demo config is
`files/agent-config.json`; only debug builds accept this agent entry point.
Disconnecting a request cancels its model subprocess and releases the relay
slot. Clients must keep their sending half of the connection open while waiting;
the relay treats a TCP half-close as cancellation too.
An extra outer `reason` around an otherwise valid structured action permits one
formatting repair within the original timeout, either inside Claude Code's
formatter flow or through one further inference attempt. The corrected response
must retain every operational action field; only its explanatory `reason` may
change. A second repair, other schema failures, tool-policy errors, and uncertain
driver actions are refused. Rejected responses remain in the local evidence.

From another terminal, pass a task file and the two allowed apps to the harness:

```bash
python3 scripts/agent-demo.py --device emulator-5554 \
  --token-file /tmp/android-agent/relay.token \
  --task-file /tmp/android-agent/task.txt \
  --app com.darkempire78.opencalculator --app org.tasks \
  --evidence-dir /tmp/android-agent/recording --record --background-seconds 12
```

Example task: use the calculator to work out six groups of seven snack packs,
then complete only the matching existing snack-order item in Tasks.org. The
model receives screenshots and its action history; it never receives fixture
oracle data. After slow inference, Android compares a new frame's decoded
pixels, target, display, rotation, and runtime identity. A changed observation
gets up to four additional captures, 150 ms apart, to observe the same blinking
caret phase. If none match exactly, the app requests a new decision. An exact
match uses the new snapshot's input handle; no pixels are masked or ignored.
The original five-second runtime freshness checks still apply.

The harness configures `adb reverse`, presses Run agent, types synthetic text
on display 0, optionally backgrounds/resumes the controller, and checks task
cleanup. It does not choose or send target-app actions. Successful completion
retains a verified PNG labeled **Last frame · workspace closed**. The raw MP4 is
a continuous Android `screenrecord` capture. Use `--stop-during-inference` for
the cancellation case and omit `--record` for ordinary qualification.
The harness requires every requested app to have been visited. Its `done`
status is the model's assertion; inspect the final image and action evidence
independently before claiming that the application's task was completed.

Additional focused checks:

```bash
python3 scripts/multiapp-smoke.py --device emulator-5554 \
  --driver ../rust/target/debug/cua-driver \
  --app com.darkempire78.opencalculator --app org.tasks \
  --evidence-dir /tmp/android-multiapp-evidence
python3 -m unittest discover -s scripts -p 'test_agent_relay.py'
./gradlew :runtime:testDebugUnitTest :sdk:testDebugUnitTest :demo:testDebugUnitTest
```

The multi-app check exercises static capture, stale handles, repeated switching
with stable task IDs, display-0 focus, and destruction of both tasks on Stop.
The recording remains emulator qualification of these selected apps and touch
operations. Physical phones, ordinary third-party app UIDs, human IME isolation,
and Fleet integration remain follow-up work.
