---
title: Isolated background input on Hyprland
authors:
  - f-trycua
created: 2026-09-04
last_updated: 2026-09-05
status: review
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

Add an optional, explicitly authorized Hyprland input integration that delivers
to an exact background target without borrowing the user's primary-seat
focus. Use two independent synthetic seats, with compositor-owned target
identity, bounded complete actions, revocable authorization, and separate
transport, dispatch, and application-effect results. Keep the portable Cua
Driver contract unchanged where possible and refuse unsupported clients or
actions. This is a proposal for Cua, not an accepted upstream Hyprland API.

The [maintainer-selected delivery scope](https://github.com/trycua/cua/issues/3550#issuecomment-5555206901)
authorizes continuing the existing implementation workstreams toward released
Driver packages and a verified Omarchy Fleet image. The intended end state is
usable, supported input for a qualified client/operation matrix, not a demo.
`status: review` remains in effect: implementation authorization is not final
contract acceptance, native certification, or permission to bypass the
repository's review and release gates.

The selected initial direction is two lanes, native Calc/Inkscape operations,
trusted-local use with shared Driver policy, and session-lifetime seats with
restart-required plugin upgrades. Production host delegation, operator
indication/Stop, and the exact qualified matrix must be implemented and
reviewed before input can ship.

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
- Bind each input operation to operator authority, one private connection,
  one compositor epoch, and an exact target lifetime.
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
- Provide more than two concurrent agent seats in the initial release.
- Claim initial Chromium/Electron, XWayland, IME, popup/subsurface, or arbitrary
  display-layout support without separate qualification.
- Claim protection from a compromised compositor, root, or an unrestricted
  hostile local-user account without a separate enforceable OS boundary.

## Terminology

| Term             | Meaning in this proposal                                                                                             |
| ---------------- | -------------------------------------------------------------------------------------------------------------------- |
| Primary seat     | The compositor input state used by the person's physical devices.                                                    |
| Agent seat       | An independent compositor-managed synthetic input identity, not a cursor overlay.                                    |
| Operator         | The human or trusted host authorized to grant and revoke an input lease; not the agent making tool calls.            |
| Input lease      | Non-transferable authority bound to one admitted connection, exact target, capabilities, expiry, and policy context. |
| Target token     | An opaque reference to a compositor-owned target lifetime; identity alone does not grant authority.                  |
| Compositor epoch | A fresh identity for a live plugin transport binding, invalidated on disable/re-enable or restart.                   |
| Dispatch         | The compositor emitted the specified events; not proof that the application accepted them.                           |

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
recordings as proof. Input remains compile-time gated and uses a test-only
external signer. The [observation dependency #3557](https://github.com/trycua/cua/pull/3557)
retains its separate certification gates.

The separate [driver integration PR #3052](https://github.com/trycua/cua/pull/3052)
reports capture and desktop-contract evidence on its own branch. Its passing
contracts include expected refusals. That work must follow its own integration
and review path; neither PR certifies the other or establishes a released
isolated-input capability.

The agent-seat direction builds on Dillon DuPont's (@ddupont808) prototype
lineage, already credited in #3547. Credit does not imply approval of this RFC.

### Related contributor proposals

Austin Dixson's (@austindixson)
[proposal #3552](https://github.com/trycua/cua/issues/3552) independently
proposes a permission-gated agent seat. The shared direction is to preserve
primary-seat state, keep direct-resource delivery an internal implementation
choice, and avoid treating a portal/libei connection as proof of isolation.
This RFC remains the decision record through #3550; the related proposal is
retained as a source of feedback, not marked accepted or superseded here.

Three differences need explicit review:

- A binary-path permission and an ASK policy are a proposed entry point, not
  proof of protected approval, connection delegation, or revocation. The
  operator-control requirements below still apply.
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

| Owner                         | Responsibility                                                                                                                                          |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Shared driver                 | Public tool/SDK/MCP schemas, exact window selection, authorization context, lifecycle ownership, and action-result mapping.                             |
| Linux adapter                 | Attested correlation from the public window target to a plugin target, screenshot-to-surface geometry conversion, negotiation, and disconnect handling. |
| Hyprland plugin               | Live target identity, lease validation, isolated seat state, event-loop dispatch, and agent-state cleanup.                                              |
| Trusted operator-control path | Approval, visible lease indication, Stop, expiry, revocation, and protected delegation to the admitted action connection.                               |
| Application fixture/harness   | Independent evidence of target effects and absence of foreground side effects.                                                                          |

The shared permission stack remains binding. An unrestricted driver profile
does not manufacture a compositor lease, and a compositor lease does not
bypass managed policy, user policy, a capability manifest, or hard invariants.
Public lifecycle session labels remain labels, not credentials, following
[RFC 3007](3007-cua-driver-lifecycle-sessions.md).

The existing shared authorization code explicitly keeps delegated sessions
disabled without a protected action transport. The plugin must not infer that
this missing boundary is supplied by its same-UID socket. See
[session authorization](../libs/cua-driver/rust/crates/cua-driver-core/src/session_authorization.rs).

### Initial scope and capability negotiation

Start with two independent lanes per compositor instance, one target and one
executing action per lane. Driver-private lifecycle owners reserve lanes
through the compositor so two Driver processes cannot independently assume
ownership of the same lane. Per-lane serialization must not serialize the
other lane's bounded drag. A third owner receives a typed busy outcome. Cancel
affects its selected lane; operator Stop revokes both. Same-Wayland-client
conflicts between lanes or with primary input refuse.

Keep pointer motion, button actions, scrolling, drag, key chords, and text as
distinct capabilities. Enable only individually certified capabilities. The
initial supported matrix targets native Calc and Inkscape complete clicks,
physical key/chord actions, scroll, and bounded drag, qualifying each operation
and runtime separately. GTK fixtures supply reproducible faults and oracles,
not an application-support claim. Chromium/Electron, raw text/IME, XWayland,
popup/subsurface delivery, modified pointer gestures, and unqualified display
layouts remain explicit refusals.

Eligibility uses host/compositor-observed live client identity and qualified
runtime details, not an app name supplied by the caller. An eligibility list
is a compatibility policy, not a sandbox: a supported application may itself
run scripts, extensions, or subprocesses.

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
lease. A PID, title, application ID, geometry match, or Wayland object number
alone is insufficient identity. Ambiguity refuses. App names and titles may
help the operator recognize the target but are not authority checks.

Bind the target to its client/surface lifetime and a separate geometry
revision. Destruction or replacement invalidates identity even if a PID or
window number is reused. Geometry changes invalidate the old coordinate
revision, not silently retarget the action. Scope expansion requires new
operator authority; a title change alone does not create a new application.

Use surface-local logical coordinates at the plugin boundary, with a declared
origin, extent, and geometry revision. The driver converts from its actual
capture frame using attested crop, scale, transform, and surface geometry.
The compositor validates that revision again at dispatch. Reject invalid,
non-finite, out-of-bounds, or stale coordinates; do not clamp or reinterpret
them as desktop coordinates. A stale capture must be refreshed before retry.

Keep normal compositor hit-testing within the authorized surface tree.
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

Keep I/O parsing off the compositor event loop, then cross a bounded queue.
Validate lease, epoch, target, operation, and geometry on admission and again
immediately before dispatch. Long operations use bounded event-loop steps,
never a blocking drag/typing loop. Publish and test maximum queue, payload,
action-duration, and cancellation-work limits before enabling any capability.

Order operations per lease and reject target conflicts. Bind a monotonic
operation sequence and request digest to the lease generation. Exact duplicate
requests may return a retained result but must never execute twice; reuse of
a sequence with a different payload refuses. Bounded replay-state exhaustion
must refuse further admission rather than forget an executable request.

Connection loss after dispatch creates an unknown outcome, not permission to
retry input. Reconnection requires fresh authorization/identity negotiation
and new application observation. Never automatically replay an action in a
new epoch or lease.

Provide idempotent cancellation and an operator Stop path. Revocation closes
admission before queued work is removed. Revalidate before each long-action
step; a step already dispatched cannot be undone. Track and release only
agent-owned held state, to its original live target where valid, and never
redirect cleanup events to the primary seat or a replacement window. If the
client disappeared, destroy agent-side state without addressing another client.

Cancel/revoke on expiry, disconnect, loss of the trusted control channel,
target destruction, lock, session switch, plugin disable/unload, or compositor
restart. Geometry change during a gesture aborts it with defined agent-state
cleanup; it does not silently recompute a new path. Cancellation reports
partial dispatch when events have already escaped; it cannot undo an
application's prior text insertion or click.

Observe desktop transitions synchronously, not only by comparing sampled
state. Lock/unlock or DPMS off/on between two timer ticks must still invalidate
old active and pending authority. Keep dispatch-time guards as defense in
depth. Revocation generations are monotonic, and exhaustion requires restart
rather than wrapping. Operator status and Stop remain reachable after an
action connection is revoked.

### Results and public parity

Keep three facts separate: receipt by the transport, dispatch by the
compositor, and application effect established through independent readback.
An emitted-event count is dispatch evidence, not accepted-input evidence.

Map into the existing [action-result contract](../libs/cua-driver/docs/action-result-contract.md).
Use the shared `synthetic_events` route where applicable; do not add a
Hyprland-specific value to the public closed route enum. Dispatch alone is
`unverifiable`, not `confirmed`; partial operations retain counts; refusal
before dispatch carries no delivery/effect evidence. Any new public error or
capability field requires shared schemas and SDK/MCP parity tests.

Unsupported clients, denied authority, stale identities, and busy queues must
produce distinguishable typed outcomes. No result triggers an automatic
foreground retry. Plugin absence leaves existing semantic/background routes
and their honest refusals intact on Linux and does not alter other platforms.

### Protocol version

Use a new major, proposed v3, for the input-bearing contract. Leases, target
lifetime, replay, cancellation, and dispatch results change security and
operation semantics beyond discovery negotiation. Preserve v2 as
discovery-only; define a distinct endpoint/version boundary in the reviewed
wire specification and reject mismatches without downgrade.

This RFC does not assign mutation payload layouts or declare v3 implemented.
The complete wire specification must resolve the accepted decisions and be
reviewed before mutation is enabled.

## Alternatives considered

| Alternative                                    | Tradeoff and disposition                                                                                                                                                                                        |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| AT-SPI semantic actions                        | Keep them as the preferred route where suitable. They cannot supply arbitrary raw canvas/game input.                                                                                                            |
| Portal/libei and virtual input                 | Useful standard foreground routes where implemented, but not proof of exact-target background isolation. A portable extension is a separate standards discussion.                                               |
| Direct `wl_pointer`/keyboard resource delivery | Potentially smaller integration, but must prove seat, serial, focus, grab, keymap, and client behavior. Retain as an alternative if independent seats fail; do not silently substitute it under the same claim. |
| Borrow primary-seat focus and restore it       | Restoration cannot establish that no intervening grab/key state changed. Reject as the isolation contract.                                                                                                      |
| Patch Hyprland directly                        | May expose cleaner internal APIs, but adds distribution/upstream maintenance. Keep as a fallback decision requiring maintainer feedback, not assumed upstream acceptance.                                       |
| Nested compositor or private VM                | Useful controlled environments with different capture/input boundaries. They do not establish isolated input in the user's existing Hyprland session.                                                           |
| One agent seat initially                       | Simpler admission, but does not meet the selected two-agent workflow. Use two bounded independent lanes with conflict/refusal and concurrent-fault evidence; defer additional lanes.                          |

## Compatibility and migration

Keep the module optional, separately packaged, disabled by default, and tied
to the exact Hyprland ABI/compiler/runtime. A package must neither edit the
user's config nor load itself. Rebuild and recertify the applicable evidence
after compositor/toolchain changes; do not relax compatibility checks to keep
an old binary loading.

The plugin foundation may be reviewed as discovery-only; input activation
waits for the accepted RFC, reviewed wire/authorization contracts, integrated
driver support, and evidence below. Switching driver channels alone does not
install the plugin. No release or image update silently grants input authority.

Create the two seats once per compositor session. Disable/re-enable revokes
authority and replaces the transport epoch, not the seat globals or existing
client bindings. Existing applications must recover after fresh approval
without restart or accumulating replacement resources.

Plugin upgrades and rollback require a planned desktop restart. Before the
restart, revoke leases, cancel work, and release agent-owned state. Unexpected
unload must leave surviving client requests safe and refuse replacement input
modules in the same desktop lifetime. It is not a supported hot-upgrade path.
The ordinary Driver remains usable without plugin input.

## Security, privacy, and telemetry

The plugin executes inside the compositor and can affect the whole session.
Review bounds, threading, lifecycle, and privileges as part of its trust
boundary, not merely as performance tuning.

The initial trust model is a local operator and trusted Driver host running
within the same desktop account. It does not contain an unrestricted hostile
same-user shell or promise an unspoofable desktop against that process. Such a
threat requires a separate enforceable OS/process boundary, not an app list,
socket mode, executable-name check, or cursor overlay.

Same-UID peer checks, socket permissions, public session names, executable
names, and requested capability bits do not authorize input. Require a
protected operator-control path and non-transferable delegation to one private
action connection. A generic MCP approval, user-writable file, or bearer token
returned to the agent is not a substitute for that design.

An input lease binds the compositor instance/epoch, connection and lease
generation, driver authorization context, exact target, operation set,
background-only delivery ceiling, policy scope, absolute/idle expiry, and
quotas. Renewal or scope expansion requires the trusted operator path. Agent
tools cannot grant, renew, expand, or suppress revocation of their own lease.

Approval must show the target, operations, duration, and background-only scope.
An operator-visible active-lease indicator and Stop control must be independent
of the target and agent-controlled cursor artwork. The implementation must
prove which processes can invoke or spoof approval/control, and state its
same-user threat model honestly. No current protected Hyprland collector or
delegation mechanism is claimed here. If no adequate mechanism is available,
mutation remains disabled.

Reuse Driver's shared permission engine and immutable trusted-host callback
instead of adding agent-facing approval tools. The callback is not invoked for
every routine or in-manifest operation, so the production bridge must also
delegate already-authorized operations to a private, target-bound compositor
lease. Unrestricted Driver mode alone does not issue a lease. Bind delegation
to runtime/lifecycle ownership and policy generation; host loss and Stop
revoke it. The test signer and compiled test public key are not a deployable
replacement, and no private signing key may be baked into an image.

Lock revokes input; unlocking does not automatically renew a lease. DPMS-off
is a separate visibility/precondition failure for this first version, not a
reason to wake displays or report a black capture as successful evidence.
Do not send input to lock, approval, Stop, or compositor-control surfaces.

Plugin diagnostics may include bounded aggregate counts and categorical
outcomes. Exclude typed text, individual key sequences, screenshots,
accessibility content, titles, URLs, raw target/lease tokens, credentials,
process paths, and stable application identifiers from logs and telemetry.
Sensitive request buffers have bounded lifetimes and are not persisted for
replay. Driver recording remains separately authorized and is not implicitly
enabled by plugin use.

## Implementation plan

Continue the existing selected workstreams; do not create competing PRs:

1. Complete foundation CI and package/lifecycle evidence in #3547 while
   keeping mutation disabled. This does not depend on choosing input semantics.
2. Reconcile the recorded implementation direction with this RFC and record
   its final disposition, including the operator delegation/wire contract and
   qualified client matrix. Preserve the existing spike's lineage and limits.
3. Add reviewed v3 framing, lease/target/geometry state, bounded scheduling,
   replay handling, and cancellation with protocol tests. Keep input feature
   flags off until the protected operator path is implemented.
4. Qualify both native application lanes through normal Driver calls, saved
   application outputs, and independent primary-input oracles. Exercise
   concurrent faults, distinct Driver processes, brief state transitions,
   keymap changes, and surviving-client config-toggle recovery.
5. Run final-candidate certification, review and merge the dependency chain,
   and verify component-tagged Driver artifacts and canonical installers.
   Unsupported rows must remain refusals, not implicit success.
6. Use the canonical amd64 image build/publish pipeline for the pinned Omarchy
   candidate. Verify package install/upgrade refusal, restart/rollback, and
   fresh SDK-driven Fleet instances. Retain the physical Omarchy gate before
   making bare-metal claims; Fleet and physical evidence are not substitutes.
   The selected shipping goal includes release/image delivery only after
   these substantive gates pass.

## Test and acceptance plan

Use the canonical commands in
[the test-harnesses guide](../libs/cua-driver/docs/test-harnesses-guide.md) and
[CI guidance](../scripts/ci/README.md). A Hyprland lane must extend the shared
typed catalog, not replace it with an unrelated scripted demo. Do not assume
an unmerged PR's runner is present on main.

| Gate                             | Required evidence                                                                                                                                                                                                                                 |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Protocol and authority           | Malformed/oversized requests, queue/replay exhaustion, denied or unavailable approval, nondelegated same-UID peers, wrong target/epoch/lease/sequence, expiry, scope expansion, and disconnect races all refuse without unsafe dispatch.          |
| Package and compositor lifecycle | Real package install, dependency-upgrade and cross-version ABI refusal, two real compositor instances, clean restart, repeated config toggles without resource growth, surviving-client recovery, and unload/replacement refusal.                  |
| First delivered action           | Normal Cua Driver call through the plugin changes the exact GTK fixture, while an independent foreground fixture continues its active grab without leaked events or changed primary-seat state.                                                   |
| Client matrix                    | Native GTK3, GTK4, Qt6, LibreOffice, Chromium/Ozone, Electron, and XWayland each declare supported/refused actions separately. Record actual toolkit/backend/version/flags; do not force another backend silently.                                |
| Gesture and text state           | Held buttons, physical modifiers, agent chords, drag cancellation, key repeat, scroll completion, and Unicode/IME behavior for every capability proposed for promotion.                                                                           |
| Identity and geometry            | Target replacement, PID/window reuse, move/resize, popup/modal/subsurface eligibility, occlusion, off-workspace targets, mixed scale, output transforms, and monitor changes yield exact delivery or the declared refusal.                        |
| Revocation and Stop              | Revocation at queue admission and every gesture stage prevents later work, cleans agent-owned state, preserves primary state, and reports already-dispatched effects honestly. Test lock, DPMS, target/control-channel loss, and plugin shutdown. |
| Shared surface parity            | Common permission, session, result, and schema tests cover Rust, Python, TypeScript, CLI, and MCP as affected. Native macOS, Windows, X11, and other Wayland paths retain behavior or an explicit limitation.                                     |
| Fleet and physical host          | SDK-driven Fleet candidate checks prove packaging/runtime behavior; the documented bare-metal Omarchy baseline supplies physical input evidence afterward. Neither replaces the other.                                                            |

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

## Unresolved questions

1. Which protected operator-control/connection-delegation mechanism can
   Hyprland actually support, including reliable indication and Stop? Which
   same-user threats remain outside its enforceable boundary?
2. Can the independent seat satisfy real client focus/grab/serial/keymap
   semantics without changing primary state across the selected Calc/Inkscape
   operation matrix? Which exact versions/layouts qualify, and what evidence
   would reject a cell or the two-lane direction?
3. What exact surface-tree roles and geometry-change behavior are safe for
   the first increment? Which popup and same-target interactions must refuse?
4. What are the reviewed lease TTLs, action/queue quotas, cancellation bounds,
   v3 payloads, and typed errors? The wire spec must settle these before input
   is enabled; this document intentionally does not assign unreviewed bytes.
5. Which existing driver capture/identity changes must land for the integrated
   candidate, and how do their independent certification results compose?
6. Which upstream Hyprland extension points would reduce internal ABI coupling?
   Feedback is welcome; no upstream review or endorsement is implied.

## Decision record

The [2026-09-05 maintainer selection](https://github.com/trycua/cua/issues/3550#issuecomment-5555206901)
sets the implementation and delivery direction above. It does not mark the
RFC accepted or completed. The final contract disposition remains tracked in
[#3550](https://github.com/trycua/cua/issues/3550). This proposal changes an
input permission boundary and should receive at least the normal seven-day
review window in [the RFC process](README.md), unless a maintainer records an
explicit exception. Acceptance must resolve the blocking authority and scope
questions or explicitly keep input disabled pending a follow-up decision.
