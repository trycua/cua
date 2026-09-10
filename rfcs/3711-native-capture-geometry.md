---
title: Native capture geometry and compositor-limited window input
authors:
  - injaneity
created: 2026-09-10
last_updated: 2026-09-10
status: draft
discussion: https://github.com/trycua/cua/issues/3711
rfc_pr:
implementation: []
supersedes:
superseded_by:
---

# RFC: Native capture geometry and compositor-limited window input

## Summary

Separate PNG-to-compositor scale consistency from agreement with the logical
native window. Preserve observational content and independently usable semantic
actions, but require the appropriate geometry evidence before window-pointer
side effects. Share assessment vocabulary and admission policy in the common
core; keep compositor-specific fact gathering in platform adapters.

This is a documentation-only proposal. It does not authorize implementation,
change runtime behavior, or resolve [the original bug](https://github.com/trycua/cua/issues/3631).
The original report is credited to @f-trycua. The RFC discussion owns the design
decision; the bug remains its problem record.

## Motivation

Follow-up diagnostics on macOS 26.5.2 with a local 0.25.0 build found a full
230×408 Calculator AX frame alongside a 31×102 compositor frame and PNG. Both
inputs to the current scale validator describe the thumbnail, so it passes.
Requesting a larger image produced the same thumbnail pixels on a larger canvas,
not restored native content. Tested alternative APIs, shell capture, cached
filters, and a live stream also retained transformed content.

A genuine 90×102 native window matched all geometry sources while active but
became 47×105 in the strip. Small dimensions, global Stage Manager state, and a
requirement that both dimensions shrink are not sufficient classifiers.

These are diagnostic observations, not release acceptance evidence. The local
binary's exact source SHA was not established, and the native API experiment had
an outstanding OS capture-consent-dialog caveat. No permission was granted by
the diagnostic workflow.

## Scope

- Add truthful, bounded geometry assessment without claiming image freshness or
  full-content fidelity from matching dimensions alone.
- Preserve existing PNG scale semantics and usable semantic actions.
- Cover explicit pixel tools, implicit pointer fallbacks, and foreground input
  without automatically activating a window or reinterpreting a thumbnail.
- Specify unsupported platform coverage explicitly and preserve its existing
  mapping requirements.

Automatic full-resolution recovery, private compositor APIs, browser-page
capture changes, generic keyboard routing, and mandatory snapshot binding for
all pixel coordinates are outside this proposal.

## Decisions to specify in this draft

1. Public assessment states, reason vocabulary, field units, and error precedence.
2. Exact-window identity, comparison tolerance, bounded sampling, and race behavior.
3. Pointer admission for known mismatches, missing native geometry, and unsupported
   platform assessment, including foreground and implicit fallbacks.
4. Compatibility impact, client migration, and release sequencing.
5. Shared tests and native acceptance oracles that do not depend on private local
   evidence directories.

## Acceptance direction

Require classifier and response-contract coverage, real small-window controls,
active/strip transitions, identity/race cases, app-observed pointer events, and
focus/cursor oracles. Cross-platform adapters must account for their coverage
without presenting an unsupported assessment as successful. Native acceptance
requires an exact candidate SHA and a logged-in, correctly authorized desktop;
follow the canonical stable-candidate E2E timing rather than treating the
preliminary investigation as certification.

## Decision record

Pending. No implementation decision has been recorded.
