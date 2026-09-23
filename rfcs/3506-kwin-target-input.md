---
title: Target-addressable KWin input delivery for KDE/Wayland
authors:
  - netbospl
created: 2026-09-01
last_updated: 2026-09-23
status: review
discussion: https://github.com/trycua/cua/issues/3506
rfc_pr: https://github.com/trycua/cua/pull/3507
implementation:
supersedes:
superseded_by:
---

# RFC: Target-addressable KWin input delivery for KDE/Wayland

## Summary

Add a trusted KWin-side target-input capability that lets Cua Driver bind
pointer and keyboard delivery to one freshly verified KDE/KWin window while
preserving the Driver's existing per-action permission, policy, resource, and
lifecycle admission. When exact delivery, target identity, policy admission, or
transport ownership cannot be proven, the driver must return a structured
refusal and must not fall back to focus-bound global portal/libei input.

This RFC is intentionally gated on two proofs before product implementation:

1. Plasma 6/KWin must expose a supported integration point that can bind the
   mutation itself to an exact target rather than merely activate a window; and
2. the mutation path must be owned by the Driver integration so cached or
   ambient compositor transport cannot bypass normal per-action Driver policy.

Per the accepted Hyprland compositor-input design in #3550/#3551, this RFC does
not require a second KWin-specific approval mode, per-window consent prompt, or
standalone signer. Same-desktop-account transport follows the project's
trusted-local model and is not claimed to sandbox hostile same-user native code.

If the target-binding or Driver-owned policy path cannot be established with
supported KWin/desktop APIs, KDE raw target-addressed input remains refused and
the implementation does not proceed.

## Motivation

Cua Driver already has partial KDE/KWin integration for window discovery and
identity. The current in-process KWin effect exports a read-only Cua-owned D-Bus
service. It reports helper-issued opaque tokens associated with live KWin
windows, PID, geometry, active/minimized state, and stacking order. The Rust
adapter verifies that the service owner is the running `kwin_wayland` process,
checks same-user ownership, requires protocol version 1, and correlates KWin
identity with AT-SPI.

The remaining delivery problem is separate from discovery. XDG
Desktop Portal/libei input is focus-bound: the event reaches whichever surface
is focused when KWin processes it. An implementation that activates a target,
checks focus, and then emits global libei input has a TOCTOU race. Focus may
change after verification but before an irreversible click or key is processed;
a later read-back cannot undo an event delivered to the wrong application.

There is also a policy-path problem. The compositor-specific mutation transport
must not become an alternate path that skips the Driver's normal per-action
permission, manifest, managed/user policy, resource, and lifecycle checks.
Verifying that the helper belongs to KWin proves server identity; it does not
prove that a mutation request passed through Driver admission.

KDE therefore needs both target binding and a Driver-owned mutation path. The
safety claim is stronger than "the intended window was focused shortly before
input": the compositor-side path must associate each accepted event with the
exact verified target and current helper/KWin generation, while the Driver
performs normal action admission before dispatch. Under the accepted trusted-
local desktop-account model, this is not a promise to isolate against arbitrary
hostile native code running as the same desktop user.

## Goals

- Define a KWin target-input contract that binds a short-lived input transaction
  to one exact verified window identity.
- Preserve and extend the current trusted helper-owner, PID, UID, window-token,
  and AT-SPI correlation checks.
- Preserve normal Cua Driver per-action admission, including `standard`,
  `bounded`, and explicitly acknowledged `unrestricted` modes plus applicable
  manifests and managed/user policy.
- Prevent compositor-specific mutation transport from bypassing Driver policy
  merely because a cached connection or session-bus endpoint exists.
- Route supported KDE pointer and keyboard mutations only through a proven
  target-bound contract.
- Guarantee that a selected KWin target never falls through to global
  portal/libei delivery when target-bound dispatch is unavailable or unsafe.
- Preserve read-only discovery compatibility for existing v1 drivers/helpers
  during capability rollout.
- Expose precise capability and refusal information through doctor/health
  reporting.
- Reuse the same KWin routing for existing-profile browser setup so browser
  setup cannot bypass the target-bound safety boundary.
- Prove failure isolation with tests that observe target state, policy outcomes,
  generation changes, and absence of leaked global input.

## Non-goals

- This RFC does not make generic Wayland background input safe.
- This RFC does not claim equivalent target-addressable semantics for GNOME,
  wlroots, X11, Windows, or macOS.
