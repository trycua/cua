# Cua Driver Permission Modes, Capability Policy, and Human Consent Plan

## Status

- **State:** Reviewed draft / local; implementation plan, not yet a plan of record
- **Plan date:** 2026-07-20
- **Base:** exact `origin/main` commit `767acf25f25ea668ffab428e4f2e8985896de98e`
- **Source version:** Cua Driver `0.9.1`
- **Scope:** A driver-wide authorization model for MCP, CLI, raw-socket, and
  embedded use, with `existing_profile` browser attachment as the first
  high-risk capability migrated to it
- **Related:**
  [existing-profile attachment plan](browser-existing-profile-attachment-plan.md),
  [browser tool implementation plan](browser-tool-implementation-plan.md),
  [permission-policy concept](../../../docs/content/docs/concepts/how-permission-policies-work.mdx),
  and [permission-policy reference](../../../docs/content/docs/reference/cua-driver/permission-policies.mdx)

### Implemented contract amendment

The implementation keeps this plan's three semantics but uses `bounded` as
the canonical middle-mode value; `autonomous` remains a compatibility alias.
This avoids conflating permission scope with an agent loop's task-continuation
autonomy. For an interactive CLI launch, `--dangerously-bypass-approvals`
selects `unrestricted` and serves as the explicit risk acknowledgement.
Programmatic and embedded launchers retain the two-part
`CUA_DRIVER_PERMISSION_MODE=unrestricted` plus
`CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS=1` contract. References to
`autonomous` below preserve the terminology used when this reviewed plan was
written.

This plan preserves the existing browser identity, endpoint, generation,
reconnect, and cleanup work. The earlier attachment plan correctly rejects the
generic MCP host marker as evidence of a human decision. This plan extends it
by downgrading the assurance of the same-user-writable, file-backed TTY
artifact and adding protected approval collectors plus driver-wide modes.

## Definition of done

This project is complete when Cua Driver has one coherent authorization model
in which:

1. `standard` is the default mode and asks through a protected channel only
   when a capability or action needs human approval.
2. `autonomous` runs without routine prompts inside a human-approved,
   technically enforced capability envelope.
3. `unrestricted` is an explicit launch-time bypass that the agent cannot
   enable for itself and that is clearly unsafe outside an isolated
   environment.
4. The existing YAML/Rego permission engine remains the maximum capability
   boundary in every mode; a mode can never turn a policy denial into an
   allow.
5. `existing_profile` cannot attach from an agent-facing transport unless a
   protected approval or an already-valid autonomous/unrestricted grant covers
   the exact operation.
6. Every live high-risk grant is visible and revocable, and revocation stops
   in-flight work, disconnects owned transports, and clears derived
   capabilities.
7. The daemon fails closed for configured policy errors, records the effective
   mode and policy provenance, and has adversarial tests proving that the
   model/tool channel cannot elevate either.
8. Documentation accurately states the boundary: the daemon protects against
   agents limited to its protocol. An agent with arbitrary code execution as
   the same OS user can bypass the daemon unless the daemon, policy, socket,
   browser, and agent are separated by an OS sandbox, service identity, VM, or
   equivalent external boundary.

Delivery requires merged code, cross-platform evidence for each advertised
collector, release documentation, and post-install smoke tests. A local plan,
passing unit tests, or a draft pull request is only a checkpoint.

## Executive decision

Adopt three user-facing modes:

| Mode | Intended experience | Security meaning |
|---|---|---|
| `standard` | One protected approval for a high-risk resource, then additional approval only for consequential actions or scope expansion | Human remains in the loop at defined boundaries |
| `autonomous` | One protected session-start approval, then unattended work inside a declared capability envelope | Prompts are replaced by narrow, pre-authorized technical policy—not by model judgment |
| `unrestricted` | No runtime approval prompts after explicit trusted launch | User accepts prompt-injection and unintended-action risk; the same built-in capability ceiling, managed/user policy, and hard integrity controls still apply |

`yolo` may be a deliberately alarming CLI alias, but `unrestricted` is the
canonical configuration value. The full flag should be
`--dangerously-bypass-approvals`.

Do not add a second permission-policy system. The July 2026 YAML/Rego engine is
the capability foundation. Add a separate mode and consent layer around it.

## Terminology and authorization equation

The design separates concepts that are easy to conflate:

- **OS permission:** TCC, Accessibility, Screen Recording, Windows integrity,
  portal access, and similar grants from the operating system to Cua Driver.
- **Capability policy:** the maximum set of tools, arguments, resources, and
  outputs an agent may use. YAML/Rego currently covers tools and input
  arguments only.
- **Permission mode:** when permitted work must stop for a human.
- **Consent grant:** a bounded authorization for a specific subject, resource,
  scope, purpose, and time.
- **Hard invariant:** an integrity rule that no mode or policy can disable.

The new `PermissionMode` name is an agent-authorization concept. It is
unrelated to the existing macOS TCC startup “permissions gate,”
`--no-permissions-gate`, and `CUA_DRIVER_RS_PERMISSIONS_GATE`; documentation
and diagnostics must label the latter **OS permissions** to avoid ambiguity.

The effective decision is an intersection:

```text
hard invariants
  AND managed capability ceiling
  AND user/session capability policy
  AND mode-specific consent/grant decision
  AND live resource identity proof
```

Evaluation rules:

1. A hard-invariant failure always refuses.
2. A managed-policy denial always refuses.
3. A user/session-policy denial always refuses.
4. If policy permits the call, the mode decides whether a protected approval
   or existing grant is also required.
5. Approval never widens policy; it only satisfies an `ask` requirement inside
   the policy envelope.
6. Resource identity is revalidated immediately before mutation and after
   reconnect, regardless of mode.

## Threat model

### In scope

