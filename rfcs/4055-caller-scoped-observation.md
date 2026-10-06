---
title: Caller-scoped observation and action settlement options for the macOS driver
authors:
  - evan-gabrielson-glean
created: 2026-09-23
last_updated: 2026-09-23
status: review
discussion: https://github.com/trycua/cua/issues/4055
rfc_pr: https://github.com/trycua/cua/pull/4056
implementation:
  - https://github.com/evan-gabrielson-glean/cua/tree/prototype/composable-targeted-reads
supersedes:
superseded_by:
---

# RFC: Caller-scoped observation and action settlement options for the macOS driver

## Summary

We should let callers ask the macOS driver to do less work on each call: narrower `get_window_state` reads (menu bar off, subtree, visible-only, matching elements, single-element re-read, changed-since, projected fields, one rendering format), a driver-side bounded wait, a running-only `list_apps`, an opt-out for the fixed post-action window-change poll, and an opt-in whole-string AX text insertion. Every option is additive and off by default, so existing callers see no change. A prototype on current `main` shows up to ~800× lower latency or ~80× smaller responses per call, depending on the option. A 197-character fill-and-confirm flow drops from ~12.5 s and ~1.7 MB of tool output to ~0.4 s and ~62 KB.

## Motivation

An agent loop pays for every call twice: in driver time and in the tokens the model must read. On `main` several of those costs are fixed, whatever the caller needs, so a caller cannot trade scope for speed even when it knows exactly what it wants to check.

Related work: #1757 (batched attribute reads), #3787 and #3906 (walking only visible/selected rows of long lists), and #3963 (script-speed computer use: fewer observations, capture only the needed modality, replace fixed sleeps). This RFC proposes the driver-side primitives that make several #3963 items possible without moving agent policy into the driver.

## Goals

- Let a caller choose the scope, content, and format of an observation, per call.
- Let a caller skip fixed waits it does not need, and wait on a condition in one bounded call instead of client polling.
- Keep full reads available: any narrowed read can be escalated to today's default call.
- Keep every default exactly as it is today.

## Non-goals

- Changing defaults, or deciding when an agent should use a narrowed read. That is caller policy.
- Replacing verification. Narrow reads make verification cheaper; they do not make it optional.
- Batching several tools in one MCP call. The measured local round trip is ~1.4 ms median, so it would save little.
- Windows and Linux implementations in the first increment (see Compatibility).

## Terminology

- **Full read**: today's default `get_window_state` call: the whole window plus the menu bar, rendered as both `tree_markdown` and `elements`.
- **Narrowed read**: a `get_window_state` call that uses one or more of the new scope, shaping, or no-walk options.
- **Post-action observation**: the ~1 s window-change poll that action tools run before returning.
- **Settle gap**: the time between a successful AX write and the moment the application reports the new value through AX.

## Current state

- `list_apps` always scans installed bundles as well as running processes: ~1.6 s per call on a typical Mac. A caller that only needs running PIDs cannot opt out.
- Every action tool (`click`, `set_value`, `press_key`, `hotkey`, `type_text`) polls for window changes for ~1 s after the action. A `click` returns in ~1,040 ms with the poll and ~14 ms without it.
- `type_text` in the default route sends one key event per character: ~3.3 s for 50 characters and ~23 s for 500 on a Chromium `textarea` in our runs.
- `get_window_state` always walks the whole window plus the application menu bar and returns both `tree_markdown` and `elements`. With a document open in Preview, the menu bar was 309 of 320 elements (~30 KB of a ~31 KB response) in our runs. To check one field after an action, a caller must re-walk and re-read the whole window.
- The walker reads each node's attributes one IPC round trip at a time (#1757 already proposes batching).
- To wait for a control to appear, a caller must poll with full snapshots from the client.

## Proposal

All additions are optional arguments with today's behavior as the default.

`list_apps`

- `scope`: `"all"` (default) | `"running"` | `"installed"`. `"running"` returns only live processes and skips the bundle scan.

Action tools (`click`, `set_value`, `press_key`, `hotkey`, `type_text`)

- `wait_for_window_changes`: default `true`. `false` returns right after the action with no window-change report.

`type_text`

