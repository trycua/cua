# Browser Semantic State Plan

## Status

- **State:** Implemented; cross-platform acceptance in progress
- **Plan date:** 2026-07-17
- **Tracking issue:** [#2299](https://github.com/trycua/cua/issues/2299)
- **Scope:** Rust browser state collection, typed browser actions, shared web test harnesses, and cross-platform acceptance
- **Documentation type:** Internal engineering plan. User-facing reference and how-to documentation are separate deliverables.

The implementation adds `semantic_v2` without changing the `dom_refs_v1`
compatibility default. Deterministic core coverage, the public reference and
how-to pages, the installed skill, and a real standalone-browser harness row
are in place. The final acceptance gate is the exact-SHA Windows and Linux X11
workflow plus the source-installed macOS replay described below.

## Objective

Make `get_browser_state` return the page content and controls that matter in the
current browser view, even when a large application retains hundreds of hidden,
offscreen, or stale nodes. After exact browser attachment, an agent should read
and operate webpage content through browser tools. Native AX and PX remain for
browser chrome, permission prompts, downloads, file pickers, and explicit
fallbacks.

The implementation must preserve these existing guarantees:

- native `pid` and `window_id` remain the binding anchor;
- browser targets, tabs, snapshots, and refs remain session-scoped capabilities;
- every mutation re-proves browser generation, frame identity, document
  identity, and backend-node liveness;
- `get_browser_state` remains read-only;
- page actions remain background-safe when the browser engine supports them;
- ambiguous state produces a structured refusal.

## Current Failure

The collector in `cua-driver-core/src/browser/engine.rs` calls
`DOM.getDocument`, walks the pierced DOM in document order, marks nodes as
interactive from broad tag and attribute rules, and truncates the result to 300
refs after collection.

This causes four problems:

1. Hidden or retained application subtrees can consume the complete ref budget.
2. Visible content has no priority over offscreen content.
3. Static page text is absent because the response contains actionable refs
   only.
4. Truncation reports a boolean but gives the caller no way to inspect omitted
   state.

Increasing the cap would increase output size while preserving the ordering
fault.

## Design Principles

### Observe before acting

`get_browser_state` produces deterministic state and capabilities. It does not
run a model, choose an action, mutate the page, or repair a failed workflow.
Callers inspect state, select a current ref, invoke a typed action, and inspect
state again.

### Separate content from actions

Readable page content and actionable elements are different data sets:

- the semantic outline contains headings, text, landmarks, form state, dialogs,
  and scroll context;
- the ref map contains only elements that support a declared browser action;
- static text may carry an opaque read identifier for scoping, but it must not
  be accepted by `browser_click` or `browser_type`.

### Rank before limiting

The collector classifies and ranks nodes before applying output budgets. The
default order is:

1. active modal, dialog, focused element, and their semantic context;
2. actionable elements inside the current viewport;
3. readable content inside the current viewport;
4. actionable elements near the viewport;
5. readable content near the viewport;
6. remaining laid-out content when requested through continuation or scope.

Document order is retained within each tier.

### Page visibility is not desktop visibility

Browser layout visibility describes whether page content is rendered in or near
the tab viewport. It does not depend on whether the native browser window is
foreground, covered by another window, or outside the visible desktop. This
distinction preserves full-background browser actions.

### Keep policy above transport details

The driver may cache protocol connections and data used within one snapshot. It
must not cache natural-language action plans or replay old selectors. Workflow
repetition and self-repair belong in the calling agent. Cua Driver continues to
require a current capability for each mutation.

## Snapshot Architecture

### Inputs collected per tab

Collect the following browser protocol state concurrently where dependencies
allow:

| Source | Purpose |
| --- | --- |
| Frame tree | Frame topology, loader identity, and parent relationships |
| Accessibility tree | Roles, names, values, states, readable text, and ignored-node pruning |
| Pierced DOM | Backend node IDs, tags, attributes, author shadow roots, and same-process frame documents |
| Layout snapshot | Computed visibility, bounds, viewport relationship, and paint order |
| Layout metrics | Viewport dimensions, scroll offsets, and device scale |

Build one DOM index for each unique CDP session during a snapshot. Same-process
frames share their session index. Out-of-process frames use the existing
capability-tested child-session route.

### Semantic node model

Add an internal `SemanticNode` model with at least:

```text
frame_ref
backend_node_id?
role
name?
value?
states
parent
children
document_order
visibility
action_kinds
```

`visibility` is a closed enum:

```text
in_viewport
near_viewport
offscreen
css_hidden
no_layout
page_occluded
unknown
```

Treat `page_occluded` conservatively. If paint-order evidence is incomplete,
use `unknown` and keep the node. Never infer CSS or page occlusion from native
window occlusion.

### Accessibility-first semantics

Use the accessibility tree for the semantic outline because it already contains
roles, names, values, state, and readable text. Remove ignored and redundant
structural nodes while preserving useful ancestors. Collapse repeated static
text only when the parent already carries the same accessible name.

Some custom controls do not appear as actionable AX nodes. Add a bounded DOM
supplement for elements with clear interaction evidence:

- native interactive tags;
- supported interactive ARIA roles;
- explicit event listeners or handlers;
- editable content;
- pointer cursor with non-zero layout bounds.

The supplement must reject `aria-hidden`, `hidden`, `display:none`,
`visibility:hidden`, zero opacity, and inherited pointer-cursor noise.

### Frame and shadow composition

Preserve the current frame security model:

- every actionable entry records `FrameRef` and document identity;
- same-process iframe entries require a frame-tree identity;
- out-of-process iframe entries require a contained child target and proven
  identity;
- author shadow roots are composed into their host frame;
- user-agent shadow roots remain excluded;
- unprovable frame content is omitted with a reason and count.

Avoid repeated full DOM reads per frame. Index each unique protocol session once
and slice frame subtrees from that index.

### Bounded protocol fallback

Large pages can fail an unbounded `DOM.getDocument` call. Use a bounded fallback:

1. request the full pierced tree;
2. retry with progressively shallower depth only for known depth or serialization
   failures;
3. hydrate missing branches with bounded `DOM.describeNode` calls;
4. enforce time, node, frame, and hydration-call budgets;
5. report partial coverage explicitly when a complete tree cannot be proven.

Transient transport failures remain errors. They must not be treated as a
capability gap or partial success.

## Public Contract

### Versioning

Add a versioned snapshot request without breaking existing clients:

```json
{
  "session": "research-1",
  "target_id": "bt-...",
  "tab_id": "tab-...",
  "snapshot_format": "semantic_v2"
}
```

Keep the existing response available as `dom_refs_v1` during migration. The
skill and harnesses move to `semantic_v2` before it becomes the default. Remove
the old format only through the normal deprecation process.

### Default response

The new response contains:

```json
{
  "status": "ok",
  "mode": "snapshot",
  "snapshot": {
    "id": "p42",
    "format": "semantic_v2",
    "complete": false,
    "scope": "viewport",
    "omitted": {
      "css_hidden": 410,
      "offscreen": 82,
      "unprovable_frame": 0
    },
    "continuation": "opaque-capability-or-null"
  },
  "page": {
    "url": "https://fixture.invalid/inbox",
    "title": "Inbox",
    "focused_ref": "p42:8"
  },
  "outline": "...compact semantic tree...",
  "refs": []
}
```

The outline is compact text for model consumption. `refs` remains structured
JSON for deterministic action routing. The response must not expose raw CDP
target IDs, backend node IDs, object IDs, or selectors.

### Actionable refs

Each ref reports enough public information to choose a typed action:

```json
{
  "ref": "p42:8",
  "role": "button",
  "name": "Reply",
  "states": { "disabled": false },
  "actions": ["click"],
  "frame": "main",
  "visibility": "in_viewport"
}
```

Internal storage keeps the existing backend node and frame evidence. Mutation
tools reject a ref when the requested action is absent from `actions`.

### Scoped observation

Support three read-only scope mechanisms in this order:

1. `scope_ref`: inspect the subtree rooted at a current semantic or actionable
   ref;
2. `query`: return role, accessible-name, and visible-text matches with ancestor
   context;
3. `continuation`: inspect the next ranked segment from the same live snapshot
   generation.

All scope tokens are opaque, session-bound, tab-bound, and generation-bound.
Do not expose XPath or CSS as the primary agent contract. A later expert-only
selector field may be considered after the capability path is accepted.

### Truncation and budgets

Replace the single `truncated` boolean with:

- whether collection was complete;
- which visibility tiers were omitted;
- counts by omission reason;
- the applied node and output budgets;
- an opaque continuation capability when more proven state is available.

Budgets are configuration constants in the first implementation. Public numeric
overrides wait until performance and abuse bounds are known.

## Code Changes

### `cua-driver-core/src/browser/engine.rs`

- split protocol collection from semantic composition;
- collect frame, AX, DOM, layout, and viewport state;
- rank semantic nodes before output limiting;
- mint refs only for declared action kinds;
- preserve OOPIF containment and cleanup;
- return coverage and omission diagnostics.

### `cua-driver-core/src/browser/store.rs`

- store snapshot format and generation;
- add action kinds to `RefEntry`;
- store opaque continuation and scope capabilities;
- keep static semantic nodes outside the actionable ref map;
- invalidate every derived capability on navigation, reconnect, newer snapshot,
  session end, and target replacement.

### `cua-driver-core/src/browser/tools.rs`

- add `snapshot_format`, `scope_ref`, `query`, and `continuation` to snapshot
  mode;
- return semantic outline, structured refs, and coverage diagnostics;
- keep bind mode unchanged;
- remove the duplicate `target_id` field in the current snapshot response;
- keep MCP read-only annotations accurate.

### Browser transport

- add typed protocol response models for AX and layout snapshots where practical;
- classify known depth and serialization failures separately from transport
  failures;
- make concurrent calls cancellation-safe;
- emit timing and count metrics without page text.

### Skills and documentation

- update `Skills/cua-driver/BROWSER.md` to use `semantic_v2`;
- teach the snapshot, choose-current-ref, act, re-snapshot loop;
- explain scoped reads and continuation;
- state that webpage content should stay on browser tools after attachment;
- retain `get_window_state` for browser chrome and native fallbacks;
- add user reference documentation only after the schema is accepted.

## Test Strategy

### Shared deterministic fixture

Extend the repo-local web harness with a large application fixture containing:

- more than 300 hidden or retained controls before the active view;
- a virtualized list and a selected detail view;
- visible heading, message body, editable reply field, and send button;
- a modal overlay that covers controls beneath it;
- nested author shadow DOM;
- same-process and out-of-process iframes;
- dynamic rerender and navigation controls;
- duplicated names in separate regions;
- offscreen controls reachable through continuation or scope.

Use fixture data only. Do not use customer domains, account names, email text,
profile paths, or browser history in source, logs, screenshots, or artifacts.

### Unit tests

Add tests for:

- AX role, name, state, and static-text normalization;
- CSS and layout visibility classification;
- ranking stability and document-order ties;
- action-kind assignment;
- hidden-node exclusion;
- DOM supplement deduplication;
- frame and shadow composition;
- bounded depth fallback and partial-coverage reporting;
- output budgets and continuation invalidation;
- query and scope ambiguity;
- stale refs after navigation, rerender, reconnect, and newer snapshots.

### Browser E2E rows

Run the same source-built Rust rows on each accepted browser and platform:

| Scenario | Foreground posture | Background posture | Required evidence |
| --- | --- | --- | --- |
| Read visible detail text | yes | yes | semantic outline contains fixture text |
| Click visible action | yes | yes | fixture journal changes once |
| Type into visible editor | yes | yes | exact delivered text and no leaked input |
| Hidden-node pressure | yes | yes | visible controls survive the budget |
| Scoped duplicate-name action | yes | yes | only the scoped region changes |
| Modal page occlusion | yes | yes | covered control omitted or marked occluded |
| Continuation | yes | yes | offscreen control becomes addressable |
| Rerender staleness | yes | yes | old ref refuses, new ref succeeds |
| Frame and shadow action | yes | yes | exact contained frame mutates |

Background rows run with the browser natively occluded and with focus, cursor,
and leaked-input sentinels active. The page viewport stays unchanged so browser
layout visibility remains comparable between postures.

### Platform gates

- **macOS:** current host or accepted VM, local source install, browser consent
  already granted through the documented flow.
- **Windows:** interactive GitHub-hosted runner or Azure RDP desktop, never
  session 0.
- **Linux X11:** interactive desktop with the standard browser harness.
- **Linux Wayland:** accepted compositor with exact browser binding; unsupported
  compositor identity remains a structured refusal.

The semantic collector should produce equivalent page results across operating
systems because it runs above the native platform layer. Platform-specific
differences are limited to attachment, endpoint proof, consent, and native
sentinels.

## Delivery Phases

### Phase 0: Reproduce and measure

- add the hidden-node-pressure fixture;
- prove the current collector omits visible content;
- record protocol time, node counts, response bytes, and output lines;
- add the failing test without changing its expectation to fit current behavior.

**Gate:** the fixture reproduces the defect deterministically on a source build.

### Phase 1: Semantic outline

- add the AX-based semantic model;
- return readable page content separately from actionable refs;
- keep the existing DOM ref collector available as v1;
- add action-kind checks.

**Gate:** visible detail text and editor state are available without native AX.

### Phase 2: Layout and ranking

- join layout snapshot and viewport metrics;
- classify visibility;
- apply visible-first ranking and coverage diagnostics;
- add conservative page-occlusion handling.

**Gate:** hidden-node pressure no longer displaces visible state.

### Phase 3: Scope and continuation

- add `scope_ref`, semantic query, and opaque continuation;
- bind each capability to the current session, target, tab, and generation;
- reject stale and ambiguous scopes.

**Gate:** duplicate names and offscreen content are addressable without raw
selectors or an unbounded response.

### Phase 4: Frame and scale hardening

- share DOM indexes by protocol session;
- add bounded depth fallback and hydration;
- verify same-process frames, out-of-process frames, and shadow roots;
- set accepted time, node, frame, memory, and response budgets.

**Gate:** large multi-frame fixtures finish within the accepted budgets and
report partial coverage truthfully when a budget is exhausted.

### Phase 5: Cross-platform acceptance

- update the skill and protocol schema tests;
- run foreground and background harness rows on macOS, Windows, X11, and the
  accepted Wayland compositor;
- publish matrix summaries, traces, and fixture-only videos;
- verify logs and artifacts contain no page text from non-fixture sessions.

**Gate:** all supported rows pass or produce their documented structured
refusal, and the old snapshot format remains compatible.

### Phase 6: Default migration

- make `semantic_v2` the skill default;
- monitor size, latency, refusal, and stale-ref metrics;
- document the old format deprecation window;
- remove v1 only after accepted clients and harnesses have migrated.

**Gate:** one release cycle completes with no unresolved compatibility or
privacy regression.

## Definition of Done

The project is complete when:

1. A source-built Cua Driver can attach to an exact browser tab and read a large
   application view without native AX inspection.
2. Visible text and actions remain available when more than 300 hidden or
   retained nodes precede them.
3. Static content and actionable refs are separate, and mutation tools enforce
   declared action kinds.
4. Scope and continuation are session-bound capabilities with tested stale
   behavior.
5. Frame, shadow, navigation, rerender, and reconnect tests preserve the exact
   mutation contract.
6. Foreground and background E2E rows pass on every advertised browser route for
   macOS, Windows, X11, and accepted Wayland environments.
7. The skill, protocol schema, support reference, CI matrix, and release notes
   describe the shipped behavior.
8. Repository scans and artifact review find no customer identifiers, account
   content, local profile paths, endpoint tokens, or raw browser IDs.

## Explicit Non-Goals

- model inference inside Cua Driver;
- natural-language `browser_act` or `browser_extract` tools;
- automatic replay or self-healing of old action plans;
- exposing XPath, CSS selectors, backend node IDs, or CDP target IDs as the
  primary public contract;
- treating native window visibility as page-element visibility;
- making arbitrary static text clickable;
- claiming complete state when protocol or budget limits produced a partial
  snapshot.
