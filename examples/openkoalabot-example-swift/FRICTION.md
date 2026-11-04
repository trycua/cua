# Friction log: building OpenKoalaBots's Spaces client

> **Status: historical log.** The components it describes (the Python
> `spaces_mcp.py` server, the stdio MCP client, the rcdp daemons and the
> `:8700` teleport receiver) are gone. Spaces now run on the cua SDK
> (`cua-spaces` in `libs/cua`, hosted by `cua daemon`) with cua-spacesd in
> the guest, and this app uses the Swift overlay in
> [`libs/spaces-sdk-swift`](../../libs/spaces-sdk-swift). The entries are kept
> as the design record.
>
> The lifecycle vocabulary changed too. `claim_space`, `get_or_create_space`,
> `local_provision_space` and `release_space` are now `create_space` (with
> `on: local|cloud`, `kind`, `runtime` and `reuse`) and `delete_space`, and the
> "Fleet" provider is `cloud`. The SDK's answer to §5, §28 and §33 is
> `attach(to:)` next to `createSpace(options:)`, whose `on` is required. The
> entries below keep the names they were written with.

Written while binding `Sources/OpenKoalaBotExample/Spaces/MCPSpacesClient.swift` to
`apps/cua-spaces/mcp/spaces_mcp.py` and testing it against a live Space
(`local:cua-space-e3c1b54907`). Every entry is a place where the MCP forced an
awkward shape on the app, with the code that had to absorb it. **This is the
design input for the Spaces SDK**: each entry is a thing the SDK should do so
that no app has to do it again.

Entries are ordered by how much code they cost, not by severity.

---

## 1. The transport drops bytes, and nothing tells you

**What happened.** The stdio framing is newline-delimited JSON. The obvious
read loop (accumulate until a newline, parse it, return) throws away
everything that arrived *after* that newline in the same `availableData` chunk.
The next call then reads the *tail* of a previous reply, and from that point
every response is matched to the wrong request. Because each reply is a valid
JSON object, nothing errors: you silently get another tool's answer.

**Who absorbed it.** `MCPSpacesClient.send`, which now carries a `pending`
buffer across calls:

```swift
private var pending = Data()   // bytes read past the end of one response
```

**What the SDK should do.** Own the framing. No app should implement a
JSON-RPC-over-stdio reader; a desync here is undetectable from the app side and
produces wrong answers rather than errors.

---

## 2. Status is a poll loop that wants to be a subscription

**What happened.** There is no way to be *told* that an agent changed state.
Every consumer writes the same loop: call `agent_status`, compare, sleep one
second, repeat, with a timeout it has to invent. A chat UI needs this for every
visible Bot, so cost scales with the roster.

**Who absorbed it.** `SpacesE2ETests.poll(_:seconds:until:)`, a helper that
exists only because the protocol has no push:

```swift
private func poll(_ runID: String, seconds: Int,
                  until done: (AgentStatus) -> Bool) async throws -> AgentStatus
```

Each poll is a full round trip that SSHes into the Space and `cat`s files, so
polling three Bots at 1 Hz is three SSH round trips per second.

**What the SDK should do.** Expose `AsyncSequence<AgentStatus>` per run (and one
merged stream per Space), with the polling, backoff and change-detection inside.
If the transport cannot push, the SDK should still be the only thing that polls.

---

## 3. Tool failures arrive looking like success

**What happened.** `spaces_mcp.py` does not report a failing tool as a JSON-RPC
error. It returns a normal result with `isError: true` and one text part reading
`error: …`. A client that reads `content[0].text` (the obvious thing)
receives the *error message* as if it were the tool's return value.

This is how the pre-existing `upload` bug survived: it sent `local_path` /
`remote_path` (the tool takes `path` / `dest`), every call failed with a
`KeyError`, and the client reported success. Nothing in the app could have
noticed.

**Who absorbed it.** `MCPSpacesClient.callTool`:

```swift
if result["isError"] as? Bool == true { throw Failure.tool("\(name): \(text)") }
```

**What the SDK should do.** Map `isError` onto the language's error channel, so
a failed call cannot be mistaken for a value. Covered by
`testUploadOfMissingHostFileThrows`.

---

## 4. One tool, two response shapes, and no common field

**What happened.** `agent_message` returns an object either way, but the two
outcomes share no key:

- delivered: `{"delivered": true, "run_id": …, "note": "…"}`
- refused:   `{"delivered": false, "run_id": …, "status": …, "reason": "…"}`

"Why" is `note` in one case and `reason` in the other, so it cannot be read
uniformly. I guessed the discriminator wrong the first time (assuming an object
meant a refusal), and the live test caught it; a client without an e2e test
would have shipped "every message was refused".

The same split appears in `download`, whose Local path returns JSON
(`{"dest": …}`) and whose Fleet path returns prose, and in `list_spaces`, which
returns an array normally but the sentence `"No Spaces. Use claim_space …"` when
there are none.

**Who absorbed it.** `MCPSpacesClient.message` (discriminates on `delivered`,
picks the key per branch) and `download` (falls back to a constructed path when
the reply is prose).

**What the SDK should do.** One result type per operation, with a populated
`reason` on both branches and an empty collection for "none" rather than a
sentence. Prose is for humans; an SDK should never make an app parse it.

---

## 5. `get_or_create_space` cannot see the Space you are using

**What happened.** The recommended entry point, `get_or_create_space`, only
enumerates **Fleet** claims. A Local (Lume macOS) Space, the kind actually
being demoed from, is invisible to it, so it skips straight to `claim_space`
and **provisions a new cloud sandbox** while a perfectly good Space sits
running. The app has no way to say "work in *this* one".

**Who absorbed it.** An environment override in the client, which is the only
reason this suite can run safely against a Space someone demos from:

```swift
static let spaceOverrideVariable = "OPENKOALABOTS_TEST_SPACE"
func ensureSpace() async throws -> String {
    if let pinned = Self.overriddenSpace { return pinned }
    …
}
```

**What the SDK should do.** `ensureSpace(preferring:)` taking a Space id, and a
provider-agnostic `get_or_create_space` that considers Local Spaces. A call that
can silently cost money must not be the recommended default.

---

## 6. Provider differences leak into every field

**What happened.** Local and Fleet Spaces disagree on their vocabulary at every
level. A ready Space has `phase: "running"` on Local and `phase: "Bound"` on
Fleet. `$HOME` is `/Users/lume` on Local and `/root` on Fleet, so the `upload`
default destination differs. Several tools (`agent_start` on the older server,
`teleport_app`) are Fleet-only and fail with prose telling you so.

**Who absorbed it.** `SpaceSummary.isReady`:

```swift
var isReady: Bool { ["running", "bound"].contains(phase.lowercased()) }
```

and every test path that names an absolute in-Space directory explicitly rather
than relying on a default.

**What the SDK should do.** A single `SpaceState` enum, a `home` property, and a
declared capability set per provider, so an app branches on a capability rather
than on a string it had to learn empirically.

---

## 7. Ids are threaded by hand, and mean different things

**What happened.** Four id namespaces, all bare strings: the Space id
(`local:cua-space-e3c1b54907`), the run id (`run-84a8dc1f`), the rcdp window
target (`target-172aad9a-…`), and the in-Space run directory
(`~/.spaces-agents/<run_id>`). Every call restates the Space id. Nothing is
typed, so passing a window id where a run id belongs compiles.

There is also a trap: `list_space_windows` names the window identifier
`window`, while `stream_space_window` takes it as `window_id`. The client
originally read `window_id` from the list and got empty ids for every window,
silently, since the JSON parse succeeded.

**Who absorbed it.** `windows(space:)`, which tries all three spellings:

```swift
SpaceWindow(id: "\($0["window"] ?? $0["window_id"] ?? $0["id"] ?? "")", …)
```

and `testWindowsReturnsLiveWindowsWithUsableIdentifiers`, which asserts the ids
are non-empty and `target-`-prefixed precisely because an empty id is the
failure mode you cannot see.

**What the SDK should do.** Distinct id types (`SpaceID`, `RunID`, `WindowID`),
one spelling per concept, and a `Space` handle that carries its own id so it is
not re-passed on every call.

---

## 8. Cleanup is the caller's problem, and it is not one call

**What happened.** An agent run leaves four things behind: the process, the run
directory `~/.spaces-agents/<run_id>`, a LaunchAgent plist, and a Terminal
window open on the Space's desktop. `agent_stop` handles the first only. A
suite that must leave a demo Space pristine has to know all four by path.

**Who absorbed it.** `SpacesE2ETests.startCleanBot`, which registers a teardown
reaching into guest paths the app should never have needed to know:

```swift
_ = try? self.bash("rm -rf ~/.spaces-agents/\(run.runID); "
                   + "rm -f ~/Library/LaunchAgents/com.trycua.agentrun.\(run.runID).plist; "
                   + "osascript -e 'tell application \"Terminal\" to close …")
```

**What the SDK should do.** `run.delete()` that removes the whole run, and a
scoped `withAgentRun { … }` that guarantees teardown on any exit path.

---

## 9. What the MCP got right, and the SDK must not lose

Not friction: the opposite, and worth recording so a redesign keeps it.

- **The status vocabulary is honest.** `running / awaiting_input / idle /
  finished / failed / crashed / unknown`, with `unknown` meaning "the probe
  failed or the signal was ambiguous" rather than standing in for a guess, and a
  `reason` string alongside. `testStatusOfUnknownRunIsUnknownNotDone` pins this.
- **`accepts_message` is published, not inferred.** The harness states when a
  follow-up is legal, so the app never has to learn the turn model by
  experiment.
- **Refusal beats silent damage.** A message sent to a Bot mid-turn is refused
  with an explanation, not queued and not applied by killing the turn in flight.
  `force: true` makes abandoning it deliberate.
- **`agent_stop` verifies.** It reports `stopped` / `alive` from an actual
  liveness probe instead of assuming the kill worked.
- **Status is read from files inside the Space**, so it survives an MCP restart.

These should be *interface* guarantees in the SDK, not implementation details
that a rewrite could quietly drop.

---

# Part 2: mounting the stream in the product, and drag-and-drop

Appended while wiring `Streaming/` into the Agent Computer tiers
(`Mobile/Screens.swift`, `Desktop/DesktopShell.swift`) and building the two
drag-and-drop gestures. Same rule as above: every entry is something an API
forced on the code, and says what an SDK should have done instead. Found
against the live Space `local:cua-space-e3c1b54907`.

## 10. The MCP's display tools draw on the *operator's* machine, so an in-app stream has to bypass the MCP entirely

`show_space_pip`, `open_space_viewer` and `stream_space_window` are the three
tools whose names say "show me the Space". All three open a window on the
machine running the MCP client, which is exactly what an app embedding the
Space in its own UI must not do. So the only way to put a Space's screen inside
a product surface is to go around the MCP: ask `local_rcdp` for an endpoint and
then speak rcdp directly. The MCP has no "give me frames" tool at all.

The effect here is structural, not cosmetic: the tiers cannot depend on
`SpacesClient`. They depend on `AgentScreenSource`, which carries a
`LiveStreamSession`, which was built against a two-method protocol that the
Spaces client does not implement. Three layers exist to route around one
missing capability.

*An SDK should* separate "present this Space to the operator" from "hand me its
frames". The second is the one a product needs, and it is the one that is
missing.

## 11. An inert interaction modifier changed the exported pixels

The tiers are rendered two ways: live in the app, and offscreen through
`ImageRenderer` for the thirteen rubric PNGs. Adding
`.onDrop(of:isTargeted:perform:)` plus an empty `.overlay` to the tier's screen
rectangle (a drop target that, on the export path, has no session to retarget
and can never fire) changed **five of the thirteen exported PNGs**. The
takeover screen grew a 44pt band above the picture containing a stray,
unconstrained copy of the screen's own content, laid out as though it had been
proposed an infinite width:

```
mobile-05-takeover: differing=46080 px maxdelta=459 bbox=(0,1178)-(2046,1266)
```

That is a rubric-scored screen (export screen M5)
silently changing because an unrelated, unreachable modifier was attached.

*Absorbed by* `StreamWindowDropTarget.body`, which returns `content` untouched
unless the source is actually live. Defensible on its own merits, but it is a
fix found by pixel-diffing an export against its baseline, not by reading code.

*The lesson for anything with two render paths:* diff the deterministic one on
every change. The export is now checked byte-for-byte against `cd4c350b4` (all
thirteen PNGs identical), and that check is the only reason this was caught.

## 12. `NSItemProvider` needs a local file *synchronously*; `download` is an async round trip

Dragging a file the Bot produced out of the app is a two-line gesture in
AppKit, until you notice that `.onDrag` must return a fully-formed
`NSItemProvider` at the instant the drag starts, while the file is inside a VM
behind an MCP `download` call that takes seconds. There is no promise, no lazy
file representation that survives the gap, and no "here is where it will be"
contract.

So the file is fetched **on row appearance** into a host temp directory and the
drag carries the already-local copy; a row dragged before its fetch lands is
refused rather than handing Finder a path with nothing behind it. Every artefact
a user might drag is therefore downloaded whether or not they ever drag it.

*Absorbed by* `AgentArtifactExport`.

*An SDK should* expose a file handle that can materialise lazily, or at minimum
a `url(for:)` that is already local once the file has been fetched, so the
caller is not maintaining its own cache keyed by remote path.

## 13. `upload` overwrites its destination silently, so the client has to invent collision-proof names

Two drops of `notes.txt` from two different folders are two different files, and
`upload` will happily make them one. There is no create-only mode, no "exists"
check that is not a `space_bash` round trip, and no returned path. Every
attachment therefore gets a UUID-prefixed remote name minted client-side
(`/tmp/openkoalabots-attachments/3f2a91bc-notes.txt`), which is then the name the
Bot sees, so the user's filename is mangled to work around a missing flag.

*Absorbed by* `AttachmentIntake.send`. Pinned by
`testTwoDropsOfTheSameNameBecomeTwoDistinctFilesInTheSpace`.

*An SDK should* return the path it actually wrote, and offer a non-clobbering
mode that allocates a unique name server-side.

## 14. `upload` publishes no limits, so the app's caps are enforced entirely client-side

The app's caps (6 attachments, 25 MB per file, 200 MB per message) are a
product contract with nothing behind it in the API. `upload` takes one file and
has no notion of a message, so the count cap and the total cap have no server
counterpart at all. The only honest implementation is to check all three before
any I/O, which is what `AttachmentAdmission` does, and to accept that two
clients will drift.

*An SDK should* carry the message-level caps as data the client can read, so a
policy change does not require shipping a new app.

## 15. The PiP panel has no seam, so a drop target had to be attached from outside

`StreamPiPController` owns its `NSPanel` and its `NSHostingView`. Neither the
panel nor its content is reachable for adding behaviour, and its delegate slot
is taken by the controller's own close handling. Making the PiP accept a
dragged window therefore meant finding the panel by scanning `NSApp.windows`
for a visible floating panel and inserting a transparent `NSView` over its
content.

That view then has to be invisible to the mouse: the PiP is interactive, and a
plain overlay would silently swallow every click and break takeover in the
pop-out, while still being found by AppKit's drag hit-testing. The trick is a
`hitTest` that returns `self` only while the drag pasteboard carries this app's
own window type:

```swift
override func hitTest(_ point: NSPoint) -> NSView? {
    let dragTypes = NSPasteboard(name: .drag).types ?? []
    return dragTypes.contains(StreamWindowDrag.pasteboardType) ? self : nil
}
```

*An SDK should* let the caller supply the PiP's content, or at least a
decoration layer, rather than owning the whole window.

## 16. One session, two observers, so "switch what the PiP shows" also switches the pane behind it

This follows directly from the (correct) decision that popping out moves which
view hosts a session rather than opening a second one. The product consequence
is that the PiP is not an independent pane: a window dropped on the PiP
retargets the shared session, and the in-app tier changes with it. Giving the
PiP its own source would mean a second stream session on the same target: legal
(`Streaming/FRICTION.md` §12), but a second decode and a second geometry
conversation.

Recorded rather than fixed, because "the PiP is a second view of the same
stream" is the honest model. An SDK that wants independent panes should make a
*session* cheap enough that two of them is an obvious choice, not a considered
one.

## 17. The window list is the drag source, and it changes under the drag

`list_windows` titles move constantly. A Terminal window enumerated as
`lume - watch.command - -zsh - 120×30` was, four seconds later and under the
same handle, `lume - watch.command - tail -f out.log - 120×30`. A drag payload
captured at mouse-down is therefore stale by the time it is dropped, including
its `target_epoch`, which is the field that decides whether the drop works at
all (`Streaming/FRICTION.md` §1).

*Absorbed by* `StreamWindowDrag.resolve`, which re-looks-up the dragged handle
in the session's current list and prefers the live entry's epoch and geometry,
falling back to the dragged value when the pane has not enumerated yet.

*An SDK should* make a window reference a *handle that can be re-resolved*,
rather than a struct whose fields are already wrong by the time the user acts
on them.

## 18. Proving a view is live needs the view, not the session

Every counter that says "the stream is working" (decoded frames, surface size,
status) lives on the session, and a session decodes perfectly well into a view
that was never mounted. That is exactly the claim that needed evidence here
("live pixels are in the tiers", not "a stream exists somewhere"), and nothing
in the stack answers it.

`ImageRenderer` cannot help: it never instantiates an `NSViewRepresentable`, so
the offscreen path that renders the rubric PNGs renders a live tier as nothing
at all. The harness (`OpenKoalaBotExample live-tiers`) therefore hosts each tier in a
real `NSWindow`, walks the view tree for the `LiveStreamInputView` that
`SpaceScreenView` mounts, and reports that view's layer contents and its
`isInteractive` flag:

```
tier2-pinned-preview: frames=32  streamViewMounted=true layerHasPixels=true interactive=false
tier3-takeover:       frames=147 streamViewMounted=true layerHasPixels=true interactive=true  inputSent=1 inputAcked=1
hand-back:            interactive=false layerHasPixels=true inputSentUnchanged=true
```

*An SDK should* publish a "this stream is being presented" signal from the view
layer (attach/detach, and frames *presented* as opposed to frames decoded), so
a caller can tell a mounted stream from a live one.

# Part 3: building the live roster and transcript on top of the client

Written while wiring `Sources/OpenKoalaBotExample/Model/BotStore.swift`,
`Model/AgentOutput.swift` and `Model/BotData.swift` to the client above and
running them against the same live Space. Part one is about binding the MCP;
these are about what the MCP *does not give an app* once the binding works.

---

## 19. Agent output is one undifferentiated text stream

**What happened.** A chat transcript is a union: prose, cards, link and file
chips, acknowledgements, status glyphs. The harness publishes one field:
`output_tail`, raw terminal text with ANSI escapes in it. There is no notion of
a message, a turn boundary, an artefact, or a structured event. So an app that
wants a structured transcript has to *infer structure from
console text*, which is guessing.

**Who absorbed it.** `AgentOutputParser`, a whole file of declared rules: strip
ANSI, split on blank lines, then classify each paragraph as a link card, a file
chip, a titled card, a computer-status glyph, or prose. It is deliberately
conservative: anything not confidently richer renders as a plain bubble,
because a wrong guess that turns a line of output into a card with buttons the
Bot never offered is worse than a boring bubble.

```swift
static func bodies(from output: String, active: Bool) -> [MessageBody]
```

The ANSI strip has its own trap worth recording: it must run over unicode
*scalars*, not `Character`s. Swift treats CR-LF as a single grapheme cluster, so
a `Character`-level `if c == "\r" { continue }` silently leaves every `\r\n`
intact while looking like it worked. `testANSIIsStrippedFromRealTerminalOutput`
caught exactly that.

**What the SDK should do.** Publish agent output as typed events
(`.text`, `.toolUse`, `.artifact(path:)`, `.question`, `.finished(exitCode:)`)
not as a terminal scrollback. Every richly-rendered agent UI will otherwise
reimplement this parser, and each one will guess differently.

---

## 20. There is no turn boundary in the output

**What happened.** `agent_status` returns the *whole session* tail. A follow-up
sent with `agent_message` resumes the same session, and its output is appended
to the same blob with nothing marking where the new turn began. A chat UI
fundamentally needs that boundary: without it the thread renders as one user
message, one giant bot message, and then a second user message with all of the
output already spent above it.

**Who absorbed it.** `BotStore.BotTurn.outputOffset`: the app remembers how
long the tail was at the moment it sent each message, and slices the tail
between consecutive offsets to attribute output to the turn that caused it:

```swift
let lower = min(max(turn.outputOffset, 0), chars.count)
let upper = i + 1 < list.count ? min(max(list[i + 1].outputOffset, lower), chars.count)
                               : chars.count
```

This is fragile by construction. It depends on the tail being append-only and on
the app having observed the tail at send time, and it cannot work out the
boundaries for a run started before the app launched. It is also why the tail
has to be normalised (ANSI-stripped) *on the way in* rather than at render time,
or the offsets index into different text than they were measured against.

**What the SDK should do.** Give each turn an id and report output per turn, or
at minimum return a monotonic cursor with each status so a consumer can ask for
"everything since X" instead of diffing lengths.

---

## 21. A run has no identity, so identity has to be smuggled through the prompt

**What happened.** The roster is the whole home screen of this app: persistent,
named, coloured coworkers. A Space knows nothing about any of that. `agent_list`
rows carry `run_id`, `agent`, `status`, `summary`, `accepts_message`,
`created_at`, and no place to put an application's own key. So after a relaunch
there is no supported way to know which run is "Inbox Manager".

**Who absorbed it.** `BotStore.marker(for:)`, which writes `[openkoalabots:<id>]`
into the *prompt*, because the prompt is echoed back as `summary` and is
therefore the only carrier available:

```swift
static func marker(for botID: String) -> String { "[openkoalabots:\(botID)]" }
```