- This RFC does not enable raw background delivery to arbitrary unfocused
  Wayland surfaces when the compositor cannot prove exact targeting.
- This RFC does not replace semantic AT-SPI actions where those actions already
  provide a stronger target-scoped operation.
- This RFC does not treat focus restoration alone as proof that input was
  delivered safely.
- This RFC does not approve private or unstable KWin internals merely because a
  prototype can call them.
- This RFC does not introduce a second KWin-specific permission mode, mandatory
  per-window approval UI, signer, or same-user sandbox beyond the existing
  Driver policy and trusted-local threat model.

## Terminology

**KWin target token**
: A helper-issued opaque token associated with a live KWin window identity. In
  the current helper it is allocated from the window's KWin `internalId()` and
  is valid only within the lifetime/generation in which it was issued.

**Helper generation**
: An identity that changes whenever a token or mutation transport binding from
  an earlier helper/KWin instance could become stale. A captured D-Bus unique
  service owner may be part of this proof, or the protocol may expose an
  explicit epoch. The chosen representation must survive review and tests for
  helper/KWin restart and PID/token reuse.

**Action admission**
: Cua Driver's existing permission, manifest/policy, resource, and lifecycle
  decision, repeated for every action before backend mutation. It is not a
  compositor-specific per-window human approval grant.

**Mutation authority**
: The live Driver-integrated ability to invoke the KWin mutation transport for
  an admitted action. It is bound to the current transport/helper generation
  and target transaction. It prevents accidental or architectural bypass of the
  Driver path; it is not claimed to sandbox hostile same-user native code.

**Target-bound input**
: Input delivery for which the compositor-side contract associates the mutation
  with one exact verified target identity rather than with whichever surface
  happens to hold focus at processing time.

**Global input**
: Focus-bound input injection, including ordinary portal/libei delivery, where
  the event is not contractually associated with the Cua-selected target.

**Structured refusal**
: A typed failure returned before unsafe mutation when identity, policy
  admission, capability, generation, or target-delivery invariants cannot be
  proven.

## Current state

The repository contains `libs/cua-driver/kwin-target-helper`, an optional KWin 6
effect loaded in the `kwin_wayland` process, and the Rust adapter at
`platform-linux/src/wayland/kwin_helper.rs`.

The helper's current contract is deliberately small:

```text
D-Bus name:   org.cua.KWinTarget
Object path:  /org/cua/KWinTarget
Interface:    org.cua.KWinTarget
GetVersion() -> 1
GetWindows() -> JSON window snapshot
```

Important current properties are:

- protocol v1 is read-only;
- the helper creates opaque numeric tokens for live KWin window identities;
- the helper does not expose activation or input mutation methods;
- the Rust adapter verifies the helper D-Bus owner, KWin process identity and
  UID, and currently accepts the helper only when `GetVersion() == 1`;
- `available()` for KWin raw input remains false;
- the focused-window mutation wrapper refuses;
- portal/libei obtains RemoteDesktop portal authorization, but the resulting
  input is focus-bound rather than target-bound.

That strict `GetVersion() == 1` check matters for migration: a simple helper
upgrade from version 1 to version 2 would cause existing v1 drivers to reject
the helper entirely, including read-only discovery. A capability rollout must
therefore avoid describing such a bump as automatically additive.

## Proposal

### 1. Feasibility gate: supported KWin target-binding primitive

Before implementation changes product behavior, the RFC must identify and
validate a supported Plasma 6/KWin extension point that can associate an input
mutation with one exact `KWin::Window` (or an equivalent stable compositor
object) at delivery time.

The proof must demonstrate that the primitive is stronger than:

```text
activate(window) -> global input
```

A KWin API that only activates a window, changes focus, or emits compositor-wide
input does not satisfy this RFC. A private/unstable symbol that cannot be
supported across the documented KWin/Qt ABI policy also does not satisfy the
production contract without an explicit maintainer decision.

The feasibility spike should be reviewable evidence, not shipped capability. If
no supported target-binding primitive exists, the disposition is to keep raw KDE
input refused and, if useful, pursue an upstream KWin API rather than weaken the
safety invariant.

### 2. Policy/transport gate: do not create an ambient Driver bypass

The current read-only D-Bus service may remain discoverable on the session bus.
Adding mutation must not create a compositor endpoint that the normal Driver
path can use without repeating action admission, or that a cached connection can
treat as authority for later actions.

