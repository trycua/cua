# OpenKoalaBots (Swift)

The Swift sample of OpenKoalaBots (package `OpenKoalaBotExample`, macOS 14,
SwiftPM). The app is called OpenKoalaBots.

The sample links Cua Spaces pieces (`CuaSpacesStreaming`, in
[`libs/spaces-app-swift`](../../libs/spaces-app-swift)), so it is
source-available under the
[Functional Source License, Version 1.1, MIT Future License](LICENSE)
(FSL-1.1-MIT), like Cua Spaces itself. See [LICENSING.md](../../LICENSING.md).

## Quick start

```sh
git clone https://github.com/trycua/cua && cd cua/samples/openkoalabot-example-swift
(cd ../../libs/cua && cargo build --release -p cua-sdk && node scripts/stage-uniffi-library.mjs --only=swift)
swift build && .build/debug/OpenKoalaBotExample
```

**Local workspace override.** `Package.swift` depends on
`../../libs/spaces-sdk-swift`, `../../libs/spaces-app-swift` and
`../../libs/cua/swift` by path, and the `Cua` package links the cua SDK dylib
staged into `libs/cua/swift/lib` by the second command. That is how the sample
is developed inside this repository. A copy taken on its own
(`npx degit trycua/cua/samples/openkoalabot-example-swift`) needs those three
packages next to it at the same relative paths, or the three
`.package(path:)` lines pointed at your checkout of them.

## About

A modern desktop chat app for agent coworkers, built on Cua Spaces, used to derive the Spaces SDK by
building a real app against the raw primitives. This is one of three
implementations with the same behaviour. The other two are
[`openkoalabot-example-tauri`](../openkoalabot-example-tauri) (Rust) and
[`openkoalabot-example-ts`](../openkoalabot-example-ts) (TypeScript). All three run the shared
headless scenario in [`openkoalabot-example-scenario`](../openkoalabot-example-scenario).

- `RUBRIC.md`: what each rendered surface is checked against, and what is
  ungradeable by design.
- `FRICTION.md`: every place the old Spaces MCP forced an awkward shape on the
  client, with the code that absorbed it. This was the design input for the SDK.

## The SDK exists now, and this app consumes it

The 46 entries in `FRICTION.md` were turned into
[`libs/spaces-sdk-swift`](../../libs/spaces-sdk-swift) and
[`libs/spaces-app-swift`](../../libs/spaces-app-swift). The hand-rolled MCP
binding that used to be `Sources/OpenKoalaBotExample/Spaces/MCPSpacesClient.swift` now
lives in the first as `CuaSpaces`, and the whole of
`Sources/OpenKoalaBotExample/Streaming/` in the second as `CuaSpacesStreaming`
(source-available, FSL-1.1-MIT). What is left in this sample is the app: its own view
models, its roster and transcript stores, and `Spaces/SDKSpacesClient.swift`,
which is the thin mapping from the SDK into this app's vocabulary.

The SDK's README carries the entry-by-entry mapping, including the entries it
deliberately does **not** absorb because they are app, platform or server
problems rather than API ones.

## Layers and status