Treat the calling model and any content it observes as untrusted. The agent
may:

- emit arbitrary MCP arguments and raw-socket bytes;
- run the documented CLI through an agent-controlled shell;
- own or synthesize a PTY and type `APPROVE`;
- encounter indirect prompt injection in web pages, documents, messages,
  terminal output, screenshots, and accessibility trees;
- replay, reorder, race, or substitute requests and session identifiers;
- attempt to hide indicators or invoke tools against Cua Driver's own UI;
- trigger redirects, popups, downloads, uploads, clipboard reads, external
  messages, purchases, account changes, and destructive actions through
  apparently ordinary clicks or keystrokes.

The primary protected deployment assumes the agent can reach Cua Driver only
through an authenticated/authorized daemon transport and cannot modify the
daemon binary, policy, launcher, approval broker, or OS security state.

### Out of scope without external isolation

The in-process policy engine is not a boundary against arbitrary same-user
native code. A local agent with unrestricted shell execution can call OS input
APIs directly, read same-user files, start a second driver, attach to exposed
debugging endpoints, or modify user-owned launch configuration.

For that threat, require at least one external boundary:

- a separate OS user or service identity for the trusted daemon;
- a sandbox that prevents the agent from changing launcher/policy files,
  opening the daemon's private broker channel, or invoking equivalent OS APIs;
- a VM/container with a dedicated browser profile;
- managed device policy; or
- approval on a second device.

`unrestricted` never claims protection from prompt injection. On a personal
logged-in profile it is an explicit acceptance of account-action and data-loss
risk, not a secure mode.

## Current-state findings

### What is already strong

- The daemon evaluates an active YAML/Rego policy before tool execution for
  MCP, CLI, and raw-socket traffic.
- Active policies are process-lifetime snapshots and deny unmentioned tools by
  default.
- Browser attachment artifacts are operation-bound, short-lived, single-use,
  and consumed before validation.
- Existing-profile grants bind session, process, native window, fingerprint,
  endpoint, generation, and reconnect budget.
- Existing-profile reconnect re-proves endpoint/process identity and
  invalidates stale capabilities.
- Session teardown and idle eviction already provide lifecycle hooks that can
  revoke grants and remove UI.

### Gaps to close

| Priority | Gap | Consequence |
|---|---|---|
| P0 | Existing-profile approval is unsigned JSON in a same-user-writable temporary directory; its schema and UUID token format are public | Any same-user process can forge the artifact directly without invoking `browser-approve`; an agent-controlled PTY is a second, weaker forgery path |
| P0 | Generic MCP provenance is injected for `browser_prepare` only for active proxy sessions, with a parallel marker for downloads | It is not proof that a person approved; existing-profile correctly rejects it, and the mechanism must not be generalized as consent |
| P0 | No policy, or a configured policy path that does not exist, permits all calls | A typo or missing mount silently becomes unrestricted operation |
| P0 | Mode/policy selection is not protected from a same-user local agent | A shell-capable agent may restart an unprotected daemon unless launch ownership is separated |
| P1 | Policy decisions are only `Allow`, `Deny`, or `Error` | There is no protected `ask`/grant resolution layer |
| P1 | The policy engine constrains tool inputs but not responses, navigation caused by clicks/redirects, quotas, or caller identity | Data access and exfiltration can occur after an apparently allowed call |
| P1 | There is no persistent, non-optional high-risk-session indicator or emergency stop | Consent is not visibly ongoing or easily revocable |
| P1 | The agent cursor can be hidden, customized, or disabled | It is useful feedback but cannot serve as the security indicator |
| P1 | Generic low-level clicks and typing do not reveal semantic consequences | A policy cannot reliably infer purchases, sends, deletions, or account changes from coordinates alone |
| P2 | Generic MCP elicitation does not mandate a protected human-only UI | A client, hook, or agent-host implementation may synthesize `accept` |
| P2 | The policy engine does not authenticate callers | Different agents sharing a socket/process can inherit the same capability ceiling |
| P2 | Audit telemetry is not yet a user-visible authorization ledger | Incident review cannot reconstruct grants, denials, scope changes, and revocations |

## Mode contract

### `standard` — default

`standard` permits ordinary work inside policy without prompt fatigue but
requires protected consent for:

- attaching to a logged-in or user-owned profile;
- first access to credentials, private data, clipboard reads, microphone,
  camera, or other sensitive sources;
- sending external messages, publishing, purchases, account/security changes,
  destructive operations, and irreversible submissions;
- expanding to a new app, profile, window, tab, origin, destination class, or
  data sink outside the current grant;
- renewal after expiry, daemon restart, identity change, OS lock, or explicit
  revocation.

Ordinary read/navigation/input inside the approved resource and scope does not
prompt again. Exact-identity reconnect does not prompt again while the grant is
live and within its reconnect budget.

### `autonomous` — bounded unattended operation

The human approves a session manifest before work begins. The manifest names:

- agent/host identity where available;
- requested duration and idle TTL;
- permitted apps, profiles, windows, tabs, origins, paths, and destinations;
- allowed capability classes and relevant argument/output bounds;
- external side effects that are permitted, denied, or still require a human;
- data-egress policy and rate/volume limits.

Inside that manifest, calls run unattended. Scope expansion stops and asks; it
never self-amends. High-consequence categories not explicitly present are
denied, not inferred from the task prompt.

This is the preferred long-running mode. It provides the productivity goal of
Codex/Claude-style automation while retaining a technical sandbox.

### `unrestricted` — explicit bypass

`unrestricted` bypasses interactive Cua approval prompts, but it does not
bypass:

- managed policy;
- daemon/process/endpoint identity proofs;
- session ownership and generation invalidation;
- persistent indicator and emergency revocation;
- audit records;
- cleanup, bounded retries, and circuit breakers;
- OS permissions or platform security controls.

