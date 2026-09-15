---
title: Independent session-label visibility for native cursors
authors:
  - dyh-sjtu
created: 2026-09-15
last_updated: 2026-09-15
status: review
discussion: https://github.com/trycua/cua/issues/3874
rfc_pr:
implementation: []
supersedes:
superseded_by:
---

# RFC: Independent session-label visibility for native cursors

## Summary
Add an opt-in, per-session way to suppress the native cursor's session-label text while retaining the cursor and existing delivery/target context indicators. Keep current rendering as the default. The session identifier and transport ownership must remain unchanged.

## Motivation and current state
An embedding application may already show the active task in its own UI. Repeating an opaque session identifier next to the native cursor adds visual noise. Renaming or rotating the session to change its appearance is not an appropriate presentation API.

Source inspection at `5fcd67326dd0406bca823e5488cf39f47491c869` confirms:
- [`StartSessionInput` and cursor setters](https://github.com/trycua/cua/blob/5fcd67326dd0406bca823e5488cf39f47491c869/libs/cua-driver/rust/crates/cua-driver-contract/src/inputs.rs#L302-L440) have no independent label-visibility option.
- [Shared render state](https://github.com/trycua/cua/blob/5fcd67326dd0406bca823e5488cf39f47491c869/libs/cua-driver/rust/crates/cursor-overlay/src/render_state.rs#L153-L206) reveals labels initially and on hover, with no public suppression setting.
- [Theme documentation](https://github.com/trycua/cua/blob/5fcd67326dd0406bca823e5488cf39f47491c869/libs/cua-driver/docs/cursor-themes.md#L18-L43) explicitly separates native badges from dotLottie artwork. Custom cursor themes cannot hide this text.

The same missing public option was observed when inspecting the `cua-driver-rs-v0.28.1` source. This is a source-level capability assessment, not a claim of fresh native runtime tests.

## Goals
- Hide label text, including initial and hover reveals, without disabling the native cursor.
- Preserve distinct named sessions and lease isolation.
- Apply the initial preference before the first cursor frame; expose the effective preference for inspection.
- Use shared policy across macOS, Windows, and Linux, preserving existing behavior when omitted.

## Non-goals
Custom label text, action-status text, font/color/CSS customization, hiding delivery/target chips, changing input delivery, hiding consent or stop controls, or changing the theme artifact format.

## Proposal
Introduce an optional `cursor_badge: { show_session_label: boolean }` on `start_session`, with `true` as the creation default. A separate `set_agent_cursor_badge` operation changes the preference for an existing owned session; `get_agent_cursor_state` reports it. Exact public API placement is a maintainer decision in this RFC.

Store visibility separately from the label and runtime key. Route it through shared cursor render state. When false, suppress text and label-only hover polling/timers; continue drawing existing context chips according to their lifecycle. Do not leave an empty label pill when no other badge content is visible. Preserve the normal chip background when chips remain visible.

Repeated `start_session` without the new field must preserve an existing session's preference. Explicit settings update it atomically before any first reveal. Ordinary actions, theme changes, idle wakeups, and metadata refreshes must not reset it. Ending a session drops the preference with the session.

## Alternatives considered
- Omit, blank, or reuse one session label: changes naming/lookup semantics and is unsuitable for independent concurrent named sessions.
- Change dotLottie artwork: the host renders labels outside the theme.
- Disable the entire cursor: removes useful action feedback.
- Add only an initial setting: smaller API, but requires session recreation to change presentation.

## Compatibility and migration
Additive, default-preserving API. Clients must feature-detect support and retain current visuals on older runtimes; do not send unknown fields to old strict schemas. Do not reinterpret the removed legacy `set_agent_cursor_style` API. No runtime implementation is included before the RFC decision.

## Security, privacy, permissions, and telemetry
Existing ownership checks, permission prompts, delivery/target indicators, and stop controls remain intact. Suppression is presentation only and does not make identifiers secret in logs or protocol responses. No new telemetry or session-content logging. The public proposal includes no private screenshots or transcripts.

## Validation and acceptance evidence
Required after acceptance: default/round-trip/schema tests; hidden-label initial, hover, idle, and theme-change tests; preserved chip layout and no empty pill; two-session isolation; end/recreate lifecycle; and native overlay evidence on macOS, Windows, X11 and supported Wayland environments. Verify that label-only hover polling stops without stopping chip/action animation. Do not mark implementation complete without the repository's platform evidence.

## Unresolved questions
- Is this best exposed as a dedicated badge operation plus initial selection, or another existing host configuration surface?
- Should changing false to true replay the normal initial reveal? Proposed: yes.

## Implementation plan

After a maintainer records acceptance in the discussion issue:

1. Add typed inputs, outputs, schemas, and binding parity using the agreed API shape.
2. Store the preference separately from session identity and propagate it through shared overlay events/state, with thin native adapters.
3. Cover visibility transitions, polling, layout, concurrency, and lifecycle with focused tests.
4. Update cursor documentation and integration examples, and gather the required native platform evidence before marking the implementation PR ready.

## Decision record

Pending maintainer review. This PR proposes a public contract; it does not implement or claim acceptance of that contract.