Two costs follow. The marker is now in the text the agent actually reads, so it
is prompt contamination. And every user-visible use of `summary` has to strip it
again, which the live suite caught being missed in exactly one place
(`refresh`, where status's own `summary` was passed through raw) before it was
fixed.

**What the SDK should do.** A `metadata: [String: String]` on `agent_start`,
returned by `agent_list` and `agent_status`. Every app that runs more than one
agent needs this, and the prompt is the wrong place for it.

---

## 22. `agent_list` and `agent_status` disagree about what a state is

**What happened.** The roster feed and the detail probe return overlapping but
unequal fields. `agent_list` gives `status` and `accepts_message` but **no
`reason`**; `agent_status` gives both, plus `exit_code` and the output tail. A
UI that shows a state chip in the roster and the same chip in the thread header
therefore has two different-quality sources for one fact, and a refresh of the
cheap one will blank the explanation the expensive one supplied.

**Who absorbed it.** `BotStore.apply(roster:)`, which merges rather than
replaces: it takes state and `accepts_message` from the list and *keeps* the
last known reason:

```swift
var p = presences[botID] ?? .unhired
p.state = row.state
p.acceptsMessage = row.acceptsMessage      // reason deliberately not touched
```

**What the SDK should do.** One state type, returned in full by both calls.
Cheaper calls may omit the output tail; they should not omit the explanation of
the state they are reporting.

---

## 23. The transcript is a fixed-size window, and truncation is silent

**What happened.** `agent_status` takes `tail: Int`. There is no way to ask for
the whole history, no cursor, and no indication in the response that anything
was dropped. A thread is supposed to be the *complete* record of a coworker
relationship, but past `tail` lines the beginning of it simply stops existing,
and the app cannot tell whether it is looking at a short run or the end of a
long one.

**Who absorbed it.** `BotStore.refresh` asks for `tail: 400` and hopes. Worse,
truncation interacts with entry #11: once the head of the tail falls off, every
recorded turn offset points at the wrong text, so the transcript silently
re-attributes output to the wrong turn.

**What the SDK should do.** Paginated history with a cursor, and an explicit
`truncated: true` when a window is returned. An app must be able to tell "this
Bot said little" from "I am only being shown the last page".

---

## 24. Refusal is right, but there is no outbox

**What happened.** Refusing a message sent mid-turn is correct behaviour and
part one (#9) records it as a thing to keep. But refusal is the *only* option
offered: the alternatives are `force: true`, which abandons the turn in flight,
or nothing. A chat UI's natural behaviour (accept the user's typing, deliver it
when the turn ends) has no support at all, so every app builds its own queue
and its own retry.

**Who absorbed it.** For now, honesty rather than a queue. `BotStore.send`
surfaces the refusal three ways instead of hiding it: in the return value, in
`notices`, and as a marked line in the transcript itself; `BotPresence.refusalHint`
lets the composer warn *before* the user types. That is the right behaviour, but
it is the app doing the harness's job of managing turn admission.

```swift
if !outcome.accepted {
    turn.refusalReason = outcome.reason.isEmpty ? "refused" : outcome.reason
    post(.refusal, botID, "\(name(botID)) did not take that message: \(turn.refusalReason!)")
}
```

**What the SDK should do.** A third delivery mode alongside refuse and force:
`.queueForNextTurn`, with the queued text visible and cancellable. Keep refusal
as the default (silently queueing would be worse), but stop making every app
implement the queue.

---

## 25. Polling cost is per-Bot, but the UI needs the whole roster

Part one #2 recorded that status wants to be a subscription. Building the roster
sharpened it into a second, separate problem: the cost is *per run*, and a
roster screen needs every Bot at once. `BotStore.startPolling` runs one loop for
the whole roster (`agent_list` once, then `agent_status` per hired Bot), and
each of those status calls is an SSH round trip into the Space that reads files.
Nine Bots on the home screen is ten round trips per tick.

There is no batch status call, and `agent_list` is not a substitute because it
omits `reason` and the output tail (#13). So the app must choose between a slow
roster and a stale one.

**What the SDK should do.** `statuses(for: [RunID])` in one call at minimum, and
ideally one merged change stream per Space so the poll frequency is the SDK's
problem rather than every app's.

---

# Part 4: turning the renders into an app you can launch

Parts 1-3 built a Spaces client and two rendered surfaces over it, but
every one of them was reached by an `export` subcommand or a proof harness.
Nothing in the repository could be *launched*. This part closes that: one
`@main`, one window, one `AppModel`, a sign-in gate, live navigation across
roster → thread → the three Agent Computer tiers, hiring, routines, group
chats, and a PiP that survives all of it.

The friction here is a different genus from Parts 1-3. Almost none of it is the
MCP's fault. It is the friction of *packaging*: of a SwiftPM executable that
has to be both a command line and an app, of an unbundled binary that the
window server barely believes in, and of proving that a thing runs when the
obvious proof is blocked by consent. It is recorded because the SDK is going to
ship sample apps, and every one of them will hit §26, §27 and §29 on day one.

---

## 26. One SwiftPM executable cannot be both `main.swift` and `@main`

The sample needs to be two programs. `export` renders the thirteen PNGs that
`RUBRIC.md` grades; `spaces-probe`, `live-tiers`, `live-shell` and
`routine-fire` are the live evidence harnesses; and now there is an app.

These do not compose. A SwiftPM executable target whose entry point is
top-level code in `main.swift` **is** its entry point, and `@main` anywhere in
the same module is rejected outright:

```
'main' attribute cannot be used in a module that contains top-level code
```

The other direction is no better: `App`'s synthesised `main()` starts
`NSApplication` before any argument could be read, so a plain
`@main struct OpenKoalaBotsApp: App` would make the subcommands unreachable.

The resolution is to stop treating either half as the entry point. `main.swift`
was renamed to `CLI.swift` and its dispatch wrapped in `CLI.run(_:) -> Bool`;
the real entry point is an `enum` that runs argument dispatch exactly once and
falls through:

```swift
@main
enum OpenKoalaBotsEntryPoint {
    static func main() {
        if CLI.run(CommandLine.arguments) { return }
        NSApplication.shared.setActivationPolicy(.regular)
        OpenKoalaBotsApp.main()          // App's own default implementation
    }
}
```

`OpenKoalaBotsApp` carries no `@main` of its own, for that reason and no other.
Every subcommand is byte-for-byte the one that shipped before, which the
thirteen md5s confirm.

**What the SDK should do.** Ship the sample app as its own target, or document
this shape. A contributor who adds `@main` to a sample that already has an
`export` subcommand gets a compiler error with no hint that renaming one file
fixes it.

---

## 27. An unbundled SwiftPM executable is not a real app, and the window never composites

With `@main` sorted, the app built, launched, ran its `.task`, attached to the
Space, and drew nothing. No window, no Dock tile, no menu bar.

A SwiftPM executable is a bare Mach-O with no `.app` bundle and no
`Info.plist`. AppKit gives such a process an activation policy that keeps it
out of the Dock and effectively off the window server: the `WindowGroup`'s
window is created and simply never composited. The fix is one line before
`App.main()`:

```swift
NSApplication.shared.setActivationPolicy(.regular)
```

`live-shell` had already needed this and it read as harness-specific. It is
not. It is the difference between "builds" and "runs" for every unbundled
SwiftUI executable, and it cost a cycle of believing the app was broken when it
was merely invisible.

**What the SDK should do.** Any `swift run`-able GUI sample needs this line or
a bundle. Say so once, in the template.

---

## 28. Launching an app must never claim a sandbox, so backend resolution has to be able to refuse

`BotStore.connect()` resolves its Space through `MCPSpacesClient.ensureSpace()`,
which honours an explicit override and otherwise calls `get_or_create_space`,
and §5 already recorded that `get_or_create_space` **claims a Fleet sandbox**.

For a harness invoked by an operator that is a defensible default. For an app
that a user double-clicks it is not: opening a window would silently provision
billable infrastructure. So `AppModel.resolveBackend()` deliberately declines
to guess. It attaches through the real MCP server only when a Space is named
explicitly, and otherwise runs against `DemoSpacesClient` with the reason in
the status strip:

```
offline: set OPENKOALABOTS_TEST_SPACE to attach to a Space
```

The app is fully usable in that state (every surface, every navigation); it
just has no live pixels. That is the correct failure mode for a sample, and it
is only reachable because the client has a seam that can say "no Space" rather
than manufacturing one.

**What the SDK should do.** Separate `attach(to:)` from `provision()`. One
call that may or may not claim a sandbox depending on ambient state is a
footgun in exactly the case (a GUI app's cold start) where it matters most.

---

## 29. Screenshotting your own running app is blocked by TCC, and it fails *silently*

A build is not evidence that an app works. The evidence is a photograph of the
running window. Getting one turned out to be the hardest single problem in this
part, and the way it failed is the interesting half.

The obvious approach (spawn `screencapture -l <windowNumber>` from inside the
app) **exits 0 and writes nothing**. Screen Recording is a TCC grant
attributed to the *responsible* process, which for a child of this binary is
this binary, and an unbundled SwiftPM executable has no grant and no stable
code identity to hang one on. There is no error; there is a zero-byte file.

Handing the shot to a *supervising* process that does hold the grant does not
help either. `screencapture -l` aimed at this app's window from outside exits 0
and writes a zero-byte file too, because the window belongs to the unbundled
process. Both of these were measured, not assumed.

The only way to change either outcome is to trigger a consent dialog, which
this work was explicitly forbidden to do.

So the capture does not read the screen at all. It renders the window's own
layer tree, in-process:

```swift
layer.render(in: ctx)   // CGContext flipped: CoreGraphics is bottom-left
```

No TCC is involved because nothing is reading the screen: the app is drawing
itself. Critically this walks the *real* layer tree of the *real* window,
including the sublayer the rcdp decoder writes frames into, so it captures live
Space pixels where `ImageRenderer` cannot (§18: `ImageRenderer` never
instantiates an `NSViewRepresentable`). The sixteen screenshots of the running
app, tier 2 and tier 3 included, all came out of that one call.

**What the SDK should do.** Sample apps need a supported self-capture path for
CI and for evidence. An in-process layer render is it; document it, because the
discoverable answer (`screencapture`) fails without saying anything.

---

## 30. The MCP client is synchronous, and the app's main actor cannot afford it

Every call in `MCPSpacesClient` is a blocking round trip to a subprocess over
stdio. In a harness that is fine: the harness has nothing else to do. In an
app, `BotStore` is an `ObservableObject` on the main actor, and a three-second
poll that runs `agent_list` plus one `agent_status` per hired Bot (§25) is ten
blocking SSH round trips *on the thread that draws*. The window stops
responding for the duration of every tick.

`BackgroundSpacesClient` exists only to wrap the concrete client and move each
call off the main actor, leaving the published state updates on it. It is a
pure adapter with no behaviour of its own, which is the tell: the seam it
adapts should not have needed one.

**What the SDK should do.** Make the client `async` natively. A synchronous
transport forces every GUI consumer to write this same wrapper, and the ones
that forget will ship a UI that hitches once per poll.

---

## 31. The graded canvas is a fixed size; a window is whatever the user drags

`RUBRIC.md` grades the phone at 1024×1820.5 and the desktop at 1280×757.5,
because those are the fixed sizes of the export canvases. A real window is
none of those sizes.

Re-laying the surfaces out responsively would make the running app and the
graded render *different layouts*, which defeats the entire purpose of having a
rubric: the app would no longer be evidence for the thing that was graded. So
`ScaledCanvas` scales the fixed canvas to fit the window instead:

```swift
let scale = min(g.size.width / size.width, g.size.height / size.height)
```

The app is therefore a true zoom of the graded artefact at every window size.
The cost is honest and worth naming: it is a phone-sized canvas in
a desktop window, not a responsive desktop app, and letterboxing is visible at
unusual aspect ratios.

---

## 32. Mounting a concurrent workstream's surface without moving a single graded pixel

Routines and group chats were built on a sibling branch as self-contained views
with documented mount points. Mounting them meant editing `Mobile/Screens.swift`,
`Mobile/Chrome.swift` and `Desktop/DesktopShell.swift` (the three files whose
output is graded) to add a thread-header overflow, a roster "new group"
action, and a real `RoutinesPanel` in the right panel where an inline empty
state used to be.

§11 recorded an *inert* modifier silently changing five PNGs. Adding visible
chrome to a graded screen is a much larger version of the same hazard.

The rule that made it safe was already latent in these files and is worth
stating explicitly: **every app-only affordance is an optional closure, and the
affordance is drawn only when the closure is non-nil.** The export path passes
nothing, so it takes a branch that is not merely equivalent to the old code but
is *literally the old code, structurally untouched*, not even wrapped in a
container, since an `HStack` around a single child is exactly the kind of
"surely inert" change §11 is about:

```swift
} else if let onOverflow {
    HStack(spacing: 18) { ellipsis; display }
} else {
    CircleIcon(symbol: "display", …)      // the graded branch, unmoved
}
```

The desktop Routines panel follows the same rule via an injected
`RoutineStore?`, so `export` still renders the inline empty state character for
character. All thirteen PNGs were md5-compared against `c5061c405` after the
mount and are byte-identical.

---

## 33. A harness that takes a `<space>` argument and attaches to a different one

`routine-fire` (the live scheduler proof, referenced by name in
`Routine.swift` and `RoutineStore.swift` but never actually written, because
the routines work and this file's rename were in flight at the same time and
neither branch owned it) took `<space>` as its first argument, threaded it
nowhere, and built a `BotStore`.

`BotStore.connect()` resolves its own Space (§28). So the first run printed:

```
attached to fleet:cua-spaces-ukey-…:claim-e0b003ad2c
```

It had been handed a local macOS Space and it claimed a Fleet sandbox instead.
Nothing warned; the only reason it was caught is that the attach line prints
what it attached to. The claim was released immediately.

This is §5 with a sharper point on it. The API shape is the bug: a caller with
a Space in hand has no argument to pass, so *every* consumer reinvents pinning
out of band (here, an environment variable read deep inside `ensureSpace`), and
the failure mode of forgetting is not an error but silent provisioning. The fix
in this file is to pin before constructing anything and then assert:

```swift
setenv(MCPSpacesClient.spaceOverrideVariable, space, 1)
…
guard attached == space else { … refusing, nothing was started … }
```

**What the SDK should do.** `attach(to: SpaceID)` as a first-class call, and
make the claiming path spell its name (now `createSpace(options:)`, with a
required `on`). An out-of-band
environment variable should never be the only way to say "use this one".

---

## 34. `.frame(maxHeight:)` does not clip, and only a real Space is big enough to show it

The desktop right panel stacks a tier-2 preview, a live window list, produced
files, and now the Routines panel. The window list was a bare `VStack` +
`ForEach` with the call site capping it at `.frame(maxHeight: 190)`.

That cap does nothing. A frame modifier changes the size *proposed* to a
`VStack`; the stack still lays its children out at their ideal heights and
draws straight through the bottom of the frame.

Nothing revealed this until the app ran against the real demo Space, which had
**92 open windows**. The list drew over the Routines panel below it and the
tier-2 preview above it, and the first screenshot of the mounted panel is a
column of overlapping text. Fixtures have three windows; every test passed; the
build was clean. The bug existed only at real-world scale, and the only thing
that found it was launching the app and looking at it.

The fix is a `ScrollView` inside the list and a hard `.frame(height:)` plus
`.clipped()` at the call site, applied to the Routines mount too, since a Bot
with a dozen routines is the same bug waiting.

**What this says about verification.** "It builds" and "the tests pass" were
both true of the broken version. A running app and a real Space found it in one
screenshot. That asymmetry is the argument for this whole part existing.

---

## 35. A proof harness that writes into the user's real data

`RoutineStore(fileURL: nil)` resolves to `defaultFileURL()`, the *user's*
routine list. `routine-fire` used that, so its first run appended a
"routine-fire proof" routine and left it there; the second run found two
routines due and fired both, one of which was the leftover.

A harness that vandalises the state it is testing produces evidence that gets
less trustworthy the more often you run it. It now writes to a temporary file
it deletes on the way out.

The store was explicitly built with an overridable `fileURL` "so tests get
their own file and the real one is never touched by `swift test`"; the seam
existed and the CLI harness simply did not use it. Worth recording because the
tests were the case everyone thought about and the harness was not.

---

## 36. The Accessibility consent dialog cannot be dismissed without the consent it gates

The demo Space carried an unresolved **"Accessibility Access"** dialog
(`universalAccessAuthWarn`) left by an earlier agent. The instruction was to
dismiss it cleanly with "Don't Allow" if that were possible.

It is not, from inside the Space:

```
$ osascript -e 'tell application "System Events" … click button "Don't Allow" …'
System Events got an error: osascript is not allowed assistive access. (-1719)
```

Clicking a button in the dialog that grants assistive access requires assistive
access. The only non-circular routes are synthesising a click through the
stream (which needs the dialog's screen coordinates, and `list_space_windows`
returns size but no origin) or killing the process, which is not the same thing as
denying it. The dialog was left alone.

**What the SDK should do.** Two things, both small. `list_space_windows` should
return window origin, not just size, so a caller can aim at a window it can
already see. And a Space needs a supported way to enumerate and dismiss pending
TCC prompts, because a consent dialog nobody can reach is a Space that quietly
degrades for every future user of it.

---

## 37. Ninety-two windows, all named the same thing, and no way to ask for "the Bot's screen"

`list_space_windows` on the live Space returns 92 entries. Sixty-odd of them
are titled some variant of `lume - watch.command - tail -f out.log - 120×30`,
left by earlier runs. The product question (*which window is this Bot working
in?*) has no answer in that payload.

§21 recorded that a run has no identity. This is the display-side twin: a run
has no *window*. The app cannot show "Sales Outbound's screen"; it can only
show a window the user picked, which is why window selection is a drag-and-drop
affordance rather than something automatic (§17), and why the tier-2 caption
reads "…'s screen" over whatever the user chose.

**What the SDK should do.** Attribute windows to runs. `agent_start` knows the
process it spawned; the window list knows the owning pid. Joining those two
inside the Space would turn an unanswerable question into a field.

---

## 38. Routines and group chats have no server-side existence, so the app owns their whole lifecycle

§40 and §42 record that the MCP has no scheduler and no fan-out. Mounting those
surfaces in a real app adds the lifecycle consequence, which is the shell's
problem rather than the feature's.

Both stores are **singletons owned by the app**, not per-Bot: routines persist
to one file and are fired by one scheduler, so a store per Bot would be a
scheduler per Bot racing over that file. Both can only be attached *after*
`BotStore.connect()`, because both send through the roster store and before
that point a fired routine has nowhere to go:

```swift
routines.attach(runner: BotStoreRoutineRunner(store: store))
routines.startScheduler()
groups.attach(messenger: BotStoreGroupMessenger(store: store))
```

And both die with the window: the scheduler is stopped and the list saved on
`willClose`, so a run this app started never outlives it (§8).

The whole of that is bookkeeping the app performs on the SDK's behalf, and
every app that ships recurring work will perform it again, differently.

**What the SDK should do.** A durable, server-side `schedule(prompt:every:)`
would delete this section, `BotStoreRoutineRunner`, the tick loop, the
persistence file, and the shutdown ordering above.

---

## 39. Refusals have to be photographable, not just handled

§24 recorded that refusal is correct behaviour with no outbox behind it. The
app shell is where that becomes a UI requirement rather than an API
observation.

`AppModel.send` keeps the outcome either way and publishes it, and the composer
draws it under the field rather than logging it:

```
Not delivered: <Bot> is mid-turn; the slot was skipped
```

The capture run deliberately sends to a Bot that may still be working, so the
refusal is what gets photographed when it happens. `routine-fire` prints the
same thing for a skipped slot. The rule the shell enforces is that **there is
no code path where a refusal is swallowed**: every one of them reaches a
surface a person or a screenshot can see.

That is only necessary because refusal is so common: with one long-lived thread
per Bot and no queue, a user who types twice in a row hits it. An outbox would
turn the app's most-exercised error path back into an ordinary send.

---

# Part 5: routines, group chats, and the affordances with no primitive behind them

Numbered from §40 deliberately: Parts 1-3 occupy §1-25 and Part 4 (the app
shell) takes §26 onward. The gap is left so two concurrent merges cannot
collide.

What this part is mostly about: four product features whose *surface* is fully
buildable and whose *substance* the MCP has no primitive for at all. Routines
have no scheduler, group chats have no fan-out, approvals have no gate, and
avatar motion and typing indicators have no signal to drive them. Each section
says exactly where the client had to invent one, so the SDK knows what it is
being asked to absorb.

---

## 40. There is no scheduler, so "recurring" lives entirely in the client

A routine is a recurring task a Bot runs on a schedule. The Space has no idea
what a schedule is: there is no `agent_schedule`, no cron surface, no deferred
`agent_start`, and no server-side object that outlives the client process. So
`RoutineStore` is the scheduler (a JSON file, a `tick(now:)`, and a `Task`
loop), and the routine exists only while something on the user's Mac is awake
to fire it.

That is a real product gap, not a stylistic one. The canonical example is an
"8am routine": the whole point is that it fires whether or not the user has
the app open. Here it fires only if the app is open at 8am.

It also forces a decision no app should have to make. A scheduler that was
asleep for six hours could fire six times on waking; `Routine.isDue(at:)`
deliberately collapses a missed backlog into a single firing, because twelve
overnight routines landing in one Bot at 9am would be worse than a skipped run,
but that policy is now baked into an app rather than into the platform, and
every other client will pick a different one.

```swift
guard let next = schedule.nextFireDate(after: last, calendar: calendar) else { return false }
return next <= now   // one firing on waking, never a backlog
```

**What the SDK should do.** Own the schedule: `space.schedule(prompt:every:)`
returning a durable object that fires inside the Space, survives the client
being closed, and reports its firing history. Failing that, at minimum a
documented "missed slot" policy so every client does not invent its own.

---

## 41. A firing produces a run with no link back to the routine that caused it

`agent_start` returns a run id and nothing else; a run carries no metadata field
(#21 already recorded that Bot identity has to be smuggled through the prompt).
A routine has the same problem one level up: after the app restarts, `agent_list`
shows a run, and nothing in it says "this was the 8am triage routine".

So the routine store keeps `lastRunID` on its own side of the wall, and the
firing prefixes the prompt with `[routine]` so the *transcript* can at least
mark scheduled work:

```swift
let text = "\(Self.prefix) \(routine.title): \(routine.prompt)"   // "[routine] 8am triage: …"
```

Two markers are now being smuggled through one prompt string (`[openkoalabots:<botID>]`
from #21 and `[routine]` from here), and both are visible to the agent, which
means both are visible in its output and both have to be stripped before
display. A prompt is not a metadata channel.

**What the SDK should do.** An opaque `metadata: [String: String]` on
`agent_start`, echoed by `agent_list` and `agent_status`. It costs the server
nothing and removes every marker hack in this file.

---

## 42. A group chat is a hand-rolled fan-out, and the Bots cannot hear each other

A group chat is 2-6 Bots plus one human in one thread. There is no group
primitive: no multi-run message, no shared session, no way for one run to see
another's output. `agent_message` takes exactly one `run_id`, so "message the
group" is a loop, and a partial delivery is the normal case rather than an error:
one Bot mid-turn refuses (#24) while the other four take it.

The consequence that actually hurts is not the loop, it is that the Bots are
deaf to each other. Every run is its own session with its own context, so the
only way a Bot learns it is in a room with four others is for the client to
write it into the message:

```swift
"[group:\(chat.title)] You are in a group chat with the user and \(others). …"
```

That framing is re-sent on every single turn, because there is nowhere to put it
once. With six members it is six copies of the same paragraph per user message,
paid for in tokens, and the peer list silently goes stale for any Bot that was
mid-turn when the membership changed.

**What the SDK should do.** Either a real group object (one message, many runs,
one merged event stream with attribution) or, much cheaper, a per-run
*persistent preamble* that the SDK maintains and the client can update once when
membership changes.

---

## 43. The approval card is a product surface with no primitive behind it

This is the bluntest gap in the file, so it is worth stating without hedging:
**nothing in this build can stop a Bot for a human decision, and the approval
card does not enforce anything.**

`agent_start` runs auto-approved. There is no approval tool, no pause, no
"waiting for consent" state in the harness vocabulary, and no callback a client
could answer. The architecture is the reason: the Space *is* the sandbox, and
the agent already has the machine: cookies, logins, `/workspace`, the lot. By
the time a client could render a card, the thing the card asks about has already
happened.

So `ApprovalCard` renders the gate (`Allow once`, `Deny`,
`Always allow` on desktop; `Allow once` and `Deny` on the mobile local-command
card), records the answer, and says so on its face. The code refuses to let a
caller believe otherwise:

```swift
/// The single source of truth for whether a human decision here can
/// actually stop a Bot. It cannot.
static let enforcementIsImplemented = false
```

and a test asserts that flag stays `false` until the Spaces client actually
gains a primitive. The card also draws a visible "Not enforced in this build"
line by default, because a gate that does not gate has to say so somewhere the
user can see, not only in a doc comment.

**What the SDK should do.** An interception point: `agent_status` gaining an
`awaiting_approval` state with a structured description of what is being asked,
plus `agent_approve(runID:decision:)`. Without those two, every chat-shaped
product ships a card that lies.

---

## 44. "Typing" cannot be distinguished from "thinking", because `running` means both

The visual treatment of a typing indicator is a free choice, but the harder half
is that there is no signal to drive it. The harness publishes `running`, which
covers a Bot that has produced nothing yet and a Bot that is streaming output
right now. The only way to tell them apart is
to diff the output tail between two polls, which means the indicator's fidelity
is bounded by the poll interval (#2, #25) and a fast burst of output between
ticks is invisible.

`BotMotionState.from(_:isComposing:hasFreshOutput:)` therefore takes two facts
the presence does not carry (whether the *user* is typing, and whether output
arrived since the last poll), because neither exists anywhere in the protocol:

```swift
case .running: return hasFreshOutput ? .speaking : .working
```

**What the SDK should do.** An output *stream* rather than a tail snapshot.
One `AsyncSequence` of output chunks per run would make typing indicators, the
speaking avatar state, and per-turn attribution (#20) all fall out of the same
primitive instead of being three separate poll-diffing hacks.

---

## 45. Six avatar states, one status vocabulary, and no overlap between them

Avatar motion has six states, and their names here are derived from the only
state vocabulary the app actually has: `AgentState`.

Mapping one onto the other exposes that they are not the same kind of thing.
`AgentState` describes a *process* (`running`, `crashed`, `exitCode`); the
avatar states describe an *interaction* (listening, thinking, speaking). Two of
the six (`listening` and `speaking`) have no source in the protocol at all and
had to be synthesised from client-side facts (§44). Meanwhile `finished` with a
non-zero exit and `crashed` and an unreadable `unknown` all collapse to the same
`blocked`, because there is no user-facing difference between them at 54pt.

The one thing worth keeping from this: `blocked` deliberately does **not** loop.
A stopped Bot that animates forever is indistinguishable from a busy one, which
is exactly the confusion `unknown` being grey rather than green was introduced
to prevent (#22).

**What the SDK should do.** Separate *liveness* from *conversational state* in
the published vocabulary. An app needs both, and today it gets one and infers
the other.

---

## 46. `ImageRenderer` has no run loop, so every animation is a render-path hazard

§11 recorded an inert interaction modifier silently changing five exported PNGs.
Building four animated affordances (typing dots, six avatar motions, the
reaction bar, the link-card loading state) meant meeting that hazard four more
times, so it is worth naming the rule that came out of it.

Animation here is never an implicit `withAnimation`/`repeatForever` modifier
attached to a view on the graded path. It is a pure function of `(state, time)`
rendered by a `TimelineView`:

```swift
static func frame(_ state: BotMotionState, at t: Double) -> Frame
```

Under `ImageRenderer`, which has no run loop, a timeline renders one
deterministic frame, so the export cannot drift; the function is testable
without a window server; and `fixedTime:` freezes any of it for a render. The
static `BotAvatar` the thirteen graded screens use was left untouched, with
motion added in a *wrapper* rather than in the view itself.

All thirteen graded PNGs were md5-compared before and after this part and are
byte-identical.

**What the SDK should do.** Nothing: this one is SwiftUI's, not the MCP's. It
is recorded because it is the second time the same trap cost real time, and the
pattern above is the thing to reach for the third time.

# Part 6: what the first real user found in the first five minutes

Parts 1-5 were written while building. This part was written after handing the
built thing to the person it was for, who opened it, clicked the top bar, and
said: *"ok this ui looks horrible, its unusable, the desktop mode is so small,
and the top bar is overlapping, its also sueor laggy"*, then *"the space has
thousands of terminal windows open within it, and i see more and more opening
infinitely"*.

Every entry below is a defect that shipped, not a rough edge that was
anticipated. The common thread is that each one was invisible to the way the
work had been checked: the rubric renders a fixed canvas offscreen with fixture
data, so nothing that only goes wrong in a *window*, over a *live* Space, with
a *real* roster, could ever have shown up in it. A passing rubric and a green
suite said the app was finished. The app was not finished.

## 47. The first thing the app did was block its own main thread

**What happened.** `AppModel.init` resolved the backend, and resolving the
backend called `MCPSpacesClient.handshake()`: two blocking JSON-RPC round trips
down a pipe to a Python subprocess. `init` is `@MainActor` and runs inside
`@StateObject`'s initialiser, so the app froze before it had a window.

`BackgroundSpacesClient` already existed to keep exactly this off the main
actor, and it was doing its job for every call *except* the one that happened
before it was constructed.

**Who absorbed it.** `BackgroundSpacesClient` gained a `prepare` closure run
once inside its worker actor before the first real call. The handshake still
happens exactly once and still happens before anything needs it; it just
happens on a cooperative-pool thread.

**What the SDK should do.** Make the expensive part of construction `async`.
A client whose initialiser performs network I/O cannot be constructed from a
UI context, and `async` on the *methods* does not help when the cost is in the
constructor.

## 48. A canvas sized for the grader is not a canvas sized for a window

**What happened.** §31 recorded the tension; this is what it cost.
`DesktopShell` hard-coded `.frame(width: 1280, height: 757.5)` because
`RUBRIC.md` diffs D1/D2/D3 as renders at exactly that size. In a 980x820
window the shell therefore laid itself out at 1280x757.5 and was simply clipped
by the window on every side: the sidebar lost its top and bottom rows, the
right-hand panel was entirely off-screen, and a wide band of dead background sat
under the toolbar. "the desktop mode is so small" is what that looks like to
someone who did not write it.

**Who absorbed it.** `DesktopShell` took a `canvas: CGSize` parameter defaulting
to `DS.desktopCanvas`. The export path passes nothing and renders byte-identical
PNGs; the app passes the window's real size from a `GeometryReader`. Everything
inside was already expressed in relative metrics, so a bigger canvas widens the
transcript and reveals more roster rows. Real layout, not magnification, which
was the explicit requirement.

The transcript column needed the same treatment in the other direction: a hard
562pt column plus a 245pt sidebar does not fit a 720pt window, so the whole
three-column `HStack` overflowed and, being centred, was clipped on *both*
edges. It is now `min(DT.contentMax, canvas.width - chrome)`, which resolves to
exactly 562 at the graded canvas and shrinks below it.

**What the SDK should do.** Nothing. This is the cost of grading a resizable app
with fixed-size renders, and the lesson is that the graded size must be a
*parameter with a default*, never a constant. The moment it is a constant, the
export path and the product path cannot both be right.

## 49. A `ScrollView` is not free, even when there is nothing to scroll

**What happened.** The desktop sidebar and transcript, and the mobile roster
list, were plain `VStack`s. With six fixture Bots that is correct and is what
the rubric graded. With the live Space's thirty-odd runs the roster ran off both
ends of its pane and painted over the chrome above and below it.

The obvious fix (wrap them in `ScrollView`) moved all three graded desktop
PNGs, even though the fixture content is far shorter than the viewport and
nothing could actually scroll. Measured, not assumed: md5s changed on
`desktop-01`, `desktop-02` and `desktop-03`.

**Who absorbed it.** `OptionalScroll`, a sibling of the existing `OptionalTap`,
which was added in Part 4 for precisely the same reason and whose doc comment
says precisely the same thing: *a modifier applied unconditionally, even one
that does nothing, can take part in layout, so the export path must take a
branch where the modifier is not applied at all.* The rule had already been
learned and written down, and was still not generalised in time.

**What the SDK should do.** Nothing. Recorded because the second instance of a
rule costs as much as the first if the rule lives only in one file's comment.

## 50. `scaleEffect` does not resize anything, and the failure is silent

**What happened.** `ScaledCanvas` fitted the phone canvas into the window
by applying `.scaleEffect(scale)` and then `.frame(g.size)`. `scaleEffect` is a
render transform: it changes what is drawn and leaves the view's *layout* size
alone. The canvas therefore went on claiming 1024x1820.5 however small it was
drawn, the outer frame did not clip it, and it overflowed its pane in every
direction, including upwards, underneath the toolbar. That is the whole of
"the top bar is overlapping".

Two further attempts got it wrong in two new ways, both worth recording because
both look plausible:

* hosting the scaled content in an `overlay` on a correctly-sized `Color.clear`
  fixed the clipping but not the layout size;
* scaling about `.topLeading` and re-framing to the scaled size worked only once
  the replacement frame was *also* given `alignment: .topLeading`. Without it
  the layout box and the drawing disagreed by half the unscaled canvas and the
  phone landed entirely outside its pane, which renders as a **blank surface**,
  not as an obviously misplaced one.

**Who absorbed it.** `ScaledCanvas`, now
`.frame(canvas)` → `.scaleEffect(s, anchor: .topLeading)` →
`.frame(canvas * s, alignment: .topLeading)` → `.frame(g.size)` → `.clipped()`,
plus a toolbar that is `.fixedSize` with `layoutPriority(1)` over a surface that
clips, so the two cannot overlap at any size on either surface.

**What the SDK should do.** Nothing. But note the diagnostic cost: the only way
to tell these three variants apart was to print `GeometryReader`'s reported size
and compare it against the rendered result, because every wrong version renders
*something* and none of them logs anything.

## 51. The roster needs every Bot; only the open thread needs `agent_status`

**What happened.** §25 said polling is per-run while the roster needs every bot.
The poll loop took that literally: every tick called `agent_list` and then one
`agent_status` per hired Bot, serially. Against a Space with thirty runs that is
thirty-one round trips per tick, and because `BotStore` is `@MainActor` the loop
returned to the main actor between every one of them to slice a 400-line tail
into characters, re-parse it, and republish the whole roster: thirty parses and
thirty full SwiftUI invalidations per tick, for ever. A click had to queue behind
whichever one was in flight.

**Who absorbed it.** `BotStore.pollOnce`. `agent_list` already returns state,
summary and `accepts_message` for every run in one call, and only one transcript
is ever on screen, so a tick is now `agent_list`, plus `agent_status` for the
focused Bot, plus `agent_status` for one other Bot round-robin. No Bot loses
coverage, which matters, because
`testPollLoopRefreshesTheRosterFromOneTask` asserts that an *unfocused* Bot's
output reaches its transcript, and that test still passes unchanged.
`rebuildThread` is additionally skipped when the tail has not moved, which on an
idle roster is always.

Measured against the live Space holding 30 runs, over an identical 55-second
scripted tour of the toolbar:

| | before | after |
|---|---|---|
| `agent_status` round trips | 33 | 4 |
| total Spaces round trips | 40 | 12 |
| main-thread stall samples > 1ms | 210 | 20 |

**What the SDK should do.** Give `agent_list` an opt-in `tail` so a caller can
get the roster *and* the transcripts it is showing in one round trip, instead of
choosing between a cheap list with no output and N expensive per-run probes.

## 52. A stream nobody is looking at is still a stream

**What happened.** The desktop source polls `get_desktop_state` twice a second
and each frame is a ~1.6 MB PNG (§10). Two things were wrong with where that
cost landed. The PNG was decoded and blitted into a `CVPixelBuffer` (three
million pixels through a `CGContext`) inside `ingestDesktop`, on the main
actor. And nothing ever stopped the poll: `SpaceScreenView` started the session
in `.task` and navigating away left it running for the life of the process.

**Who absorbed it.** The conversion moved into `DesktopFrameSource`'s own
executor, so the main actor only takes delivery of a finished buffer; and
`SpaceScreenView.onDisappear` stops the session unless it has been popped out
into the PiP, which is the one case where the session must outlive the view that
started it.

**What the SDK should do.** A frame source that is a poll loop in disguise
should expose backpressure or a subscription. Failing that, it should at least
not hand callers a decode step they will naturally perform wherever the callback
happens to run.

## 53. "Laggy" is not a bug report, and a screenshot cannot become one

**What happened.** The user's most actionable complaint (*"im unable to click
anything in the top bar without it freezing for a few seconds"*) was the one
thing no screenshot could show and no existing test could measure.

**Who absorbed it.** `MainThreadProbe`: a `Timer` on the main run loop in
`.common` mode, whose overshoot past its own interval *is* the time the main
thread was unavailable, and therefore is the latency of a click landing at that
moment. Deliberately a run-loop timer rather than a `Task`, because a
synchronous block of the thread (which is exactly what the MCP client does) is
invisible to anything scheduled on the cooperative executor.

Two things it had to learn to be honest. It excludes the capture harness's own
`CALayer.render(in:)`, which is hundreds of milliseconds of main-thread work
belonging to the instrument rather than the program. And it timestamps its worst
samples, which is what showed that the residual ~350ms stalls in both the before
and after runs sit at `t+39s` and `t+41s`: the two `setContentSize` calls in
the tour, i.e. AppKit relaying out on a window resize, identical before and
after and not a poll-loop defect at all.

It also counts Spaces round trips per tool, because wall-clock stall moves with
SSH latency and machine load while the number of round trips a tick costs does
not. The table in §51 is that counter.

**What the SDK should do.** Nothing. Recorded because the measurement was harder
to build than the fix, and because "it feels slow" stays unfixable until
somebody makes it a number.

## 54. Cleanup that only runs when the test passes is not cleanup

**What happened.** The live suites cleaned up per test, from a list of closures
drained in `tearDown`, the right shape. But several call sites appended their
teardown *after* an assertion about the thing they had just started:

```swift
let fired = await routines.tick(now: Date())
XCTAssertEqual(fired.count, 1, …)          // fails here…
guard case .started(let runID) = fired[0].firing else { … }
registerCleanup(for: runID)                 // …and this never runs
```

A routine fires on a timer and a group chat fans out to one run per member, so
those ids only exist after the fact, and a failing assertion, or a throw
anywhere in between, leaked every run it had just started, permanently, into the
user's demo Space.

**Who absorbed it.** Two changes. Tests that can name their run register the
teardown on the line after they get the id, before any assertion. Tests that
cannot (the scheduler and group-chat ones) call `adoptNewRuns()`, which diffs
`agent_list` against a baseline taken at the top of the test and registers
teardown for everything new. That needs no id from the test at all, so there is
nothing for a failure to skip past.

Underneath both sits a **sweeper**: an `XCTestObservation` that snapshots the
Space's run ids and window count before the bundle and, in `testBundleDidFinish`
(the one hook guaranteed to run however the tests ended) removes every run
that appeared while the bundle was running and asserts the window count is back
where it started. It is not decoration: on its first green run it found and
removed `run-ca37ba33`, a run no test had registered.

And the runs themselves no longer open anything. `agent_start` grew a `show`
parameter and the suite sets it false, because the cheapest cleanup to get right
is the one where nothing was created.

**What the SDK should do.** §8 asked for `run.delete()` and a scoped
`withAgentRun { … }`. This part is the evidence for why the *scoped* half
matters more than the delete: every leak here was a teardown that existed and
did not run.

## 55. A suite that cannot prove it cleaned up has not cleaned up

**What happened.** Three separate times during this work, the suite was believed
clean and was not. Each time the belief came from having written cleanup code,
and each time the refutation came from counting.

**Who absorbed it.** The sweeper prints the Space's window count before and
after and fails the run (via exit status, since `testBundleDidFinish` is past
the point where `XCTFail` attaches to anything) when the Space is not back
where it started. That assertion is what caught §56 and §57 below; without it
both would have shipped as "the tests pass".

**What the SDK should do.** Nothing. The entry exists to argue that "leaves the
environment as it found it" is a property a suite should *assert*, in the same
way it asserts anything else, rather than a property it claims in a doc comment.

## 56. `kill` lets Terminal save its windows; `kill -9` does not

**What happened.** This is the "more and more opening infinitely" the user saw,
and it took four instrumented attempts to find.

Leaked `watch.command` windows accumulated in the Space. §8 and §37 already
explain why nothing closed them. The obvious remedy is to kill Terminal, and it
appears to work: the windows vanish from `list_space_windows`. They come back.
Not gradually: *all of them*, the next time anything launches Terminal, which
on this Space is the next agent run.

The reason is that `pkill` sends SIGTERM, Terminal treats that as a graceful
quit, and a graceful quit **writes its window-restoration state**. Killing
Terminal to clear a hundred windows does not clear them; it saves them.

Everything reachable by configuration was tried and measured, and none of it
helped: `NSQuitAlwaysKeepsWindows = false`, `ApplePersistenceIgnoreState = YES`,
and deleting `~/Library/Saved Application State/com.apple.Terminal.savedState`.
The last of which is a race that is lost, because the dying process writes the
file after the `rm`. With the state directory verifiably absent and both
preferences set, relaunching Terminal still produced **100** shells.

`kill -9` produced **1**. A process killed outright never gets to save anything.

**Who absorbed it.** `SpaceHygiene.forgetRestorableTerminalWindows`: SIGKILL,
then remove the state directory, then set the preference for good measure. All
signals and files: no `osascript`, no Automation, no TCC prompt, which was a
hard constraint throughout.

**What the SDK should do.** This is the strongest argument in the file for
`agent_start` not opening a window by default. A cosmetic terminal that a run
cannot close, whose title does not name the run (§37), and which the operating
system resurrects after it is killed, is not a convenience; it is a leak with a
restore feature. The run-scoped `watch-<run_id>.command` name and the
self-terminating watcher added here make the window identifiable and stop the
orphaned `tail`, but `show=false` is the only setting that is actually safe for
an automated caller.

## 57. `ProcessInfo.environment` is a snapshot, so a test cannot set its own

**What happened.** The suite disabled agent terminal windows by calling
`setenv("OPENKOALABOTS_AGENT_WINDOWS", "0", 1)` before any test ran, and
`MCPSpacesClient` read that variable through
`ProcessInfo.processInfo.environment`. Foundation caches that dictionary on
first read. The value never arrived, every run in the suite went on opening a
window on the demo machine, and nothing said so.

What made it findable was arithmetic, not a log: the sweeper reported 114
windows against a baseline of 107, the suite had started 7 runs, and
114 − 107 = 7.

**Who absorbed it.** `MCPSpacesClient.showsAgentWindows` became a stored
property seeded from the environment once and settable afterwards; the suite
sets it directly. The environment variable still works for callers who set it
*before* launching the process, which is the only time it could ever have
worked.

**What the SDK should do.** Nothing: this is Foundation's. Recorded because the
failure mode is silent, the workaround looks identical to the broken version at
the call site, and the only reason it was caught at all is that something was
counting.

# Part 7: what landing the SDK on top of the UX fixes taught

Parts 1-6 were written by building the app. This part was written by **merging**
two branches that had been right about different things at the same time: one
extracted the Spaces SDK out of OpenKoalaBots, the other fixed the four defects
the first real user found. They were cut from the same parent an hour apart and
neither could see the other.

The merge is interesting because the conflicts were not textual. Six files
conflicted; the two that mattered were `modify/delete`: the SDK deleted
`MCPSpacesClient.swift` and `BackgroundSpacesClient.swift`, and the UX branch
had just put **measured performance fixes inside them**. Git cannot tell the
difference between "this file was deleted because its job moved" and "this file
was deleted along with the fix that was in it". Both look like a deletion.

## 58. A refactor deletes fixes it cannot see, and only a test notices

**What happened.** `f593e1573` added two things to `MCPSpacesClient`: a
process-wide tool-call counter (the instrument behind "round trips 40 → 12"),
and `showsAgentWindows` plus `agent_start`'s `show: false` (the reason the test
suite stopped covering a demo machine in terminal windows). The SDK branch
deleted that file and rewrote the app against `CuaSpaces`. Neither addition
existed anywhere in the SDK. Taking the deletion (which is the *correct*
resolution, the whole point being that the app consumes the SDK) silently
reverted both.

Nothing failed. The suite was green either way, because the suite asked for no
windows by setting a flag whose new home did not exist, and got windows. That
is §57's failure mode again, one layer up: **a request that goes nowhere looks
exactly like a request that was honoured.**

**What made the difference.** `MainThreadProbe.report()` referenced
`MCPSpacesClient.callCounts` and therefore did not compile. The counter had a
*consumer*, so its deletion was a build error. `showsAgentWindows` had no
consumer that the compiler could see (the suite set it, the wire read it, and
nothing checked the two were connected), so its deletion was silent. One was
caught by the toolchain and one had to be caught by reading the diff.

**Who absorbed it.** The counter became `CuaSpaces.SpacesCallCounter`, recorded
inside `MCPStdioTransport.callTool`, which is strictly better than where it was:
a counter living in the app could only ever see the app's own calls, and after
the extraction almost every call is the SDK's. `showsWindow` became a field on
`AgentStartRequest` that reaches `agent_start`, and, the actual fix,
`testAgentStartCarriesShowSoASuiteCanOpenNoWindows` asserts it reaches the wire
in both positions.

**The rule.** A measured property that no test asserts is not a property, it is
a coincidence that currently holds. It will not survive the next refactor, and
the refactor will be green while it removes it. If a number was worth measuring,
the thing that produces it is worth a test; otherwise the measurement's only
durable record is a commit message, and commit messages do not fail builds.

## 59. Two branches can move in opposite directions on the same hazard

**What happened.** The UX branch removed every `osascript` that drove Terminal,
because Automation consent is not granted in the demo Space: unasked it raises a
consent dialog on the user's desktop, and asked-and-refused it exits 1 while
looking like it worked. It replaced them with POSIX signals and wrote §56 about
why `kill -9` specifically.

The SDK branch, concurrently, wrote `AgentRun.delete()`, correctly fixing the
real bug, that `terminalWindowClosed` was set `true` unconditionally after a
close that matched on run id, when Terminal windows are titled after the
*script*, which is how ~112 orphaned windows accumulated. Its fix verifies
against the Space's own window list and names survivors in `residue`. That part
is right and is kept.

But the close it verifies was still an `osascript`. The branch that removed the
hazard and the branch that added a new instance of it were both correct locally,
and a mechanical merge keeps both. The close is now the same `pkill -9` on the
run's own `watch-<run_id>.command` that §54 and §56 arrived at; the verification,
which is the actual fix, is untouched.

**The rule.** When one branch removes a class of call, grep the other branch for
that class before merging. Conflict markers only find the lines that disagree
textually; two branches can agree on every line and still disagree about whether
a thing is allowed to exist.

## 60. A latency number without its workload is not a baseline

**What happened.** `f593e1573` recorded "main-thread stall samples over 1ms
210 → 20", measured over a scripted toolbar tour **with 30 runs in the Space**.
Re-running the same tour after the merge gave **187**, which reads as a
catastrophic regression and is not one: the suite had cleaned up after itself,
so the Space held **zero** runs, the poll loop had nothing to call
`agent_status` on, and the run is simply a different workload. The honest
comparison needed the *pre-merge commit re-measured today*: 185 stalls against
the merge's 187, p50 1.5 ms on both, two stalls over 100 ms on both, both at
t+39 s and t+41 s where the harness deliberately resizes the window.

The interaction timings (the number a user actually feels) are ≤ 0.2 ms on
both sides, and `BotStore.swift` is byte-identical across the merge, so the
poll-loop property is preserved by construction rather than by luck.

**The rule.** A performance baseline is a pair: the number *and* the state of
the world it was taken in. "210 → 20" is not reproducible; "210 → 20, scripted
toolbar tour, 30 runs in `local:cua-space-e3c1b54907`" is. A baseline that does
not record its workload cannot distinguish a regression from a Tuesday, and the
first thing anyone does with it is panic.

## 61. `RosterStream` cannot yet say "every Bot, eventually"

**What happened.** §25 asked for a roster subscription that costs one poll per
tick, and the SDK has one: `Space.roster(pollingEvery:detailed:tail:)` yields a
`RosterUpdate` per tick carrying `roundTrips`, so the claim is checkable rather
than asserted. It is the right shape and the app should consume it. It was not
adopted here, and the reason is worth recording.

`RosterStream` fetches `agent_status` for the runs a caller has named with
`watch(_:)` and for no others. The app's loop does something the stream cannot
express: `agent_list`, plus `agent_status` for the focused Bot, plus
`agent_status` for **one other Bot, round-robin**. The rotation is not
decoration: `testPollLoopRefreshesTheRosterFromOneTask` hires a Bot, never
opens its thread, and waits for its output to appear. Under a watch-set-only
stream nothing is watched, no `agent_status` is ever issued, and that test hangs
until it fails.

So the two sides genuinely disagree, and the usual resolution (keep the SDK's,
delete the sample's) would have traded a live-tested coverage property for a
tidier dependency graph. The sample keeps its loop. It is not duplicated
transport: it sits on the SDK's client and issues SDK calls; what it owns is the
*schedule*, which is app policy.

**What the SDK should do.** Give `RosterStream` a rotation (a bounded number of
unwatched runs refreshed per tick, cycling) so "the open thread is live and
every other Bot is eventually fresh" becomes expressible. Until it is, a roster
UI that wants that guarantee has to write the loop itself, which is the friction
§25 was trying to remove.

# Part 8: rebuilding the desktop shell

## 62. A fixture is not a default, and a default roster is a lie

`BotStore.init` took `identities roster: [Bot] = Fixtures.bots`. It had been
written that way so that a store with no Space attached was still a usable data
source, which is a real property and is tested. The cost was that the *running*
app opened onto nine coworkers the user had never created, each with a
plausible last message, none of which had a conversation behind it. The user's
verdict ("a bunch of fake threads in my DMs") is exactly right, and the
severity is worth naming: this was not a cosmetic wrong default, it was the app
asserting facts about the user's work that were not true.

The default was easy to write because the fixtures and the live roster have the
same type. `RUBRIC.md` grades thirteen renders built from `Fixtures`, so
`Fixtures` is load-bearing and has to exist; a default parameter then makes it
one keystroke from becoming production data. The seam that was supposed to keep
them apart (`BotDataSource`, with `FixtureDataSource` on one side and
`BotStore` on the other) did keep the *views* honest and did nothing at all
about this, because the leak was inside the live implementation rather than
across the seam.

**The rule.** Demo data may be reachable from a test or an export path and must
not be reachable from a default argument on a production type. The empty case is
not an edge case to be papered over with samples; it is the first thing a new
user sees, and if it has to be filled with something to look acceptable, the
empty state is the thing that is wrong.

## 63. "Not hired yet" was a label for a state that should not exist

Two roster rows in nine had no agent thread, so the chrome that renders presence
needed something to say, and `BotPresence.label` said `Not hired`. The composer
went further: `Not hired yet: no agent thread to message.`

The instinct on being told this is bad UX is to rename it. That is the wrong
repair, and it is worth being precise about why. The roster model needs no
status, started or provisioned field at all, only `isHidden`, if creation
starts the agent first and only then refreshes the roster. A row cannot
precede an agent, so no word is needed for a row that has no agent.

So the fix was an **ordering** change, not a string change:
`BotStore.createConversation` starts the run first and registers the identity
only once that returns. A failed start leaves no row. The unreachable state kept
its internal representation (`BotPresence.unstarted`, `hasThread`) because the
store genuinely has a moment between minting an identity and the `agent_start`
resolving, but nothing renders it, and a test now asserts that no user-visible
label or hint in the shell's vocabulary contains the word.

**The rule.** When a label is bad, check whether the *state* it names should be
reachable. A state that only exists because of the order in which you did two
things is a scheduling bug wearing a vocabulary problem.

## 64. Adopting every run in the Space is a defensible rule that produces the same defect

`apply(roster:)` gave a row to every run `agent_list` returned, including ones
this app did not start, on the stated reasoning that "a run doing work in the
user's Space that the UI cannot see is worse than an ugly row". Against the live
demo Space, which has thirty-odd runs in it, that reasoning produces a sidebar
full of threads the user never opened: the same symptom as the fixture roster,
from the opposite direction, and one the fixture fix alone would not have cured.

The reasoning is not wrong; it is answering a different question. "Is there work
happening I cannot see?" is an operator's question. The sidebar answers "what
conversations have I had?". Adoption is now off by default and behind a named
flag, and the test asserts **both** settings so that turning it back on is a
decision rather than a regression.

## 65. A screenshot of the running app found what the tests could not

The suite was green and the exported PNGs were byte-identical when I
photographed the app for the second time and saw the previous launch's
conversation in the sidebar named `claude-code`.

The Space stores *runs*. A run has an agent, a state and a prompt; it has no
name, colour or shape. This app joins a run back to a Bot by a marker in the
prompt, which recovers the **id** and nothing else, so on restart every Bot the
user had named fell back to `improvisedBot` and came back wearing its agent's
name. No test could have caught it: every test constructs its own store and
never outlives it, and the defect only exists across two processes.

Fixing it introduced the mirror-image risk immediately. The first implementation
restored the saved `order` as well as the saved identities, and a Bot whose run
no longer existed got a row again: the fixture-roster defect rebuilt from a
different file. So the roster's membership test is now explicitly "is this id in
`order`", and `order` is appended to only from `agent_list`; the saved file
supplies names and sort position and can never add a row. There is a test for
that specifically, because the safe version and the broken version differ by one
line.

**The rule.** Persisting presentation state next to derived state invites the
persisted copy to start deciding what exists. Keep the question "does this
exist?" answerable from exactly one source, and let the file answer only "what
is it called?".

## 66. Shipping the model's words as literals

A choice card reads like `What can I take off your plate?` with four lettered
options. None of that is app copy: it is model output, arriving per
conversation.

Putting it in the source would have produced an app that is *more*
screenshot-stable and unusable: every Bot asking the same four questions
forever, about nothing. The component is built; the content is carried
over the wire, with the agent emitting a small block that
`AgentOutputParser.choiceCard` turns into a card. That wire format is this app's
invention and is marked as such.

Generalised: **a string belongs in the source only if it is chrome.**
If it is something the model said, it is data, and hard-coding it converts a
live surface into a diorama.

## 67. `frame(maxWidth:)` is "flexible up to", not "at most"

The transcript bubble is capped at the smallest of 88% of the column, 640pt,
and the column minus 82pt, so the obvious spelling is `.frame(maxWidth: cap)` on
the bubble inside an `HStack` with a `Spacer` opposite it. That renders every
two-word message as a 607pt slab.

Inside a stack, `frame(maxWidth:)` makes a view *flexible* up to that width, so
it competes with the spacer for the slack and wins. The cap has to be expressed
instead as the gutter the opposing `Spacer` reserves,
`Spacer(minLength: column − cap)`, which leaves the text inflexible, taking its
ideal width and wrapping only when it genuinely runs out of room. Both spellings
satisfy the same cap on paper. Only one of them is shrink-to-fit, and
the difference is visible only in a photograph of the running app.

## 68. A control that unmounts is not a control that changes its icon

When the details pane opens, the header's monitor button looks as though it
turns into a `»`. In this design it does not: that control unmounts once the
pane is up, and the pane's own header mounts a separate close button, a double
chevron whose accessibility label and tooltip are both `Close details`, in
roughly the same place.

Built as one control toggling an icon, the pixels look the same and two things
come out wrong: the tab order, because a control that persists keeps
its position in it, and the ownership, because the close button belongs to the
pane and should disappear with it rather than surviving in the header. Modelling
it as "what is mounted in each of two places, as a function of one state value"
made both correct and made the whole swap testable without a window server,
which is how the gear's second swap (to a highlighted back `‹`, with the close
button staying put) got verified as well.

## 69. (Retired entry)

This entry no longer applies to the current code. The radius scale it
concerned is now a single table, `Corner`, pinned by a test.

## 70. Deriving a colour from a position makes it a different colour tomorrow

New Bots were coloured by rotating a palette on the roster index. The fix
hashes the agent id with FNV-1a and runs the result through a fixed PRNG, so a
Bot is the same colour on every machine, in every session, with nothing stored.

The palette rotation is unstable in a way that shows: a Bot created third is the
third colour, and after any restart that reorders the roster it is a different
colour. The user does not know why their Bot changed colour, and there is
nothing to tell them. Identity a user is
expected to recognise at a glance has to be a function of identity, not of
position in a list.

The code is small and the arithmetic is the trap: the multiply is a wrapping
32-bit multiply and every shift is unsigned, so every step has to be `UInt32`
with `&*` and `&+`. Getting it wrong does not crash; it silently yields
different colours, which is exactly the failure the hash existed to prevent.

## 71. Estimating a size when the design value is written down

The sidebar was first built at "~208pt", an estimate made by eye. The design
value is 280 by default, draggable between 240 and 400, with an 88 rail when
collapsed. There is no 208 anywhere in it.

An estimate is fine when nothing better exists, and it stops being fine the
moment the design value is written down. The failure mode is quiet: a
plausible number, arrived at honestly, that nothing later contradicts because
everything downstream was built to agree with it. The three graded desktop PNGs
in this repo keep their fixed-canvas geometry; the live shell takes its
geometry from the design values, and the two sets of numbers live side by side in `DT`
and `Metrics` rather than one quietly overwriting the other.

# Part 11: parsing the terminal the harness actually publishes

## 92. A repainting TUI has no transcript, and concatenation invents one

§19 and §20 said the agent's output is one undifferentiated text stream and
every app would therefore write its own parser. Both are true, and both
understate the problem, because the stream is not even the text. One Claude
Code spinner tick is:

```
ESC[?25l ESC[H CR ESC[28B ✻ ESC[34;1H ESC[32;3H ESC[?25h
```

Hide the cursor, home, move down 28 rows, stamp one glyph, park the cursor,
show it again. The CLI addresses cells; it does not append lines. Concatenating
what arrives on the pty gives a wall of glyphs in an order nobody ever saw, and
the first thing that wall does is look plausible: it contains all the right
words. Every app that writes "its own parser" for `output_tail` is writing a
parser for that wall.

The layout is worse than repaints alone. Claude Code emits absolute column
moves *between individual words*: `Enter ESC[8G to ESC[11G confirm`. A
substring search for `"Enter to confirm"` against the byte stream finds nothing,
which is how the recorder's first `expect` step failed against a dialog that was
plainly on the screen. There is no cheap approximation of a terminal here. The
only thing that yields what a human saw is a real VT emulator with a cell grid,
and once you have one the rest of the work is small.

## 93. "Observed" and "inferred" have to be different types, not different care

The binding rule is that the SDK must never mint a structured element the agent
did not offer. Stating it is easy; the discipline it actually demands only shows
up line by line.

`⏺ Update(calc.py)` is a reading: those characters were drawn, so
`toolCall(name: "Update", argumentSummary: "calc.py")` says nothing the agent
did not. A tool row with no result under it is *not* a reading of "running";
it is a reading of an absence, and "still running" is a conclusion. A folded
`Read 1 file` row is worse again: the CLI never wrote a tool name there, "Read"
is a past participle in a status sentence, and turning it into
`ToolCall.name == "Read"` is entirely the parser's idea.

Three different honesty levels in three adjacent rows means one flag per element
is not enough. `ToolCall` carries `provenance` for the element and
`statusProvenance` for the status separately, because the name can be observed
while the status is concluded, and collapsing them would either overstate the
status or understate the name. The same split covers diffs: 2.1.x draws a
literal `+` between the line number and the source, so an addition is
`observed`; when only the background colour distinguishes it, the same
classification is `inferred`, and a consumer can decide whether a
colour-derived diff is good enough to render as a diff.

The rule also has to survive being right by accident. A test that checks the
parser's output against a golden proves it agrees with itself. A test that takes
every `observed` element and asserts its strings appear in the frame it came
from proves the stronger thing, and that is the test worth having.

## 94. Recording a CLI means impersonating a terminal, not just owning a pty

`pty.fork` and `execvpe` get a shell recorded in twenty lines. Claude Code
exited immediately, cleanly, with the UI torn down: no error, nothing to read.

Two causes, neither visible from the output. The first: a bare pty has nothing
behind it, so nothing answers the device-attribute, kitty-keyboard and
XTVERSION probes a modern TUI sends on startup. The recorder now answers them
itself, minimally and honestly, and records each reply as an `i` event so the
cast shows exactly what the CLI was told. The second: the recording was driven
from inside a Claude Code session, and `CLAUDECODE=1` was still in the child's
environment, so the child concluded it was nested and quit. Stripping `CLAUDE*`
fixed it and also kept `CLAUDE_CODE_MESSAGING_TOKEN` (a live credential) out
of a process whose output gets committed.

Both failures presented identically: a valid cast, ten events long, ending in a
tidy teardown. A recorder that cannot tell "the CLI declined to run" from "the
CLI ran and did nothing" produces fixtures that look fine and describe nothing,
which is why every cast now carries `cua_notes` and `cua_exit_code` in its
header rather than only the bytes.

## 95. The cast has to name the version, because the layout is the contract

Claude Code 2.1.278 draws the composer as two horizontal rules with a `❯` row
between them. Earlier builds drew a rounded box. It says `⏸ manual mode on` and
`⏵⏵ auto mode on`, neither of which is a wording any prior note predicted. And
when a turn is interrupted with Esc it does not mark the turn; it *removes*
it and restores the prompt to the composer.

That last one is the useful one. The obvious thing to do with an interrupted
turn is emit `turnBoundary(.interrupted)`, and the rule to do it was already
written. It fires on `[Request interrupted by user]`, which this build never
draws. Emitting it anyway, on the grounds that an interrupt happened and the
transcript changed, would have been a structured element minted from an
inference about an absence: the exact defect, arrived at through helpfulness.
The rule stays, covered by a synthetic test, unfired against this build, and
the golden for the interrupted recording pins what the CLI actually shows:
a restored composer and no boundary at all.

So every cast records the CLI's own `--version` verbatim, and it travels into
`ParsedFrame.cliVersion` and the JSON document's `source.cliVersion`. A parse
result that cannot name the build it parsed is not evidence, and
`layoutProfile: null` (nothing matched, here is the text) is a supported
outcome rather than an error.

# Part 12: a capability declared from a comment, and a primitive the client threw away

## 96. The SDK refused a call the server would have served

`ProviderCapabilities.teleport` was `false` for Local, with the note
"teleport_app is Fleet-only". It is not. `teleport_app` in `spaces_mcp.py`
opens by routing Local Spaces to `_local_teleport_app`, which pushes to the
Space's own in-guest rcdp-handoff receiver on :8700 (the same protocol Fleet
uses, aimed at the VM instead of the gateway). The user teleports Chrome into a
Local Space routinely, and the Space this suite runs against has a mode-600
`.credentials.json` sitting in it that arrived that way.

The wrong belief came from a **real** error message on `_sandbox()`, a Fleet
helper that says "agent-CLI orchestration and app teleport are Fleet-only for
now". That message is unreachable for teleport: `teleport_app` returns on the
line above it. It was true once, the Local path was added, the short-circuit
was added above it, and the message stayed: accurate-looking prose on a branch
nothing takes any more. Every other clause in it had gone stale the same way:
all seven of its callers short-circuit Local, and the agent tools never reach
it at all, so the only genuinely Fleet-only things left are provisioning and
the hotspot.

An SDK that declares capabilities is strictly better than one that discovers
them by calling a tool and reading the prose it fails with (§6), but only if
the declaration is checked against behaviour. This one was transcribed from a
comment, and the comment was documentation of a code path, not of the product.
The declaration then made the bug *worse* than having no capability model at
all: without it the call would have gone to the server and worked, and instead
the SDK refused locally, on its own authority, with a confident message.

A capability flag is a claim about the server. It needs the same evidence a
test needs: the call, made, against the thing it describes.

## 97. The protocol shipped presence; the client decoded it to `.unsupported`

Multi-participant cursor presence (join, per-participant identity and colour,
~30 Hz cursor broadcast, removal on disconnect) has been in `rcdp-protocol`
since v1. `join`, `cursor`, `joined`, `presence`, `remote_cursor`, a
`PresenceHub` in the daemon, a synthetic `host` participant for the person at
the physical machine, and a reserved `cua-agent` identity so the agent's own
pointer is a participant rather than a mystery. None of it needed designing.

`CuaSpacesStreaming` modelled a single local cursor overlay and let every one
of those messages fall through its `decode` switch to `.unsupported`, which is
the *correct* forward-compatibility rule applied to messages that are not from
the future. So the SDK's window onto the protocol was narrower than the
protocol, silently, and an app building on the SDK could not reach a primitive
the daemon was already broadcasting at it. The shipping viewer had the same
gap in the other direction: it renders `remote_cursor` but ignores `presence`
entirely, so it has no roster, no departure handling and no sweeper: a peer
whose socket dies without a trailing `visible: false` leaves a frozen arrow on
the canvas until the session closes.

Two things made that easy to miss. Presence is *daemon*-scoped while everything
else the client does is session-scoped, so it does not appear anywhere near the
session lifecycle a reader follows. And `docs/protocol-v1.md` does not mention
presence at all: the feature is in the types and the tests, and the document a
client author reads says nothing.

The general shape: a forward-compatibility catch-all cannot distinguish "newer
than me" from "older than me and never implemented". `.unsupported` should be
countable, and a client that has been dropping the same known message type for
a year should be able to say so.

Departure is worth stating on its own. The daemon has **no leave message**: a
participant goes away by its socket closing, which the daemon reports as a
shorter `presence` roster. A client that waits for a leave waits forever. The
diff belongs in one place, which is why `PresenceRoster` does it and publishes
`.participantLeft` rather than handing every consumer a snapshot and the same
bug to write.

# Part 13: rendering what the agent actually wrote, and deleting what was never wired

These three parser defects were found in the app's copy of the tail parser and
are described below in those terms: `body(for:)`, `paragraphs`, the card rule.
By the time they landed, that parser had moved into `CuaSpaces.AgentOutputParser`
(§19/§20: every app on Spaces otherwise writes it), and the app is a mapper from
`AgentOutputSegment` onto its own rows. **The fixes went into the SDK**, which is
the only place they are worth having: a defect one layer down is a defect every
consumer inherits, and fixing it in one app leaves the next one to find it again.
The app-level tests below still hold, because they exercise the whole path.

## 98. A blank line is a paragraph break until it is inside a fence

The tail parser split an agent's output on blank lines, and every blank-line
separated chunk became its own bubble. That is defensible for a chat transcript
and wrong for an agent, because an agent's reply is a *document*: a heading, its
paragraph, a code block and a list are blank-line separated by construction, so
one answer arrived as six bubbles with a code fence stranded across two of them.

Two separate fixes, and the order matters. The splitter had to learn about
fences first: inside a fence a blank line is content, and no amount of
downstream markdown parsing can put back a block that was already torn in half.
Then consecutive prose had to be joined back together, while a card, a link row
or a computer-status line still breaks the run, because those genuinely are
separate things rather than another paragraph of the same answer.

The general shape: a normalisation applied *before* the thing that needs the
structure destroys information that cannot be recovered afterwards. Splitting is
lossy and it happens early.

## 99. The normalisation that ate the indentation

`body(for:)` trimmed every line of a paragraph and dropped the empty ones, then
rejoined them. That existed for the card heuristics, which want a tidy list of
non-empty lines to match `Title:` shapes against, and it was also what built
the prose that got rendered.

So every leading space in every agent reply was deleted. In a code block that
means the code no longer compiles; in a nested list it means a sub-bullet
renders as a sibling of the bullet it belongs to. Both are invisible in a
one-line test case, which is why this survived a suite that was otherwise
detailed about the parser: the fixtures were all flat.

The fix is to let the heuristics keep their normalised view and return the
user-visible text verbatim. A normalised copy for matching and the original for
display is the right shape; reusing the matching copy as the display copy is the
bug, and it looks like a harmless `let` at the top of a function.

## 100. A spinner needs an exit for every way out, not a timer

The typing indicator existed, was correct, and never appeared on send. It was
gated on `typing`, which means "this Bot owes its opening greeting" and is
cleared as soon as the run's tail is non-empty. On a send the tail is *already*
non-empty, so that test could never fire: a flag doing an adjacent job, which
reads as if it should work.

The replacement stores the tail length at the moment the user spoke, so "the
reply has begun" is a fact about the tail rather than an elapsed-time guess. The
part worth writing down is the exits. A send stops pending on the first token,
on the turn ending, on refusal, on a transport throw, on a failed status probe,
and on an explicit stop: six, and the last two are the ones that would have
shipped broken. A failed probe is the only thing that could ever take the
indicator down, so "unknown" has to clear it; read as "still working" it spins
forever on a Space that went away. And a refusal is an *exit*, not a pause:
there is no outbox (§24), so a refused message is never going to be answered and
a bubble left waiting promises a reply that cannot arrive.

There is one test per exit, and they are the reason this is worth more than the
feature: a stuck spinner is worse than no spinner, because it is a claim.

## 101. The typing-indicator timing is a choice

The typing indicator has no timing spec: no per-dot delay, no opacity cycle.
So the timing here is chosen: a 2.6 rad/s sine, 0.32 rad of stagger per dot,
~2.4pt of travel. `RUBRIC.md` keeps it out of the export renders.

## 102. Deleting a surface is mostly finding what was leaning on it

`Mobile/` looked self-contained. It was not: `CircleIcon`, `TypingIndicator`,
`ReactionChip`, `FileGlyph`, the `Optional*` layout modifiers, the fixture
remote screen, and a `refusalPrefix` constant the *store* wrote with were all
defined there and used by the desktop. A straight `rm -rf` breaks the build in
places that have nothing to do with phones.

The useful move was to grep for every symbol the directory defined *before*
deleting it, and sort them into "phone-only" and "lives here by accident". The
second list moved to `Design/SharedComponents.swift` and the model; only then
was the delete safe. The same applied to the approval card, whose one genuinely
important fact (`enforcementIsImplemented = false`, the claim that this gate
gates nothing) was a static on a *view* in the deleted directory. It belongs on
the model and now is, because the desktop still draws approvals and the honesty
has to outlive the view that happened to host it.

## 103. `sed -i` with `\b` on macOS silently does nothing

A tree-wide symbol rename was written as one `sed -i '' -e 's/\bOLD\b/NEW/g' …`
pass. It reported success, touched every file, and changed nothing: BSD `sed`
has no `\b`, so the pattern matched literally and never fired. The rename
"worked" and a follow-up grep showed the old symbols still there.

What made it safe was checking rather than trusting the exit status: the
command's success said nothing about whether it had done the job. `perl -pi -e`
has `\b` and did it correctly. A find-and-replace that silently no-ops is
particularly bad in a rename, because the half-renamed state usually still
compiles.

## 104. A control with no action is a lie the compiler cannot catch

Six account-menu rows, a share button, two composer-menu rows, a microphone and
a voice button: eleven affordances, all drawn, all tooltipped, none wired. They
type-check, they lay out, they highlight on hover, and they look exactly like
working controls. That is what makes this hard to see: the surface is correct,
and the thing missing is behind it.

`tapping(.share)` returning `self` is the clearest single artefact: a function
whose whole body says "this does nothing", sitting in a switch beside four cases
that do something. Drawing a control's appearance is not building the
control, and a UI built screen-first accumulates these faster than anything
else, because appearance is what it builds first.