For every mutation, the Driver must apply its existing permission, manifest,
managed/user policy, resource, and lifecycle checks before dispatch. The
transport design must then bind the admitted operation to the current
helper/KWin generation and exact target transaction.

Consistent with #3550/#3551:

- `standard` and explicitly acknowledged `unrestricted` use the existing
  Driver rules;
- `bounded` requires the applicable approved manifest;
- applicable managed/user policy remains binding;
- compatibility qualification is not a second permission system; and
- the same-desktop-account transport is trusted-local, not a sandbox against
  arbitrary hostile native code running as that account.

A separate per-window approval panel, signer, or KWin-specific permission mode
is therefore not required merely because the backend is compositor-specific.
If maintainers choose an additional KWin desktop consent mechanism for platform
reasons, it must compose with rather than replace normal Driver admission.

The helper/transport must fail closed when the target, generation, requested
capability, or Driver admission is stale, denied, missing, or no longer valid
for the operation.

### 3. Contract ownership and compatibility

Keep the current service namespace unless review finds a reason to split the
mutation surface:

```text
D-Bus name:   org.cua.KWinTarget
Object path:  /org/cua/KWinTarget
Interface:    org.cua.KWinTarget
```

Because existing drivers require `GetVersion() == 1`, the preferred additive
shape is to treat the current value as a wire-compatibility major and negotiate
new behavior separately, for example with an optional method such as:

```text
GetVersion()      -> 1
GetCapabilities() -> named/typed capabilities
```

A v1 driver would continue to call only `GetVersion()` and `GetWindows()` and
would therefore retain read-only discovery. A new driver talking to an old v1
helper would observe no target-input capabilities and keep raw input refused.

The final capability encoding may differ, but it must distinguish at least:

- identity/discovery;
- target activation, if separately meaningful;
- target-bound pointer input;
- target-bound keyboard input.

If implementation needs a wire-incompatible protocol change, it must use an
explicit migration strategy such as a parallel interface/path or dual-version
support. A bare `GetVersion(): 1 -> 2` change is not considered additive because
current v1 drivers reject non-1 helpers.

A helper that only implements the current `GetVersion()` and `GetWindows()` must
never be reported as mutation-capable.

### 4. Target identity and generation

A target-input transaction is opened only from a fresh snapshot that resolves
exactly one target using a tuple equivalent to:

```text
(pid, kwin_token, helper_generation)
```

The exact wire representation is implementation-defined, but these properties
are required:

- the token is opaque to callers;
- PID ownership is verified and cannot be substituted by title/app-id lookup;
- the generation changes whenever an old token could become stale after helper
  reload, KWin restart, or other identity reset;
- the mutation transport binding is tied to the compatible live generation;
- the target is revalidated immediately before each irreversible mutation or
  input frame;
- duplicate or ambiguous identities refuse rather than selecting a best match.

The existing helper-owner/session/UID checks remain mandatory. The implementation
must not assume the monotonic numeric token alone is globally unique or durable.

### 5. Target-input transaction

The Rust adapter gains a target-input transaction abstraction conceptually like:

```rust
with_target_input(pid, token, generation, admission, |target| {
    // bounded pointer/keyboard operations
})
```

The concrete API does not need to match this signature. Opening the transaction
must:

1. run normal Driver action admission for the requested operation;
2. obtain a fresh KWin snapshot;
3. resolve exactly one verified target;
4. validate the live helper/KWin generation;
5. negotiate the required target-input capability;
6. bind the operation to the current mutation transport and target identity; and
7. reject stale, ambiguous, missing, policy-denied, or unsupported targets
   before mutation.

During the transaction, the KWin-side path must ensure that delivery remains
associated with the bound target. If the invariant cannot be maintained across
multi-frame operations such as drag or type sequences, the transaction stops and
returns a structured refusal before the next frame.

The implementation may internally activate the target when required by KWin,
but activation is not the safety guarantee. The guarantee is that each accepted
event is associated with the bound target and current generation at delivery
time, after the Driver has admitted the action, or the operation is refused.

### 6. Supported operations

Only operations proven to satisfy the same Driver-admission and target-binding
contract may be enabled:

- click / pointer button actions;
- pointer movement required by click and drag;
- scroll;
- drag;
- text typing;
- individual key presses;
- hotkeys.

Capability negotiation may expose pointer and keyboard support separately.
Operations that cannot satisfy the invariant remain structured refusals even if
another KWin/global mutation mechanism exists.

