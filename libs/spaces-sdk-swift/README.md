# Cua Spaces SDK for Swift

A Swift package for driving [Cua Spaces](../../apps/cua-spaces) from an
application: create or attach to a Space, run agents in it, watch them, move
files in and out, and put the Space's live screen inside your own UI.

The package is MIT. The live screen (`CuaSpacesStreaming`: stream sessions,
H.264 decode, interactive input, presence cursors and the SwiftUI stream
views) is part of Cua Spaces and lives in
[`libs/spaces-app-swift`](../spaces-app-swift), source-available under
FSL-1.1-MIT. See [LICENSING.md](../../LICENSING.md).

It is a **thin overlay on the generated cua SDK** ([`libs/cua/swift`](../cua/swift),
product `Cua`). Every call runs the Rust `cua-spaces` implementation, either in
this process (`SpacesConnection.embedded()`) or in a running `cua daemon`
(`SpacesConnection.daemon()`), whose registry, hotspots and host reads are
shared by every process on the machine. There is no Python server, no stdio
framing and no rcdp client in this package.

```swift
import CuaSpaces

let spaces = try Spaces.local()          // the cua daemon if one runs, else embedded
let space  = try await spaces.attach(to: "direct:10.0.0.5:3211")
// or: try await spaces.add(url: "10.0.0.5:3211", token: token)
// or: try await spaces.createSpace(on: .local)   // free; on: .cloud is metered

let run = try await space.startAgent(prompt: "Tidy the inbox",
                                     metadata: ["bot": "inbox"])

for await snapshot in run.stateUpdates() {
    print(snapshot.state, snapshot.reason)
}

_ = try await run.delete()
```

## Where this design came from

This SDK was not designed and then validated. It was **extracted**. A full
product,
[`samples/openkoalabot-example-swift`](../../samples/openkoalabot-example-swift), was built
against the raw Spaces protocol first, and every place the protocol forced an
awkward shape on the app was written down as it happened. That log,
[`samples/openkoalabot-example-swift/FRICTION.md`](../../samples/openkoalabot-example-swift/FRICTION.md), has
46 entries, and it is the requirements document for this package. OpenKoalaBots
now consumes the SDK instead of its own binding.

So the mapping below is not documentation-after-the-fact. Each row is a thing
that cost real code in a real app, and the API that absorbs it.

