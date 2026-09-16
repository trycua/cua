# Optional policy head (`suggest_action`)

Cua Driver is mechanical on purpose. It snapshots a window and dispatches
clicks and keystrokes; the caller decides what to do next. This document
describes an optional tool that offers a second path for that one
decision, what it deliberately will not do, and how to turn it on.

It is off unless you configure it. With no credential set, the tool is
never registered and `tools/list` is byte-identical to a build without
this feature.

## The problem it addresses

In a snapshot-decide-act loop, the act is fast and the snapshot is
bounded, but the decide is open-ended. A general-purpose model re-reads
the whole accessibility tree, reasons in prose, and emits one element
index. On a seven-step web flow measured with the same harness on both
sides, that produced:

| phase | share of wall clock |
| --- | --- |
| snapshot | under 1% |
| decide | 96% |
| act | 3.5% |

The decision is almost all of the time, and the thing being decided is
small: which of these visible elements is next. A model sized for that
question answers it in a couple hundred milliseconds.

## What the tool does

`suggest_action` takes a goal and a window. It snapshots the window
itself through the registry (no screenshot), projects the tree down to
named, enabled, interactive elements capped at 120, and asks a small
classifier for one typed answer.

```jsonc
// cua-driver call suggest_action '{ ... }'
{
  "goal": "open the Downloads folder",
  "pid": 4711,
  "window_id": 82,
  "deny": ["Move to Trash", "Eject"],
  "history": ["pressed Sidebar"]
}
```

```jsonc
{
  "element_index": 34,
  "element_token": "s0000001f:34",
  "role": "AXRow",
  "label": "Downloads",
  "kind": "press",
  "needs_text": false,
  "confidence": 0.94,
  "probabilities": { "34": 0.94, "31": 0.04, "__none__": 0.01 },
  "done": false, "done_p": 0.02,
  "blocked": false, "blocked_p": 0.05,
  "elements_total": 61, "elements_considered": 59, "elements_denied": 2,
  "truncated": false,
  "snapshot_ms": 310, "decide_ms": 188,
  "model": "jev-1.13.0", "input_tokens": 512
}
```

Pass `element_token` straight to `click` or `type_text`, act, then
re-snapshot. Element indices are replaced by the next `get_window_state`,
exactly as for any other element-indexed action.

## What it will not do

**It never generates text.** A text field comes back as `needs_text: true`
naming the field. What goes in it stays the caller's decision. A policy
head whose entire vocabulary is 120 element labels has no business
composing a message.

The `needs_text` test is the element's role, not whether the field looks
empty. The value-based test is a web-forms assumption that breaks on
native controls: a macOS `AXSearchField` reports its placeholder as its
`AXValue`, so an empty search box reads as filled and the caller is told
to press it instead of typing into it.

**It never acts.** The response names an element. The caller still calls
`click` or `type_text`. Nothing in this path dispatches input, which is
why the tool is annotated `read_only`.

**It is not a planner.** One bounded question, one window, one step.
Goals spanning several windows or applications stay yours to decompose.

## `deny` is enforced in code

Elements whose label matches a deny term, case-insensitively as a
substring, are removed from the projection before the model is asked, and
the returned pick is re-checked against the list afterwards.

The distinction from a prompt instruction is the point. A denied element
is not disfavoured in the distribution, it is absent from the question, so
no answer can name it. A model that ignores an instruction cannot produce
a denied pick, because the enforcement is not an instruction.

Name the irreversible controls on the screen. Deny terms are substrings,
so `"Send"` also covers `"Send Later"` and `"Resend"`.

## What the projection keeps

The tool describes named, enabled, interactive elements and nothing else.
Two consequences are worth knowing before you write a goal.

**Menus are excluded.** On macOS the accessibility walk of any window
reaches the whole application menu bar, which on a real machine is
hundreds of rows deep and includes the Recent Items list. Left in, those
rows crowd the window's own controls out of the element budget and send
local file names to the provider for nothing, and a menu row is not
actionable by element index anyway — `invoke_menu` takes a path. So this
tool cannot suggest a menu route. A goal only reachable through a menu
comes back as `__none__` with a low confidence, and the menu path stays
yours to choose.

Measured on one macOS host, excluding menu rows took Calculator from 143
named interactive elements to 24 and Font Book from 112 to 26, and cut
input tokens per Calculator step from 9,512 to 1,686.

**An unlabelled row borrows its children's text.** macOS puts a list
row's label in a child `AXStaticText` and leaves the `AXRow` itself
unnamed; Windows and AT-SPI do the same with `DataItem` and `table row`.
A strict named-only filter therefore drops every row in a sidebar, a
table, or a search-result list — which is to say it drops exactly what a
navigation goal needs. The projection lends a labelled child's text
upward, joined in tree order and capped at 80 characters, and the row
keeps its own `element_index` because the row is what you click.

## Reading the answer

`confidence` is calibrated over the labels that were offered. Below about
0.5 the head is choosing between options it cannot distinguish; read the
tree yourself. `probabilities` shows what the alternatives were, which is
usually more informative than the scalar.

`done` and `blocked` are independent readings, not branches of the choice.
A screen can have a sensible next element and also already satisfy the
goal, and you want to know both.

`truncated` reports whether the 120-element cap bit. When it did, the
answer is over a prefix of the window, not the window.

`elements_considered: 0` is an answer about the window, not a failure. On
macOS it usually means the accessibility tree came back empty, so take a
`get_window_state` screenshot and act by pixel.

## Enabling it

The bundled provider is TypeSafe's System One (Jev). Set the credential
before starting the daemon:

```sh
export TYPESAFE_API_KEY=...
cua-driver serve
```

| variable | meaning |
| --- | --- |
| `TYPESAFE_API_KEY` | Bearer credential. Unset means the tool is not registered. |
| `TYPESAFE_BASE_URL` | Endpoint override, for a proxy or self-hosted deployment. Defaults to `https://api.typesafe.ai/v1/systemone`. |
| `TYPESAFE_MODEL` | Model override, for pinning a published revision. Defaults to `jev-latest`. |

## Failure is never silent

The tool is an accelerator, not a dependency. Every provider failure
returns an MCP error naming the cause and telling the caller to decide the
step itself, with a stable `error` code in the structured payload:

| code | cause |
| --- | --- |
| `jev_not_configured` | no credential |
| `jev_unauthorized` | 401 or 403 |
| `jev_payment_required` | 402 |
| `jev_rate_limited` | 429 |
| `jev_upstream_unavailable` | 5xx, including 529 |
| `jev_rejected_request` | 422 and other 4xx |
| `jev_unreachable` | DNS, TLS, connect, or timeout |
| `jev_malformed_response` | 2xx that was not the documented envelope |

Nothing about the window changes on any of these paths. The snapshot has
already happened and no input was dispatched.

## What leaves the machine

A `suggest_action` call sends the goal, the caller-supplied history, the
window's pid, id and application name, and for each surviving element its
role, label, value when present, and selected state. No screenshot, no
frames, no element tokens, no full tree.

The window title is deliberately not sent. The caller already named the
exact window, so the title disambiguates nothing the elements do not
already carry, and it is the string in a window most likely to be a
document name, a message subject, or a chat name.

Element labels are user-visible text, so on an application holding
personal content they carry it. This is why the tool is classified `R3`,
the external-side-effect tier: there is no local side effect, but data
movement is the strongest thing it does. Treat it like any other egress
and point it at applications whose contents you are willing to send.
