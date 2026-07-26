# Active Permission Adapters and Trusted Per-Session Modes

**Status:** Reviewed; implementation sequencing accepted

**Issues:** #2385 and #2437

**Base:** `origin/main` at `544bff3ac739e51185dbaf465160779817dca230`

**Date:** 2026-07-24

## Goal

Finish the permission-mode model in two dimensions:

1. A trusted embedding host can run concurrent `standard`, `bounded`, and
   `unrestricted` sessions on one daemon without allowing an agent to choose or
   widen its own mode.
2. The daemon can promote reviewed capability groups from risk metadata to
   real protected-consent enforcement without duplicating policy logic or
   prompting for every action.

The public `session` argument remains a user-visible lifecycle label. It is
never authority by itself.

## Current state

- `PermissionMode` is a process-global `OnceLock` selected at daemon startup.
- `authorize_tool_call` evaluates hard invariants, managed/user policy, risk
  classification, and the process-global bounded manifest.
- Since RFC 2447, the public SDK can own a same-process native runtime and MCP,
  HTTP, CLI, and daemon transports are downstream adapters. The same-process
  `CuaDriver::create` path currently invokes the tool registry directly and
  does not pass through `authorize_tool_call`; authorization therefore has to
  move into the SDK/native-runtime dispatch boundary rather than remain a
  daemon-only concern.
- `session` is caller-declared. The daemon mirrors it into reserved arguments,
  while the MCP proxy supplies a separate transport session ID.
- The only active consent adapter is
  `browser_prepare(strategy.kind = existing_profile)`.
- `ProtectedConsentProvider` and `ApprovalBroker` are owned by `BrowserEngine`,
  so the canonical dispatch coordinator cannot use them for desktop or file
  capabilities.
- Production registries currently construct `BrowserEngine` without a
  protected provider. Standard/bounded existing-profile attachment therefore
  correctly refuses unless a certified host adapter is installed.
- The compatibility `EmbeddedCuaDriverHost` owns daemon lifecycle and a private
  parent-liveness stdin pipe, but its action transport still uses a
  path-addressed socket. The preferred `CuaDriver::create` topology is
  same-process and needs no shared-daemon session delegation at all.

## Non-negotiable security decisions

### Daemon ceiling and effective session mode are separate

The daemon starts with an immutable `SessionModeCeiling`:

- allowed session modes;
- whether unrestricted sessions were explicitly acknowledged;
- managed/user policy hashes;
- maximum session TTL and idle TTL;
- whether trusted session delegation is enabled.

Every call resolves an immutable `EffectiveAuthorizationContext` containing:

- daemon instance and generation;
- public session;
- transport session;
- effective permission mode;
- optional bounded manifest hash;
- trusted-host execution lease ID;
- policy hashes and expiry.

No context means the call inherits the daemon's legacy process mode. This keeps
standalone CLI/MCP/raw-socket behavior compatible.

### Session IDs are labels, not credentials

For the certified embedded route, delegated authority is bound to an
already-connected session channel rather than represented by a serializable
bearer token. The daemon binds the channel to the full authorization context
when the trusted host creates it. Authority never enters tool arguments,
policy input, logs, telemetry, or responses.

The following can never select a mode:

- public tool arguments or reserved-argument lookalikes;
- MCP metadata or elicitation;
- a caller-selected public or transport session ID;
- ordinary daemon control methods;
- model-visible environment variables or files.

### Only a trusted host can mint delegated sessions

The first supported minting route is the Rust-owned embedded host. It uses a
dedicated host-control channel inherited at spawn and unavailable to the
model-facing MCP stream. The channel is versioned and bound to the daemon
generation. EOF revokes every delegated session and shuts the embedded daemon
down.

The ordinary daemon listener never exposes create/upgrade mode operations.
Future native hosts may implement the same closed control protocol. Hosts that
cannot protect this channel must use one daemon per mode.

### Bind authority to an authenticated accepted connection

The pre-review recommendation is to avoid serializable bearer tokens entirely
for the certified embedded route. The trusted host creates an already-connected
session channel and supplies one endpoint to the daemon and the other to the
SDK or MCP proxy through an explicit inherited-handle allowlist. Every request
arriving on the channel inherits its immutable authorization context; no
request field can select it.

