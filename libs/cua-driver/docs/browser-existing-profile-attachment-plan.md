# Existing-Profile Browser Attachment and Reconnect Plan

## Status

- **State:** Proposed for review
- **Plan date:** 2026-07-16
- **Base:** `main` at `0835daa6` (`feat(cua-driver): add capability-aware browser tools`)
- **Primary tracker:** [#2192](https://github.com/trycua/cua/issues/2192)
- **Scope:** First-class, user-approved attachment to an existing Chromium profile, followed by bounded autonomous reconnection
- **Delivery:** Multiple independently reviewable pull requests in one implementation stack, not one large pull request

This plan is based on a read-only audit of the merged browser tools, platform
adapters, approval path, CDP transport, legacy macOS page implementation,
recording system, and Rust E2E harnesses. A separate Fable review challenged the
public contract, consent boundary, reconnect ordering, concurrency model, issue
grouping, and pull request sequence.

## Definition of Done

The project is done when an agent can attach to an existing authenticated
Chromium profile after one genuine host/user approval, continue through bounded
socket reconnects without asking the model to approve security UI, and either
act on the exact intended tab or return an exact structured refusal. Completion
requires:

1. A real user approval path that cannot be forged by a model or inferred from
   ordinary MCP transport.
2. One browser-level CDP connection per approved connection generation.
3. Exact prompt ownership and semantics before any automated consent click.
4. Endpoint, process, profile, native window, and CDP target identity re-proven
   after reconnect.
5. Old target, tab, and page-ref capabilities invalidated across generations.
6. Browser mutations serialized at the canonical CDP target boundary, with
   honest partial-delivery results.
7. Deterministic unit, adversarial, and real-browser E2E evidence on every
   advertised platform.
8. Recording, telemetry, docs, and bundled skills that do not leak profile or
   authenticated-page data and do not overstate support.

## Problem

The merged browser tools support exact Chromium binding and driver-owned
`isolated_new` or `isolated_named` profiles. They deliberately do not prepare a
person's already-running authenticated profile.

The legacy macOS page route proves that an existing Chrome profile can expose a
browser-level CDP endpoint through `chrome://inspect/#remote-debugging`. That
route has three limitations:

- it is outside the first-class browser capability and ownership model;
- Chrome may show an `Allow remote debugging?` prompt for every genuinely new
  WebSocket connection;
- a dropped socket causes a fresh connection and therefore another prompt.

The current core `CdpPool` replaces a closed socket and retries once, but it
does not have a consent state machine, connection generations, cancellation,
or a mutation coordinator. The current MCP server also injects an internal
approval marker for every `browser_prepare` call. That marker is protected from
client forgery, but it proves transport provenance, not an interactive user
decision. It is insufficient for attaching to a personal browser profile.

## Goals

1. Add a typed existing-profile strategy to `browser_prepare` without changing
   the meaning of driver-owned isolated profiles.
2. Require one operation-bound user approval before enabling or attaching to an
   existing profile.
3. Let the driver handle the browser's exact consent prompt while that grant is
   live, including bounded reconnects.
4. Keep `get_browser_state` strictly read-only.
5. Preserve exact-or-refused native window to CDP target binding.
6. Make socket loss and browser restart explicit state transitions rather than
   transparent retries with stale capabilities.
7. Apply the same concurrency and completion guarantees to all browser
   mutations so reconnect does not introduce partial or cross-target actions.
8. Roll out only where real platform evidence supports the contract.

## Non-Goals

- Generic approval of system or application dialogs.
- Matching consent UI by screenshot, fuzzy title, or button text alone.
- Treating a model-provided Boolean, public token, or ordinary MCP call as user
  approval.
- Persisting grants across daemon restart in the first release.
- Restarting a user's browser with remote-debugging flags.
- Copying, modifying, or disclosing an existing profile directory.
- Safari, Firefox, or WebDriver BiDi support in this project.
- Mutation on a Wayland compositor where exact native identity cannot be
  proven.
- Fixing native-route issues [#2201](https://github.com/trycua/cua/issues/2201)
  or [#2202](https://github.com/trycua/cua/issues/2202) through browser-specific
  workarounds.
- Migrating the complete legacy `page` surface in the same pull request stack.

## Locked Design Decisions

### Existing profile is an attachment strategy, not a profile lifecycle mode

`PrepareProfileMode` continues to mean driver-owned profile lifecycle and keeps
only `isolated_new` and `isolated_named`. Existing profiles are not owned by the
driver and must not inherit isolated-profile creation or cleanup behavior.

Add an optional tagged `strategy` object. Calls that omit it retain today's
behavior. Current `allow_launch` and `profile` fields remain accepted as a
compatibility form and are normalized internally.

```json
{
  "pid": 4242,
  "window_id": 991,
  "session": "agent-session",
  "strategy": {
    "kind": "existing_profile"
  }
}
```

`window_id` is required for `existing_profile`. The endpoint grant is
process/profile-wide, but the selected native window provides the user-visible
approval anchor and an exact initial ownership proof. Browser product is
derived from the process and is never trusted from a caller claim.

Supplying `strategy` together with legacy `allow_launch` or `profile` fields is
an invalid request. A later cleanup may expose tagged isolated strategies, but
that is not required for this feature.

### Approval is out of band and operation bound

The public request never contains `approved: true` or a reusable grant ID.

When no valid grant exists, `browser_prepare` returns the existing structured
`browser_consent_required` refusal with a short-lived approval request ID in
`detail`. The request ID correlates a host UI action; possession does not grant
permission.

Approval providers are:

- an interactive `browser-approve` CLI artifact extended to bind the exact
  existing-profile request digest; and
- a host approval broker that injects an internal, operation-bound approval
  certificate only after a real user action.

The current automatically injected MCP host marker remains sufficient only for
the existing isolated-launch contract. It must never authorize
`existing_profile`.

An approved request mints an in-memory `ExistingProfileGrant` scoped to:

- daemon instance;
- public and transport session;
- browser product and platform identity proof;
- opaque profile identity when the platform can prove it;
- current process fingerprint;
- approved side effects and reconnect budget;
- idle and absolute expiry.

The internal profile identity may be a daemon-salted hash of a canonical path,
but the path or hash is never returned, logged, or recorded. If a platform
cannot prove profile identity independently, the initial release binds the
grant to the current process fingerprint and requires approval after a browser
restart.

Daemon restart always discards the grant. Durable grants backed by Keychain,
DPAPI, or Secret Service require a separate security design and are deferred.

### One browser-level socket per connection generation

Chrome may prompt once per genuinely new WebSocket, not once per operation.
The coordinator therefore owns exactly one browser-level
`/devtools/browser` socket for an attachment generation and multiplexes tabs
through flattened `Target.attachToTarget` sessions.

Core must not open independent per-target sockets for an approved existing
profile. If a browser product cannot support the browser-level route, that
product is unsupported until its prompt count and connection semantics are
modeled explicitly.

The new route becomes the sole socket owner for a grant-owned endpoint. The
legacy page cache must refuse access to such an endpoint and direct callers to
the first-class browser tools. This prevents duplicate prompts and split-brain
connection ownership.

### Prompt handling occurs while connection is pending

The browser prompt is caused by opening the WebSocket, so the driver cannot
click it before starting the connection. The state machine must initiate a
bounded connect, observe the exact resulting prompt, press its exact semantic
action, and then confirm that the same pending connection completed.

```text
approval_required
  -> grant_active
  -> endpoint_preflight
  -> connecting_pending_consent
       -> socket_ready
       -> consent_prompt_observed -> consent_pressed -> socket_ready
       -> consent_dismissed -> grant_revoked
       -> timeout_or_ambiguity -> refused
  -> endpoint_reproved
  -> capabilities_invalidated
  -> ready_for_rebind
```

On socket loss:

```text
ready
  -> reconnect_singleflight
  -> mutation_gates_closed
  -> process_profile_endpoint_revalidated
  -> connecting_pending_consent
  -> endpoint_generation_incremented
  -> old_capabilities_invalidated
  -> mutation_gates_reopened
  -> ready_for_rebind
```

The first implementation uses a fixed policy rather than caller-tunable
security limits:

- one reconnect leader per approved browser identity;
- at most three connection attempts per loss;
- one exact consent action per attempt;
- one total deadline for the transition;
- immediate revocation when a person dismisses or denies the prompt;
- cancellation when the owning session ends.

### Internal desktop primitives, never recursive MCP calls

Prompt handling reuses the same native perception and semantic action
primitives that support `get_window_state` and AX/UIA/AT-SPI clicks, but it does
not invoke public MCP tools from inside `browser_prepare` or CDP reconnect.
Recursive public calls would create policy, recording, and lifecycle deadlocks.

Each platform provides a narrow internal `BrowserConsentUi` adapter that can:

1. derive or verify a browser/profile identity;
2. observe browser-owned consent candidates within a bounded scope;
3. return proof-rich role, subrole, identifier, action, process, window, modal
   relationship, and bounds metadata;
4. press one exact semantic action;
5. report whether the prompt was accepted, dismissed, ambiguous, or stale.

A candidate is actionable only when all available independent evidence agrees:

- the prompt belongs to the approved browser process or a platform-proven
  browser-owned helper;
- the binary identity matches the approved browser;
- the prompt has the expected native role, modal relationship, semantic
  identifier, and advertised action;
- the action belongs to the exact prompt snapshot;
- the endpoint being opened belongs to the same approved browser identity.

Localized text may corroborate identity but cannot be the sole proof. If a
browser/version does not expose stable semantics, the adapter refuses it.

### Reconnect invalidates capabilities

Every connection has an internal monotonically increasing endpoint generation.
`TargetRecord`, `TabRecord`, and page refs record the generation that minted
them. A successful reconnect increments the generation and invalidates every
older target, tab, snapshot, frame, and ref capability.

The caller receives `capabilities_invalidated: true` and must call
`get_browser_state` again with the new native `(pid, window_id)` before any
mutation. The driver never silently remaps an old capability to a new CDP
target.

### Mutation serialization is part of reconnect safety

Add a mutation gate keyed by canonical browser identity plus CDP target ID, not
by public session ID. This serializes mutations from multiple sessions that
resolve to the same real tab while allowing proven independent tabs to proceed
in parallel.

`browser_navigate`, `browser_click`, `browser_type`, and future file upload
hold the gate across revalidation, dispatch, and outcome verification. A
reconnect closes all gates for that browser identity. In-flight actions either
complete on the old generation or return a stale-generation refusal; they
never continue on the new socket.

`browser_type` additionally reports requested and delivered character counts.
Keystroke mode updates the delivered count only after the full key-down/key-up
pair succeeds. A partial write returns a structured `browser_input_incomplete`
refusal with retryability metadata instead of `status: ok`.

This prevents the first-class browser surface from repeating the partial and
interleaved delivery classes tracked in
[#2255](https://github.com/trycua/cua/issues/2255) and
[#2256](https://github.com/trycua/cua/issues/2256). Those issues' native
`type_text` paths remain separate fixes.

### Consent evidence is privacy bounded

Ordinary before/after screenshots of a live authenticated browser can expose
URLs, page text, or authentication state. Consent turns therefore record:

- the structured approval request digest and grant scope, with identifiers
  hashed or redacted;
- prompt ownership, role, action, and bounds proof;
- one prompt-bounds crop only when it excludes surrounding browser content;
- otherwise no screenshot for the consent phase;
- endpoint ownership method, never port or WebSocket URL;
- transition outcome, attempt count, and elapsed time.

Recording and telemetry must never contain profile paths, profile hashes,
ports, WebSocket URLs or tokens, cookies, storage, page content, tab URLs,
JavaScript, or approval artifacts.

## Public Result Contract

Existing successful isolated results remain unchanged. Existing-profile
success is additive:

```json
{
  "status": "ok",
  "prepared": true,
  "action": "attached_existing_profile",
  "prepared_pid": 4242,
  "attachment": {
    "kind": "existing_profile",
    "browser": "chromium",
    "capabilities_invalidated": true,
    "next_action": "get_browser_state"
  },
  "endpoint_ownership": {
    "method": "listening_socket_pid",
    "owner_pid": 4242
  },
  "side_effects": {
    "launched_browser": false,
    "restarted_browser": false,
    "created_profile": false,
    "reused_driver_profile": false,
    "copied_profile_data": false,
    "changed_preferences": false,
    "displayed_consent_prompt": true
  }
}
```

Do not return a grant ID, profile identity, endpoint generation, port, or
WebSocket URL. Those are internal security and lifecycle facts, not caller
capabilities.

Use existing refusals where their meaning is exact:

- `browser_consent_required` for no valid user grant;
- `browser_endpoint_owner_mismatch` for endpoint identity failure;
- `browser_binding_stale` after generation change;
- `browser_wrong_target_refused` for prompt/window/target mismatch;
- `browser_route_unavailable` for unsupported browser/platform setup.

Add only the codes callers need to branch on distinctly:

- `browser_consent_revoked` when the person dismisses or denies the prompt;
- `browser_reconnect_exhausted` after the bounded reconnect policy fails;
- `browser_input_incomplete` with requested and delivered counts.

All refusal additions must also update the testkit's closed `RefusalCode`
mirror and serialization tests.

## Browser Restart and Endpoint Setup

Three events have different authorization semantics:

| Event | Default behavior |
| --- | --- |
| Socket drop, same process/profile | Reconnect under the live grant after full revalidation |
| Browser restart | Reuse the grant only if exact browser and opaque profile identity can be re-proven; otherwise require approval |
| Driver daemon restart | Always require approval in v1 |

If no remote-debugging endpoint exists, v1 must not restart the browser with
flags. The first macOS implementation may automate Chrome's own internal
remote-debugging setup UI only after the same user grant and only when every
navigation, toggle, prompt, and cleanup step is exact and recorded as a
declared side effect. Until that path has real evidence, return
`browser_requires_setup` with a human action and leave the profile untouched.

## Platform Rollout

| Platform | Initial scope | Required proof before support | Unsupported/refusal behavior |
| --- | --- | --- | --- |
| macOS | Chrome first; Edge only after separate evidence | Bundle/team identity, process fingerprint, CGWindow ownership, exact AX prompt semantics, PID-owned loopback endpoint, exact native/CDP rebind | Refuse unrecognized prompt or browser build |
| Windows | Chrome and Edge after a UIA feasibility spike | Authenticode/process identity, HWND ownership, UIA modal/action semantics, exact PID listener, interactive desktop E2E | Refuse when prompt behavior or interactive session is unproven |
| Linux X11 | Chromium/Chrome after AT-SPI spike | Executable/process identity, `_NET_WM_PID`, exact geometry, AT-SPI modal/action semantics, `/proc` socket inode owner | Refuse unrecognized desktop/browser combinations |
| Linux Wayland | Exact compositors only, beginning with validated Sway/GNOME paths | Exact compositor pid/window identity plus AT-SPI prompt proof | Generic Wayland remains read-only or returns `browser_route_unavailable` |

Support is per browser version range and desktop environment, not inferred from
the existence of AX/UIA/AT-SPI alone.

## Internal File Map

### Shared core

| Path | Planned responsibility |
| --- | --- |
| `crates/cua-driver-core/src/browser/platform.rs` | Tagged prepare strategy, approval scope, platform consent/identity contracts |
| `crates/cua-driver-core/src/browser/approval.rs` | Existing-profile request digest and single-use approval artifact validation |
| `crates/cua-driver-core/src/browser/grant.rs` (new) | In-memory grant store, expiry, scope, revocation, session cleanup |
| `crates/cua-driver-core/src/browser/reconnect.rs` (new) | Bounded state machine, singleflight, cancellation, endpoint generations |
| `crates/cua-driver-core/src/browser/mutation.rs` (new) | Canonical-target mutation gates and reconnect exclusion |
| `crates/cua-driver-core/src/browser/cdp_ws.rs` | One browser-level socket per generation, flattened target sessions, generation-aware pool eviction |
| `crates/cua-driver-core/src/browser/store.rs` | Generation on target/tab/ref records and browser-wide invalidation |
| `crates/cua-driver-core/src/browser/engine.rs` | Coordinator integration, rebind and mutation gate lifecycle |
| `crates/cua-driver-core/src/browser/tools.rs` | Additive schema/result fields and honest type completion metadata |
| `crates/cua-driver-core/src/browser/refusal.rs` | Additive reconnect, revocation, and incomplete-input refusals |
| `crates/cua-driver-core/src/server.rs` and `crates/cua-driver/src/serve.rs` | Operation-bound host approval broker; no automatic existing-profile approval |
| `crates/cua-driver/src/cli.rs` | Interactive existing-profile approval request and exact scope display |

### Platform adapters

Each `*BrowserPlatform` becomes stateful enough to own a narrow consent adapter
constructed from the platform's existing internal desktop primitives. Do not
give core access to the full platform `ToolState`.

| Platform | Planned paths |
| --- | --- |
| macOS | `platform-macos/src/browser/platform.rs`, new `browser/consent_ui.rs`, registration in `tools/mod.rs`, reuse `ax` and `input/ax_actions.rs` |
| Windows | `platform-windows/src/browser_platform.rs`, new `browser_consent_ui.rs`, registration in `tools/impl_.rs`, reuse UIA semantic actions |
| Linux | `platform-linux/src/browser_platform.rs`, new `browser_consent_ui.rs`, registration in `tools/impl_.rs`, reuse AT-SPI and compositor identity paths |

### Tests and documentation

| Path | Planned responsibility |
| --- | --- |
| `cua-driver-core/src/browser/v2_tests.rs` | Fake grant, prompt, endpoint, generation, and concurrent mutation tests |
| `cua-driver-testkit/src/e2e.rs` | New refusal codes, browser attachment case declarations, evidence contract |
| `cua-driver/tests/standalone_browser_behavior_test.rs` | Disposable existing-like Chromium profile E2E and adversarial endpoint rows |
| `.github/workflows/e2e-rust-standalone-browsers.yml` | Maintainer-dispatched Windows/Linux compatibility evidence |
| `tests/runners/macos-lume/` | TCC-stable macOS real-browser evidence |
| `docs/content/docs/` | Diataxis tutorial, how-to, reference, and explanation updates |
| `rust/Skills/cua-driver/WEB_APPS.md` and platform skill docs | Agent flow, refusal handling, and platform limits |

## Validation Strategy

### Unit and model tests

1. Grant cannot be minted by a public Boolean, forged reserved argument, or
   ordinary MCP call.
2. CLI approval artifact is request-bound, single-use, expiring, and consumed
   on mismatch.
3. Grant cannot cross public session, transport session, daemon instance,
   browser identity, profile identity, or process fingerprint.
4. The state machine has no unbounded transition, retry, or wait.
5. Concurrent reconnect requests produce one leader, one socket, and at most
   one semantic prompt action per attempt.
6. Human dismissal revokes the grant and prevents an automatic retry.
7. Generation bump evicts the old socket and invalidates every old browser
   capability.
8. A mutation racing reconnect cannot continue on the new generation.
9. Same-target mutations serialize across sessions; distinct proven tabs may
   proceed concurrently.
10. Partial keystroke delivery reports exact requested and delivered counts.

### Adversarial tests

1. An unrelated process displays a pixel-identical consent prompt.
2. Another Chrome process or profile displays the real prompt.
3. A conventional debugging port belongs to an unrelated browser.
4. PID or port reuse occurs between approval, prompt observation, and socket
   completion.
5. The prompt becomes stale between snapshot and semantic press.
6. Two matching prompt candidates exist.
7. A localized browser exposes no stable semantic identifier.
8. Socket loss happens during click, navigation, typing, and file upload.
9. Browser restart preserves the product but changes profile identity.
10. Daemon restart reuses a stale approval request, artifact, target, tab, or
    ref.
11. Legacy page attempts to connect to a grant-owned endpoint.
12. Recording artifacts are scanned for profile paths, URLs, ports, endpoint
    tokens, page text, and approval artifacts.

### Real-browser E2E

Tests use a disposable repository-created profile that behaves like an
existing profile but contains no personal data. The driver does not own or
delete it during the test. Independent fixture journals verify page effects;
native sentinels verify focus, z-order, cursor, and leaked input where the
operation claims background behavior.

Required rows per supported platform/browser:

- first approved attach;
- repeated operations on one socket without another prompt;
- tab switch through flattened sessions without another socket;
- socket drop and successful prompt-assisted reconnect;
- browser restart with profile identity re-proof or exact reapproval refusal;
- user dismissal and grant revocation;
- decoy prompt and unrelated endpoint refusal;
- multi-window ambiguity refusal;
- stale target/tab/ref refusal after generation change;
- long and concurrent `browser_type` completion guarantees;
- artifact/video privacy validation.

## Issue Grouping

| Issue | Relationship | Landing decision |
| --- | --- | --- |
| [#2192](https://github.com/trycua/cua/issues/2192) | Direct existing authenticated-profile tracker | Closed by this implementation stack |
| [#2255](https://github.com/trycua/cua/issues/2255), [#2256](https://github.com/trycua/cua/issues/2256) | Partial and concurrent native text delivery; same bug class applies to browser mutations | Browser regression guarantees in this stack; native fixes remain separate |
| [#2176](https://github.com/trycua/cua/issues/2176) | Web fallback should prefer a background browser route over foreground native input | Update recommendation/escalation to typed browser tools in this stack; close only when live behavior matches |
| [#2240](https://github.com/trycua/cua/issues/2240) | CDP file upload action | Separate stacked PR after mutation gates land |
| [#2084](https://github.com/trycua/cua/issues/2084) | Legacy page typing parity | Mark superseded by shared browser tools where applicable; legacy delegation remains separate |
| [#2245](https://github.com/trycua/cua/issues/2245) | Legacy macOS page fallback provenance | Separate legacy migration/cleanup PR |
| [#2201](https://github.com/trycua/cua/issues/2201), [#2202](https://github.com/trycua/cua/issues/2202) | Native Chrome click/drag behavior | Linked compatibility issues, not closed by browser attachment |
| [#1616](https://github.com/trycua/cua/issues/1616) | Electron/Chromium native accessibility exposure | Separate native/embedded surface issue |

These issues should be addressed in the same initiative where they share core
primitives, but not placed in one code review. A single large pull request would
mix approval security, transport lifecycle, mutation correctness, three native
accessibility stacks, and legacy migration.

## Proposed Pull Request Sequence

| PR | Scope | User-visible change | Exit gate |
| --- | --- | --- | --- |
| 1 | Approval and schema contract | `existing_profile` request returns a real approval requirement; no attachment yet | Security review; forged/replayed/cross-session approval tests pass; isolated behavior unchanged |
| 2 | Reconnect coordinator and connection generations | No advertised platform support yet | Fake-CDP state machine, singleflight, cancellation, prompt-dismissal, generation invalidation, and pool ownership tests pass |
| 3 | Mutation gates and honest typing | Browser mutations serialize by canonical target; partial type is explicit | Concurrent same-tab/cross-session tests, cross-tab parallel test, mid-write failure counts, reconnect race tests pass |
| 4 | macOS Chrome existing-profile adapter | Experimental user-approved attach and reconnect on macOS | Lume real-Chrome first attach, repeat, socket loss, restart/refusal, decoy, dismissal, privacy, and regression rows pass |
| 5 | Windows Chrome/Edge adapter | Experimental support for products proven by the UIA spike | Interactive Windows E2E matches PR 4 evidence; unsupported product builds refuse |
| 6 | Linux X11 and exact Wayland adapters | Experimental support only for validated browser/desktop combinations | X11 and each advertised compositor pass real-browser rows; generic Wayland refusal is tested |
| 7 | Docs, skills, diagnostics, and issue reconciliation | Public setup/reconnect guidance and accurate support matrix | Diataxis docs, bundled skills, schemas, diagnostics, and issue states agree with evidence |
| 8 | Stacked file upload from #2240 | Typed browser file-input action through the mutation gate | CDP fixture verifies files, count, input/change events, path validation, and reconnect race behavior |

No incomplete platform is advertised between PRs. Experimental registration or
capability reporting remains disabled until that platform's E2E exit gate is
accepted.

## Documentation Deliverables

Use the Diataxis structure already established for CUA Driver:

### Tutorial

- Attach to a disposable existing Chromium profile, approve once, bind a tab,
  and survive a reconnect.

### How-to guides

- Attach to an authenticated existing profile safely.
- Handle `browser_consent_required`, revocation, and reconnect exhaustion.
- Rebind after capability invalidation.
- Validate existing-profile behavior with the Rust harnesses.

### Reference

- `browser_prepare` strategy schema and result fields.
- Grant, connection generation, target, tab, and ref lifetimes.
- Structured refusal vocabulary.
- Browser/product/platform support matrix and exact evidence links.
- Recording and telemetry redaction contract.

### Explanation

- Why an existing profile is not a driver-owned profile mode.
- Why one approval does not imply a permanent ambient permission.
- Why reconnect invalidates browser capabilities.
- Why the driver may press one exact browser consent action but may not approve
  arbitrary dialogs.
- Why generic Wayland and unproven browser versions refuse mutation.

## Review Questions

Reviewers should explicitly decide these points before PR 1 implementation:

1. Is the host approval provider a daemon-native prompt, an MCP host callback,
   an MCP elicitation flow, or an initial CLI-only contract? Every supported
   path must prove a real user action.
2. Should v1 automate Chrome's internal remote-debugging toggle after approval,
   or require the endpoint to be enabled manually and automate only connection
   prompts?
3. Which exact browser/profile identity can each platform prove without
   disclosing a path?
4. Is browser restart under the same grant required for v1, or enabled only on
   platforms that prove the same opaque profile identity?
5. Should `browser_input_incomplete` include a caller-retry hint when repeating
   the remaining suffix may be unsafe for stateful editors?
6. Should [#2240](https://github.com/trycua/cua/issues/2240) land immediately
   after the mutation gate or wait until macOS existing-profile evidence is
   accepted?

## Recommendation

Proceed with PRs 1 through 4 first. They establish the security boundary,
transport lifecycle, mutation correctness, and one fully evidenced platform.
Do not start Windows or Linux implementation until the macOS route proves that
one approved browser-level socket, exact prompt semantics, generation
invalidation, and privacy-bounded evidence work together in a real browser.

Treat [#2240](https://github.com/trycua/cua/issues/2240) as the first consumer of
the mutation gate in a separate stacked PR. Keep legacy page migration and
native Chrome input defects linked but independent. This preserves one coherent
browser initiative without recreating a single oversized pull request.