| Layer | Status |
|---|---|
| Spaces client (`Sources/OpenKoalaBotExample/Spaces/`, on `CuaSpaces`): sandboxes, agent threads, windows, files | **live**, e2e-tested against a real Space |
| Screen export (`export <dir>`) | live; renders from `Model/Fixtures.swift` |
| Live data layer (`Sources/OpenKoalaBotExample/Model/`): one Space, a roster of Bots, live transcripts | **live**, e2e-tested against a real Space |
| Roster / transcript views | rendered from an injected `BotDataSource`: `FixtureDataSource` for export, `BotStore` at runtime |
| Picture-in-picture: the desktop (the button on the stream) or one window (the pane's window list), each in a floating panel (`StreamPiPSet`) | **live**, `live-pip` and `PiPLiveTests` against a real Space |
| One drop zone (`SpaceDropZone`): files dropped or picked with "Send file…" upload into the Space | live |

App teleport (moving a host app or window into a Space) ships with Cua
Spaces (source-available) and is not part of this sample.

`presentScreen` is deliberately inert: the Spaces tools that display a Space
(`show_space_pip`, `open_space_viewer`, `stream_space_window`) draw onto the
*operator's* Mac. An in-app stream reads frames from the Space's media plane
(`SpaceStreamProvider`) and belongs to the streaming layer. `capabilities.liveScreenPixels` reports `false` so nothing
mistakes the placeholder for live pixels.

## The runtime underneath

`SDKSpacesClient` is built on the
Swift overlay [`libs/spaces-sdk-swift`](../../libs/spaces-sdk-swift). The
overlay sits on the generated `Cua` package, so every tool is the Rust
`cua-spaces` implementation, reached in one of two ways:

| `OPENKOALABOTS_SPACES` | Backend |
|---|---|
| unset | a running `cua daemon` if its discovery file exists, otherwise embedded (`Spaces.local()`) |
| `daemon` | `SpacesConnection.daemon()`: `cua daemon`, shared by every process on the machine |
| `embedded` | `SpacesConnection.embedded()`: the runtime in this process. `OPENKOALABOTS_SPACES_HOME` moves its registry off `~/.cua` |

The app streams through `SpaceStreamProvider`: a Space is its own stream
source.

## The app window

One opaque window (no materials, light and dark): a sidebar with the Bots,
routines and group chats, a centred conversation column, and the Agent
Computer pane on the right when it is open. An empty conversation is a centred
prompt; typing into it creates a Bot and sends the message. The koala artwork
is drawn from `samples/openkoalabots-assets` geometry in `Design/KoalaArt.swift`,
including the Dock icon.

### New Space

`New Space` (sidebar, `Shift-Cmd-N`) opens a four-step wizard: System (OS,
image, Cua Cloud or This machine, and an optional Engine), Resources, Options,
Summary. "Where it runs" starts from your default location (`cua config set
default.on cloud|local`, `CUA_DEFAULT_ON`). The Engine chooser offers only what
the image runs there: gVisor or runc for a local container, QEMU or Lume for a
local VM, gVisor or KubeVirt in Cua Cloud, and Automatic by default. `Create`
makes one call on the generated cua SDK, `Spaces.create(options:)` with `on`,
`kind`, `runtime`, the image as is, and CPUs and memory for a local Space
(`Spaces/SDKSpaceCreation.swift`). A Cua Cloud Space is metered.

The image dropdown lists the published entries of
`libs/images/sandbox-images.json`, generated into
`Spaces/SandboxImages.generated.swift`:

```sh
node scripts/gen-images.mjs           # regenerate
node scripts/gen-images.mjs --check   # exit 1 on drift
```

### Screenshots

`OPENKOALABOTS_DESIGN_CAPTURE=<stage>` puts the window in one state with fixture
Bots and transcripts, offline, for a screenshot taken from outside the app.
Stages: `signin`, `empty`, `thread`, `computer`, `wizard`, `wizard-local`,
`wizard-resources`, `wizard-options`, `wizard-summary`.
`OPENKOALABOTS_APPEARANCE=light|dark` pins the appearance, and
`OPENKOALABOTS_DATA_DIR` moves the roster and routines files off
`~/Library/Application Support/OpenKoalaBots` (set it for any throwaway run).

## Running

Stage the cua SDK library first. The Swift package links the host dylib from
`libs/cua/swift/lib`:

```sh
(cd ../../libs/cua && cargo build --release -p cua-sdk && node scripts/stage-uniffi-library.mjs --only=swift)
swift build
.build/debug/OpenKoalaBotExample export ./renders          # every screen to PNG (no windows)
.build/debug/OpenKoalaBotExample spaces-probe embedded     # the Spaces contract tools, from the linked SDK
OPENKOALABOTS_TEST_SPACE=space://direct/127.0.0.1:3211 .build/debug/OpenKoalaBotExample   # the app, attached to a registered Space
```

The last command opens the app's window. Run it yourself; the test suite and
the scenario never do.

## The shared scenario

```sh
../openkoalabot-example-scenario/run.sh --impl swift --lane fixture   # hermetic
../openkoalabot-example-scenario/run.sh --impl swift --lane docker    # a linux container (runsc, 4 GiB)
```

`OpenKoalaBotExample scenario --spec … --lane … --out …` runs the spec through this
app's own code, headlessly: `SDKSpacesClient` for the Space and the agent
thread, `SpaceStreamProvider` for frames, and the SDK handle for files,
teleport and presence (`Sources/OpenKoalaBotExample/Scenario/ScenarioRunner.swift`).
Session teleport ships with Cua Spaces (source-available). The runner's
embedded runtime refuses it with `HostCapabilityMissing`, so that step skips.

## Picture-in-picture

```sh
.build/debug/OpenKoalaBotExample live-pip 127.0.0.1:3211 "$TOKEN" /tmp/pip "terminal"
```

Adds the spacesd to a temp registry, pops out the desktop and the first
window whose title matches, and prints both sessions' frame counts sampled
twice. `OPENKOALABOTS_PIP_CAPTURE=<dir>` runs the same tour in the app.

## Tests

```sh
scripts/test.sh            # `swift test`, plus the swift-testing paths the Command Line Tools need
```

The suites run on swift-testing. XCTest ships only with Xcode. The assertion
lines keep XCTest's names through `Tests/OpenKoalaBotExampleTests/XCTAssertCompat.swift`.
Offline, 212 tests pass and the 22 live ones skip. Against a local
linux container (gVisor, `OPENKOALABOTS_TEST_SPACE_URL`), 212 pass
and one skips: the Terminal-window test, which only runs on a macOS Space.

The e2e suite drives `SDKSpacesClient`, and through it the SDK, against a
**live Space** through the cua SDK's Spaces runtime. Nothing is mocked.

### `OPENKOALABOTS_TEST_SPACE_URL`: a spacesd

```sh
OPENKOALABOTS_TEST_SPACE_URL=http://127.0.0.1:32768 OPENKOALABOTS_TEST_SPACE_TOKEN=… scripts/test.sh
```

The machine is added to a **temp** registry in an embedded runtime and pinned
for the run. Install the scenario's fake agent CLI in the guest first
(`samples/openkoalabot-example-scenario/fixtures/fake-claude.sh` as `~/.local/bin/claude`),
so the agent tests make no model calls. Otherwise the harness installs the
real CLI. `~/.cua` is never touched. A local linux container
works for this. The few assertions that are about a macOS Space, such as
opening a Terminal window, skip on Linux and say why.

### `OPENKOALABOTS_TEST_SPACE`: the warm-Space override

Point the suite at a Space that is already up, instead of creating one:

```sh
OPENKOALABOTS_TEST_SPACE=local:cua-space-e3c1b54907 scripts/test.sh
```

With the variable set, `ensureSpace()` returns exactly that Space: no
`create_space`, no `delete_space`, and no call that displays
the Space on the operator's desktop. Every artefact a test creates (an uploaded
file, an agent run, a window it opened) is named with a fresh UUID and removed
in teardown whether the test passes or fails, so the suite is safe to run
against a Space you demo from.