| Friction | What the raw protocol did | What the SDK does |
|---|---|---|
| §1 | stdio framing dropped every byte after the first newline in a chunk, silently mismatching every later reply | gone: every tool is a typed `CuaSDK.Spaces.callToolJson` call (embedded or over the daemon's gRPC), so no app code frames bytes |
| §2 | status is a poll loop with an invented timeout | `AgentRun.stateUpdates(every:)` → `AsyncStream<RunSnapshot>`, with backoff and change detection inside; `AgentRun.wait(upTo:until:)` for one-shot waits |
| §3 | a failing tool returned `isError: true` as a *successful* result | every failure is a `throw` of `SpacesError`; no SDK call returns a value that might be a failure |
| §4 | `agent_message`'s two branches shared no key; "none" came back as prose | `Delivery` with `accepted` as the only discriminator and `reason` populated on **both** branches; prose "none" becomes an empty collection |
| §5, §28, §33 | the recommended entry point could silently create a cloud sandbox, and could not see local Spaces | `SpacesConnection.attach(to:)` vs `createSpace(options:)`: two calls, and the one that can cost money takes where it runs (`on: .local` or `.cloud`) as a required argument |
| §6 | `running` vs `Bound`, `/Users/lume` vs `/root` leaked into every field | `SpaceState`, `SpaceProvider.home`, `ProviderCapabilities`; the raw phase is kept on `rawPhase`, never discarded |
| §7 | four untyped id namespaces, two spellings for "window" | `SpaceID` / `RunID` / `WindowID`; every spelling read, one published; `Space` and `AgentRun` carry their own ids |
| §8 | cleanup was four guest paths the app had to know | `AgentRun.delete()` → `RunCleanup`, and `Space.withAgentRun { }` which tears down on every exit path. All four paths are reported separately, and the Terminal window is **verified against the Space's own window list** rather than assumed closed. The verification needs no automation consent, so deleting a run cannot raise a TCC prompt |
| §9 | *(what it got right)* | the status vocabulary, published `accepts_message`, refusal over silent damage, and a verified `stop` are all interface guarantees with tests naming them |
| §10 | the display tools draw on the operator's machine; there was no "give me frames" | `Space.streamEndpoint()` (frames) is a different call from `Space.present(_:)` (operator) |
| §13 | `upload` clobbered silently and returned no path | `UploadPlacement.collisionSafe(in:)` keeps the user's filename and returns the `RemoteFile` actually written |
| §14 | the app's attachment caps had no API counterpart | `TransferLimits` is `Codable` data, checked before any I/O, naming the file that broke the rule |
| §18 | "the stream is live" could only be proven by walking the view tree | `StreamPresentation` publishes attach/detach and frames *presented* |
| §21, §41 | app identity had to be smuggled into the prompt, twice, and stripped everywhere | `metadata:` on `AgentStartRequest`, encoded in one place and stripped from every `summary` the SDK publishes |
| §22 | `agent_list` and `agent_status` disagreed about what a state is | one `RunSnapshot`; the cheap call may omit `outputTail` (and says so with `nil`) but never the `reason` |
| §23 | truncation was silent | `RunSnapshot.outputTruncated` |
| §24 | refusal was the only option, so every app built a queue | `DeliveryMode.queueUntilIdle(timeout:)`, with `.refuseIfBusy` still the default |
| §25 | a roster cost one round trip per Bot per tick | `Space.roster(pollingEvery:)`: one `agent_list` per tick regardless of size, output only for runs you `watch(_:)`, and `roundTrips` published so the claim is checkable |
| §30 | the client was synchronous and blocked the main actor | `async` throughout, actor-isolated transport, safe from `@MainActor` |
| §37 | no way to ask for "the Bot's screen" | `AgentRun.window()` joins the run's process family against the window list's owner pid, and returns `nil` rather than guessing by title |

### Deliberately not absorbed

Some entries are about the app, the platform, or the server, and contorting the
API around them would make it worse:

- **§11, §32, §46**: SwiftUI render-path hazards (`ImageRenderer` has no run
  loop; an inert modifier changed exported pixels). These belong to the app's
  own two render paths.
- **§12**: `NSItemProvider` needs a local file synchronously. A remote file
  cannot be made synchronous; a caching file handle is worth building, but it
  is not a protocol fix.
- **§15, §16, §17**: PiP window ownership, one session with two observers, and
  a stale drag payload. The first two are addressed by `CuaSpacesStreaming`
  owning less; the third is genuinely the app's, because only the app knows
  when the user acted.
- **§19, §20, §44, §45**: typed agent output, turn boundaries, typing-vs-
  thinking, and avatar state. All four need the **harness** to publish
  structured events. An SDK that parsed terminal scrollback into typed events
  would be guessing in a more official-looking place; the SDK publishes the
  tail honestly and says it is a tail.
- **§26, §27, §29, §31**: SwiftPM packaging, activation policy, TCC, and a
  fixed graded canvas. Platform and sample facts, documented rather than
  wrapped.
- **§34, §35, §38, §39**: app-level: list clipping, a harness writing to real
  user data, store lifecycle, and where a refusal is drawn.
- **§36, §43**: a Space needs a way to enumerate TCC prompts, and agents need
  an approval interception point. Both are **server** features. The SDK cannot
  invent enforcement, and `ApprovalCard.enforcementIsImplemented = false` in
  the sample stays `false` until the server has one.
- **§40, §42**: scheduling and group fan-out have no server-side existence.
  The SDK does not fake durability it does not have.

## The four nouns

A Space, its agents, its files, its screen, plus the one that makes Spaces
different: its sessions. Each hangs off the Space handle.

```swift
let space = try await Spaces.local().attach(to: "space://local/cua-space-e3c1b54907")

let agent = try await space.agents.start("Summarise ~/report.pdf into ~/summary.md")
for await event in agent.events() {
    print(event.text)                  // .text / .stateChanged / .finished
}
let summary = try await space.files.file("~/summary.md").url()
```

`space.agents`, `space.files`, `space.sessions`, and `space.screen` (in
`CuaSpacesStreaming`, from `libs/spaces-app-swift`). Scoped lifecycles (`space.agents.withAgent {}`,
`space.withAgentRun {}`, `space.screen.watching {}`) tear down on every exit
path including a throw and a cancellation.

## What the SDK refuses to claim

Every one of these is a published value, so a product can render the truth and
so the day a backend gains the feature nothing at the call site breaks.

| Published | Value today | Because |
|---|---|---|
| `AgentEvent.isInferred` | `false` for everything `CuaSpaces` produces | the harness publishes terminal scrollback and nothing else, so `.toolUse`, `.question` and `.artifact` are guesses. They come only from the opt-in `CuaSpacesTranscript` module, and every one of them says so. |
| `Scheduler.isServerBacked` | `false` | the Spaces contract has no schedule tool. A schedule fires while your process runs, and not after. |
| `TransferLimits.isServerPublished` | `false` | no tool publishes limits (the Rust server has no transfer cap at all), so the count and batch caps are the caller's own. |
| `AgentRun.approvalsAreEnforced` | `false` | `agent_start` runs auto-approved and the Space *is* the sandbox. `approve(_:)` throws rather than no-oping. |
| `AgentKind.isProductionReady` | `true` for `claude-code` and `codex` | the other backends are stubs; a picker should grey them out rather than discover it by failing. |
| `ProviderCapabilities.serverBackstop` | `false` | nothing server-side ends work a client stopped watching. `AgentStartRequest.timeout` and `Space.setIdleTimeout(_:)` are reserved against the day it does. |
| `OutputPage.truncatedBefore`, `Turn.outputLost` | as observed | `output_tail` is a fixed window, so a run that talks faster than you read loses lines. The SDK detects a window that slid past unseen output instead of stitching it silently. |
| `DeliveryMode.queue(timeout:queuedUntil:)` | `.processExit` | there is no backend queue. A queued message is held here and dies with this process. Refusal remains the default. |

## Tool coverage

Anything you can do in the Spaces MCP you can do here **by name**. The contract
(`libs/cua/spaces-contract/manifest.json`, 45 tools, the same list
`spacesToolMethods()` exports from the linked SDK) has a typed call for every
tool, and `Tests/CuaSpacesTests/ToolCoverageTests` drives every one of them and
fails if a tool loses its path.

| Tool | SDK |
|---|---|
| `add_space` | `SpacesConnection.add(url:token:name:)`; registers an existing machine, never creates one |
| `remove_space` | `SpacesConnection.remove(_:)`: forgets a Space without touching it |
| `list_spaces` | `SpacesConnection.spaces()` |
| `create_space` | `SpacesConnection.createSpace(options:)` / `createSpace(on:kind:runtime:image:name:reuse:wait:)`: `on` is required (`.local` free, `.cloud` metered); `kind` (`.auto`, `.container`, `.vm`) and `runtime` (`.auto`, `.gvisor`, `.runc`, `.qemu`, `.lume`, `.kubevirt`; `SpaceRuntime.offered(on:kind:)`) default to `.auto`; `reuse: true` is get-or-create |
| `delete_space` | `Space.delete()`, `SpacesConnection.deleteSpace(_:)`: deletes a created Space; one added by address is only forgotten |
| `stop_space` | `Space.stop()`, `SpacesConnection.stopSpace(_:)`: suspends (memory kept) or stops (disk kept) a Space whose provider can |
| `start_space` | `Space.start()`, `SpacesConnection.startSpace(_:)`: resumes or boots it again |
| `space_bash` | `Space.bash(_:)` (the SDK's typed `bash`, stdout only) |
| `space_write` | `Space.write(_:to:)`: literal text, no local file, no shell quoting |
| `upload` | `space.files.send(_:to:within:)` |
| `send_file` | `Space.sendFile(_:intoDownloads:respectIgnoreFiles:)` |
| `download` | `space.files.file(_).url()` / `Space.download(_:into:)` |
| `stream_endpoint` | `Space.streamEndpoint(forceRefresh:)` (`rcdpEndpoint()` is a deprecated alias) |
| `list_space_windows` | `Space.windows()`, `space.screen.presentableWindows()` |
| `stream_space_window` | `space.operatorDisplay.streamWindowToOperatorDesktop(_:)` |
| `show_space_pip` | `space.operatorDisplay.pinPictureInPictureOnOperatorDesktop()` |
| `hide_space_pip` | `space.operatorDisplay.unpinPictureInPictureFromOperatorDesktop()` |
| `open_space_viewer` | `space.operatorDisplay.openViewerOnOperatorDesktop()` |
| `list_tools` | `space.services.tools(of:matching:)`, `space.services.list()` |
| `call_tool` | `space.services.call(_:on:_:)` |
| `agent_start` | `space.agents.start(...)`, `Space.startAgent(_:)` |
| `agent_message` | `AgentRun.send(_:mode:)` |
| `agent_status` | `AgentRun.status(tail:)`, `.events()`, `.wait(upTo:until:)` |
| `agent_events` | `AgentRun.eventPage(after:max:)` |
| `agent_interrupt` | `AgentRun.interrupt()` |
| `agent_stop` | `AgentRun.stop()`, `.delete()` |
| `agent_list` | `space.agents.list()` / `.mine()` / `.all()`, `space.agents.live(_:)` |
| `agent_capabilities` | `space.agents.harnessCapabilities()` |
| `teleport_manifest` | `space.sessions.manifest(for:scope:)` |
| `teleport_app` | `space.sessions.send(_ approval:)` (carries the sensitive-item acknowledgement the server re-checks) |
| `hotspot_start` | `connection.hotspot.start(sharingWith:)`, `.sharing(with:) {}` |
| `hotspot_stop` | `connection.hotspot.stop()` |
| `hotspot_status` | `connection.hotspot.status()` |

Anything the overlay does not model is one call away on the generated SDK:
`try await space.native()` returns the `CuaSDK.Space` (typed files, streams,
presence, hotspot, agents). Import `Cua` alongside `CuaSpaces` only in files
that need it: both modules name a `Space`, `SpaceInfo` and `SpaceWindow`.

Nothing is reachable only through `SpacesConnection.callTool`. That escape
hatch stays (an SDK that cannot be gone around gets forked instead), but it is
for reaching something *new*, before the SDK models it, not for reaching
something the SDK should have modelled.

Two distinctions the names carry rather than the docs:

- **`space.screen` vs `space.operatorDisplay`.** `show_space_pip`,
  `open_space_viewer` and `stream_space_window` all draw a window on the
  **operator's own Mac**. An app embedding a Space in its own UI sees nothing
  from them and wants `space.screen` (in `CuaSpacesStreaming`). Every method on
  `OperatorDisplay` says `onOperatorDesktop` and returns nothing renderable.
- **`connection.callTool` vs `space.services.call`.** The first calls a tool on
  the Spaces MCP, on your machine. The second calls a tool on an MCP service
  running *inside* the Space: cua-driver's computer-use MCP, or an app MCP such
  as blender, unity or get-skills. A service that is reachable but advertises
  zero tools **exists and is not ready** (Unity lists nothing until an Editor
  has a project open), and `ServiceCatalog.notReadyWarning` carries the server's
  own words for that rather than letting an empty array read as "no such
  service".

## Layout

- **`CuaSpaces`**: the app-shaped surface over the cua SDK.
  `SpacesConnection` (`embedded()`, `daemon()`, `init(cua:)`,
  `init(transport:)` for tests), `Space`, the four namespaces, `AgentRun` and
  its typed events, `RosterStream` and `RosterPolicy`, session teleport,
  transfers. `CuaSpacesTransport` is the one transport: every contract tool is
  `CuaSDK.Spaces.callToolJson`.
- **`CuaSpacesTranscript`**: the guessing module. Importing it is the act of
  consent: it turns scrollback into tool cards and questions, and labels every
  one of them `isInferred == true`.
- **`CuaSpacesStreaming`** (in [`libs/spaces-app-swift`](../spaces-app-swift),
  FSL-1.1-MIT): frames. `LiveStreamSession` opens a
  `CuaSDK.SpaceStreamSession` (spacesd media plane, RVD2 wire, ticketed,
  keyframe-gated in Rust)
  for a window or the whole display, decodes its H.264 access units with
  VideoToolbox, sends `interactive_input` batches, and runs presence through
  `CuaSDK.SpacePresence`. `space.screen.watch(agent:)` hands back a `Viewer`;
  `SpaceScreenView`, `LiveStreamView` and `StreamPiPWindow` are the SwiftUI
  views over it. Coordinates are normalized `0...1` and nothing else.

## Tests

Offline and hermetic (Command Line Tools are enough: the suites run on
swift-testing through `Tests/*/XCTestShim.swift`):

```bash
# the Rust side the fixture-backed suite needs
(cd ../cua && scripts/build-test-fixtures.sh \
   && cargo build -p cua-cli && node scripts/stage-uniffi-library.mjs --only=swift)
../cua/swift/scripts/fresh-abi.sh . debug   # after the binding changed
CLT=/Library/Developer/CommandLineTools/Library/Developer
CUA_REQUIRE_FIXTURES=1 swift test -Xswiftc -F -Xswiftc $CLT/Frameworks \
  -Xlinker -F -Xlinker $CLT/Frameworks -Xlinker -rpath -Xlinker $CLT/Frameworks \
  -Xlinker -rpath -Xlinker $CLT/usr/lib
```

`fresh-abi.sh` recompiles every module when `libs/cua/swift`'s generated
binding changed: SwiftPM reuses modules compiled against the old one, whose
baked-in vtable slots then call the wrong SDK method.

- `FakeSpacesBackend` answers with the Rust server's shapes (including the
  awkward ones: `isError` results, prose for "none", an `agent_message` whose
  branches share no key) for the state-machine suites.
- `CuaBackedTests` drives the same overlay against the real Rust runtime
  (embedded and through a cua daemon) and a confined in-process
  cua-spacesd from `cua-test-fixtures`, so the fake cannot drift unnoticed.
  Session teleport: embedded is refused with `requires_cua_app`; through the
  fixture's daemon, whose Keyvault is test-only (temp passphrase vault, fake
  presence gate), the test approves the request and the session lands. A
  fixture built from another commit refuses to start: rebuild it with
  `libs/cua/scripts/build-test-fixtures.sh`.

There is no live suite against a real Space in this package any more; the
live Spaces e2e lives with the Rust crate (`libs/cua/crates/cua-spaces/tests`).
