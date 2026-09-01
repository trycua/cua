---
title: Target-addressable KWin input delivery for KDE/Wayland
authors:
  - netbospl
created: 2026-09-01
last_updated: 2026-09-01
status: review
discussion: https://github.com/trycua/cua/issues/3506
rfc_pr:
implementation:
  - https://github.com/trycua/cua/issues/2283
supersedes:
superseded_by:
---

# RFC: Target-addressable KWin input delivery for KDE/Wayland

## Summary

Add a trusted, versioned KWin-side target-input contract that lets Cua Driver
bind pointer and keyboard delivery to one freshly verified KDE/KWin window.
When exact delivery cannot be proven, the driver must return a structured
refusal and must not fall back to focus-bound global portal/libei input.

## Motivation

Cua Driver already has partial KDE/KWin integration for window discovery and
identity. The current helper exports trusted KWin window metadata and lets the
driver correlate compositor identity with PID, geometry, activity/stacking, and
AT-SPI. It intentionally does not expose mutating input operations.

The remaining problem is not discovery but delivery. XDG portal/libei input is
bound to the surface that is focused when the compositor processes the event,
not to a Cua-selected KWin window token. An implementation that activates a
window, verifies focus, and then sends global libei input has a race: focus can
change between verification and irreversible delivery. A later read-back cannot
undo a click or key event that reached the wrong application.

KDE therefore needs a target-addressable mutation path whose contract is owned
at the KWin boundary. The safety requirement is stronger than "the intended
window was focused shortly before input"; the compositor-side path must either
associate the event with the exact verified target or refuse it.

## Goals

- Define a versioned KWin target-input contract that binds a short-lived input
  transaction to one exact verified window identity.
- Preserve and extend the current trusted identity checks rather than replacing
  them with title, app-id, geometry, or focus heuristics.
- Route supported KDE pointer and keyboard mutations through that target-bound
  contract.
- Guarantee that a selected KWin target never falls through to global
  portal/libei delivery when target-bound dispatch is unavailable or becomes
  unsafe.
- Expose precise capability and refusal information through doctor/health
  reporting.
- Reuse the same KWin routing for existing-profile browser setup so browser
  setup cannot bypass the target-bound safety boundary.
- Prove failure isolation with tests that observe both target state and absence
  of leaked global input.

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

## Terminology

**KWin target token**
: The compositor-issued opaque identity currently used by the Cua KWin helper
  to distinguish one KWin window from another.

**Target generation**
: A process/helper/compositor generation value that prevents stale tokens or PID
  reuse from being accepted after restart or recreation.

**Target-bound input**
: Input delivery for which the KWin-side contract associates the mutation with
  one exact verified target identity rather than with whichever surface happens
  to hold focus at processing time.

**Global input**
: Focus-bound input injection, including portal/libei delivery, where the event
  is not cryptographically or compositor-contractually associated with the
  Cua-selected target.

**Structured refusal**
: A typed failure returned before unsafe mutation when identity, capability, or
  transaction invariants cannot be proven.

## Current state

The repository already contains a KWin target helper and Rust adapter. Protocol
v1 is intentionally read-only: it exposes a version and window snapshots but no
activation or raw input mutation methods. The Rust adapter verifies the D-Bus
owner, KWin PID/session ownership, UID, protocol version, and correlates KWin
window identity with AT-SPI.

The current safety posture is deliberate:

- the KWin helper can enumerate trusted windows;
- `available()` for mutating KWin foreground input remains false;
- the focused-window mutation wrapper refuses;
- raw KWin pointer and keyboard actions remain disabled because portal/libei is
  focus-bound rather than target-bound.

This is preferable to activating a target and then sending global input because
focus can change between any check and compositor-side event processing.

## Proposal

### Contract ownership

Extend the existing KWin helper contract under the current service namespace:

```text
D-Bus name:   org.cua.KWinTarget
Object path:  /org/cua/KWinTarget
Interface:    org.cua.KWinTarget
```

The helper remains the authority for compositor window identity. Protocol v2
(or a later compatible version selected during review) adds target-input
capabilities while retaining v1 read-only discovery semantics for older clients.

The public contract must expose enough information for the driver to distinguish
at least these capabilities:

- identity/discovery available;
- target activation available;
- target-bound pointer input available;
- target-bound keyboard input available.

A helper that only implements `GetVersion()` and `GetWindows()` must never be
reported as mutation-capable.

### Target identity

A target-input transaction is opened only from a fresh snapshot that resolves
exactly one target using a tuple equivalent to:

```text
(pid, kwin_token, generation)
```

The exact wire representation is implementation-defined, but the following
properties are required:

- the token is opaque to callers;
- PID ownership is verified and cannot be substituted by title/app-id lookup;
- the generation changes when token/PID reuse could make an old identity stale;
- the target is revalidated immediately before each irreversible mutation or
  input frame;
- duplicate or ambiguous identities refuse rather than selecting a best match.