### 7. No-global-fallback rule

The key delivery rule is:

> After cua-driver selects the trusted KWin target route for an operation, any
> inability to prove or maintain Driver admission, generation validity, and
> target-bound delivery MUST return a structured refusal. The operation MUST NOT
> fall back to global portal/libei input.

This includes policy denial, capability loss, helper restart, target closure,
generation change, ambiguous identity, unsupported input kinds, or any
focus/user-interaction transition that the target-bound primitive cannot handle
safely.

Representative refusal categories include `target_input_unavailable`,
`target_identity_stale`, and an existing/common policy-denial result; final
names should follow the current typed driver error taxonomy.

### 8. libei relationship

The existing global `libei.rs` worker carries pointer/keyboard commands but no
KWin target identity. EIS focus-bound delivery does not by itself establish
exact-target association.

Portal/libei remains valid for compositors and operations where its documented
semantics match the accepted contract. It may also remain valid for explicitly
global operations outside this RFC.

If the accepted KDE design retains libei/EIS, the implementation must prove that
KWin binds the admitted operation to the exact target *before delivery*.
Pre/post focus checks and post-event read-back are insufficient. Any portal or
desktop authorization used by that lower-level transport is additive platform
plumbing, not a replacement for normal Driver action admission.

### 9. Browser existing-profile setup

The browser setup path has compositor-specific foreground routing that can differ
from the general Wayland path. KDE existing-profile setup must use the same
accepted KWin policy/target transaction, directly or through common
Wayland routing, rather than bypassing it through a GNOME-oriented helper or a
global input fallback.

This keeps browser approval, exact target identity, generation, reconnect, and
mutation under one safety boundary. Related broader browser work is tracked in
#2283.

### 10. Doctor and health reporting

Health output must distinguish discovery, policy/transport readiness, and
mutation capability. Representative states are:

```text
KWin identity adapter: available
KWin target input: unavailable (helper is read-only)
```

```text
KWin identity adapter: available
KWin target input: blocked (Driver policy denied or mutation transport unavailable)
```

and, only after all gates pass:

```text
KWin identity adapter: available
KWin target input: pointer and keyboard target-bound delivery available
Wayland backend: KDE/KWin target-addressable foreground dispatch available
```

Doctor should distinguish at least:

1. helper absent;
2. helper present but read-only;
3. incompatible wire version;
4. helper missing required target-input capability;
5. Driver policy denial or mutation-transport unavailability;
6. target identity ambiguous or stale;
7. helper/KWin generation mismatch; and
8. target-bound transaction unavailable for the requested operation.

The platform support documentation remains experimental until the live acceptance
evidence in this RFC is recorded.

## Alternatives considered

### Activate then use global portal/libei

Rejected. Focus can change after activation or verification and before
compositor-side delivery. Post-event checks cannot undo an event delivered to
another application.

### Recheck focus before every libei command or add sleeps

Rejected. This narrows a timing window but does not remove it.

### Expose target mutation directly on the session bus to same-UID callers

Rejected as the default integration shape. Same-UID transport is acceptable
inside the project's trusted-local desktop-account threat model, but a raw
ambient mutation endpoint would not prove that a request passed through normal
Driver action admission or belongs to the current Driver-controlled operation.
The transport should preserve Driver ownership and generation/target binding
without claiming isolation from arbitrary hostile same-user native code.

### Trust D-Bus sender PID, process name, or executable path

Rejected as a substitute for Driver action admission. These may contribute to
server/client identity, lifecycle checks, diagnostics, or defense in depth, but
they do not replace the existing permission/manifest/policy decision for each
action.

### Increment `GetVersion()` from 1 to 2 and call the rollout additive

Rejected without a compatibility layer. Current v1 drivers require exact version
1 and would lose even read-only discovery when presented with version 2.

### Use title, app-id, or geometry matching

Rejected for target identity. These values are not unique stable target
identities and may change or collide.

### `wmctrl` / `xdotool`

Rejected for native KWin/Wayland target delivery. These X11-oriented mechanisms
do not establish a native Wayland target-bound input contract.

### Enable the existing adapter by setting `available() = true`

Rejected while the mutation body remains global/focus-bound or Driver admission
and transport ownership are unproven. Capability reporting must derive from the
actual accepted contract.

### Focus transaction with read-back after each event

Insufficient without compositor-side target binding. It can detect some races
after the fact but cannot retract an event already delivered to the wrong
surface.