The control channel can mint a delegated session, but minting is not binding.
Binding action calls without a serializable bearer value requires either
#2410's inherited connected action transport or multiplexing action calls onto
another authenticated accepted channel. #2410 is therefore one valid
implementation, not a blanket prerequisite for the context, ceiling, control,
or inventory foundations. A path-addressed same-user socket plus a
caller-declared session ID cannot carry delegated authority.

```text
trusted host                 daemon                    model-facing proxy
    | inherited control pair   |                              |
    |-------------------------->|                              |
    | create connected pair    |                              |
    | create_session(mode, manifest, host endpoint)           |
    |-------------------------->| bind context to connection  |
    | inherit peer endpoint ---------------------------------->| MCP only
    |                           |<==== context-bound calls =====|
    | revoke / host EOF ------->| close, revoke, teardown      |
```

On Unix the control plane can pass additional descriptors with `SCM_RIGHTS` or
create all required channels before spawn. On Windows the host uses an explicit
handle-inheritance/duplication allowlist in the interactive user session. The
implementation must never fall back to a discoverable named endpoint for a
session advertised as protected.

### Unrestricted remains suppress-only

An unrestricted session skips Cua runtime consent prompts, but it never widens
managed/user policy, hard invariants, identity proofs, cleanup, revocation, or
resource scoping. The daemon may host unrestricted sessions only if trusted
startup explicitly enabled and acknowledged that ceiling.

## Architecture

### 1. Session authorization registry

Add a process-owned `SessionAuthorizationRegistry` in core:

- create a delegated session from a trusted control message and bind it to the
  supplied connected endpoint;
- resolve the connection plus exact public/transport session into an immutable
  context;
- reject connection substitution, session mismatch, expiry, ended sessions,
  daemon generation mismatch, and modes outside the daemon ceiling;
- revoke one context or every context owned by a dead host lease;
- register a session-end hook so grants, indicators, browser bindings, and
  mode authority share one teardown signal.

The first foundation slice stores the context and ceiling while calls continue
to inherit the legacy process mode. Certified per-session selection remains
disabled until action calls arrive on an authenticated connection.

Per-session `bounded` also requires replacing the process-global
`SessionManifest` `OnceLock` with an immutable per-session manifest store. A
manifest hash in the context is not sufficient. Until that store exists, no
mixed-mode status or documentation may claim concurrent bounded manifests.

The registry exposes no handle or resource contents through health. Status may
report counts by mode, allowed modes, provider readiness, and ceiling
provenance.

### 2. Trusted embedded control protocol

Replace the liveness-only stdin reader with a bounded framed control reader.
The host writes versioned messages for:

- `hello` with daemon generation and protocol version;
- `create_session` with public session, requested mode, expiry, and optional
  bounded manifest;
- `revoke_session`;
- provider decision/indicator lifecycle messages in the later provider slice;
- `shutdown` or EOF.

The daemon sends acknowledgements and provider requests over a separate
inherited response handle. Neither handle is inherited by an MCP proxy or
returned in `EmbeddedDriverConnection`.

The SDK returns a host-side `EmbeddedAuthorizedSession` object owning the peer
endpoint. Its MCP launch method explicitly inherits an authenticated action
endpoint into a host-owned proxy process, or multiplexes calls over the
authenticated channel; the model sees ordinary MCP schemas only. Direct SDK
calls use the same accepted connection. A plain command/args/environment
record is insufficient for the protected path because it cannot prove handle
ownership or cleanup.

### 3. Canonical runtime authorization coordinator

Place the coordinator at the native `ToolRegistry::invoke` chokepoint (or the
immediately enclosing `DriverRuntime::invoke`) so the typed SDK, daemon, MCP,
HTTP, CLI, and raw-socket routes cannot bypass it. This corrects an
authorization omission in the RFC 2447 implementation: the canonical
same-process SDK path currently reaches the registry without calling
`authorize_tool_call`.

Replace the process-global lookup inside `authorize_tool_call` with:

1. sanitize reserved arguments;
2. resolve connection-bound authority into `EffectiveAuthorizationContext`;
3. enforce hard invariants;
4. intersect managed, user, and optional session policy;
5. classify the exact operation;
6. route active classifications through the typed enforcement adapter;
7. invoke the tool only after the adapter returns an active grant or confirms
   unrestricted coverage.

The daemon transport supplies authenticated connection context to that runtime
coordinator. In-process SDK runtimes use a trusted constructor-owned context
and do not pretend to be shared daemons. Transport adapters may reject early
but can never mint or satisfy authority.

### 4. Generic protected grant broker