- `text_insertion`: `"auto"` (default) | `"ax"`. `"ax"` focuses the target and writes the whole string through `AXSelectedText` / `AXValue`, with no per-character fallback. When an empty field reports its placeholder as `AXValue`, the placeholder is not treated as existing text. The result stays "unverifiable" unless a fresh read confirms it.

`get_window_state`: walk scope, applied during the walk (not as a post-filter)

- `include_menu_bar` (default `true`)
- `root`: an `element_token` from a prior snapshot of this window; walk only that subtree
- `exclude_roles`: roles whose subtrees are skipped
- `visible_only` (default `false`): prune subtrees whose frame lies outside the window bounds

`get_window_state`: result shaping

- `find`: `{role?, label?, value?, limit?}`; return only matching actionable elements (tokens stay valid)
- `fields`: project each element to these keys (`element_index`, `role`, `depth` are always kept)
- `format`: `"both"` (default) | `"elements"` | `"markdown"`
- `since`: a prior `snapshot_id` for this window; return only added/removed/changed elements plus an unchanged count

`get_window_state`: modes that skip the walk

- `element`: an `element_token`; re-read only that element (role, label, value, enabled, focused, frame)
- `at`: `{x, y, scale?}` window-local screenshot pixel; hit-test and return the nearest actionable ancestor
- `crop`: `{x, y, w, h}`; crop the returned screenshot

`get_window_state`: bounded wait

- `wait_for`: `{timeout_ms?, interval_ms?, gone?}` with `find`; poll in the driver until `find` matches (or, with `gone`, until it matches nothing). The prototype defaults to 5 s, caps at 20 s, and uses a 10 ms minimum interval.

Internally, per-node attribute reads use `AXUIElementCopyMultipleAttributeValues` and fall back to single reads when the batch call fails. This part has no contract change and could land through #1757.

## Alternatives considered

- **New tools** (`find_elements`, `get_element`, `element_at`, `wait_for`) instead of `get_window_state` options. They are clearer as tools, but each needs its own authorization and risk classification, manifest entries, and cross-platform parity. Options on an existing read-only tool keep that surface unchanged. We are open to either shape.
- **Role-based subtree roots** (for example, "the largest `AXWebArea`"). We tried this and rejected it: one Electron app has several nested web areas, and the heuristic dropped the composer. Token-rooted scope is explicit and reliable.
- **Changing defaults** (menu bar off, no post-action poll). This is faster for everyone but can break existing menu flows and change-reporting callers. We prefer opt-in first, with default changes considered separately once there is data.
- **A multi-call batch tool.** This is not worth it at a ~1.4 ms round trip (see Non-goals).

## Compatibility and migration

- Additive only. Omitting every new argument gives byte-identical behavior. We compared batched and unbatched walks on the same windows (Finder, Preview, a local Chromium page, and three Electron chat apps); `tree_markdown` matched in every case except a live timer label.
- Tool schemas use `additionalProperties: false`. Until Windows and Linux accept the same keys, callers that send them there will get a validation error. Options: (a) land macOS first and document per-platform support, (b) accept and ignore unsupported keys on other platforms with a capability flag, or (c) land all three together. We suggest (b), and would like maintainers to decide.
- Rollback: remove or ignore the new arguments. No persisted state or config changes. The `since` baseline is per-process memory, keyed by (pid, window_id).

## Security, privacy, and telemetry

- No new permission is required. Every mode uses the same Accessibility and Screen Recording APIs and the same pid/window targeting as today's call.
- Narrowed reads return a subset of what the full read already returns. `at` hit-tests only inside the target pid.
- `text_insertion: "ax"` writes through the target element instead of synthesized key events, so no keystrokes can reach another focused window. It still needs a fresh read to confirm the text landed.
- `wait_for` is bounded (20 s cap) and ends early on a match.
- No telemetry changes. The prototype adds `timings_ms` to `get_window_state` results only for local measurement.

## Implementation plan

Each increment is independently reviewable, additive, and can be reverted on its own. The prototype branch has one commit per increment.

