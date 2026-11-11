---
title: Typed native-window SDK flow and explicit click addressing
authors:
  - f-trycua
created: 2026-09-09
last_updated: 2026-09-09
status: accepted
discussion: https://github.com/trycua/cua/issues/3682
rfc_pr: https://github.com/trycua/cua/pull/3683
implementation:
  - https://github.com/trycua/cua/pull/3683
supersedes:
superseded_by:
---

# RFC: Typed native-window SDK flow

## Decision

Expose the existing native discover / snapshot / act / verify flow through
generated Python and TypeScript methods. Accept an intentional breaking SDK
revision so one click operation supports either coordinates or a snapshot-bound
element token, with an explicit target and delivery mode.

The maintainer selected this scope for implementation. The shortened proposal
window allows the SDK to converge on existing native capabilities in one
revision; cross-platform verification and implementation review remain gates.
This changes the additive SDK-surface assumption for this bounded flow in
[RFC 2549](2549-cua-driver-sdk-owned-runtime.md), without changing its runtime
topology. It does not change the snapshot lifetime work in issue #3473 or add
the post-action rediscovery proposed in pull request #3373.

## Motivation

In SDK 0.25.0, consumers must call `callTool("get_window_state", ...)` and parse
`structuredJson` to inspect a native window. The typed `click` requires x/y and
cannot express an element token or background delivery. The native runtime
already supports these operations. The SDK should expose the full flow without
JSON construction or parsing in application code.

## Public contract

- Both drivers and bound sessions expose `list_apps`, `list_windows`, and
  `get_window_state` (camelCase in TypeScript).
- Discovery and snapshot methods return typed successful results. Snapshots
  include exact window identity, elements and opaque tokens, bounds, screenshot
  data/metadata, and completeness/degradation information. Missing platform
  information remains optional; it is never invented.
- `click` accepts an explicit coordinate-or-element-token sum type, an exact
  window or desktop target, and an explicit background/foreground mode. It
  returns the existing typed action result. Refusals become SDK errors carrying
  the native error code and message.
- Shared Rust declarations generate both bindings. They serialize to the
  existing runtime tool inputs; CLI/MCP wire behavior stays compatible.
- Generic `call_tool` / `callTool` remains available for platform extensions.

## Platform and trust boundaries

Native adapters retain discovery, capture and delivery. Existing permissions,
session authority, snapshot replacement and token ownership remain enforced.
Background delivery never silently escalates to foreground. A dispatched action
does not establish application effect: callers take a fresh snapshot or use
`verify_state`. OS/compositor limitations remain explicit refusals or missing
optional information. Snapshot data is not added to telemetry.

## Alternatives

An additive `clickElement` preserves the old call but splits one operation into
competing APIs. Independent optional x/y/token fields admit ambiguous input.
Generic JSON calls leave output typing and safe addressing to each application.
Use one generated sum type instead.

## Compatibility and migration

Advance the contract shape version. Existing SDK coordinate calls migrate to
the addressing type, explicit target and delivery mode, and direct action
result. Update compatibility fixtures to record the intentional change while
retaining checks for the rest of the public surface. Regenerate bindings and
publish Python and TypeScript migration examples. Pinning the previous SDK or
reverting this product change provides a rollback path.

## Implementation and acceptance

1. Define portable Rust discovery/snapshot records and click addressing; prove
   serialization against every native backend schema.
2. Expose typed driver/session methods and normalize successful results and
   refusals once in the shared SDK boundary.
3. Regenerate bindings, migrate fixtures/examples, and compile and execute the
   complete flow through Python and TypeScript.
4. Run focused contract/SDK tests during development. Certify the stable SHA
   with canonical Windows/Linux desktop E2E and the macOS Lume matrix, including
   application-effect, token freshness, focus, cursor and input-isolation checks.
5. Complete CI and independent review before landing. Record exact candidate
   evidence in the implementation pull request.

## Remaining implementation questions

Finalize portable optional snapshot fields and the binding-friendly addressing
representation from the current platform schemas. The implementation must not
change native delivery semantics to make the SDK appear more capable.