## Compatibility and migration

The preferred rollout preserves the current wire-compatible discovery surface:

- old driver + new helper: `GetVersion() == 1` and `GetWindows()` continue to
  work; the old driver ignores optional capabilities/methods;
- new driver + old helper: discovery works, target-input capability is absent,
  and raw KDE input remains refused;
- new driver + new helper: target input is enabled only after capability,
  Driver admission, generation, and target checks pass.

If a wire-incompatible change becomes necessary, the RFC must be updated with a
parallel/dual-version migration before implementation. The design must not
silently trade away existing read-only discovery compatibility.

No unsafe fallback is introduced during rollout. Mismatch, missing capability, policy denial, or invalid transport/generation
produces a precise refusal rather than degraded global input.

Rollback is straightforward only if discovery remains separable from mutation:
disable/remove the new mutation capability and the driver returns to the current
read-only KWin posture.

The first user-visible implementation should use a `feat(cua-driver): ...` pull
request title because safe target-addressable KDE raw input is a new capability.

## Security, privacy, and telemetry

The KWin helper runs inside the compositor process and is therefore part of a
privileged desktop trust boundary. Mutating its API is materially more sensitive
than the current read-only snapshot surface.

Required properties:

- verify the D-Bus service owner and expected KWin session process;
- verify same-user/session ownership as server-identity evidence;
- run normal Driver permission/manifest/policy/resource/lifecycle admission for
  every mutation action, including actions using cached compositor connections;
- do not treat D-Bus sender PID, process name/path, token possession, or cached
  transport state as a substitute for that per-action Driver decision;
- follow the accepted trusted-local model: do not claim the transport is a
  sandbox against arbitrary hostile native code running as the desktop account;
- do not trust caller-provided titles, app IDs, geometry, or PID without a fresh
  compositor snapshot;
- bind mutation to an opaque live target plus helper/KWin generation and the
  current admitted operation;
- invalidate transactions on helper/KWin restart, stale identity, policy/lifecycle
  invalidation, or capability loss;
- refuse ambiguous target resolution;
- never send global raw input as a recovery path after target routing is chosen;
- avoid telemetry containing typed text, key sequences, window titles, document
  contents, target application data, credentials, or private transport material.

Permitted telemetry should be limited to capability/policy state, wire/capability
version, structured refusal category, operation class, and coarse timing/error
counters that cannot reconstruct user input.

## Implementation plan

Implementation begins only after this RFC is accepted according to the Cua RFC
process.

### Increment 0: feasibility and transport-ownership spike

Before production routing changes:

- identify the supported KWin API that can bind delivery to an exact target;
- demonstrate a positive target-binding canary with two competing windows;
- define and prototype the Driver-owned mutation transport so every action still
  passes through the common Driver admission path;
- document the trusted-local same-account threat model, ABI/support constraints,
  lifecycle invalidation, and rollback; and
- return to RFC review if the spike requires private KWin internals, weakens the
  common Driver policy contract, or changes the public security boundary beyond
  this proposal.

No raw KDE input support is advertised from this spike alone.

### Increment 1: compatible capability and target identity contract

- preserve v1 read-only discovery for existing clients;
- add compatible capability negotiation or an explicitly reviewed dual-version
  interface;
- add generation-aware target validation;
- bind mutation transport state to the live helper/target generation;
- add the Rust target transaction abstraction;
- add contract/unit tests for stale, duplicate, ambiguous, policy-denied and
  restarted targets.

### Increment 2: pointer/keyboard routing and no-fallback enforcement

- route only proven KDE pointer/keyboard operations through the admitted
  target transaction;
- hard-block transition from selected KWin routing to global libei;
- add structured refusal coverage for mid-transaction target, generation,
  capability, or policy/lifecycle changes;
- verify multi-frame drag/type cancellation behavior.

### Increment 3: browser setup and health reporting

- route existing-profile browser setup through the common accepted KWin path;
- distinguish identity-only, policy/transport readiness, and target-input
  capability in doctor/health output;
- add compatibility/version diagnostics.

### Increment 4: live Plasma 6 evidence and documentation

- run the compositor-specific Linux desktop harness on the exact candidate SHA;
- test representative Chromium, Firefox, GTK, Qt and Electron targets where the
  harness supports them;
- prove pointer, keyboard, drag, scroll, target closure, focus takeover,
  policy/lifecycle invalidation, and focus restoration behavior;