With it **unset the live tests skip** rather than run. That is deliberate: the
ordinary `ensureSpace()` path calls `create_space` with `reuse: true` in your
default location, which creates a **new sandbox** (metered in Cua Cloud) when it
finds none to reuse (see `FRICTION.md` #5). Costing a reviewer a sandbox for
typing `swift test` is not acceptable, so the suite never takes that path.

Per-test cleanup, which used to be XCTest's async `tearDown`, and the
end-of-suite sweeper, which used to be `XCTestObservation`, are now the
`.liveSpace` trait (`Tests/OpenKoalaBotExampleTests/LiveSpace.swift`).

### What is covered

| Primitive | Tool | Test |
|---|---|---|
| Space inventory | `list_spaces` | `testListSpacesDecodesTheRealInventory`, `testWarmSpaceIsRunning` |
| Attach to a Space | override (never `create_space`) | `testEnsureSpaceAttachesToWarmSpaceWithoutCreating` |
| Start a Bot | `agent_start` | `testBotLifecycleStartMessageStatusStop` |
| Steer a Bot | `agent_message` | `testBotLifecycleStartMessageStatusStop`, `testMessageToARunningBotIsRefusedNotQueued` |
| Poll a Bot | `agent_status` | `testBotLifecycleStartMessageStatusStop`, `testStatusOfUnknownRunIsUnknownNotDone` |
| Stop a Bot | `agent_stop` | `testBotLifecycleStartMessageStatusStop` |
| Bot roster | `agent_list` | `testBotLifecycleStartMessageStatusStop` |
| Live window view | `list_space_windows` | `testWindowsReturnsLiveWindowsWithUsableIdentifiers`, `testWindowsReflectsAWindowOpenedAndClosedByTheTest` |
| Attachment in | `upload` | `testUploadDeliversFileContentsIntoTheSpace`, `testUploadOfMissingHostFileThrows` |
| Attachment out | `download` | `testDownloadBringsAFileBackOutOfTheSpace`, `testDownloadOfMissingPathThrows` |

### What is **not** covered, and why

- **`create_space` / `delete_space`.** Implemented and typed, but untested
  here: exercising them means creating and deleting a sandbox, which this suite
  is forbidden to do against the Space it runs on. The shared scenario's
  `cloud` lane covers them.
- **`ensureSpace()`'s create branch.** Same reason: it can create a sandbox.
  The override branch is tested; the `create_space` branch is not.
- **`presentScreen` / tier 2 and 3 pixels.** Not implemented in this layer.
  `testPresentScreenIsInertAndCapabilitiesSaySo` asserts it stays inert. That
  is proof the build does not open a window on the operator's Mac, **not**
  coverage of a streaming operation.

### Screen → Spaces operation → test

Screens whose behaviour is backed by a Spaces operation:

| Screen | Operation | Test |
|---|---|---|
| `mobile-02-thread-card`, `mobile-03-thread-links` | agent thread: `agent_start` → `agent_status` → `agent_message` | `testBotLifecycleStartMessageStatusStop` |
| `mobile-01-roster` (populated) | `agent_list` | `testBotLifecycleStartMessageStatusStop` |
| `mobile-05-takeover`, `mobile-07-pinned-preview`, `mobile-09-takeover-controlled` | live window view (`list_space_windows`); the *pixels* are `stream_space_window` | window list: `testWindowsReturnsLiveWindowsWithUsableIdentifiers`. Streaming and take/hand-back: **not implemented** |
| `mobile-08-attachments` | `upload` (and `download` on the way back) | `testUploadDeliversFileContentsIntoTheSpace`, `testDownloadBringsAFileBackOutOfTheSpace` |
| `mobile-06-approval` | approval gate before a Bot uses a saved login | **no Spaces operation exists for this.** The Spaces tool contract has no approval primitive: `agent_start` runs auto-approved because the Space *is* the sandbox. The screen is deliberately uncovered rather than faked (RUBRIC.md) |
| `desktop-01-dark`, `desktop-02-light`, `desktop-03-no-panel` | the desktop shell hosts the same thread and window operations as above | same tests; the shell itself is fixture-rendered |

UI-only screens, with no Spaces interaction and **no test**, correctly:

- `mobile-04-signin`: sign-in, entirely local.
- `mobile-10-empty-roster`: the zero-Bot empty state; by definition no run to list.

### The live data layer

`Model/BotStore.swift` is the app's data layer: it attaches to **one** Space and
starts **one long-lived agent thread per Bot** inside it (never a Space per
Bot; `Spaces/SpacesClient.swift` argues out why), keeps a live roster from
`agent_list` joined to local Bot identity, and derives each Bot's transcript
from real `agent_status` output.

| Live behaviour | Operation | Test |
|---|---|---|
| Attach to the one shared Space, create nothing | override (never `create_space`) | `testConnectAttachesToTheWarmSpaceAndCreatesNothing`, `testEveryBotRunsInTheOneSharedSpace` |
| Real agent output becomes a transcript | `agent_start` → `agent_status` | `testLiveThreadCarriesRealAgentOutputRosterStateAndAttachments` |
| Per-turn attribution of output | `agent_message` + tail offsets | `testLiveThread…`, `testOutputIsAttributedToTheTurnThatCausedIt` |
| Live roster with per-Bot state and `accepts_message` | `agent_list` | `testLiveThread…`, `testRosterJoinsRunsBackToLocalBotIdentity`, `testUnmarkedRunsStillAppearInTheRoster` |
| A refusal is shown, not swallowed | `agent_message` | `testMidTurnRefusalIsShownToTheUser`, `testRefusalSurfacesInOutcomeNoticesAndTranscript` |
| Attachments in, files out | `upload` / `download` | `testLiveThread…`, `testAttachmentIsEchoedIntoTheTranscript` |
| One poll loop for the whole roster | `agent_status` | `testPollLoopRefreshesTheRosterFromOneTask` |
| A failed probe degrades to `unknown`, never to health | `agent_status` | `testFailedStatusProbeDegradesToUnknown` |

The views read through `BotDataSource`, which has two implementations:
`FixtureDataSource` (what `export` renders, so the regression renders
`RUBRIC.md` describes do not move; the PNGs were byte-identical before and
after this layer landed) and `BotStore` (live). `BotStatusChip` and the composer's refusal
line surface `accepts_message` in the UI, so a user can see that a Bot will
refuse a message before typing one.

### Routines, group chats, and the transcript affordances

`Sources/OpenKoalaBotExample/Routines/` and `Sources/OpenKoalaBotExample/Groups/` are
self-contained views the app shell mounts:

| Feature | Mount point | Backing |
|---|---|---|
| Routines | `RoutinesPanel(botID:botName:store:dark:scale:)`: desktop right panel under the Agent Computer preview; mobile thread overflow | `RoutineStore` (JSON on disk) + `BotStoreRoutineRunner` → real `agent_start` / `agent_message` |
| Group chat | `GroupChatScreen(chatID:store:bots:)`, opened from `NewGroupSheet` | `GroupChatStore` + `BotStoreGroupMessenger` → one agent thread per member in the one shared Space |

A routine is a saved prompt plus a clock. `RoutineStore.tick(now:)` is the whole
scheduler and takes the instant to evaluate, so the policy is testable without
sleeping; `startScheduler(every:)` is a thin loop over it. Firing an unhired Bot
**hires it** (a real `agent_start`), and firing a Bot that is mid-turn is
*refused*, never an interruption. Routines persist to
`~/Library/Application Support/OpenKoalaBots/routines.json` and survive a restart,
firing history included, so a relaunch does not re-fire the past.

| Live behaviour | Operation | Test |
|---|---|---|
| A due routine starts a real run in the Space | `agent_start` → `agent_list` → `agent_stop` | `RoutinesLiveTests.testADueRoutineStartsARealAgentRunInTheSpace` |
| The background loop fires without being ticked | same | `testTheBackgroundSchedulerLoopFiresWithoutBeingTicked` |
| A group fans out to one real thread per member | `agent_start` / `agent_message` per member | `testAGroupChatFansOutToRealAgentThreads` |

A group chat holds **2-6 Bots plus one human**, enforced in `GroupChat`'s
initialiser rather than in a view, so an out-of-bounds group cannot be built by
going round the UI. Hitting either limit produces a named error, a banner, and a
line in the group transcript saying what the limit is.

Everything in `Design/BotAvatar.swift`'s motion section and the affordance
section of `Mobile/Transcript.swift` (typing indicator, reaction bar, link card,
approval card) is outside the export renders `RUBRIC.md` describes. The
approval card **enforces nothing**: there is no approval primitive in the
Spaces tool contract (`FRICTION.md` §43), `agent_start` runs auto-approved, and
`ApprovalCard.enforcementIsImplemented` is `false` so no caller can mistake the
card for a control.