It must be enabled outside the tool protocol through a trusted launcher,
native settings surface protected by OS authentication, or managed
configuration. It cannot be entered mid-session. The daemon must restart, show
a high-severity warning, record the mode, and default back to `standard` on the
next ordinary launch.

Administrators can disable the mode entirely. On a normal personal profile the
UI must state that it provides no defense against prompt injection. Product
documentation should recommend it only for disposable profiles, VMs, or other
externally isolated environments.

### Built-in baseline and compatibility boundary

All three modes use the same built-in capability ceiling. Mode changes prompt
resolution, not which classified capabilities exist. Managed and user policy
can narrow this ceiling; neither approval nor `unrestricted` can widen it.

The built-in ceiling is reviewed and deny-by-default at the classification
boundary:

- R0 public metadata is included.
- R1 reversible control is included, with no prompt when it targets a
  driver-owned isolated resource.
- R2 observation/control is included but requires a resource grant in
  `standard`, a covering manifest in `autonomous`, and no prompt in
  `unrestricted`.
- R3 consequential operations are included only for typed, reviewed tools and
  require an action grant in `standard` or explicit manifest entry in
  `autonomous`.
- R4/open-ended operations are included only when their implementation has a
  reviewed enforcement adapter. `standard` uses takeover or exact approval;
  `autonomous` requires a separately explicit entry; `unrestricted` runs them
  without Cua prompts.
- New or unclassified tools/sub-operations are denied in every mode until risk
  metadata and an enforcement mapping are reviewed.
- Hard-invariant targets, including Cua's own approval/revocation UI, are never
  included.

Before changing the default, maintainers must publish and test the exact
current-tool inventory mapped to R0-R4, including legacy aliases and platform
stubs. To limit breakage, all current tools remain representable in the
ceiling, but `standard` may add prompts or refusals where current behavior is
unprotected. The compatibility sequence remains opt-in, warning, then an
announced default flip.

## Risk and intervention model

Classify capabilities, not model prose:

| Class | Examples | `standard` | `autonomous` | `unrestricted` |
|---|---|---|---|---|
| R0: public metadata | tool schema, display size, driver version | Allow by policy | Allow by policy | Allow without prompt, inside the same ceiling |
| R1: local reversible control | inspect a test app, click/type in an isolated profile | Allow inside scope | Allow inside manifest | Allow without prompt, inside the same ceiling |
| R2: sensitive observation/control | screenshots, clipboard read, user documents, logged-in browser | Approve initial resource grant | Session-start manifest approval | Allow without prompt, inside the same ceiling; warn |
| R3: consequential external action | send message, publish, upload, download, submit form | Action confirmation unless explicitly pre-authorized | Allow only if named in manifest; otherwise deny/ask | Allow without prompt only if the operation remains inside the same ceiling |
| R4: critical/irreversible | credentials, payments, security settings, account deletion, legal acceptance | Human takeover or exact confirmation at the downstream action | Deny by default; require separately explicit grant | Allow without prompt only if inside the same ceiling; no safety claim |

This classification is metadata attached to typed operations. It must not rely
only on an LLM or classifier guessing what a generic click means.

For generic pixel/keyboard tools, Cua cannot promise semantic interception.
Therefore, when such tools target an authenticated or sensitive application:

1. bind them to an approved app/window region;
2. deny interaction with Cua-owned consent/indicator UI as a hard invariant;
3. use typed downstream tools for known R3/R4 actions where possible;
4. require takeover or a broader explicit grant when semantic action identity
   cannot be proven;
5. document that unrestricted generic input can perform any action visible to
   that application.

## Relationship to the YAML/Rego permission engine

Keep the existing policy contract backward-compatible: policy answers whether
a call is inside the capability envelope. A new authorization coordinator then
resolves consent.

```rust
enum CapabilityDecision {
    Allow,
    Deny(String),
    Error(String),
}

enum AuthorizationDecision {
    Allow { grant_id: Option<GrantId> },
    RequireApproval(ApprovalRequest),
    Deny(String),
    Error(String),
}

// Coordinator composition, not caller-controlled input:
// (CapabilityDecision::Allow, typed_risk, startup_mode, live_grant)
//     -> AuthorizationDecision
```

Do not force existing Rego policies to return a new ternary value in the first
release. `data.cua.policy.allow: boolean` remains valid. Risk metadata and mode
resolution occur after policy evaluation.

### Policy composition

Load and intersect these layers:

1. **Hard invariants:** compiled code, never configurable.
2. **Managed policy:** administrator-owned; user and agent cannot widen it.
3. **User policy:** optional YAML/Rego file; can only narrow managed policy.
4. **Session manifest:** autonomous grant; can only narrow the prior layers.

The daemon reports hashes and provenance for every active layer, never the
policy contents or sensitive arguments in telemetry.

### Fail-closed changes

- If `CUA_DRIVER_POLICY_FILE` is explicitly set and missing, unreadable, empty
  in an unintended way, or unsupported, daemon startup fails.
- Absence of a user file must no longer implicitly mean `unrestricted` once
  modes reach general availability. `standard` loads a built-in baseline.
- `--permission-mode unrestricted` suppresses prompts within the same built-in
  ceiling; it does not select a broader policy. Managed policy may reject the
  mode.
- The daemon eagerly loads and validates policy and mode before binding the
  public socket. This requires moving current lazy `OnceLock` initialization
  ahead of `UnixListener::bind`, not merely changing the missing-path branch.
- Status/health output reports mode, policy-active booleans, hashes, collector
  availability, and whether the launcher is protected.
- No MCP tool may reload policy, change mode, enable bypass, or mint a grant.

### Policy coverage extensions

Add enforcement beyond `(tool, input arguments)`:

- caller/transport identity and session ownership;
- resource identity (app, PID, window, profile, tab, origin);
- derived navigation, redirects, popups, and downloads;
- output classification and bounds for screenshots, DOM/text, clipboard,
  files, and browser data;
- destination/egress policy for uploads, messages, and network requests;
- per-session rate, count, and byte quotas where meaningful.

Rego can receive a sanitized authorization context, but secrets, raw page
content, cookies, typed text, and full paths must not be included by default.

### MVP authorization vocabulary

The repository already exposes a versioned dotted capability vocabulary from
the centralized `default_capabilities_for` map in `tool.rs`. The first
autonomous manifest uses canonical existing tool names plus that live map; it
does not invent tokens outside the advertised vocabulary. Static reviewed risk
metadata maps each canonical tool—and narrowly defined sub-operations where
the implementation can prove them—to R0-R4 before mode resolution.

Resource bindings such as PID, window, profile, tab, origin, and destination
remain typed grant fields enforced by the relevant adapter; they are not
reduced to regexes over arbitrary JSON. A later driver-wide phase may add
tokens such as `browser.cookie.read`, but only after every additive name has a
complete tool/sub-operation mapping, input/output semantics, and tests. New
vocabulary cannot be used in an accepted manifest until enforcement exists.

## Protected consent architecture

### Approval request

The daemon creates an internal request containing:

- unguessable nonce and monotonically increasing generation;
- daemon instance identity;
- requesting host/transport identity where proven;
- mode and policy hashes;
- operation and risk class;
- exact resource subject (for example PID, fingerprint, window, profile, tab);
- human-readable scope, side effects, destinations, and duration;
- request digest and expiry.

The agent may receive a correlation ID and a redacted description. Possession
of either never grants authority.

### Approval broker

Only an `ApprovalBroker` can resolve `RequireApproval`. The broker uses a
private daemon-owned channel with OS peer credentials and returns the decision
directly to the daemon. It never emits a bearer token into MCP results, CLI
stdout, inherited stdin, environment variables, or files readable by the
agent.

The collector contract is pluggable. The MVP collector is a trusted host UI;
OS authentication and second-device flows are optional stronger or hostless
collectors that can ship independently:

| Environment | Collector | Required property |
|---|---|---|
| Supported agent host | `ProtectedHostCollector` rendered by Codex, Claude, or another recognized client | Human response is outside the model/tool stream, hooks cannot auto-accept, and the authenticated result is bound to the exact request digest |
| macOS GUI (optional) | `LAContext` biometric approval plus daemon-owned explanatory UI | Enrolled biometric binds the decision to the device owner rather than merely a person at the trusted host |
| Windows GUI (optional) | Windows Hello/User Consent flow in a trusted daemon-owned UI | OS-authenticated result bound to the request |
| Linux desktop (optional) | polkit action with a registered authentication agent | Authenticated action through the desktop agent, with exact Cua action identity |
| Headless (optional) | second-device/device authorization flow | Approval occurs on a separately authenticated device and is bound to the request digest |

Biometric authentication is not an MVP dependency. It strengthens the claim
from “a person using the trusted host approved” to “the enrolled device owner
approved,” and provides a possible local collector when no trusted host exists.
If no recognized trusted host or optional protected collector is available,
`existing_profile` refuses in `standard` and `autonomous`. It never falls back
to passcode, TTY, or an agent-hosted confirmation. `unrestricted` remains the
explicit exception after trusted launch-time risk acceptance.

`/dev/tty` typed `APPROVE` is not a high-assurance collector when the agent can
own a PTY or drive the terminal. It may remain temporarily as a clearly labeled
legacy compatibility path for isolated-profile launch, but it must not
authorize `existing_profile` under `standard` or `autonomous`.

### MCP elicitation

Generic `elicitation/create` is a transport feature, not automatically a
protected consent boundary. The MCP specification permits clients to choose
their interaction model, and automation/hooks may synthesize a response.

Use elicitation only when the client negotiates a Cua-recognized protected
approval-provider capability that guarantees:

- the agent/model cannot generate or modify the response;
- programmatic hooks cannot auto-accept protected requests;
- the UI identifies Cua Driver and the exact operation;
- accept/decline/cancel provenance is authenticated and request-bound;
- replay and cross-client substitution are rejected.

Without that contract, use an optional daemon-native or second-device
collector if one is installed; otherwise refuse the request in `standard` and
`autonomous`.
Form elicitation may present a confirmation but must never request passwords,
API keys, cookies, or biometrics.

### Grant properties

All grants are:

- in-memory by default;
- bound to daemon instance, policy hashes, mode, subject identity, resource,
  session, scope, and purpose;
- short-lived, with idle and absolute TTLs appropriate to risk;
- monotonically generated so stale capabilities fail;
- revocable and cancellation-aware;
- non-transferable between transport sessions unless the approved manifest
  explicitly identifies the shared host;
- auditable by opaque ID without exposing a bearer credential.

The current five-minute artifact TTL, 30-minute idle TTL, and eight-hour
absolute browser grant are starting values, not universal defaults. R3/R4
action confirmations should normally be single-use and expire in minutes.

## Persistent indicator and revocation

Every R2+ grant creates persistent visible state owned by the trusted daemon:

- agent/session identity and color;
- current mode;
- controlled app/profile/window and, where possible, origin;
- idle and absolute expiry separately, with the idle countdown visibly reset
  only by activity that legitimately refreshes the grant;
- **Stop** and **Details** controls;
- high-severity styling for `unrestricted`.

The indicator is separate from the agent cursor. The cursor remains optional
activity feedback; the security indicator cannot be hidden, recolored into
invisibility, or disabled while a high-risk grant is live.

Add three revocation paths:

1. persistent UI **Stop**;
2. registered emergency keyboard shortcut as a human convenience;
3. `cua-driver revoke [--session <id>|--all]` from a trusted local operator.

The daemon must reject actions targeting its own broker, indicator, status
item, or protected screen regions. Revocation closes browser/debug transports,
cancels in-flight mutations, invalidates refs/targets, clears clipboard or
temporary data owned by the grant, invokes session cleanup, and records the
reason.

The shortcut is not an authorization boundary because an input-capable agent
may synthesize the same keys. Enforcement comes from the protected Stop broker,
grant revocation, and self-target refusal. On a surface where a persistent
tamper-resistant indicator cannot be rendered, R2+ grants are unavailable
unless the approving out-of-band device or host provides a continuously
reachable indicator and Stop control bound to the grant.

This is a meaningful boundary only for agents constrained to the daemon. Same-
user arbitrary native code still requires external isolation.

## Existing-profile browser migration

### Default behavior

- `isolated_new` remains low-friction and uses the built-in standard policy.
- `isolated_named` is treated as potentially persistent/sensitive and may need
  an initial R2 grant depending on whether it contains authenticated state.
- `existing_profile` is R2 at attach and may produce R3/R4 actions afterward.
- In `standard`, `existing_profile` requires trusted consent before attach.
- In `autonomous`, `existing_profile` requires a trusted approval covering it
  in the session manifest; subsequent in-scope use is unattended.
- `unrestricted` may attach without runtime approval after explicit trusted
  launch-time risk acceptance. It cannot be entered or enabled by the running
  agent.
- If trusted consent is unavailable, `existing_profile` is unavailable in
  `standard` and `autonomous`; it must not fall back to TTY approval.

### Attach scope

An existing-profile grant binds at least:

- exact browser PID and process fingerprint;
- browser product and proven profile identity where available;
- native approval window and initial exact tab;
- public session and authenticated transport identity;
- endpoint owner, CDP generation, reconnect budget;
- permitted origins/destination classes;
- allowed browser capability classes;
- idle/absolute expiry and policy hashes.

The profile-wide CDP transport does not imply profile-wide authorization.
Target attachment and command dispatch must be limited to tabs/resources
covered by the grant.

### Data and navigation controls

- Do not expose cookie, credential, password, history, or unrestricted storage
  extraction APIs to ordinary browser sessions.
- Disable or separately gate legacy `page`, generic JavaScript, raw CDP, and
  shell routes that could bypass typed browser restrictions.
- Apply origin policy to every top-level navigation, including redirects,
  `window.open`, target creation, history traversal, form submission, and
  server/client-driven navigation—not only `browser_navigate(url)` arguments.
- Pause or detach on unapproved cross-origin expansion.
- Gate downloads, uploads, file pickers, clipboard, credential entry,
  notifications, external-protocol launches, and permission prompts as
  separate capabilities.
- Never return secrets in refusal details, logs, recordings, or policy input.

### Consequential actions

Typed browser tools should surface known submit/action metadata when it can be
proved, but Cua must not claim perfect semantic detection. For payments,
credentials, account/security changes, legal acceptance, and destructive
actions, use one of:

- human takeover with model screenshots/input suspended;
- downstream site/application integration that performs exact authorization;
- exact single-use confirmation immediately before the action; or
- an explicit `unrestricted` risk acceptance.

Model-emitted “are you sure?” text and classifiers may reduce mistakes but are
not authorization boundaries.

### Transport direction

Keep the current CDP transport while this consent work lands. Independently
spike `chrome.debugger` extension/native-messaging transport. Current Chrome
documentation lists the `Input` domain, so the spike should now test actual Cua
command coverage, target/window identity, extension install/update trust,
DevTools conflicts, service-worker lifecycle, enterprise controls, and
cross-platform packaging rather than treating Input availability as unknown.

Do not block the permission-mode project on that transport. If it proves
viable, it is attractive for the real default profile because Chrome owns the
debugger attachment UI and revocation semantics.

## Driver-wide capability rollout

After browser attachment, migrate capability groups in this order:

1. **Screen and accessibility observation:** screenshot, window state, OCR,
   accessibility text, recordings.
2. **Input and application control:** mouse, keyboard, app launch/quit,
   foreground changes, desktop-absolute input.
3. **Clipboard and file transfer:** clipboard read/write, downloads, uploads,
   file pickers.
4. **Browser-authenticated control:** existing/named profiles, browser storage,
   cross-origin navigation.
5. **Sensitive devices:** microphone, camera, location, notifications.
6. **Open-ended execution and egress:** shell, JavaScript, raw protocol access,
   network destinations.
7. **Consequential workflows:** messages, publishing, purchases, security and
   account changes, deletion.

Each group needs typed risk metadata, resource scoping, output/egress controls,
grant lifecycle, indicator text, and adversarial tests before it is advertised
as protected.

## Proposed configuration surface

Illustrative CLI:

```text
cua-driver serve --permission-mode standard
cua-driver serve --permission-mode autonomous --session-policy /trusted/task.yaml
cua-driver serve --permission-mode unrestricted --dangerously-bypass-approvals
```

Rules:

- `--permission-mode` is accepted only by trusted daemon startup, never by
  `call`, MCP, raw tool arguments, or a session tool.
- `autonomous` without a protected session-manifest approval refuses to bind
  the public socket or starts in a pending state that serves only health.
- `unrestricted` requires both the mode and deliberately named danger flag in
  CLI environments; native UI uses OS authentication and explicit warning.
- Managed policy can restrict allowed modes, maximum TTLs, capabilities,
  collectors, profiles, apps, origins, and egress.
- Mode does not persist by default. Managed installations may persist it only
  in configuration the agent cannot modify.
- `status`, `doctor`, and `health_report` expose the effective mode and policy
  provenance without exposing sensitive policy contents.