Existing owner/session/UID checks remain mandatory.

### Transaction model

The Rust adapter gains a target-input transaction abstraction conceptually like:

```rust
with_target_input(pid, token, generation, |target| {
    // bounded pointer/keyboard operations
})
```

Opening the transaction must:

1. obtain a fresh KWin snapshot;
2. resolve exactly one verified target;
3. negotiate the required target-input capability;
4. bind the transaction to that identity;
5. reject stale, ambiguous, missing, or unsupported targets before mutation.

During the transaction the KWin-side path must ensure that delivery remains
associated with the bound target. If the invariant cannot be maintained across
multi-frame operations such as drag or type sequences, the transaction must
stop and return a structured refusal before the next frame.

The implementation may internally activate the target when required by KWin,
but activation is not itself the safety guarantee. The safety guarantee is that
the compositor-side input path associates each delivered event with the bound
target identity or refuses the event.

### Supported operations

Once the contract proves target-bound delivery, KDE routing may use it for:

- click / pointer button actions;
- pointer movement required by click and drag;
- scroll;
- drag;
- text typing;
- individual key presses;
- hotkeys.

Operations that cannot satisfy the same target-binding invariant remain
structured refusals even if some other KWin mutation path is available.

### No-global-fallback rule

The key safety rule is:

> After cua-driver selects the trusted KWin target route for an operation, any
> inability to prove or maintain target-bound delivery MUST return a structured
> refusal. The operation MUST NOT fall back to global portal/libei input.

This includes capability loss, helper restart, target closure, generation
change, ambiguous identity, user focus takeover where the contract cannot
preserve target binding, and unsupported input kinds.

A representative refusal code is `target_input_unavailable`; final typed error
names should follow the existing driver error taxonomy.

### libei relationship

The preferred design does not route KWin target-bound mutation through the
existing global `libei.rs` worker because those commands do not carry a window
identity today and because EIS focus-bound delivery does not by itself provide
target association.

Portal/libei remains valid for other compositors where its documented semantics
match the accepted operation contract. It may also remain available for
non-targeted or explicitly global operations that are outside this RFC.

If an implementation later proposes using libei inside the KWin transaction,
it must first provide compositor-level evidence that every event is target-bound
before delivery. Pre/post focus checks alone are insufficient.

### Browser existing-profile setup

The browser setup path currently has compositor-specific foreground routing that
can differ from the general Wayland routing. KDE existing-profile setup must use
the same KWin target transaction (directly or via the common Wayland target
routing) rather than bypassing it through a GNOME-oriented shell helper or global
input fallback.

This keeps browser approval, exact target identity, generation, reconnect, and
mutation under one safety boundary.

### Doctor and health reporting

Health output must distinguish identity from mutation capability. Representative
states are:

```text
KWin identity adapter: available
KWin target input: unavailable (helper is read-only)
```

and, once v2 is present and verified:

```text
KWin identity adapter: available
KWin target input: pointer and keyboard target-bound delivery available
Wayland backend: KDE/KWin target-addressable foreground dispatch available
```

Doctor should distinguish at least:

1. helper absent;
2. helper present but read-only;
3. incompatible protocol version;
4. helper missing required target-input methods/capabilities;
5. target identity ambiguous or stale;
6. target-bound transaction unavailable for the requested operation.

The platform support documentation remains experimental until the full live
acceptance evidence described below is recorded.

### Control flow

The intended flow is:

```text
cua-driver
    |
    | fresh (PID, token, generation) validation
    v
KWin target-input transaction
    |
    | compositor-bound delivery or refusal
    v
exact KWin Window
```

The disallowed flow is:

```text
activate target -> check focus -> global libei input
```

because no amount of pre-delivery focus checking makes a global irreversible
event target-addressable.

## Alternatives considered

### Activate then use global portal/libei

Rejected as the baseline design. Focus can change after activation or focus
verification and before compositor-side event delivery. Post-event checks cannot
undo an event delivered to another application.

### Recheck focus before every libei command or add sleeps

Rejected. This narrows a timing window but does not remove it, and therefore
cannot support an exact-target safety claim.

### Use title, app-id, or geometry matching

Rejected for authorization. These values are not unique stable identities and
can collide or change. They may be diagnostic metadata but not the mutation
selector.

### `wmctrl` / `xdotool`

Rejected for native KWin/Wayland target delivery. These X11-oriented mechanisms
do not establish a native Wayland target-bound input contract.

### Enable the existing adapter by setting `available() = true`

Rejected while the mutation body remains global/focus-bound. Capability must be
reported from the actual target-input contract, not helper presence.

### Focus transaction with read-back after each event

Insufficient without compositor-side target binding. It can detect some races
after the fact but cannot retract an event already delivered to the wrong
surface.

## Compatibility and migration

The change is additive at the protocol level:

- existing v1 helpers remain valid for identity/discovery;
- existing clients continue to see KDE raw target input as unavailable;
- a v2-capable helper advertises target-input capability explicitly;
- the driver enables KDE mutation only after verifying both protocol/capability
  and target transaction invariants.