Move provider ownership out of `BrowserEngine` into a daemon-owned
`AuthorizationCoordinator`. Generalize the current broker request with a typed
`ProtectedResource` enum and a canonical resource digest.

The current `ConsentRequest`, digest binding, `IndicatorLease`, and revocation
primitives are retained. The missing work is coordinator ownership and a
certified production host adapter, not reinvention of those primitives.

Initial resources:

- `ExistingBrowserProfile { pid, window_id, fingerprint, endpoint_owner }`;
- `UserWindowObservation { pid, window_id }`;
- `DesktopObservation { display_generation }`;
- `UserWindowInput { pid, window_id, delivery_ceiling }`;
- `DesktopInput { display_generation, delivery_ceiling }`;
- `BrowserFileTransfer { binding, tab, paths_or_destination_digest }`;
- `BrowserConsequentialAction { binding, tab, action_kind }`.

Grants are bound to daemon generation, effective session, transport session,
mode, policy hashes, exact resource digest, expiry, indicator lease, and
revocation generation. They contain no raw typed text, page content, file
contents, or screenshot data.

### 5. Stable enforcement adapter inventory

Replace the hard-coded status string with a machine-readable inventory. Each
entry declares:

- operation selector and risk class;
- state: `active`, `metadata_only`, or `not_exposed`;
- resource kind and scope keys;
- grant type and TTL;
- indicator requirement;
- revocation triggers;
- stable refusal code;
- supported modes and required provider capability.

`tools/list`, authorization status, and generated docs derive from the same
inventory. Unknown tools and unrecognized operation variants remain denied.

## Adapter rollout for the currently exposed surface

### Group A: private observation

Prepare scoped adapters for user-owned `get_window_state`,
`get_accessibility_tree`, and `get_desktop_state`. Keep their public state
`metadata_only` until #2411 and representative platform certification pass.

- Driver-owned/disposable resources remain prompt-free.
- Standard obtains one window/display grant and reuses it inside scope.
- Bounded requires a manifest entry and activates the indicator without a
  second prompt.
- Unrestricted skips the prompt but retains session/resource binding.
- Screenshot-to-file arguments are file egress and are covered by Group C,
  never by an observation-only grant.

### Group B: desktop input

Prepare click, typing, key, pointer, scroll, drag, focus, and set-value routes.
Keep user-owned enforcement `metadata_only` until #2411 and platform
certification pass.

- Window-targeted AX/PX/background/foreground variants map to one exact
  window-input resource with a delivery ceiling.
- Desktop coordinate input maps to a display resource.
- Foreground delivery is a scope expansion unless already approved.
- Cua consent/indicator UI and known OS security prompts remain hard-denied.
- Coordinate input is semantically opaque; the adapter does not claim to
  detect purchases, sends, deletes, or account changes.

### Group C: browser file and consequential routes

Prepare `browser_set_input_files`, `browser_download`, screenshot-to-file,
mutating `browser_dialog`, and mutating `page` operations. Keep them
`metadata_only` until #2411 and platform certification pass.

- Bind file grants to canonical paths and the exact browser binding/tab.
- Prevent aliases, generic page operations, and raw routes from bypassing the
  typed adapter.
- Operations whose consequence cannot be bounded are refused in standard and
  bounded rather than silently treated as ordinary input.

### Not currently exposed

Microphone, camera, generic shell, and generic network tools are not part of
the current registry. The inventory records them as `not_exposed`; no dormant
adapter or unsupported public claim is added.

## Intervention behavior

- Standard: one approval per exact user-owned window/display/browser-file
  scope; no prompt for repeated ordinary actions within that grant. Foreground
  escalation, a new app/window/display, file destination, or consequential
  action may require a new approval.
- Bounded: one trusted session creation/manifest approval; no runtime prompts
  inside the manifest, but persistent indication remains mandatory.
- Unrestricted: one trusted host launch/session selection acknowledgement; no
  Cua runtime prompts.

## Delivery sequence

1. **Canonical SDK/runtime authorization boundary:** eliminate the direct-SDK
   bypass introduced by the new native-core topology while preserving typed
   SDK behavior.