Illustrative autonomous manifest:

```yaml
version: 1
mode: autonomous
expires_after: 8h
idle_timeout: 30m

resources:
  applications:
    - bundle_id: com.google.Chrome
  browser:
    profile: Work
    origins:
      - https://docs.example.com
      - https://app.example.com

allow:
  tools:
    - start_session
    - end_session
    - get_browser_state
    - browser_navigate
    - browser_click
    - browser_type
    - wait

deny:
  tools:
    - page
    - shell_execute

ask:
  tools:
    - browser_download
    - browser_set_input_files
```

`ask` belongs to the new session-manifest schema, not the existing YAML policy
schema. The manifest is compiled into an immutable in-memory authorization
layer and intersected with YAML/Rego and managed policy. The agent may propose
a manifest but cannot approve or install it.

## Human-intervention expectations

| Scenario | Expected human interventions |
|---|---:|
| Read-only task in an isolated browser under `standard` | 0 |
| Ordinary task in an already-approved logged-in profile under `standard` | 1 initial attach |
| Same-identity reconnect while grant is live | 0 |
| Send/publish/purchase/account change under `standard` | 1 immediately before each consequential action or takeover |
| Eight-hour bounded task under `autonomous` | 1 session-manifest approval; 0 inside scope |
| Autonomous scope expansion | 1 approval for the expansion |
| `unrestricted` daemon session | 1 explicit trusted launch enablement; 0 runtime prompts |

Expiry, daemon restart, browser identity change, OS lock/user switch, explicit
Stop, managed-policy change, and reconnect-budget exhaustion require a new
grant or produce a refusal.

## Implementation architecture

Add these core concepts without coupling them to one transport:

```text
AuthorizationCoordinator
  ├── CapabilityPolicyStack
  │     ├── HardInvariants
  │     ├── ManagedPolicy
  │     ├── UserPolicy (existing YAML/Rego)
  │     └── SessionManifest
  ├── RiskClassifier (typed metadata, not model authority)
  ├── ApprovalBroker
  │     ├── ProtectedHostCollector (MVP)
  │     ├── MacOSBiometricCollector (optional)
  │     ├── WindowsHelloCollector (optional)
  │     ├── PolkitCollector (optional)
  │     └── DeviceFlowCollector (optional)
  ├── Grants (generalized from ExistingProfileGrants/GrantLookup)
  ├── IndicatorController
  └── AuditSink
```

Primary code areas:

- `cua-driver-core/src/policy.rs`: policy stack, fail-closed loading,
  authorization context, and composition.
- `cua-driver-core/src/server.rs` and `cua-driver/src/serve.rs`: one canonical
  authorization call before registry dispatch; remove consent inference from
  generic transport provenance.
- `cua-driver/src/proxy.rs`: early denial only; the daemon remains canonical
  for grants and protected approval.
- `cua-driver-core/src/session.rs`: grant ownership, teardown, idle eviction,
  and transport identity.
- `cua-driver-core/src/browser/approval.rs`: replace public artifacts for
  `existing_profile` with broker-mediated grants; retain a compatibility path
  only for low-risk isolated launch if needed.
- `cua-driver-core/src/browser/grant.rs`, `prepare.rs`, `engine.rs`, and
  `store.rs`: consume generic grants, add origin/capability scope, and enforce
  revocation during attach/reconnect/action.
- platform crates: trusted collectors and protected indicator adapters.
- CLI/config/health schemas: startup mode, policy provenance, revoke/status.

Today policy evaluation is duplicated in `serve.rs`, `server.rs`, and the
separate stdio proxy process. Before adding the coordinator, refactor the two
daemon dispatch paths to call exactly one daemon-side authorization function
covering stdio, HTTP/MCP, CLI, and raw socket dispatch. The proxy keeps a
redundant early-deny check as defense in depth and can never mint or satisfy a
grant. The generic grant implementation should evolve
`ExistingProfileGrants`, `GrantLookup`, and `PrepareAuthorization` rather than
create a parallel browser-independent store.

## Failure behavior

| Failure | Required result |
|---|---|
| Policy explicitly configured but missing/unreadable | Daemon does not expose action socket |
| User policy unset during migration window | Preserve documented allow-all compatibility and emit migration status/warning |
| User policy unset after default flip | Load the reviewed built-in ceiling and `standard` mode |
| Policy parse/evaluation error | Fail closed; stable structured error |
| Unsupported/absent protected collector | Sensitive request refuses; no TTY fallback |
| Approval timeout/cancel/decline | No grant; pending operation canceled |
| Agent replays approval correlation ID | Refuse; ID is non-authoritative |
| Agent changes PID/window/session after approval | Refuse and consume/cancel request |
| Policy or mode changes | Existing incompatible grants revoked before new calls |
| Indicator fails to render | R2+ grant is not activated or is immediately revoked |
| Stop during in-flight action | Cancel where possible, report honest partial-delivery state, revoke derivatives |
| Redirect/popup leaves approved origin scope | Pause/refuse/detach before further input or data release |
| Browser/daemon restart | In-memory grant lost; require new approval |
| Agent targets Cua consent/indicator UI | Hard refusal independent of mode |

## Verification plan

### Unit and property tests

- Every combination of hard, managed, user, session, and mode decisions obeys
  intersection semantics.
- No `Deny` can become `Allow` after approval or mode resolution.
- Explicit missing policy paths fail startup; absent policy selects built-in
  `standard` after migration.
- Policy aliases and reserved-argument sanitization still work.
- Grants reject nonce replay, wrong daemon, wrong transport/session, wrong
  resource, wrong policy hash, expiry, and generation mismatch.