No unsafe fallback is introduced during rollout. If driver and helper versions
are mismatched, the behavior is a precise refusal rather than degraded global
input.

Rollback is straightforward: disable/remove the v2 mutation capability and the
driver returns to the current read-only KWin posture without changing window
identity discovery.

The first user-visible implementation should use a `feat(cua-driver): ...` pull
request title because safe target-addressable KDE raw input is a new capability.

## Security, privacy, and telemetry

The KWin helper is part of a privileged desktop trust boundary and must expose
only the minimum metadata needed for identity and delivery.

Required properties:

- verify the D-Bus service owner and expected KWin session process;
- verify same-user/session ownership;
- do not trust caller-provided titles, app IDs, geometry, or PID without a fresh
  compositor snapshot;
- bind mutation to an opaque compositor target plus generation;
- invalidate transactions on helper/compositor restart and stale identity;
- refuse ambiguous target resolution;
- never send global raw input as a recovery path after target routing is chosen;
- avoid telemetry containing typed text, key sequences, window titles, document
  contents, or target application data.

Permitted telemetry should be limited to capability state, protocol version,
structured refusal category, operation class, and coarse timing/error counters
that cannot reconstruct user input.

## Implementation plan

Implementation begins only after this RFC is accepted according to the Cua RFC
process.

### Increment 1: KWin target-input contract and adapter

- extend the KWin helper with a versioned target-input capability;
- add generation-aware target validation;
- add the Rust target transaction abstraction;
- retain read-only behavior for v1 helpers;
- add focused contract/unit tests for stale, duplicate, ambiguous and restarted
  targets.

### Increment 2: pointer/keyboard routing and no-fallback enforcement

- route supported KDE pointer/keyboard operations through the target transaction;
- hard-block transition from selected KWin routing to global libei;
- add structured refusal coverage for mid-transaction target changes;
- verify multi-frame drag/type cancellation behavior.

### Increment 3: browser setup and health reporting

- route existing-profile browser setup through the common KWin target path;
- distinguish identity-only and target-input capability in doctor/health output;
- add compatibility/version diagnostics.

### Increment 4: live Plasma 6 evidence and documentation

- run the canonical KDE/Wayland lane on the exact candidate SHA;
- test representative Chromium, Firefox, GTK, Qt and Electron targets where the
  existing harness supports them;
- prove pointer, keyboard, drag, scroll, target closure, focus takeover, and
  focus restoration behavior;
- update platform support/roadmap/action-support documentation only to the level
  demonstrated by the evidence.

These increments may be separate PRs when that keeps review focused. The first
implementation PR links this RFC and the decision issue.

## Test and acceptance plan

### Contract and unit tests

Cover at least:

- snapshot parsing and protocol negotiation;
- duplicate tokens;
- two windows owned by one process;
- minimized/hidden targets where relevant;
- ambiguous AT-SPI correlation;
- stale token after window close;
- helper/KWin generation change;
- browser restart and PID reuse;
- workspace and geometry changes;
- target loss before mutation;
- unsupported operation capability.

### Safety tests

Explicitly trigger:

- user focus change immediately before delivery;
- focus change between drag frames;
- close target during text typing;
- helper/KWin restart during a transaction;
- target replacement with possible PID reuse;
- another window of the same process becoming active.

For every unsafe case, acceptance requires both:

```text
structured refusal
no global input sent
```

A successful driver response alone is not evidence.

### Live Plasma 6 evidence

Use the repository's compositor-specific Linux desktop harness on the exact
candidate SHA. At minimum, prove one exact target receives pointer and keyboard
input while a sentinel/other window proves no leaked input. Expand the matrix to
representative Chromium/Chrome, Firefox, GTK, Qt, Electron, two-window, covered,
and alternate-workspace scenarios as supported by the harness.

Evidence must observe fixture-owned state and relevant focus/z-order/no-leak
oracles. Focus restoration after bounded foreground operations must be verified.

The complete expensive desktop matrix should run only once the implementation
is stable, consistent with repository agent guidance.

## Unresolved questions

- Which supported KWin extension/plugin API should implement the target-bound
  delivery primitive on Plasma 6 without relying on private unstable internals?
- Should protocol v2 expose coarse `BeginTargetInput` / `EndTargetInput`
  transactions with typed event methods, or one atomic method per operation?
- What exact generation source most reliably prevents stale token and PID reuse
  across KWin/helper/browser restarts?
- Can target-bound delivery preserve user foreground posture without activating
  the target, or is bounded activation/restoration required for some event
  classes?
- Which refusal/error names best align with the current typed driver error
  contract?
- Which minimum live application matrix is required before documentation may
  advance KDE from experimental identity support to target-input support?

## Decision record

Pending maintainer review. Record material feedback, accepted changes, rejected
alternatives, remaining risks, and final disposition in issue #3506 before
implementation begins.