2. **Generic coordinator and inventory (#2385 P0):** context-aware canonical
   dispatch, generic broker ownership, generated active/metadata inventory,
   and stable refusal vocabulary.
3. **Provider ownership refactor:** hoist the existing `ApprovalBroker` out of
   `BrowserEngine` without adding a new active group.
4. **Trusted session authority foundation (#2437):** context model, daemon
   ceiling, immutable per-session manifest store, status, teardown, and
   adversarial tests while mode still inherits legacy configuration.
5. **Authenticated session minting and action binding:** private embedded
   control channel plus either #2410 or actions on that authenticated channel.
6. **Embedded protected provider dependency (#2411):** exact request/response,
   persistent indicator leases, Stop, channel-death revocation, and packaged
   host contract.
7. **Observation and input adapters (#2385 P1):** exact resource extraction,
   grant reuse, foreground expansion, self-target refusal, and platform tests.
8. **File/consequential adapters (#2385 P2/P4):** browser upload/download,
   dialog/page mutations, path constraints, and bypass tests.
9. **Docs and support claims:** expected intervention counts, embedding guide,
   status reference, migration, and unsupported-host fallback.

Keep the slices in dependency order and reviewable. Do not activate a group
until its provider, indicator, revocation, bypass, and platform matrix pass.

## Verification

### Deterministic tests

- mode-ceiling lattice and policy intersection;
- delegated session creation only through the trusted control seam;
- tool/MCP/reserved-argument mode injection denied;
- wrong connection/session/transport/generation, replay, expiry,
  reconnect, and cross-session substitution denied;
- concurrent standard/bounded/unrestricted calls preserve isolation;
- standard grant reuse stays within exact scope;
- bounded manifests cannot self-amend or auto-accept `ask`;
- unrestricted cannot widen a denial;
- active inventory matches every registered tool/alias and generated docs;
- indicator failure and host-control EOF revoke before further side effects.

### Integration and representative environments

- public MCP, CLI, raw socket, same-process SDK, and compatibility-daemon SDK
  calls all reach the same coordinator;
- embedded host creates mixed-mode sessions while the model-facing schemas
  contain no mode/token fields;
- standard without a certified provider returns a stable refusal and produces
  no side effect;
- macOS logged-in VM validates TCC attribution, protected host UI, Stop, and
  window/display observation/input scopes;
- Windows runs in a verified interactive RDP session, never Session 0;
- Linux validates X11 and Wayland separately and reports missing protected UI
  as environment-unavailable before behavior assertions;
- before/after application state, focus, z-order, cursor, and input-isolation
  oracles prove each background-input row.

## Completion bar

- Mixed-mode shared-daemon sessions are supported only through a certified
  trusted host and cannot be selected from the model/tool channel.
- Every currently exposed sensitive capability is either actively enforced
  with complete adapter evidence or explicitly listed as metadata-only with a
  precise reason and no broader security claim.
- The first activated observation/input and file/consequential groups pass
  exact-head unit, integration, adversarial, and representative-platform tests.
- Documentation and machine-readable status agree on the active inventory and
  realistic human-intervention count.
- Delivery is split into dependency-ordered release-scoped pull requests; a
  local plan or unit-only result is not completion.

## Independent review outcome

Claude Code Opus reviewed the plan against
`544bff3ac739e51185dbaf465160779817dca230`. The accepted must-fix findings are:

1. close the shipping same-process SDK authorization bypass first;
2. keep user-owned capability groups `metadata_only` until #2411 and
   representative platform certification exist;
3. bind delegated authority to an authenticated accepted connection, never a
   public session string;
4. hoist provider ownership before generalizing adapters;
5. treat #2410 as one action-binding implementation, not a blanket dependency;
6. classify screenshot-to-file as egress; and
7. make concurrent bounded sessions depend on a real per-session manifest
   store.

The accepted delivery shape is a dependency-ordered PR series. Neither #2437
as a whole nor any user-owned capability group's switch to `active` can be
honestly completed in one PR.

## Prior baseline evidence

On `849e0db2118e17f80433d7a518bae8815e5ca5f6` (Cua Driver `0.11.0`), before
implementation:

- `cargo test -p cua-driver-core authorization::tests`: 10 passed;
- `cargo test -p cua-driver-core consent::tests`: 6 passed;
- `cargo test -p cua-driver-core session_manifest::tests`: 6 passed;
- `cargo test -p cua-driver-sdk embedded::tests::authorization_modes_require_explicit_acknowledgements`:
  1 passed.

These are baseline contract checks, not evidence for the new behavior.

The plan must be re-baselined on `544bff3ac739e51185dbaf465160779817dca230`
(Cua Driver `0.12.4`) after independent review because RFC 2447 changed the
canonical runtime boundary.
