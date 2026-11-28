# Session journal

Turns a Cua Driver trajectory recording into one self-contained HTML page that
reads like a comic journal: a cover with the session facts, a contents page and
one panel per tool call. Failed and refused calls, and input that was sent but
left the screen unchanged, are drawn in red so they can be found at a glance.

The tool only reads the recording format written by
`rust/crates/cua-driver-core/src/recording.rs`; it changes nothing in the
driver. It needs Node.js 20 or later and has no dependencies.

## Use

Record a session, then build the journal from the repository root:

```bash
cua-driver recording start ./my-session
# drive apps through Cua Driver
cua-driver recording stop
node libs/cua-driver/tools/session-journal/session-journal.mjs ./my-session
```

The page is written to `./my-session/journal.html`. Screenshots are embedded, so
the file opens offline and can be attached to a bug report on its own. Its
content security policy blocks all network access.

| Option              | Effect                                                       |
| ------------------- | ------------------------------------------------------------ |
| `-o, --out <file>`  | Write the page somewhere else.                               |
| `--title <text>`    | Cover title. Defaults to the recording folder name.          |
| `--show-typed-text` | Show typed text, values and scripts instead of their length. |
| `--include-desktop` | Show full-desktop screenshots.                               |
| `--link-images`     | Link screenshots by relative path instead of embedding them. |

## What a panel shows

Each `turn-NNNNN/` folder becomes one panel:

- the action in plain words: `Click at (160, 50)`, `Type 21 characters, hidden`,
  `Hotkey Ctrl+A`;
- a screenshot: `click.png` with the driver's crosshair for click turns, cropped
  around `click_point` when the capture is large; otherwise `after.png`, or
  `before.png` when the target window was gone afterwards;
- the delivery (`background`, `foreground`) and how the target was addressed:
  by element, by pixel, the focused control or the whole desktop;
- an outcome stamp from `action_truth.effect` (Confirmed, Partial, Unverified,
  Suspected no-op, Refused), or Failed and Done for tools that report only
  `result_error`;
- `No visible change` when `before.png` and `after.png` are byte-identical;
- the driver's summary, refusal code and escalation advice, with the arguments
  and the before and after screenshots under Details.

## Privacy defaults

Recordings are made on real desktops, so two things stay hidden unless asked
for:

- typed text, values and JavaScript (the `text`, `value` and `javascript`
  arguments), including where a result summary echoes them, and URL query
  strings;
- screenshots of turns without a target pid, which the recorder takes of the
  whole screen.

Screenshots of the target window are shown as recorded and can still show what
was typed. Turns that Cua Driver recorded as `privacy_suppressed` have no
screenshots to show.

## Tests

```bash
node --test libs/cua-driver/tools/session-journal/test/session-journal.test.js
```

The tests write a synthetic recording with generated PNGs, so the repository
holds no screenshots.

## Known gaps

- A recording keeps element tokens such as `s00000002:8`, not the element they
  resolved to, so element clicks show the token. If `action.json` recorded the
  resolved role and label, a panel could say `Click MenuItem "Format"`.
- `recording.mp4` is not used, and `cursor.jsonl` only sets the session length.
- Computer History (`cua-driver history list --json`) could feed a
  screenshot-free version of the same page.
