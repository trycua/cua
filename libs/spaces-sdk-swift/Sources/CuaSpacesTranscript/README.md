# CuaSpacesTranscript

Terminal scrollback in, structured UI out.

`AgentTypes.swift` carries `output_tail`: the Spaces harness publishes what the
agent's terminal looks like, not what the agent did. Every proposal for
`agent.events` is blocked on that, and
[`samples/openkoalabot-example-swift/FRICTION.md`](../../../../samples/openkoalabot-example-swift/FRICTION.md) §19 and
§20 predicted the consequence: one undifferentiated text stream with no turn
boundary, so every app writes its own parser and they all guess differently.

This target is that parser, written once, with golden data recorded from the
real CLI so it is checkable rather than plausible.

## Four pieces

| Piece | Type | What it does |
| --- | --- | --- |
| Emulator | `TerminalEmulator` → `ScreenBuffer` | VT100/xterm subset. Claude Code repaints; concatenating the byte stream gives a wall of glyphs no human saw. |
| Playback | `CastPlayer` → `RenderedFrame` | The rendered screen at time *T* in an asciinema v2 recording. |
| Parser | `ClaudeCodeParser` → `ParsedFrame` | A frame becomes a typed model of what is on screen. |
| Encoding | `ParsedFrame.jsonUIDocument()` | That model becomes `cua.transcript.jsonui/1`, a document with no Swift in it. |

Stages three and four are deliberately separate: a change to the Swift model
that does not change the document is not a wire change, and a non-Swift client
gets the same structure a Swift one does.

## Without a recording

The `output_tail` case needs no cast:

```swift
let frame = RenderedFrame.render(stream: snapshot.outputTail ?? "",
                                 columns: 100, rows: 34,
                                 cli: "claude", cliVersion: knownVersion)
let document = ClaudeCodeParser().parse(frame: frame).jsonUIDocument()
```

Terminal size is the caller's to supply, because the tail does not carry it and
guessing it changes where every line wraps.

## The honesty rule

The rule is binding: **the SDK must never mint a structured element the agent
did not actually offer.** Every element carries a `Provenance`, and so does the
JSON:

* `observed`: the text is on the screen and this element is a direct reading
  of it. `⏺ Update(calc.py)` gives `toolCall(name: "Update")` because those
  characters were drawn.
* `inferred`: the parser concluded it from layout, colour or adjacency and
  the agent never said it. A tool call with no result row under it is
  `status: running, statusProvenance: inferred`, because "running" is our
  reading of an absence. A diff classified from a background colour rather
  than a drawn `+` is `inferred` too.
* `unrecognised`: the parser could not classify the region and is handing
  back the text unchanged. This is the required behaviour when the CLI's
  layout moves, not a failure.

`ToolCall` and `Subagent` split `statusProvenance` out from the element's own
`provenance` on purpose: the *name* can be read off the screen while the
*status* is concluded, and one flag for both would overstate one or understate
the other.

A renderer that draws a confident card for an `inferred` element is
misrepresenting the agent. `ParsedFrame.observed` and `.inferred` exist so that
is a choice rather than an accident.

## Versions

Claude Code's rendering changes between releases. Every cast records the CLI's
own `--version` output, and it travels into `ParsedFrame.cliVersion` and into
the JSON's `source.cliVersion`. `layoutProfile` names the layout that matched:
`claude-code/2.x`, or `null` when nothing did and the whole frame degraded to
text.

## Goldens

`Tests/CuaSpacesTranscriptTests/Goldens` holds real recordings and the expected
JSON-UI at named timestamps. `swift test --filter GoldenTests` replays, parses
and compares. A parser change that alters output must change a golden, and the
diff is visible in review.

To re-bless after an intentional change:

```
CUA_TRANSCRIPT_RECORD=1 swift test --filter GoldenTests
```

Read every changed file before committing it. Re-blessing without reading the
diff defeats the mechanism.

To re-record against a newer CLI, see [`tools/cli-cast/`](../../../../tools/cli-cast/README.md).
