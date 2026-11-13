---
title: Passive native observations without action handles
authors:
  - injaneity
created: 2026-09-16
last_updated: 2026-11-12
status: accepted
discussion: https://github.com/trycua/cua/issues/3903
rfc_pr: https://github.com/trycua/cua/pull/3904
implementation:
  - https://github.com/trycua/cua/issues/2958
supersedes:
superseded_by:
---

# RFC: Passive native observations without action handles

## Summary

`get_window_state` gains an optional, typed `observations` list next to the
unchanged `elements`. An observation is a display-only row of the same native
walk (static text, a label, a read-only value) with role, text or value, and
frame, but no `element_index` or `element_token`. No action tool accepts one.
The shared `verify_state` searches `elements` first and `observations` after,
so display-only text such as Calculator's result stops being
`observation_unavailable`.

## Motivation

[#2958](https://github.com/trycua/cua/issues/2958): after `4 + 2 =` through
background accessibility clicks, Calculator shows `6`, and `tree_markdown`
carries `AXStaticText = "\u200e6"`, but `structuredContent.elements` holds
only the indexed rows. A caller that reads the typed surface cannot see the
result and has to parse Markdown or interpret a screenshot. Retrying the click
does not fix missing observation data and can change the application again.

## Goals

- Publish readable display-only state without inventing action authority.
- Let `verify_state` check a uniquely selected raw value in that state.
- Keep action handles, counts, cache isolation and trust provenance unchanged.
- Bound the list and keep it consistent with `query`.

## Non-goals

No new tool, observation cache, action retry, ordinal selector, implicit
bidi/number/whitespace normalization, or first-match semantics. A role-only
selector is not promised to tell Calculator's expression row from its result
row. Addressable completeness (#3817) and Linux traversal scope (#3814) are
separate work.

## Terminology

- **Element:** an indexed row in the action projection (`elements`).
- **Observation:** a readable native row with no action index or token.

## Current state

Before this change the macOS, Windows and Linux builders projected only
indexed rows into `elements`. Since #4531 the macOS internal observation read
behind `verify_state` already appended display-only rows marked
`display_only: true`, so verification on macOS could read them; the public
response could not, and Windows and Linux could do neither.

## Proposal

### Typed contract

`WindowElement` is unchanged. The generated contract adds:

```rust
struct WindowObservation {
    role: String,
    depth: u32,
    label: Option<String>,
    value: Option<String>,
    frame: Option<ElementFrame>,
    parent_index: Option<u64>,
    in_web_content: Option<bool>,
}

struct WindowStateOutput {
    // existing fields unchanged
    observations: Option<Vec<WindowObservation>>,
    observations_truncated: Option<bool>,
}
```

`parent_index` names the nearest addressable ancestor in `elements`. Values
keep their raw native text.

### Data flow

```diff
 get_window_state(exact_window)
   ↳ bounded native walk
     ↳ indexed rows → elements
-    ↳ display-only rows → markdown only
+    ↳ display-only rows → observations + markdown
 verify_state(exact_window, expect)
   ↳ observation-only window state
-    ↳ elements → search domain
+    ↳ elements, then observations (marked display_only) → search domain
     ↳ evaluate_predicates(expect, snapshot)
```

Both lists come from one walk. The shared projection lives in
`cua_driver_core::element_query::publish_observations`; each platform builder
supplies its own non-indexed rows:

- macOS AX: every unindexed node of the walk that carries text.
- Windows UIA: every unindexed node of the walk that carries text.
- Linux AT-SPI: the unindexed, showing, named rows the renderer already wrote
  to Markdown, only when the walk is native and proven window-scoped. The X11
  property fallback observes none and leaves the field out.

A platform that cannot supply them leaves `observations` out; the docs say
that means "not observed", never "empty".

### Visibility and bounds

`observations` travel with `elements`: they are returned for
`tree_format: "elements"` and `"both"` (and `full_output`), and dropped for the
default Markdown read and for `since` diffs, whose text already renders those
rows. That follows the payload budget decided in
[#2969](https://github.com/trycua/cua/issues/2969). The list has a hard cap of
500 rows; a cut list carries `observations_truncated: true`. `verify_state`
always reads the field internally.

A `query` keeps the observations whose role, label or value contains it
(case-insensitive), the same rows its filtered Markdown shows.

### Verification

`verify_state` evaluates `elements` first and `observations` after. An
addressable match wins over a display-only one, so a control's caption never
turns a value read into `multi_match`. Two observations matching one selector
remain `unknown / multi_match`. Web-content rows remain `untrusted_source`.
`observations_truncated` keeps absence unknown even when the walk is
complete.

## Alternatives considered

1. **Nullable `element_index` in `elements`.** Rejected in the decision record:
   it breaks typed consumers that require the field.
2. **Assign indices to passive rows.** Conflates observation with
   actionability and alters cache and dispatch behavior; rejected.
3. **Parse Markdown in verification.** Makes presentation authoritative;
   rejected.
4. **Normalize values or pick the first match.** Weakens exact comparison or
   ambiguity handling; excluded.

## Compatibility and migration

Additive, with one minor contract bump (`contract_version` 0.8.0 → 0.9.0).
Existing elements, tokens and counts are unchanged. Clients must tolerate a
missing `observations` from older producers.

## Security, privacy, and telemetry

Existing read-only admission, exact-window authorization and accessibility
permissions apply. Observations never grant action authority. Web-content
trust markers are preserved. No new telemetry, persistence or raw-value
logging.

## Implementation plan

One pull request: the typed record, the shared projection and evaluator, the
three native builders, regenerated contract and bindings, and docs.

## Test and acceptance plan

- Shared contract tests: observations carry no index or token, follow `query`,
  stop at the cap and say so; a unique display value satisfies, a different
  value is unsatisfied, a bare `6` against `\u200e6` is unsatisfied, two
  matching rows are `multi_match`, a cut list keeps absence unknown.
- Platform builder tests on macOS and Windows; Linux type-checked.
- A macOS Calculator run: `4 + 2 =` through element tokens, then
  `observations` holds `\u200e6` and `verify_state` with a unique selector and
  the raw value is satisfied.

## Unresolved questions

None.

## Decision record

Accepted on [#3903](https://github.com/trycua/cua/issues/3903#issuecomment-5896355997)
by the maintainers: an optional typed `observations` list next to unchanged
`elements`, rows with role, text or value and frame but no token, accepted by
no action tool; `verify_state` evaluates `elements` then `observations`; the
projection lives in the core crate and all three native builders supply their
non-indexed text rows; a platform that cannot supply them leaves the field
out; a hard row cap, with default visibility following #2969; nullable
`element_index` rejected; additive, one minor contract bump; this RFC merges
with the first implementation pull request, which fixes #2958.