- Revocation is idempotent and invokes every cleanup hook exactly once.
- Mode cannot be changed through any tool schema, reserved argument, MCP
  metadata, environment injection into an existing process, or session revive.
- A same-user process that writes a syntactically valid legacy approval JSON
  file directly into the temporary artifact directory still cannot authorize
  `existing_profile` in `standard` or `autonomous`.
- Fuzz YAML/Rego inputs, authorization context sanitization, approval messages,
  and grant serialization boundaries.

### Adversarial integration tests

- Agent-controlled PTY types `APPROVE`; `existing_profile` remains denied.
- MCP client synthesizes elicitation `accept`; denied unless the protected-host
  capability and authenticated response are present.
- Approval for browser A is replayed against browser B, a new process
  generation, another window, or another session.
- Agent races Stop/revoke against connect, click, type, redirect, download, and
  reconnect.
- Agent tries to hide/click the indicator through element, pixel, background,
  foreground, and browser routes.
- Allowed navigation redirects or opens a target to a denied origin.
- Allowed browser tool attempts cookie/storage extraction through legacy
  `page`, JavaScript, raw CDP, shell, file, or response side channels.
- Same-user untrusted launch attempts are tested in the supported sandboxed
  deployment, and documentation explicitly marks unsandboxed bypasses.

### Real-platform acceptance

For every advertised platform/collector, record exact-SHA evidence for:

- accept, decline, cancel, timeout, unavailable collector;
- daemon restart and OS lock/user switch;
- persistent indicator and all Stop paths;
- standard initial attach plus prompt-free same-generation actions;
- autonomous manifest approval plus hours-long bounded activity;
- scope expansion, expiry, and revocation;
- unrestricted launch warning and non-escalation from a standard session;
- Chrome/Edge existing-profile attach, reconnect, wrong-target refusal, and
  cross-origin enforcement;
- installed artifact provenance and representative post-install smoke test.

No platform/host combination advertises protected `existing_profile` until its
selected collector and indicator rows pass in the real environment. Headless
support requires a certified protected host or second-device flow;
`/dev/tty` is not substituted.

## General-release gate and explicit deferrals

The initial protected-browser release requires all of the following:

1. Explicitly configured missing/unreadable policy fails before socket bind;
   unset remains compatible until the announced default flip.
2. One canonical daemon-side authorization call covers every transport; proxy
   checks are defense in depth only.
3. Mode selection and grant minting are unreachable from the model/tool
   channel.
4. The exact built-in tool/risk baseline and migration behavior are published
   and tested.
5. `existing_profile` cannot use the file-backed artifact in `standard` or
   `autonomous`; a protected local/host/device collector exists or the feature
   remains unavailable.
6. R2+ browser grants have a persistent indicator/Stop surface, including an
   out-of-band substitute where local UI is unavailable.
7. Existing browser identity, generation, reconnect, and teardown invariants
   continue to pass.
8. Browser-specific target/origin transitions and legacy/raw bypass routes are
   enforced and adversarially tested.

The following do not block the first-platform protected-browser release:

- a fine-grained driver-wide dotted capability taxonomy;
- generic output classification, egress quotas, and a user-visible audit
  ledger beyond the browser-specific controls above;
- collectors on platforms that remain explicitly unsupported;
- full caller authentication for every legacy transport; and
- the `chrome.debugger` transport spike.

They remain required before Cua advertises the corresponding driver-wide or
platform guarantee.

## Delivery sequence

### Phase 0 — immediate containment

1. Mark the current `existing_profile` approval path experimental.
2. Disable it by default for agent-facing production use unless an explicit
   legacy feature flag is set by a trusted launcher.
3. Change an explicitly configured missing policy path from allow-all to an
   error and eagerly validate policy/mode before socket bind. Keep an unset
   policy compatible during the migration window.
4. Document that the file-backed approval artifact is same-user-forgeable and
   is not a human-consent boundary.

Exit: no accidental policy typo or ordinary agent transport silently gets a
logged-in profile attachment.

### Phase 1 — mode and policy foundation

1. Refactor `serve.rs` and `server.rs` to call one canonical daemon-side
   authorization function; retain the separate proxy check as early denial.
2. Add `PermissionMode` and immutable startup provenance.
3. Add the reviewed built-in ceiling and managed/user policy intersection.
4. Add typed risk metadata and the canonical authorization coordinator.
5. Expose mode/policy health without secrets.
6. Prove every transport reaches the canonical daemon check.

Exit: standard/autonomous/unrestricted decisions are deterministic and the
agent cannot change them in-process.

### Phase 2 — approval broker and grants

1. Add broker protocol, request digest, private channel, grant store, and audit
   vocabulary.
2. Ship protected-host capability negotiation and certify the first Codex or
   Claude trusted consent UI; do not trust generic elicitation.
3. Add macOS biometrics, Windows Hello, polkit, and device flow as optional
   collectors behind independent availability gates.
4. When no certified collector is available, refuse `existing_profile` in
   `standard` and `autonomous`; never downgrade to TTY confirmation.

Exit: adversarial PTY and synthetic-MCP accept tests cannot mint a grant.

### Phase 3 — indicator and revocation

1. Add persistent status UI independent of agent cursor.
2. Add Stop, emergency shortcut, trusted CLI revoke, and self-target denial.
3. Wire every grant into session cleanup and in-flight cancellation.

Exit: no R2+ grant can remain live without indicator and revocation coverage.

### Phase 4 — migrate `existing_profile`

1. Replace public artifact consumption with generic protected grants.
2. Add tab/origin/capability scoping and redirect/popup enforcement.
3. Gate legacy/raw routes and sensitive browser data.
4. Certify attach, action, reconnect, expiry, and revocation on supported
   browsers/platforms.

Exit: one protected approval supports ordinary autonomous browser work, with
new intervention only for scope expansion or consequential actions.

