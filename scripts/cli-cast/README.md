# cli-cast

Records a coding CLI running inside a real pseudo-terminal, as an asciinema v2
cast. The casts it produces are the golden *inputs* for
`libs/spaces-sdk-swift`'s `CuaSpacesTranscript` target, and are copied into
the cross-language fixtures in `libs/cua/spaces-contract/fixtures/transcript`.

## Why Python here and Swift there

The parser ships in the SDK because that is where consumers need it: an app
holding a `RunSnapshot.outputTail` has to turn it into structure in-process,
and `FRICTION.md` §19 is precisely that every app otherwise writes this itself.

The recorder does not ship. It is developer tooling that runs once per CLI
release, it needs `pty`, `termios` and `select`, and Python's standard library
has all three with no dependencies at all. Writing it in Swift would add a
platform-specific target to a shipping package in exchange for nothing a
consumer can use.

## Recording

```
python3 record.py --script scripts/claude-basic-turn.json \
                  --cwd /path/to/scratch-project \
                  --out ../../libs/spaces-sdk-swift/Tests/CuaSpacesTranscriptTests/Goldens/claude-basic-turn.cast
```

`./record-all.sh <scratch project>` does every script in `scripts/`.

**The project directory must be a scratch checkout.** Never point this at a
live Space, and never at a directory holding credentials: the recorder scrubs
known secret shapes, but the only reliable defence is not recording them.

## The step language

A script is `{"name", "description", "cmd", "steps"}`. Steps:

| Step | Effect |
| --- | --- |
| `{"wait": 2.5}` | let the CLI draw |
| `{"send": "text"}` | write bytes to the pty |
| `{"key": "enter"}` | `enter`, `esc`, `ctrl-c`, `ctrl-d`, `tab`, `shift-tab`, `up`, `down`, `backspace` |
| `{"expect": "text", "timeout": 30}` | wait for text to be drawn |

`expect` matches against the stream with escape sequences and whitespace
removed, because Claude Code interleaves absolute column moves *between
individual words* (`Enter\x1b[8Gto\x1b[11Gconfirm`) and a raw substring search
finds nothing a human would call present.

A timeout is recorded in the header's `cua_notes`, not raised. A stuck or
interrupted session is exactly the kind of golden this exists to capture.

## What the recorder has to do that a naive `pty.fork` does not

* **Answer terminal queries.** A bare pty has nothing behind it, so nobody
  replies to the device-attribute, kitty-keyboard and XTVERSION probes a modern
  TUI sends at startup. Claude Code exits. The recorder answers them minimally
  and honestly and records each reply as an `i` event.
* **Strip the parent harness from the environment.** Driving a recording from
  inside a Claude Code session leaves `CLAUDECODE=1` in the child's
  environment; the child decides it is nested and quits. `CLAUDE*` is removed,
  which also keeps `CLAUDE_CODE_MESSAGING_TOKEN`, a live credential, out of a
  process whose output gets committed.
* **Not split UTF-8 across events.** Read boundaries land wherever the kernel
  put them, regularly mid-sequence in the box-drawing characters every frame is
  made of. An incomplete trailing sequence is held back and prepended to the
  next chunk.

## Format

asciinema v2: a JSON header line, then one `[time, "o"|"i", data]` array per
chunk. It was chosen over anything bespoke because a golden input should stay
inspectable with `head` and `jq`, diff legibly in review, and be playable by
`asciinema play` so a human can check a fixture with their own eyes.

The header carries extensions the spec allows: `cua_cli`, `cua_cli_version`
(the CLI's own `--version`, verbatim), `cua_script`, `cua_exit_code`,
`cua_scrubbed` and `cua_notes`. **A recording that does not name the version
that drew it is not evidence about anything**: Claude Code's rendering changes
between releases.

## Scrubbing

`record.py` scrubs as it records, so a fresh cast is already clean. Rules cover
Anthropic and OpenAI-shaped keys, GitHub tokens, AWS access key ids, bearer
tokens, JWTs, the Claude Code resume session id, and `$HOME` (rewritten to
`/Users/operator`). Replacements keep the original shape where possible: a
different-width line wraps differently, and a golden whose wrapping differs
from the real session tests the scrubber rather than the CLI.

`scrub.py <casts>` re-applies the rules to casts already on disk, for when a
rule is added after a recording was taken.

## Inspecting a cast

```
swift run cast-render <cast> info            header, duration, CLI version
swift run cast-render <cast> times           every instant the screen changed
swift run cast-render <cast> frame <seconds> the rendered screen at T
swift run cast-render <cast> json  <seconds> the JSON-UI document at T
```
