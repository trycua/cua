---
title: 'RFC 2512: Python and Fleet first delivery slice'
created: 2026-09-07
status: review
discussion: https://github.com/trycua/cua/issues/2512
parent_rfc: 2512-cua-driver-environment-convergence.md
---

# RFC 2512: Python and Fleet first delivery slice

## Decision requested

Adopt a narrow first delivery of [RFC 2512](2512-cua-driver-environment-convergence.md):
one typed Driver desktop contract, first composed with Python Sandbox on one
Linux Fleet image, followed by one independently qualified local Linux guest.
Keep MCP alongside the typed SDK for agent harnesses. Keep shell, files, PTYs,
and machine lifecycle outside Driver.

This supplement updates the implementation baseline and proposes stacked review
boundaries. It does not mark RFC 2512 accepted, advertise a released remote
Python API, or authorize a merge or image rollout. The RFC issue remains the
decision record; this is not a second competing architecture RFC.

## Source baseline and work already available

Read-only audit baseline: Cua main
`ddc7a632a9bc02af400ac052503bd9f6c8503b9f` on September 7, 2026.
The original parent RFC proposal predates this code. Its historical statement that no
remote backend exists is no longer accurate for Rust.

- The Rust SDK already implements `CuaDriver::connect_remote` using
  `DriverEnvelopeChannel`, alongside embedded, daemon, and private-worker
  execution. Desktop calls retain the same typed method surface. See
  [SDK implementation](https://github.com/trycua/cua/blob/ddc7a632a9bc02af400ac052503bd9f6c8503b9f/libs/cua-driver/rust/crates/cua-driver-sdk/src/lib.rs).
- The existing channel includes negotiation, request/response envelopes,
  authenticated principal and connection generation, session binding,
  cancellation, and close. The client preserves uncertain action completion
  rather than replaying an action. See
  [remote connection](https://github.com/trycua/cua/blob/ddc7a632a9bc02af400ac052503bd9f6c8503b9f/libs/cua-driver/rust/crates/cua-driver-sdk/src/remote.rs).
- This remote constructor is Rust-only. The Python binding does not yet expose
  a production remote constructor, and this audit found no production channel
  implementation or guest HTTP receiver for these envelopes.
- Fleet already offers authenticated, buffered named-service requests. It
  validates the service, constructs its destination, filters caller credential
  and hop-by-hop headers, and does not replay service 401 responses. See
  [service routing](https://github.com/trycua/cua/blob/ddc7a632a9bc02af400ac052503bd9f6c8503b9f/libs/fleet/sdk/src/services.rs)
  and [service tests](https://github.com/trycua/cua/blob/ddc7a632a9bc02af400ac052503bd9f6c8503b9f/libs/fleet/sdk/tests/service_flow.rs).
- Existing Sandbox screenshot/input operations still use computer-server.
  Named services provide generic requests, not typed Driver calls. See
  [Sandbox Fleet transport](https://github.com/trycua/cua/blob/ddc7a632a9bc02af400ac052503bd9f6c8503b9f/libs/python/cua-sandbox/cua_sandbox/transport/fleet.py).

Preserve the implemented SDK-owned runtime behavior from
[PR 2561](https://github.com/trycua/cua/pull/2561) and its
[RFC 2549](https://github.com/trycua/cua/blob/ddc7a632a9bc02af400ac052503bd9f6c8503b9f/rfcs/2549-cua-driver-sdk-owned-runtime.md).
RFC 2447 is superseded by that runtime-ownership document. Do not restore its
older topology or session-capture assumptions while implementing this slice.

## Public contract

Applications retain the same typed desktop methods, input records, and result
records. Connection factories and ownership differ. The integration must
return the canonical generated `CuaDriver`, not a handwritten Python class
that mirrors a subset of methods or wraps MCP tool dictionaries.

Existing in-process usage remains unchanged:

```python
from cua_driver import CuaDriver, GetDesktopStateInput


async def observe_this_machine():
    driver = CuaDriver.create()
    try:
        return await driver.get_desktop_state(
            GetDesktopStateInput(session=None, screenshot_out_file=None)
        )
    finally:
        await driver.shutdown()
```

Proposed Sandbox accessor; this constructor is not implemented or released:

```python
from cua_driver import GetDesktopStateInput


async def observe_claim(pool):
    async with pool.claim(service="server") as sandbox:
        async with sandbox.driver.connect() as driver:
            desktop = await driver.get_desktop_state(
                GetDesktopStateInput(session=None, screenshot_out_file=None)
            )
        result = await sandbox.shell.run("printf 'guest execution\\n'")
        return desktop, result
```

The example assumes an authorized existing pool with working computer-server
and a future declared Driver envelope service. An image exposing only Driver
MCP does not satisfy this prerequisite. Claim acquisition can use billable
capacity; claim release does not delete the pool or establish zero charges.

| Boundary                | Required behavior                                                                                                                                                   |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Machine target          | In-process creation runs here. A Sandbox connection binds to its guest and immutable lifetime identity. Never fall back to host control.                            |
| Typed desktop interface | Reuse canonical methods and records. Unsupported operations fail explicitly; identical types do not certify every platform capability.                              |
| Connection close        | End only owned Driver client/session resources. Preserve the guest daemon, other clients, Sandbox handle, claim, and pool.                                          |
| Runtime shutdown        | Keep existing in-process shutdown semantics. Remote shutdown closes its channel; it must not become a machine-delete operation.                                     |
| Paths and artifacts     | Define caller versus guest paths per operation. For the first slice, reject unsupported output-path options before dispatch; returned image data remains typed.     |
| Permission authority    | Guest policy and authenticated session binding determine authority. Host callbacks, public session labels, and caller-supplied JSON cannot grant remote privileges. |
| MCP                     | Keep the official agent adapter downstream of the typed SDK. Do not make it the remote application contract.                                                        |

The shared scenario uses `screenshot_out_file=None`. Recording, downloads,
host-management operations, and path-bearing calls need an explicit capability
disposition before they are advertised remotely. A remote endpoint must reject
unsupported host-only operations, not forward arbitrary methods indiscriminately.

## Transport recommendation

Reuse the existing Rust remote backend and versioned envelopes. Add a bounded
unary service adapter in the guest and an environment-owned authenticated
carrier. Do not redesign desktop methods, add a second protocol schema per
language, or put Fleet dependencies into Driver's native core.

The first carrier must implement all existing `DriverEnvelopeChannel` lifecycle
obligations. In particular, the current SDK refuses action dispatch when a
carrier does not support cancellation. Reporting cancellation support without
implementing request tracking and cancel behavior is not an acceptable shortcut.

Fleet's buffered request/response abstraction does not prove streaming SSE,
WebSocket, or stdio forwarding. Keep any new streaming support outside this
first slice. Existing MCP integrations remain separate, with their own tested
transport limits.

The receiver must sit behind an authenticated, claim-scoped guest route. A
loopback bind alone neither makes a service reachable from Fleet nor provides
authentication. The review must select and test either verified proxy-injected
service identity or a separately scoped guest credential, including revocation
and refresh. Do not assume the current proxy provides an unverified identity
assertion or send a broad control-plane bearer into the guest.

The bound Fleet record exposes claim, namespace, sandbox name, and services.
Those names are not by themselves proof of an immutable boot/binding generation.
Establish how the adapter obtains and verifies a generation that changes when a
guest is replaced; an old handle must never attach to a replacement by name.

## Stacked PR sequence

Use one workstream under RFC 2512. Each implementation PR is based on its
immediate dependency, with a description listing the full stack, incremental
diff, tests, and unresolved gates. Keep implementation branches current with
main after the RFC decision; do not build products from the historical RFC base.

| Phase | Review boundary                                              | Required evidence                                                                                                                    |
| ----- | ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------ |
| A     | This contract supplement, stacked on the existing RFC PR     | Current-source audit, typed examples, acceptance table, and recorded maintainer decision before B                                    |
| B1    | Guest envelope receiver and canonical dispatcher             | Auth/session boundary, operation allowlist, deadlines, cancellation, duplicate handling, close isolation, version and payload limits |
| B2    | Environment carrier and generated Python construction bridge | Reuse the ordinary `CuaDriver`; claim/generation binding; typed results; no uncertain-action replay; generated binding drift checks  |
| C     | Additive Sandbox accessor                                    | Claim-owned connection invalidation; close ordering; old screenshot/input/shell/files/PTY behavior remains independent               |
| D     | One local Linux guest connection                             | Exact guest endpoint discovery and packaging; no host fallback; same typed consumer; independently verified cleanup                  |
| E     | One agent integration recipe                                 | Harness-owned MCP or typed integration; separate guest shell/files; verified useful UI result and normal cleanup                     |
| F     | Reusable qualification tests                                 | Exact-source and exact-image evidence; typed parity, useful UI postcondition, representative legacy compatibility, negative controls |
| G     | Exact-release Diataxis docs                                  | Tested commands, supported combinations, honest limitations, links and render checks; no unreleased API presented as available       |

B1 and B2 may each need separate Driver and integration-package commits. Split
the review instead of hiding a server, new credentials, bindings, and SDK API
inside one transport PR. Image packaging belongs in its owning image repository
with explicit cross-repository dependencies; Git cannot directly stack branches
from different repositories.

Python and one Linux Fleet image are the first application proof. A Linux Docker
guest is a local candidate because its runtime already publishes exposed ports
on host loopback. That is not a qualification result: service identity, endpoint
mapping persistence, Driver packaging, readiness, and cleanup remain work.

TypeScript, Rust application distribution beyond the common implementation,
additional operating systems/images, direct Lume composition, and benchmark
migration remain explicit later slices of the parent RFC. Do not claim the
parent RFC complete after the Python pilot. No computer-server removal, facade
deprecation, pool migration, image-default change, or general execution service
rewrite is included.

## Acceptance tests

The shared typed scenario must run against an owned in-process runtime and the
advertised remote clients. Frozen API fixtures verify signatures and canonical
input/result records; transport mocks alone do not prove desktop behavior.

Required deterministic checks:

- Code generation and binding drift, type checking, serialization round trips,
  and complete dispatch coverage of the advertised operations.
- Protocol/version mismatch, absent service, unknown operation, malformed or
  oversized payload, invalid response identity, and sanitized failures.
- Expired/revoked authority, wrong claim, replacement guest, stale target/session
  handles, and unsupported host-only calls fail before effectful dispatch.
- Known versus unknown completion, cancellation before/during action, deadline
  expiry, duplicate request identity, and no automatic input replay.
- Closing one client does not close another, stop the runtime, release the claim,
  or break existing Sandbox/server operations. Runtime-owned shutdown remains
  independently covered.
- No credentials, user content, screenshot data, raw request bodies, or guest
  identity values in logs or error text exposed as generic diagnostics.

Required live candidate checks:

1. Record source SHA, generated/native package versions, guest image digest,
   runtime/display identity, and declared capabilities.
2. Establish that the typed client observes the intended guest, not the host.
3. Prepare synthetic input with guest execution/files; perform the meaningful
   edit through Driver UI; independently verify the saved result.
4. Verify foreground preservation and a negative control only where background
   behavior is advertised.
5. Exercise representative existing screenshot/input, shell, file, and PTY
   calls while direct Driver is available and after its client closes.
6. Independently verify task-owned session/claim/resource cleanup; preserve
   unrelated resources and document any remaining allocation.
7. Repeat the shared contract on the selected local guest before claiming local
   support. Run affected canonical desktop lanes on the stable candidate.

Account, target image, spend, test actions, and cleanup require the applicable
live-test authorization. A mock, published image, or successful MCP tool call
is not typed Fleet SDK end-to-end proof.

## Decisions still required before implementation

Record the decision on the existing RFC issue, without presenting the absence
of review as acceptance:

1. Accept this Python/Fleet-first scope under RFC 2512 while retaining its
   generated typed contract and leaving broader parity work outstanding.
2. Select the guest receiver's authenticated ingress and immutable generation
   proof, including revocation and caller-to-session binding.
3. Confirm the owning package and generated Python construction mechanism, the
   bounded remote operation set, and unsupported host/path operations.
4. Confirm the local candidate and exact Fleet image qualification/packaging
   scope. Existing MCP-only artifacts do not expose the proposed receiver.

Rollback remains additive: keep the previous package/image pinned and use the
unchanged Sandbox/server route. Close only task-owned Driver resources. Do not
delete pools, retire services, publish images, or merge this stack automatically.