### Phase 5 — autonomous and unrestricted UX

1. Add session manifests and human review UI.
2. Add managed limits and non-interactive deny behavior for undeclared actions.
3. Add explicit unrestricted startup, severe warnings, admin disable, and
   disposable-environment guidance.

Exit: long-running tasks need one bounded approval; bypass cannot be entered by
an already-running standard/autonomous agent.

### Phase 6 — driver-wide coverage

Migrate screen, input, clipboard/files, devices, shell/network, and
consequential typed workflows in the priority order above. Each group ships
only after input, output, egress, revocation, and live-platform tests pass.

## Pull-request strategy

Keep changes reviewable and release metadata accurate:

1. `fix(cua-driver): fail closed before binding for configured permission policies`
2. `refactor(cua-driver): unify daemon authorization dispatch`
3. `feat(cua-driver): add permission modes and policy composition`
4. `feat(cua-driver): add protected approval broker and grants`
5. `feat(cua-driver): add persistent authorization indicator and revocation`
6. `fix(cua-driver): require protected consent for existing browser profiles`
7. `feat(cua-driver): add bounded autonomous session manifests`
8. Platform collector PRs, split where implementation and evidence are
   independently reviewable.

Before each PR becomes ready, inspect its final files and live GitHub title,
apply the appropriate release label/type, and wait for release-metadata CI.

## Migration and compatibility

1. Release the explicit-missing-policy fail-closed fix immediately.
2. Add warnings and health diagnostics while the built-in standard policy is
   opt-in during one compatibility window.
3. Make `standard` the default in the next announced release boundary.
4. Preserve existing YAML/Rego boolean policy syntax.
5. Offer a migration command that prints a proposed policy/manifest but never
   installs or approves it without trusted user action.
6. Keep legacy TTY artifacts only for isolated-profile compatibility during a
   bounded deprecation window.
7. Do not silently map “policy absent” to `unrestricted`; require the explicit
   mode/danger flag after the migration boundary.

## Documentation requirements

- One concept page separating OS permissions, capability policy, mode, and
  grants.
- One how-to per mode with realistic intervention expectations.
- One managed-deployment guide explaining the required external boundary for
  local shell-capable agents.
- Browser guide warnings for logged-in profiles, prompt injection, raw data,
  and unrestricted mode.
- CLI/config/health reference generated from the implementation.
- Threat-model and limitation statements repeated near the relevant controls,
  not hidden in a security appendix.
- Upgrade notes for policy fail-closed behavior and the standard-default
  migration.

## Open questions requiring spikes

1. Is an optional macOS `LAContext` collector worth shipping for hostless use,
   and can the installed daemon present it reliably without passcode fallback?
2. What OS-authenticated protected-host attestation can Claude Code, Codex, and
   other MCP clients provide beyond generic elicitation?
3. Which indicator form remains persistent across Spaces, full-screen apps,
   RDP, Wayland compositors, and accessibility tooling without becoming an
   input obstruction?
4. Can Cua reliably enforce tab/origin scope on every CDP target transition,
   service-worker navigation, popup, external protocol, and download?
5. Which generic input surfaces must be denied in protected authenticated
   sessions because semantic consequence cannot be proven?
6. What service ownership, authentication, expiry, audit, and offline failure
   model should the required second-device fallback use?
7. Does the `chrome.debugger` transport cover Cua's exact input, frame,
   target-binding, and recording requirements in practice, and can its
   extension/native-host supply chain be secured?

## Source anchors

- [MCP elicitation specification, revision 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/client/elicitation)
- [OWASP LLM06:2025 Excessive Agency](https://genai.owasp.org/llmrisk/llm062025-excessive-agency/)
- [Chrome remote-debugging security change](https://developer.chrome.com/blog/remote-debugging-port)
- [`chrome.debugger` API and available CDP domains](https://developer.chrome.com/docs/extensions/reference/api/debugger)
- [Claude Code permission modes](https://code.claude.com/docs/en/permission-modes)
- [Running Codex safely at OpenAI](https://openai.com/index/running-codex-safely/)

## Review record

### Claude Code / Fable — 2026-07-20

- **Method:** Read-only security and architecture review against exact base
  `767acf25f`, including `policy.rs`, `serve.rs`, `server.rs`, `proxy.rs`,
  `cli.rs`, browser approval/grant/prepare code, current policy docs, and the
  earlier browser plans.
- **Verdict:** Accept with major changes.
- **Accepted corrections:** treat direct same-user artifact-file forgery as the
  primary P0; make `unrestricted` suppress prompts without widening policy;
  define the built-in baseline and default-flip boundary; make daemon dispatch
  unification a prerequisite; eager-load policy before bind; accurately frame
  the earlier plans; classify the emergency shortcut as an affordance; require
  out-of-band indicator/Stop when local UI is unavailable; use existing tool
  names for MVP manifests; add device-flow fallback; generalize the existing
  browser grant store; disambiguate TCC permissions; and test direct artifact
  forgery.
- **Deferrals accepted:** generic driver-wide output/egress classification,
  fine-grained dotted capability taxonomy, later-platform collectors, caller
  authentication, user-visible audit ledger, and `chrome.debugger` do not block
  the first-platform protected-browser release. Browser-specific origin/data
  bypass controls remain mandatory.
- **Rejected recommendations:** None during the review. A subsequent product
  decision keeps biometrics and device flow optional, makes a certified
  protected-host consent UI the MVP collector, and refuses protected attach
  when no collector exists. It retains suppress-only `unrestricted`, a shared
  reviewed ceiling, existing-tool MVP manifests, and an out-of-band
  indicator/Stop substitute.
- **Remaining work:** The technical spikes in the preceding section still need
  implementation evidence; review acceptance does not make this a plan of
  record.
