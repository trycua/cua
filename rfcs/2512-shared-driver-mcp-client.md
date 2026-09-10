---
title: 'RFC 2512: Share the typed Driver MCP client across language bindings'
created: 2026-09-10
status: accepted
discussion: https://github.com/trycua/cua/issues/2512
parent_rfc: 2512-mcp-envelope-carrier.md
---

# One Driver connection implementation

Implement the typed MCP client once in the Cua Driver Rust SDK and expose it
through UniFFI to Python and TypeScript. Preserve Python Sandbox's
`sb.driver.connect(service="mcp", transport="mcp")`. TypeScript applications
compose Fleet and Driver directly; this work does not create a TypeScript
Sandbox SDK.

The maintainer selected this narrow follow-up for implementation and draft PR
review. The linked issue records the decision. The broader RFC remains separate;
this acceptance does not authorize merge, release, image changes, or rollout.

## Ownership

| Layer | Owns |
| --- | --- |
| Fleet | Credentials, authenticated named-service routing, pools, and claims |
| Driver Rust SDK | MCP initialization, typed-extension negotiation, envelope exchange, strict response parsing, session state, cancellation, and bounded connection cleanup |
| Generated Python and TypeScript bindings | The canonical Driver object and shared connection API |
| Thin environment adapters | Forward bounded service requests through an existing Fleet client; bind the connection lifetime to the caller's live claim |
| Python Sandbox | Existing convenience accessor and cleanup before its Fleet transport closes |

The adapter is not a second MCP implementation. Driver does not acquire Fleet
credentials, construct a second Fleet lifecycle client, change permissions, or
assume separately packaged UniFFI objects can share native pointers.

## Contract and compatibility

The shared connector accepts a host-provided asynchronous service transport,
already bound to one authenticated target. Requests carry method, relative
path, headers, bytes, and timeout; responses carry status, headers, and bytes.
No public raw URL, credential, or permission grant is negotiated with the guest.

The resulting owned connection exposes the actual generated `CuaDriver`, the
host-bound public session label, and explicit close. Closing it only tears down
its Driver receiver and MCP session. The caller still owns claim/pool release.
Keep the existing remote-envelope API, direct socket connection, computer-server
operations, and ordinary MCP tools unchanged.

Require protocol `2025-06-18` and the existing
`ai.cua.driver.envelopes` v1 capability. Preserve receiver generations,
host-selected authority, exact request correlation, strict bounded JSON/SSE,
late-result rejection, and unknown-completion errors. Never replay an uncertain
action, reconnect to another guest, fall back to local desktop control, or
rebind the trusted session. Cancellation does not undo desktop actions.

Python event-loop ownership and Node callback cancellation must be tested
through the real generated bindings. Native TypeScript bindings remain
Node-only; this does not add browser-native Driver support. Fleet's browser
entry point must remain independent of native Driver dependencies.

## Phases and proof

1. **Shared Rust connector.** Implement the transport boundary and MCP state
   machine alongside the existing remote Driver backend. Port deterministic
   parser, negotiation, failure, cancellation, late-open, and cleanup tests.
2. **Bindings and Fleet adapters.** Generate Python/TypeScript bindings from
   the Rust source. Add thin optional Fleet entry points and real-binding
   fixtures proving both return the canonical typed interface. Verify callback
   ownership, disposal, error sanitization, and no native import in Fleet browser.
3. **Python Sandbox migration.** Delegate the explicit MCP path to the shared
   connector, preserve the public accessor and direct-envelope path, and remove
   the duplicate Python MCP protocol implementation. Run Sandbox regressions.
4. **Reviewable delivery.** Add exact API examples, run affected formatting,
   generation drift and deterministic suites, review the final diff, and keep
   the draft PR's evidence and limitations current.

Before promotion beyond draft review, qualify the exact candidate against
Linux and Windows guests: useful desktop effect, computer-server compatibility,
cancel/replace behavior, independent sessions, and resource cleanup. Ordinary
PR checks and protocol fixtures are not desktop qualification. Existing image
evidence does not qualify a new client automatically. No image publication or
production enablement is included here.

## Alternatives and rollback

Do not duplicate the Python MCP state machine in TypeScript, add desktop
methods to generated Fleet resource records, or build on the legacy Computer
SDK. A native dependency from Driver to Fleet is not needed when the host
already provides authenticated service access.

Rollback reverts this additive connector and Sandbox delegation together,
retaining existing package versions and images. It does not migrate pools,
terminate shared daemons, or modify computer-server.