- update platform support/roadmap/action-support documentation only to the level
  demonstrated by evidence.

These increments may be separate PRs when that keeps review focused. Each
implementation PR must link this RFC and issue #3506.

## Test and acceptance plan

### Contract and compatibility tests

Cover at least:

- old-driver/new-helper read-only discovery compatibility;
- new-driver/old-helper read-only discovery and target-input refusal;
- capability negotiation and malformed/unknown capabilities;
- snapshot parsing and duplicate tokens;
- two windows owned by one process;
- minimized/hidden targets where relevant;
- ambiguous AT-SPI correlation;
- stale token after window close;
- helper/KWin generation change;
- browser restart and PID reuse;
- workspace and geometry changes;
- target loss before mutation;
- unsupported operation capability.

### Policy and transport-ownership tests

Explicitly prove:

- every mutating Driver action is admitted through the normal permission,
  manifest/policy, resource, and lifecycle path, including cached connections;
- a policy-denied action reaches no KWin mutation dispatch;
- stale transport state from a previous helper/KWin generation cannot be replayed;
- capability/transport material is not exposed in logs, health output, or
  telemetry;
- standard, bounded, and acknowledged unrestricted modes preserve their existing
  semantics; and
- the test plan does not claim same-user hostile-code isolation beyond the
  accepted trusted-local threat model.

Where the chosen design interacts with sandboxed applications or portal policy,
include a representative isolation/compatibility test supported by the
repository harness.

### Target-safety tests

Explicitly trigger:

- user focus change immediately before delivery;
- focus change between drag frames;
- close target during text typing;
- helper/KWin restart during a transaction;
- target replacement with possible PID reuse;
- another window of the same process becoming active;
- policy/lifecycle invalidation or generation loss during a multi-frame operation.

For every unsafe case, acceptance requires:

```text
structured refusal
no event delivered to a non-target window
no fallback global input sent
```

A successful driver response alone is not evidence.

### Live Plasma 6 evidence

At minimum, prove one exact target receives pointer and keyboard input while a
sentinel/second window proves no leaked input. The positive canary must also
show that deliberately selecting the second target changes only the second
target; this demonstrates target addressability rather than coincidental focus.

Expand the stable candidate matrix to representative Chromium/Chrome, Firefox,
GTK, Qt, Electron, two-window, covered, and alternate-workspace scenarios as
supported by the harness. Evidence must observe fixture-owned state and relevant
focus/z-order/no-leak/policy and generation oracles. Focus restoration after bounded
foreground operations must be verified when activation is part of the accepted
implementation.

The expensive desktop matrix should run only after the implementation is stable,
consistent with repository agent guidance, and must record the exact candidate
SHA.

## Related work

- #2283 tracks exact existing-profile browser setup across Wayland compositors.
- #2194 tracks trustworthy Wayland cursor-preservation evidence.

These are related constraints/evidence streams, not substitutes for this RFC's
policy-path and exact-target decision.

## Unresolved questions

- Which supported KWin extension/plugin API, if any, can implement exact
  target-bound delivery on Plasma 6 without relying on private unstable
  internals?
- Which mutation transport/interface best preserves per-action Driver policy
  while avoiding an ambient architectural bypass: compatible methods on the v1
  discovery surface, a parallel D-Bus interface/path, or another Driver-owned
  channel?
- Can EIS/libei be bound to an exact KWin target before delivery, or does KDE
  require a different supported primitive?
- Should compatible capability negotiation stay on wire version 1, or should a
  parallel interface/path carry a future incompatible major version?
- What exact generation source most reliably prevents stale target tokens and
  mutation transport state across helper/KWin/browser restarts?
- Should the mutation API expose a bounded transaction or atomic operation calls
  so partial multi-frame delivery and cancellation semantics are unambiguous?
- Can target-bound delivery preserve user foreground posture without activating
  the target, or is bounded activation/restoration required for some event
  classes?
- Which refusal/error names best align with the current typed Driver error
  contract and common policy-denial results?
- Which minimum live application matrix is required before documentation may
  advance KDE from experimental identity support to target-input support?

## Decision record

Pending maintainer review. The accepted shared-policy and trusted-local baseline
from #3550/#3551 applies to this RFC unless maintainers record a KWin-specific
exception. The decision summary in issue #3506 must record the chosen KWin
primitive, mutation transport/ownership model, compatibility strategy,
generation semantics, rejected alternatives, remaining risks, and final
disposition before implementation begins.

