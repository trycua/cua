# Host authority implementation boundary

This is an implementation note for [RFC #3550](https://github.com/trycua/cua/issues/3550),
not an accepted wire specification. It does not enable input, replace the
experiment's external signer, or establish production support.

## Implemented shared primitive

`cua-driver-core::isolated_input_authority` compiles only for unit tests or the
nondefault `experimental-isolated-input-authority` feature. No tool, service,
SDK, or platform adapter calls it. The normal plugin remains discovery-only.

The trusted host retains a non-cloneable `InputControl`. Action-side
delegations and reservations hold weak references, so retaining them cannot
keep the control owner alive. Each authority binds one exact in-memory
`EffectiveAuthorizationContext`, compositor epoch, target generation, geometry
revision, operation set, deadline, and action quota. Matching public session
labels or even cloning the context's contents cannot transfer authority.

The candidate accepts only live trusted-host contexts. A process-level
unrestricted context is insufficient. The shared registry must perform final
policy and resource admission and call `commit_authorized_dispatch` before a
bridge reserves work; the primitive neither authorizes a tool nor refreshes
session idle lifetime. Policy hashes stay bound to the immutable session
context. Changing policy requires a new session, not an input grant that
overrides the old ceiling. A failed policy lookup is distinct from a known
absence of policy.

There is at most one pending or active reservation per authority. Admission
consumes quota even if canceled before delivery. Starting the same reservation
twice refuses. The primitive rechecks authority at start and every bounded
gesture step. Dropping a reservation clears only its own slot. Operator Stop,
host loss, session revocation, expiry, or changed target/policy permanently
invalidate that authority, including unused action handles. Fresh approval
must create a new authority; there is no agent-facing renewal method.

The 60-second TTL and 256-reservation cap are bounded candidate values for
tests and review, not accepted v3 limits. This primitive does not allocate
compositor lanes or qualify application versions and operations.

## Required bridge integration

The following work remains disabled and unimplemented:

1. Authenticate a live operator-host control connection and bind its private
   descriptors to the owning runtime/session. Same-UID access is a transport
   prerequisite, not approval. Do not replace signatures with a generic
   same-UID `APPROVE` command or expose a grant/renewal tool to the agent.
2. Obtain exact target and compatibility evidence through the compositor
   bridge, then present the operator with the qualified operations, duration,
   and background-only scope. Public addresses and labels are lookup inputs,
   not authority. Never select primary-seat input after refusal.
3. Create authority only through that trusted host path. Delegate a private
   connection-bound action descriptor only after the shared dispatch policy
   boundary; a protected-consent callback alone is insufficient because not
   every admitted input action invokes one.
4. Bind the reviewed compositor lease to the live control connection and
   action descriptor. Revalidate epoch, target, geometry, sequence, policy,
   scope, and revocation immediately before each event. A local successful
   check is not an atomic guarantee across remote dispatch.
5. On control EOF, Stop, or session revocation, close admission, invalidate
   pending and active descriptors, and close/revoke the affected compositor
   connections. The compositor cleans only synthetic state and reports any
   already-dispatched effects. Local cancellation cannot undo an app action.
6. Provide host-owned activity indication and Stop independent of agent cursor
   artwork. `InputActivity` reports local idle/pending/active/revoked state,
   not live compositor state or application success. Reconcile it with an
   authoritative compositor snapshot; missing observations must show unknown
   or unavailable state, not idle. Loss of the required control/indicator
   lifetime must revoke authority.

Existing reuse points are the final dispatch checks in core `tool.rs`, the
opaque trusted-host contexts in `session_authorization.rs`, SDK runtime/session
teardown, the service's authenticated embedded-host connection and detach
path, and the experimental Linux adapter's private connection cleanup. The
service's parent-PID check authenticates its embedding host, not the operator's
approval or a compositor delegation. After-call activity events are not a
live lease ledger.

The trust model excludes a malicious unrestricted same-user process. Neither
socket permissions, a PID check, an app list, nor an indicator makes that
process contained or the desktop unspoofable. A stronger claim requires an
enforceable OS/process boundary and a separate review.

## Verification and acceptance

From `libs/cua-driver/rust`, run the focused portable checks:

```bash
cargo test -p cua-driver-core isolated_input_authority --lib --locked
cargo check -p cua-driver-core --features experimental-isolated-input-authority --locked
```

These tests cover scope, exact context identity, target/policy drift, missing
observations, expiry, quota, pending/active state, Stop, host and session loss,
independent authorities, concurrent cancellation, and poisoned-state refusal.
They are not native delivery, host-control-channel, operator-UI, or desktop
certification evidence.

Before production input can be enabled, the RFC and wire review must settle
the control bootstrap/delegation and operator surface, qualified client and
layout scope, limits, and typed outcomes. The integrated candidate then needs
native fault/recovery and independent foreground-input evidence, canonical
affected-platform certification, package lifecycle validation, component
release verification, and fresh Fleet image validation. The external test
signer remains a test mechanism until that replacement is implemented and
accepted.