1. **Batched per-node attribute reads.** Internal only; no contract change. Coordinate with #1757 instead of landing twice. Gate: identical `tree_markdown` for batched and unbatched walks on the harness fixtures, and no regression in the macOS harness.
2. **`list_apps` `scope`.** Gate: `all` output unchanged; `running` matches the running subset of `all`.
3. **`wait_for_window_changes` on action tools.** Gate: default results unchanged; `false` returns no change report and skips the poll.
4. **`get_window_state` walk scope** (`include_menu_bar`, `root`, `exclude_roles`, `visible_only`). Gate: defaults unchanged; each option's result is a subset of the full read.
5. **`get_window_state` shaping and no-walk modes** (`find`, `fields`, `format`, `element`, `at`, `since`, `crop`). Gate: each result is derivable from a full read taken at the same moment.
6. **`wait_for`.** Gate: bounded by its cap and ends early on a match or `gone`.
7. **`type_text` `text_insertion:"ax"`.** Gate: harness rows show the text landed in fixture state, including the settle gap, for AppKit, SwiftUI, and web fixtures.
8. **Windows and Linux parity**, or the chosen capability mechanism (see Compatibility).

## Test and acceptance plan

Prototype: https://github.com/evan-gabrielson-glean/cua/tree/prototype/composable-targeted-reads (4 commits on `main` 37212d2, one per increment). `cargo test -p platform-macos --lib` passes: 384 passed, 3 ignored. `protocol_schema_test`, `compatibility_contract_test`, `capture_contract_test` and `cross_platform_behavior_test` also pass. We have not run the canonical macOS E2E harness yet.

Measured on macOS 26 (arm64), 0.28.2 release vs the prototype, medians of 5 calls. Only stock apps and a local test page were used:

| Option                                                       | Before                         | After                  |
| ------------------------------------------------------------ | ------------------------------ | ---------------------- |
| `list_apps` `scope:"running"`                                | 1,611 ms, 19 KB                | 2 ms, 3 KB             |
| `click` `wait_for_window_changes:false`                      | 1,039 ms                       | 14 ms                  |
| batched reads, Finder list view (145 el)                     | 921 ms                         | 340 ms                 |
| batched reads, local Chromium page (421 el)                  | 228 ms                         | 145 ms                 |
| `since` after one change (421 el)                            | 115 KB                         | 1.4 KB                 |
| `include_menu_bar:false`, local Chromium page                | 194 ms, 572 el                 | 139 ms, 422 el         |
| `find` one `textarea` vs full read, local page               | 50 KB text / 182 KB structured | 0.3 KB / 1.2 KB        |
| `element` re-read vs full read, local page                   | 194 ms                         | 4 ms                   |
| `visible_only`, Finder list view (/System/Applications)      | 613 ms, 145 el                 | 90 ms, 89 el           |
| `wait_for` vs client polling (control appears after ~800 ms) | 4 calls                        | 1 call, same wall time |
| `type_text` 500 chars, `text_insertion:"ax"`                 | ~23 s                          | ~15 ms                 |

End to end on the local page (fill a 197-character `textarea`, click a toggle, confirm): 0.28.2 defaults took 12.4–12.7 s and returned ~1.7 MB across 5 calls. The composed options took 0.36–0.40 s and returned ~62 KB across 5 calls.

Known gaps found by the prototype:

- An `element` re-read immediately after an AX write was stale in 2 of 10 Chromium runs. It was correct after 50 ms and in every fresh walk. Verification after `text_insertion:"ax"` therefore needs a short settle or a `wait_for` on the value.
- `at` returns the deepest element; the prototype climbs to the nearest actionable ancestor. It failed on a Finder inline field and depends on the point not being covered.
- `since` keys elements by role+label+depth, so a label change is reported as add+remove, not as a change.
- `visible_only` gave no gain on the Electron apps we tried, whose off-screen content is already virtualized.

Acceptance for implementation PRs: unit and schema tests for every argument; macOS harness rows showing unchanged defaults; E2E evidence that each opt-in path lands (fixture state, not tool success); and a documented per-platform support matrix.

## Unresolved questions

1. Options on `get_window_state`, or separate read-only tools?
2. Cross-platform strategy for new keys under `additionalProperties: false` (see Compatibility).
3. Should `find` support exact or regex matching, and should it return non-actionable text?
4. Should `since` use a stable identity key (for example, the `AXIdentifier` or DOM id when present) so edits report as changes?
5. Should `wait_for` also accept value predicates (for example, a text length), to cover the AX-write settle gap?
6. Should `timings_ms` become a stable result field?
7. Should any default change later (for example, `include_menu_bar:false`), and what evidence would justify it?

## Decision record

Pending maintainer review in #4055.
