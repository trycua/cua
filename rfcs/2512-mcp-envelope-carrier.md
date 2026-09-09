---
title: 'RFC 2512: Carry typed Driver envelopes through an existing MCP endpoint'
created: 2026-09-09
status: review
discussion: https://github.com/trycua/cua/issues/2512
parent_rfc: 2512-python-fleet-first-slice.md
---

# RFC 2512: Reuse an existing MCP endpoint

## Recommendation and decision needed

Keep the canonical generated `CuaDriver` behind `sandbox.driver`. Investigate
carrying its existing versioned envelopes through an explicitly enabled MCP
extension, without making ordinary `tools/call` the application contract.
Keep computer-server and every existing Sandbox operation unchanged.

This is a transport addendum to the [accepted first slice](2512-python-fleet-first-slice.md),
not a replacement for that decision. The maintainer selected the phased
existing-route investigation and tested, unmerged implementation proposals.
The exact extension below still needs its recorded decision in RFC issue 2512
before production implementation. This document does not mark it accepted.

The earlier supplement correctly excludes an MCP-only endpoint from typed
receiver qualification. Reusing MCP as a carrier would not remove that rule:
an endpoint must explicitly advertise and implement the envelope extension.
Old MCP-only images must fail negotiation before any desktop action.

## User-facing boundary

The proposed explicit selection is not available in the released SDK:

```python
from cua_driver import GetScreenSizeInput


async def observe_guest(pool):
    async with pool.claim() as sb:
        async with sb.driver.connect(service="mcp", transport="mcp") as driver:
            size = await driver.get_screen_size(GetScreenSizeInput(session=None))
        return size
```

`driver` must be the actual generated `cua_driver.CuaDriver`. Do not introduce
a reduced facade, reinterpret arbitrary MCP dictionaries as Driver metadata,
or use client-generated values as proof of the guest generation.

Preserve `CuaDriver.connect(socket_path)` and `sb.driver.connect(service="driver")`.
Only the explicit MCP selection uses the new carrier. No automatic transport
fallback, request replay after uncertain completion, or local-desktop fallback
is permitted. A closed or replaced guest requires explicit reacquisition.

Shell, files, terminals, allocation, claims, and pool cleanup remain outside
Driver. Existing Sandbox desktop operations continue using computer-server.

## Evidence and limits

At Cua source `75b04aac03ed6cb1e08c41b2384595e5c5ab2d9f`:

- `libs/cua-driver/rust/crates/cua-driver/src/proxy.rs` discards notifications
  in both inspected stdio loops and serially awaits ordinary tool dispatch.
  MCP defines cancellation; this is an implementation observation, not a
  claim that the protocol cannot support it.
- `libs/cua-driver/rust/crates/cua-driver-sdk/src/remote.rs` requires receiver
  cancellation support. A tools-only client cannot truthfully synthesize it.
- `remote_receiver.rs` already owns the request ledger, runtime generation,
  session binding, cancellation, deadlines, duplicate refusal, and close.
  Reuse it rather than implementing a second lifecycle protocol.

An inert local fixture tested supergateway 3.4.3 with MCP SDK 1.18.2 and Node
26.7.0. It verified namespaced request forwarding, structured error forwarding,
cancellation-notification delivery while another request was outstanding,
distinct child-instance identities, and independent HTTP session close with
stale-session rejection. This is wrapper evidence only, not Driver behavior,
deployed-image qualification, authentication proof, or desktop E2E evidence.

The inspected wrapper implementation is
[stdioToStatefulStreamableHttp.ts at v3.4.3](https://github.com/supercorp-ai/supergateway/blob/v3.4.3/src/gateways/stdioToStatefulStreamableHttp.ts).
Its ordinary per-request response stream can be buffered; this proposal does
not require a long-lived unsolicited-event stream in the Fleet SDK.

## Candidate same-port design

The following names and schema are proposals for review, not a wire contract
that consumers may depend on:

| Concern           | Proposed disposition                                                                                                                                                                                           |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Discovery         | An explicit versioned `capabilities.experimental` entry advertises the typed envelope carrier. Ordinary MCP initialization and tools remain compatible.                                                        |
| Requests          | Namespaced JSON-RPC methods carry receiver open, exchange, cancel, and close. Reuse the existing envelope types and receiver; do not expose these as agent tools.                                              |
| Authority         | Guest startup selects permission mode. The authenticated environment owns access. Caller parameters, session names, and connection IDs cannot grant authority.                                                 |
| Session ownership | Bind receiver connections to the owning transport. A connection ID from one transport must not operate or close another transport's connection.                                                                |
| Concurrency       | Read cancellation and close while an exchange is pending. Keep ordinary action dispatch serialized through the existing receiver. Bound in-flight tasks and payloads.                                          |
| Generation        | Use the receiver instance's real generation. A restarted receiver or replaced guest rejects existing handles; the client does not silently initialize again.                                                   |
| Cancellation      | Use the existing request ledger and early-cancel handling. After dispatch, preserve unknown completion and invalidate the connection as required. Cancellation does not undo a native action.                  |
| Cleanup           | Explicit receiver close affects only owned resources. HTTP DELETE is carrier teardown, not proof of action rollback or guest deletion. EOF/error cleanup must preserve unrelated sessions and shared runtimes. |
| Errors            | Preserve typed envelopes and sanitized error codes. Do not export credentials, raw request bodies, screenshots, or private identities in generic diagnostics.                                                  |

The launcher must opt in. Before enabling the extension, review every supported
stdio/runtime ownership path and wrapper logging behavior. Do not enable it on
an unqualified public listener, add a credential issuer, change authorization,
or assume a new network policy, port, or image rollout is necessary.

The implementation should be a small transport-specific module around the
existing receiver, with minimal integration in the stdio dispatcher. Coordinate
the dispatcher seam with [MCP modernization PR 3609](https://github.com/trycua/cua/pull/3609);
do not duplicate or supersede its modernization work. This addendum does not
claim that PR supplies the typed extension or modern HTTP support.

## Stack and acceptance gates

1. **Contract evidence and this addendum.** Record exact wrapper/image behavior,
   capability gaps, cleanup, and a maintainer decision on the same-port design.
2. **Guest carrier, after the decision.** Add opt-in receiver dispatch and
   deterministic tests for concurrency, early/late cancellation, malformed and
   oversized requests, duplicate IDs, transport ownership, EOF, and close.
3. **Sandbox carrier, stacked on the guest contract.** Return the canonical
   generated object. Test real native bindings, typed records, schema failures,
   unsupported operations, no retry/fallback, and the unchanged envelope path.
4. **Qualification and exact-version docs.** Test Linux and Windows images
   independently: useful guest UI postcondition, interleaved sessions,
   cancellation, replacement, authorization, computer-server compatibility,
   and verified session/resource cleanup. Advertise only qualified combinations.

Tests must distinguish HTTP session separation from Driver context isolation.
A tools list, mock carrier, successful wrapper test, or published image is not
typed Fleet E2E sign-off. Record untested rows rather than weakening checks.

## Rollback and authorization

Keep the feature opt-in and existing images pinned. Rollback closes only the
new client sessions, disables the extension, and uses the unchanged connection
selection and computer-server paths. No automatic pool migration or runtime
termination is part of rollback.

The current delivery stage is draft PR review. Maintainers own the exact
transport decision and overlap resolution; implementation owns the tests and
evidence. Merge, package release, image publication, deployment, and production
enablement are separate gates after the applicable evidence passes.
