---
title: Isolated background input on Hyprland
authors:
  - f-trycua
created: 2026-09-04
last_updated: 2026-09-06
status: accepted
discussion: https://github.com/trycua/cua/issues/3550
rfc_pr: https://github.com/trycua/cua/pull/3551
implementation:
  - https://github.com/trycua/cua/pull/3547
  - https://github.com/trycua/cua/pull/3557
  - https://github.com/trycua/cua/pull/3572
supersedes: null
superseded_by: null
---

# RFC: Isolated background input on Hyprland

## Summary

Add an optional, explicitly enabled Hyprland input integration that delivers
to an exact background target without borrowing the user's primary-seat
focus. Use two independent compositor-allocated synthetic input lanes, with
compositor-owned target identity, bounded complete actions, per-action Driver
policy admission, and separate transport, dispatch, and application-effect
results. Keep the portable Cua Driver contract unchanged where possible and
refuse unsupported clients or actions. This is a proposal for Cua, not an
accepted upstream Hyprland API.

The [maintainer-selected delivery scope](https://github.com/trycua/cua/issues/3550#issuecomment-5555206901)
authorizes the existing implementation workstreams toward released Driver
packages and a verified Omarchy Fleet image. The intended end state is
supported input for a qualified client/operation matrix.

The [scope correction in #3550](https://github.com/trycua/cua/issues/3550#issuecomment-5556178980)
selects implementation through Driver's existing permission and lifecycle
contract. Standard and acknowledged unrestricted modes without a manifest
run without additional prompts or per-window grants; bounded requires an
approved manifest. Applicable
manifests and user or managed policy remain binding. No separate approval
panel, signer, or indicator-lifetime approval condition is required.

The [maintainer decision](https://github.com/trycua/cua/issues/3550#issuecomment-5558699892)
accepts this narrowed design. Acceptance is not native certification or a
release. The production candidate must still prove its supported client/action
cells and recovery behavior before implementation merge and release.

## Motivation

An agent should be able to act in an application while a person continues
typing or dragging in another application on the same desktop. A private VM
isolates desktops from each other, but does not solve interference between
actors inside one desktop session.

Wayland has coordinates. What ordinary clients lack is a general
target-addressed raw-input contract for another unfocused surface. Geometry
reconstruction, a virtual pointer, or an extra cursor image does not establish
an independent input focus or keyboard state. AT-SPI can provide useful
semantic background actions, but those are not arbitrary raw input for
canvases, games, and other controls without suitable accessibility actions.

Omarchy uses Hyprland. The proposed integration is Hyprland-specific, not
Omarchy-only, and its behavior must be validated separately from Sway, Mutter,
KWin, and XWayland.

## Goals

- Deliver to a compositor-validated target or refuse before unsafe dispatch.
- Keep primary-seat pointer focus, keyboard focus, held state, cursor,
  workspace, stacking, and foreground gestures independent from agent input.
- Bind each input operation to fresh common policy admission, one private
  connection, one compositor epoch, and an exact target lifetime.
- Bound execution, queueing, replay state, and cancellation work so an agent
  cannot monopolize the compositor event loop.
- Preserve shared driver targeting, permission, and action-result semantics.
- Publish operation/client-specific evidence, including honest refusals.

## Non-goals

- Enable mutation in the existing discovery-only foundation by merging this
  document or changing a driver channel.
- Add a universal Wayland protocol, promise upstream adoption, or claim parity
  with other compositors.
- Train a model, redesign capture or AT-SPI, or add compositor administration
  to ordinary background actions.
- Unlock the session, wake displays, raise windows, switch workspaces, modify
  the clipboard, or fall back to primary-seat input to rescue a failed action.
- Support more than two concurrent synthetic input lanes in this release.
- Claim initial Chromium/Electron, XWayland, IME, popup/subsurface, or arbitrary
  display-layout support without separate qualification.
- Claim a sandbox against hostile native code running as the desktop account,
  a compromised compositor, or root.

## Terminology

| Term             | Meaning in this proposal                                                                                                      |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Primary seat     | The compositor input state used by the person's physical devices.                                                             |
| Agent seat       | An independent compositor-managed synthetic input identity, not a cursor overlay.                                             |
| Action admission | Driver's common policy/resource and lifecycle decision, repeated for every action before backend mutation.                    |
| Target binding   | A private connection's short-lived compositor binding to an exact target, operation, epoch, and geometry; not human approval. |
| Target token     | An opaque reference to a compositor-owned target lifetime; identity alone does not grant authority.                           |
| Compositor epoch | A fresh identity for a live plugin transport binding, invalidated on disable/re-enable or restart.                            |
| Dispatch         | The compositor emitted the specified events; not proof that the application accepted them.                                    |

## Current state

The [foundation PR #3547](https://github.com/trycua/cua/pull/3547), inspected at
`fc6d064d9287cb04a16152d056f87a809b8e3cf6`, adds an optional plugin with:

- exact Hyprland ABI checks and a matching compiler/runtime requirement;
- disabled-by-default, bounded same-UID `SOCK_SEQPACKET` transport;
- `hyprctl -j cua:status`, negotiation, and liveness;
- no target enumeration, issued target tokens, synthetic seat, or driver
  integration; and
- `background_unavailable` for every defined mutation message.

The [v2 protocol](https://github.com/trycua/cua/blob/fc6d064d9287cb04a16152d056f87a809b8e3cf6/libs/cua-driver/hyprland-plugin/protocol/cua-inject-v2.md)
authenticates a Unix UID, not a Cua process or operator approval. It explicitly
gates input on an authorization contract and second-seat evidence.

The [validation report](https://github.com/trycua/cua/blob/fc6d064d9287cb04a16152d056f87a809b8e3cf6/libs/cua-driver/hyprland-plugin/tests/validation.md)
records native build and repeated lifecycle tests on Hyprland `0.56.2-1`.
Its Fleet environment used Omarchy `4.0.1-1` and Cua Driver `0.22.2`.
The report identifies the tested source archive and subsequent non-executable
changes; it is not application-delivery or physical-input-isolation evidence.

The [input implementation #3572](https://github.com/trycua/cua/pull/3572)
contains historical two-lane Calc/Inkscape delivery, saved-output, foreground
input, and fault/recovery evidence at explicitly recorded source revisions.
Its `22d4c863576a300ba561b4783822b90ba173423a` checkpoint adds persistent
seats, keymap refresh, compositor-owned lane reservations, and synchronous
desktop-transition revocation. Ten host CTests and 82 Python checks passed;
that checkpoint has not been native-certified and does not inherit the old
recordings as proof. At that checkpoint, input was compile-time gated and
used a test-only external signer. The [observation dependency #3557](https://github.com/trycua/cua/pull/3557)
retains its separate certification gates.

The separate [driver integration PR #3052](https://github.com/trycua/cua/pull/3052)
reports capture and desktop-contract evidence on its own branch. Its passing
contracts include expected refusals. That work must follow its own integration
and review path; neither PR certifies the other or establishes a released
isolated-input capability.

The agent-seat direction builds on Dillon DuPont's (@ddupont808) prototype
lineage, already credited in #3547. Credit does not imply approval of this RFC.

Subsequent work is tracked in [observation #3557](https://github.com/trycua/cua/pull/3557)
and [input #3572](https://github.com/trycua/cua/pull/3572). The earlier
[Calc/Inkscape experiment](https://github.com/trycua/cua/blob/e71ed83f43e3d8cd2963c040b8d7d287bcbc9ba8/libs/cua-driver/hyprland-plugin/tests/realapp-validation.md)
used native source `571bbe356e41d531e13182363575d566b68b5e16` and an external
test signer. Its results belong to that source and experimental protocol 0;
they do not certify production v3. Discovery-image boot and observation
evidence likewise do not establish input support. Keep those historical
artifacts separate from the exact-SHA production qualification below.

### Related contributor proposals

Austin Dixson's (@austindixson)
[proposal #3552](https://github.com/trycua/cua/issues/3552) independently
proposes a permission-gated agent seat. The shared direction is to preserve
primary-seat state, keep direct-resource delivery an internal implementation
choice, and avoid treating a portal/libei connection as proof of isolation.
This RFC remains the decision record through #3550; the related proposal is
retained as a source of feedback, not marked accepted or superseded here.

The decision resolves three differences:

- A binary-path permission and an ASK policy are that proposal's entry point.
  This RFC uses Driver's shared admission and trusted desktop-account model;
  executable names and same-UID transport checks do not establish human consent.
- The reported `send_shortcut` keyboard results are client-specific evidence,
  not proof of modifier, repeat, grab, or concurrent foreground isolation.
  They do not authorize substituting primary-seat dispatch for the proposed
  agent-seat keyboard capability.
- InputCapture and RemoteDesktop have different roles. Receiving an EIS file
  descriptor does not establish input delivery or an isolated seat. A portable
  portal implementation must prove both separately.

The cursor-observer investigation in
[#2194](https://github.com/trycua/cua/issues/2194), including contributions by
@LikelyLucid and @austindixson, and
[draft PR #3553](https://github.com/trycua/cua/pull/3553) provide a separate
evidence workstream. The PR reports Hyprland cursor queries and live canaries;
those reports are not independently certified by this RFC. Review its overlap
with #3052 before integrating the observer. Cursor observation does not itself
implement input delivery.

## Proposal

### Ownership and integration boundary

The normal request path remains: agent tool call, shared Cua Driver policy and
target resolution, Linux adapter, optional plugin, selected application.

| Owner                       | Responsibility                                                                                                                                                       |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Shared driver               | Public tool/SDK/MCP schemas, exact window selection, policy/resource and lifecycle admission for every action, private runtime ownership, and action-result mapping. |
| Linux adapter               | Attested correlation from the public window target to a plugin target, screenshot-to-surface geometry conversion, negotiation, and disconnect handling.              |
| Hyprland plugin             | Live target identity and geometry, two independently owned lanes, technical target lifetime and conflict checks, bounded dispatch, and synthetic-state cleanup.      |
| Application fixture/harness | Independent evidence of target effects and absence of foreground side effects.                                                                                       |

The [shared permission stack](../libs/cua-driver/rust/crates/cua-driver-core/src/session_authorization.rs)
remains binding. Standard and unrestricted desktop
input without a manifest require no protected per-window grant. Unrestricted
retains its explicit launch acknowledgement. Bounded requires an approved
manifest; manifests, managed policy, user policy, and hard invariants apply in
every applicable mode. The common registry must admit each new action before
the adapter can mutate plugin state, including on a cached connection.

Carry only the required private operation, target, and lifecycle binding into
the adapter. A cached connection or target reference cannot independently
authorize the next action. Public JSON arguments, session labels, and
environment switches cannot supply that authority. Public lifecycle session
labels remain labels, not credentials, following
[RFC 3007](3007-cua-driver-lifecycle-sessions.md).

The explicitly enabled plugin trusts the desktop-account runtime. Same-UID
transport checks and matching-compositor attestation protect ownership and
prevent accidental misrouting; they neither authenticate human consent nor
contain hostile same-user native code. This route does not enable delegated
sessions or introduce a separate authorization service. Existing Driver
activity/cursor behavior, runtime stop, session termination, and teardown must
remain integrated with the backend's owner-scoped cleanup.

### Initial scope and capability negotiation

Provide two independent compositor-allocated lanes, each owned by one private
runtime connection, with one target and one executing action at a time per
lane. Two separate Driver processes must perform overlapping gestures without
sharing held state. A third claimant receives a typed busy/refusal outcome.
Discovery remains bounded and does not claim an input lane. Canceling one
owner must preserve the other lane and primary input.

Qualify complete click, key/chord, scroll, and bounded drag separately. The
initial candidate targets native LibreOffice Calc (`libreoffice-fresh`
`26.2.5-3`) and Inkscape (`1.4.4-6`) with the exact compiled default
`evdev`/`pc105`/`us` keymap. These are qualification candidates, not passed
support claims. Check application/package/version/operation eligibility before
connecting; unknown raw-input cases refuse. Layout names alone are insufficient:
variants, options, remaps, multiple groups, and keymap changes require refusal
unless separately qualified. Existing semantic routes retain their behavior.
GTK fixtures supply reproducible faults and oracles, not an application-support
claim. Eligibility uses host/compositor-observed live client identity and
qualified runtime details, not an app name supplied by the caller. This is a
compatibility gate, not authorization or a sandbox: a supported application
may itself run scripts, extensions, or subprocesses.

Do not route public raw key-down/button-down streams through this first
version. Complete click, bounded drag, and complete key
chord operations own their press/release lifecycle. Unsupported public tool
shapes retain an explicit refusal or their existing independently safe route.

### Target identity and coordinates

Cua Driver keeps its public exact `(pid, window_id)` selection and
window-screenshot pixel coordinates. No agent receives a raw compositor
address or has to reason about the wire token.

The plugin resolves an eligible live surface through compositor-owned state
and returns an opaque, unguessable, non-reused token scoped to the epoch and
private connection. A PID, title, application ID, geometry match, or Wayland
object number alone is insufficient identity. Ambiguity refuses. App names and titles may
help the operator recognize the target but are not authority checks.

Bind the target to its client/surface lifetime and a separate geometry
revision. Destruction or replacement invalidates identity even if a PID or
window number is reused. Geometry changes invalidate the old coordinate
revision, not silently retarget the action. Every later action requires fresh
Driver admission and `TARGET` checks, even if the connection remains open;
a title change alone does not create a new application.

Use surface-local logical coordinates at the plugin boundary, with a declared
origin, extent, and geometry revision. The driver converts from its actual
capture frame using attested crop, scale, transform, and surface geometry.
The compositor validates that revision again at dispatch. Reject invalid,
non-finite, out-of-bounds, or stale coordinates; do not clamp or reinterpret
them as desktop coordinates. A later new action needs a fresh capture after
staleness; this does not permit replay of a partially or possibly delivered
action.

Keep normal compositor hit-testing within the bound surface tree.
Popup, modal, and subsurface rules need explicit compatibility evidence;
until a tree/role is certified, refuse it rather than routing to the active
popup or another top-level surface. Off-workspace and occluded targets require
exact capture/identity evidence and must not trigger a workspace change.

### Independent seat and client behavior

Create agent pointer and keyboard resources independently from primary-seat
focus, grabs, serial bookkeeping, modifiers, repeat, and held buttons/keys.
Do not temporarily borrow and restore primary-seat focus.

Use a valid compositor keymap and explicit agent modifier/repeat state. Do not
inherit the person's held modifiers or trigger global compositor shortcuts.
Keymap replacement revokes old authority and updates independent state and
client resources; it must not disconnect applications or require replacing
their seats. Qualify layouts and physical modifier interactions separately.
Client-visible agent-seat focus is distinct from activating the desktop
window. Toolkit policies may still reject it; report those limits rather than
pretending synthetic events were accepted.

Text is a separate capability. Do not claim Unicode or IME correctness from
ASCII keycode tests, mutate the shared keymap, or use the global clipboard as
a hidden text-delivery fallback. Leave text disabled until its mechanism and
client coverage are agreed and proven.

For an active primary-seat grab on another application, supported delivery
must proceed without perturbing that grab. A blanket refusal whenever the
person interacts does not meet the goal. For simultaneous physical and agent
interaction with the same target, initially refuse or cancel the agent action
at the defined compositor serialization point. Do not claim arbitrary
same-application multi-user isolation.

### Scheduling, replay, and cancellation

Keep transport nonblocking and bound parsing work per compositor event-loop
iteration. No network, filesystem, or long-running work may block dispatch.
Validate lane ownership, epoch, target lifetime, operation, and geometry on
admission and again immediately before dispatch. Long operations use bounded event-loop steps,
never a blocking drag/typing loop. Publish and test maximum queue, payload,
action-duration, and cancellation-work limits before enabling any capability.

Order operations per connection and reject target-client conflicts, including
two lanes targeting the same Wayland client. Use a strictly increasing
operation sequence across fresh target selections. V3 refuses repeated or
lower sequences with `replay`; it never executes an operation twice or returns
a duplicate as a newly delivered action. Keep admission and replay state bounded.

Connection loss after dispatch creates an unknown outcome, not permission to
retry input. A later new action may reconnect or reacquire a target only after
fresh common policy/resource and lifecycle admission, identity, geometry,
compatibility, and conflict checks, plus new application observation. Never
automatically replay canceled, partial, or unknown delivery in any epoch.

Use Driver's existing stop and lifecycle paths for idempotent owner-scoped
cancellation. Revocation closes admission before queued work is removed.
Revalidate before each long-action
step; a step already dispatched cannot be undone. Track and release only
agent-owned held state, to its original live target where valid, and never
redirect cleanup events to the primary seat or a replacement window. If the
client disappeared, destroy agent-side state without addressing another client.

Cancel/revoke on expiry, runtime/session termination, disconnect, target
destruction, primary interaction with the target client, keymap change, lock,
DPMS-off, session switch, plugin disable/unload, or compositor restart.
Geometry change during a gesture aborts it with defined agent-state
cleanup; it does not silently recompute a new path. Cancellation reports
partial dispatch when events have already escaped; it cannot undo an
application's prior text insertion or click.

Observe desktop transitions synchronously, not only by comparing sampled
state. Lock/unlock or DPMS off/on between two timer ticks must still invalidate
old active and pending target bindings. Keep dispatch-time guards as defense
in depth. Revocation generations are monotonic, and exhaustion requires restart
rather than wrapping. Driver's existing status and stop paths must remain
usable after an action connection is revoked.

`CANCEL` and `STOP` on the private v3 transport affect only the owning lane;
an unclaimed control connection cannot cancel another runtime. They invalidate
the target and release synthetic state while retaining that lane reservation.
EOF also frees the departing owner's reservation. A later new action can
obtain a fresh `TARGET` after all admission checks, without a new per-window
approval requirement. Measure cleanup with a
responsive compositor and report stalls separately: Stop is neither undo nor
instantaneous cleanup under every fault.

Do not equate the private transport's cancellation messages with cancellation
on every public transport. Direct stdio MCP executes calls serially and ignores
cancellation notifications. An `end_session` call or input EOF waits behind the
active call; neither is an immediate-stop mechanism. SDK invocation cancellation
and termination of the owning Driver process are separate paths with separate
qualification. A SIGTERM timeout must not be reported as successful SIGTERM
cleanup merely because a subsequent SIGKILL worked.

### Results and public parity

Keep three facts separate: receipt by the transport, dispatch by the
compositor, and application effect established through independent readback.
An emitted-event count is dispatch evidence, not accepted-input evidence.

Map into the existing [action-result contract](../libs/cua-driver/docs/action-result-contract.md).
Use the shared `synthetic_events` route where applicable; do not add a
Hyprland-specific value to the public closed route enum. Dispatch alone is
`unverifiable`, not `confirmed`; partial operations retain acknowledged counts.
Refusal before dispatch carries no delivery/effect evidence. Any new public error or
capability field requires shared schemas and SDK/MCP parity tests.

Acknowledged progress does not prove that no later events landed. A missing
final acknowledgement or connection loss can leave additional delivery unknown;
retain that uncertainty separately from the known gesture progress.

Unsupported clients, denied authority, stale identities, and busy queues must
produce distinguishable typed outcomes. No result triggers an automatic
foreground retry. Plugin absence leaves existing semantic/background routes
and their honest refusals intact on Linux and does not alter other platforms.

### Protocol version

Use the distinct production v3 candidate for the input-bearing contract. Target
lifetime, replay, cancellation, and dispatch results change security and
operation semantics beyond discovery negotiation. Preserve v2 as
discovery-only and retain signed experimental protocol 0 as historical test
evidence. Reject incompatible modules and protocol 0 on the production route,
without downgrade.

The [v3 source candidate](https://github.com/trycua/cua/blob/feat/hyprland-isolated-input-spike/libs/cua-driver/hyprland-plugin/protocol/cua-input-v3.md)
specifies distinct `cua-input-v3.sock` and `cua-input-v3-2.sock` endpoints,
`HELLO`, `CLAIM`, a fresh `TARGET` for each admitted action, complete bounded
operations, and owner-scoped cancellation. It has no signer, challenge, or
`APPROVE` command. The target binding is a technical lifetime check, not a
second permission grant. The candidate includes framing, quotas, expiry, and
typed errors for review; source implementation and portable tests do not
establish native certification. No new public route enum or capability field
is required just for the plugin.

Uninitialized connections expire five seconds after acceptance; malformed or
out-of-order traffic cannot renew that deadline. Initialized connections use a
separate 60-second idle deadline. Test sustained pre-HELLO traffic at the full
connection limit and verify that all slots recover without restarting the
compositor. Connection quota/handshake recovery does not prove lane capacity
or application-input behavior.

## Alternatives considered

| Alternative                                    | Tradeoff and disposition                                                                                                                                                                                        |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| AT-SPI semantic actions                        | Keep them as the preferred route where suitable. They cannot supply arbitrary raw canvas/game input.                                                                                                            |
| Portal/libei and virtual input                 | Useful standard foreground routes where implemented, but not proof of exact-target background isolation. A portable extension is a separate standards discussion.                                               |
| Direct `wl_pointer`/keyboard resource delivery | Potentially smaller integration, but must prove seat, serial, focus, grab, keymap, and client behavior. Retain as an alternative if independent seats fail; do not silently substitute it under the same claim. |
| Borrow primary-seat focus and restore it       | Restoration cannot establish that no intervening grab/key state changed. Reject as the isolation contract.                                                                                                      |
| Patch Hyprland directly                        | May expose cleaner internal APIs, but adds distribution/upstream maintenance. Keep as a fallback decision requiring maintainer feedback, not assumed upstream acceptance.                                       |
| Nested compositor or private VM                | Useful controlled environments with different capture/input boundaries. They do not establish isolated input in the user's existing Hyprland session.                                                           |
| One agent seat initially                       | Does not meet the selected two-agent workflow. Qualify two independent lanes with conflict/refusal and concurrent-fault evidence.                                                                               |
| More than two simultaneous agent lanes         | Expands client and scheduling coverage beyond this release. Defer additional lanes; prove the selected two-lane contract with independent owners and overlapping gestures.                                      |

## Compatibility and migration

Keep the module optional, separately packaged, disabled by default, and tied
to the exact Hyprland ABI/compiler/runtime. A package must neither edit the
user's config nor load itself. Rebuild and recertify the applicable evidence
after compositor/toolchain changes; do not relax compatibility checks to keep
an old binary loading.

The plugin foundation may be reviewed as discovery-only; production input activation
waits for the accepted RFC, reviewed wire/authorization contracts, integrated
driver support, and evidence below. Source candidates may be tested before
acceptance in disposable environments. Switching Driver channels alone does
not install the plugin. A generic package does not enable it; an image may
explicitly own startup configuration and must document that choice. Image
configuration does not override Driver policy.

Config disable cancels work and closes input transports while preserving
session-lifetime seats and client resources. Re-enable opens fresh transports
with new epochs. Existing applications must recover after fresh admission
and target checks without restarting or accumulating replacement resources.
Validate surviving applications and startup ordering on the candidate source.
Plugin replacement requires a planned desktop restart;
hot replacement in the same compositor instance is unsupported. Unexpected
unload must leave surviving client requests safe and refuse replacement input
modules in the same desktop lifetime. Rollback releases owned synthetic state,
disables the module, and restores the previous
compatible package/configuration through that restart path. The ordinary
Driver remains usable.

## Security, privacy, and telemetry

The plugin executes inside the compositor and can affect the whole session.
Review bounds, threading, lifecycle, and privileges as part of its trust
boundary, not merely as performance tuning.

The selected model trusts native code running as the desktop account. Private
socket permissions, same-UID peers, and compositor attestation constrain
transport ownership and routing; they do not prove consent or form a sandbox
against hostile same-user native code. Driver enforces its existing policies
at every action admission. Public session names, executable names, requested
capability bits, and a previously admitted adapter connection cannot replace
that check.

The plugin independently binds the compositor epoch, private connection,
exact target lifetime, one operation, geometry revision, background-only
delivery limits, expiry, and quotas. These protect dispatch correctness and
cleanup under the local trust model. App/package/version/layout qualification
is a compatibility gate, not an authorization sandbox.

Production does not require a signing service, agent-facing approval tool,
per-window consent workflow, standalone operator panel, or key baked into an
image. Preserve Driver's existing activity/cursor behavior and stop/teardown
paths; indicator lifetime is not a separate approval condition. Tests must
prove policy denials cause no plugin mutation and that only a runtime's private
ownership can control its lane through Driver.

Lock revokes current input; unlocking never replays it. A later new action
requires fresh common admission and target checks. DPMS-off
is a separate visibility/precondition failure for this first version, not a
reason to wake displays or report a black capture as successful evidence.
Do not send input to lock, permission, or compositor-control surfaces.

Plugin diagnostics may include bounded aggregate counts and categorical
outcomes. Exclude typed text, individual key sequences, screenshots,
accessibility content, titles, URLs, raw target/lease tokens, credentials,
process paths, and stable application identifiers from logs and telemetry.
Sensitive request buffers have bounded lifetimes and are not persisted for
replay. Driver recording remains separately authorized and is not implicitly
enabled by plugin use.

## Implementation plan

Continue the selected work in #3547, #3557, and #3572 under the recorded
decision. Design acceptance does not mark the candidate certified:

1. Complete foundation CI and package/lifecycle evidence in #3547 while
   keeping mutation disabled. This does not depend on choosing input semantics.
2. Review the shared per-action admission, trusted-local boundary, v3 framing,
   target/geometry state, two-lane ownership, operation limits, and recovery
   contract. Keep discovery v2 and signed experiment 0 separate. Record the
   RFC disposition through the normal review process before or with merge.
3. Prove a normal standalone stdio MCP/CLI and SDK action through common Driver
   admission into the plugin, without a signer or bespoke operator host. Verify
   the target effect while a foreground fixture holds a drag. Exercise standard
   and acknowledged unrestricted modes with and without manifests, and bounded
   allow/deny cases; applicable policy denials must cause no plugin mutation.
4. Finish click, key/chord, scroll, and drag qualification on both initial apps.
   Use separate Driver processes for overlapping gestures, third-lane refusal,
   and one-owner cancellation. Prove lifecycle, keymap, target, and desktop
   recovery on current source; unknown or partial actions are never replayed.
5. Certify the stable exact source candidate with the affected canonical desktop
   matrix and native Hyprland real-app proof, including disposable Fleet tests.
   Retain supported/refused/unproven cells and diagnose recurring discovery/IPC
   failures. Review and merge the RFC and implementation dependencies, then run
   a main-branch smoke and account for differences from the certified SHA.
6. Publish Driver through its component release stream and an exact-compatible
   plugin package or relocatable pinned-source recipe. Verify the canonical
   installer resolves the intended component-tagged release, then install and
   test the actual published artifacts in a fresh disposable environment.
   Record checksums, source/compiler/runtime provenance, exact ABI checks,
   planned-restart upgrade, and rollback. A local checkout or tag alone is
   insufficient release evidence.
7. Only after released Driver/plugin verification, publish the final immutable
   Omarchy Fleet image through the canonical amd64 build/publish pipeline,
   pinned to those artifacts. Test supported input on two
   distinct fresh concurrent instances, including both apps, normal Driver
   sessions, cleanup, provenance, and an uninstrumented production-package
   smoke. Verify temporary-resource cleanup before promoting the documented
   image pin; retain the previous pin for rollback. Source-built Fleet tests
   and image-recipe preparation may run earlier, but do not replace this gate.

Deliver install/use instructions, the support matrix, recordings, exact release
and image identities, and rollback. Bare-metal support requires the separate
physical Omarchy gate; label Fleet evidence accurately until then.

## Test and acceptance plan

Use the canonical commands in
[the test-harnesses guide](../libs/cua-driver/docs/test-harnesses-guide.md) and
[CI guidance](../scripts/ci/README.md). A Hyprland lane must extend the shared
typed catalog, not replace it with an unrelated scripted demo. Do not assume
an unmerged PR's runner is present on main.

| Gate                              | Required evidence                                                                                                                                                                                                                                                                                                                                                                                                                     |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Protocol and authority            | Malformed/oversized requests, quota exhaustion, wrong target/epoch/sequence, expired bindings, disconnect races, and incompatible protocols refuse without unsafe dispatch. Standard/unrestricted manifest and no-manifest cases, bounded allow/deny, managed/user policy, and cached-connection re-admission prove the common contract. Public labels/arguments cannot confer ownership; policy denial causes no plugin mutation.    |
| Package and compositor lifecycle  | Published package install, dependency/ABI mismatch refusal, two compositor instances, clean restart, repeated disable/re-enable without resource growth, surviving clients and retained seats, unload/replacement refusal, startup ordering, and planned-restart replacement/rollback at exact package identities. Historical reload results do not qualify hot replacement.                                                          |
| First delivered action            | A normal signer-free Cua Driver call changes an exact native target while an independent foreground fixture retains its active grab without leaked events or changed primary-seat state. Include MCP/CLI and SDK paths as affected.                                                                                                                                                                                                   |
| Two independent lanes             | Distinct Driver processes perform overlapping gestures on Calc and Inkscape; third-lane and same-client conflicts refuse. Canceling or disconnecting one owner releases only its synthetic state while the other lane continues.                                                                                                                                                                                                      |
| Client matrix                     | Qualify native Calc `26.2.5-3` and Inkscape `1.4.4-6` per click/key/scroll/drag cell. Record toolkit/backend/package/flags and exact compiled US keymap. Other GTK3, GTK4, Qt6, Chromium/Ozone, Electron, and XWayland cells remain explicitly refused or unproven; unknown raw input refuses without changing existing semantic routes.                                                                                              |
| Gesture and keymap state          | Physical modifiers, agent chords, complete click/scroll/drag, held-state cleanup, and cancellation meet each promoted cell. Keymap changes invalidate stale qualification; variants/remaps/multiple groups, Unicode/IME, arbitrary held-key streams, and modified pointer gestures remain outside this candidate.                                                                                                                     |
| Identity and geometry             | Target replacement, PID/window reuse, move/resize, popup/modal/subsurface eligibility, occlusion, off-workspace targets, mixed scale, output transforms, and monitor changes yield exact delivery or the declared refusal.                                                                                                                                                                                                            |
| Revocation and Stop               | Revocation at admission and every gesture stage cleans owner state and preserves other lanes/primary input. Test runtime/session stop, process death, brief lock/unlock and DPMS off/on transitions, keymap/target loss, config transitions, and compositor restart. Later actions reacquire only after fresh checks; partial/unknown actions are never replayed. Measure responsive-compositor cleanup and report stalls separately. |
| Shared surface parity             | Common permission, session, result, and schema tests cover Rust, Python, TypeScript, CLI, and MCP as affected. Native macOS, Windows, X11, and other Wayland paths retain behavior or an explicit limitation.                                                                                                                                                                                                                         |
| Release, Fleet, and physical host | Pre-release Fleet source tests qualify the candidate. The final image consumes verified merged/released Driver/plugin artifacts and passes on two fresh concurrent Fleet instances before pin promotion. The documented bare-metal Omarchy baseline supplies separate physical-input evidence.                                                                                                                                        |

Each promoted delivery cell needs fixture-owned target state and independent
focus, stacking, workspace, cursor, primary-seat held-state, and input-leak
oracles. An unobservable oracle is an evidence gap, not a pass. Declared
unsupported cells must refuse before delivery; an all-refusal matrix cannot
satisfy a delivered-input milestone.

For the Hyprland cursor oracle, require a working compositor-owned query plus
positive and negative preflight canaries in the disposable test session. The
positive canary deliberately moves the cursor and proves observation before
restoring it; the negative canary checks that observation does not change
cursor, focus, or workspace. A session environment variable alone is not
capability evidence. Distinguish unsupported observation from environment
errors; missing samples must not pass an equality comparison. Before/after
position equality also does not rule out transient motion, so any stronger
claim needs independently validated sampling or motion-stream coverage.
Include a deliberate warp-and-return negative control to prove the oracle can
detect transient cursor theft despite unchanged endpoints. Split-view video
and saved app output support the independent continuous input oracles; video
alone does not replace the canonical harness.

Bound each foreground-grab episode to the helper's 60-second maximum. Do not
dispatch a new action or accept a proof after helper exit or deadline. Longer
plans may use separately recorded episodes only if they preserve the complete
action order, per-action effects, final saved-output checks, and required
two-lane overlap. Report each episode's primary mode and trace boundaries;
separate traces do not establish continuous isolation across their gaps.

The existing [plugin acceptance baseline](https://github.com/trycua/cua/blob/1c000fb07f88f73f1c3111d09ac326c5e3aa7647/libs/cua-driver/hyprland-plugin/tests/README.md)
names Omarchy `4.0.2-1`, Hyprland `0.56.2`, portal `1.4.1`, and driver
`0.23.2`. That is a target environment, not a passed run or a claim that the
released driver contains future input integration. For new integration tests,
record the exact source-built driver SHA/version in addition to those host
packages. Any revised baseline needs an explicit maintainer decision and
fresh evidence, not an undocumented environment substitution.

During implementation use focused checks. Run the complete affected desktop
matrix on the stable candidate SHA before readiness/merge; account for every
later executable, harness, or environment change. Retain sanitized results,
environment/package identities, and source provenance. Post-merge smoke and
release-path checks remain separate delivery requirements.

Define the repeat set before execution, preserve sanitized diagnostics for
discovery/IPC failures, and diagnose every recurrence rather than retrying a
failed cell into a pass. Passing repeats do not identify a historical failure's
cause. Test both an instrumented candidate for detailed event/cleanup evidence
and an uninstrumented production package; neither portable tests nor an
uninstrumented endpoint-only smoke proves continuous native isolation.

## Implementation verification and upstream follow-up

1. Does the v3 candidate's framing, target lifetime, action/connection quotas,
   sequence handling, and typed result model cover current-source dispatch,
   cancellation, epoch changes, and surviving-client recovery? Review the
   accepted wire specification against the implementation and measure cleanup
   bounds before implementation merge. Design acceptance does not supply native
   evidence.
2. Do both lanes satisfy real Calc/Inkscape focus/grab/serial/keymap semantics
   through normal Driver admission, including simultaneous primary input and
   partial/unknown results? Exact-SHA native evidence must qualify each cell.
3. Do the proposed top-level-only, same-client conflict, and stale-geometry
   refusals cover popup/modal/subsurface and primary-target transitions at the
   actual compositor serialization points?
4. Which existing Driver capture/identity changes must land for the integrated
   candidate, and how do their independent certification results compose?
5. What causes the outstanding discovery/IPC failures, and do the predeclared
   repeats retain enough sanitized evidence to diagnose recurrence?
6. Which upstream Hyprland extension points would reduce internal ABI coupling?
   Feedback is welcome; no upstream review or endorsement is implied.

## Decision record

The [2026-09-05 maintainer selection](https://github.com/trycua/cua/issues/3550#issuecomment-5555206901)
sets the implementation and delivery goal. The
[2026-09-05 scope correction](https://github.com/trycua/cua/issues/3550#issuecomment-5556178980)
selects the shared-policy implementation and supersedes this draft's earlier
requirements for per-window consent, a protected operator panel, independent
indicator-lifetime approval, and a one-seat-only milestone. It preserves
Driver policy, existing activity/lifecycle behavior, historical experiment
evidence, and contributor attribution. No RFC acceptance, native certification,
or production release is recorded by this edit.

The [2026-09-06 maintainer decision](https://github.com/trycua/cua/issues/3550#issuecomment-5558699892)
accepts the shared-policy, trusted-local, two-lane plugin design. It accepts
separate v3 input endpoints, complete bounded operations, fresh per-action
target bindings, passive pointer focus without held input or authority,
owner-scoped cancellation, and restart-required package replacement. Discovery
v2 remains unchanged. There is no automatic replay, foreground fallback, wake,
or unlock.

The decision preserves the contributor feedback and alternatives above. It
rejects borrowing primary-seat focus and the earlier separate consent/signer
requirements. A patched compositor, direct-resource delivery, and portable
standards work remain separate alternatives rather than silent substitutions.

The maintainer records a shorter-than-seven-day design-review window because
the approved scope is optional, narrowly qualified, and reuses Driver's
existing cross-platform permission contract without a new public permission
mode or automatic activation. This does not waive implementation review,
exact-candidate native qualification, package lifecycle tests, canonical
desktop coverage, or release and Fleet validation.

The implementation-verification questions remain release gates, not unresolved
permission-UI choices. Keep status `accepted` until the required criteria ship;
then record completion. Certify and release the Driver/plugin dependency chain
before publishing the final supported Fleet image. No upstream Hyprland
endorsement or physical-host parity is implied.
