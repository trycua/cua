---
rfc: 2562
title: "Cua Driver: Self-Hosted Paired Multi-Host Remote Connection for Physical PCs"
authors:
  - skyzea1
created: 2026-07-25
last_updated: 2026-07-25
status: review
discussion: https://github.com/trycua/cua/issues/2562
rfc_pr: https://github.com/trycua/cua/pull/2563
implementation:
supersedes:
superseded_by:
---

# RFC 2562: Cua Driver — Self-Hosted Paired Multi-Host Remote Connection for Physical PCs

## Summary

Cua Driver should support a **self-hosted paired multi-host** connection backend so a controller process on one physical machine can obtain a typed `CuaDriver` for **other physical machines** after an explicit pairing and consent ceremony.

This is the “bring your own PCs” profile of the remote Driver connection: the same public `CuaDriver` contract used for local, Sandbox, Lume, and Fleet targets, with a credential issuer and lifecycle owned by **user pairing** rather than a cloud claim service.

One daemon (or SDK-owned runtime) still owns **one physical desktop**. Multi-host is **routing across N host attachments**, not one runtime spanning machines.

This RFC depends on RFC 2549's runtime ownership, authorization-session, and
remote-envelope model. It does not modify or supersede that model. It defines
an optional paired physical-host connection backend and its pairing lifecycle.

## Motivation

### User problem

Operators run an agent harness (Hermes, Claude Code, Codex, OpenClaw, custom MCP clients) on **comp1** and want structured computer-use on **comp2** and **comp3**:

- capture / SOM / accessibility,
- click, type, key, drag, scroll,
- list apps/windows,

without installing a full LLM agent and API keys on every target, and without driving an RDP/VNC **viewer window** as a pixel proxy.

Today the documented and implemented Driver path is **local IPC** (named pipe / Unix socket) to a machine-owned daemon or an SDK-owned local runtime. Cua Fleet and related environment products address **managed** remote desktops. There is no first-class, secure, self-hosted path for paired **physical** secondary PCs on a LAN or tailnet.

### Why now

1. Agent products already integrate local `cua-driver` MCP; users immediately ask for multi-PC control.
2. Workarounds are worse than a Driver-native design: full agent per box, Session 0 SSH hacks on Windows, or unauthenticated HTTP desktop APIs.
3. RFC direction already requires a **single typed remote connection** for environments ([#2512](https://github.com/trycua/cua/issues/2512) / [PR #2513](https://github.com/trycua/cua/pull/2513)). This RFC specifies the missing **self-hosted physical host** backend under that umbrella rather than inventing a second desktop contract.
4. Security incidents and reports around broad binds and weak auth (for example [#1892](https://github.com/trycua/cua/issues/1892)) make it important to define **pairing-first** defaults before ad-hoc HTTP servers become de facto API.

### Related work (not duplicates)

| Reference | Relationship |
| --- | --- |
| [#2512](https://github.com/trycua/cua/issues/2512) / [PR #2513](https://github.com/trycua/cua/pull/2513) | Parent architecture: one `CuaDriver`, environment-specific connection backends. This RFC is one backend profile. |
| [RFC 2549](2549-cua-driver-sdk-owned-runtime.md) | Normative foundation for runtime ownership, immutable authorization sessions, authenticated connection binding, and remote Driver envelopes. This RFC adds one optional connection backend and does not redefine those concepts. |
| [RFC 2447](2447-cua-driver-native-core-and-mcp-adapter.md) | Typed SDK + MCP as downstream adapter — preserved. |
| [#1858](https://github.com/trycua/cua/issues/1858) | Same pain (remote Windows interactive session). That issue proposes REST + bearer token. This RFC keeps the **typed Driver contract** as source of truth; optional HTTP may appear later as a generated adapter. |
| [#1892](https://github.com/trycua/cua/issues/1892) | Negative example: unauthenticated non-loopback desktop control must not ship as default. |
| Process model | “A daemon drives one physical machine” remains true per host. |

## Goals

- Preserve **one generated `CuaDriver` interface** for local and paired remote physical hosts.
- Ship a **thin target install**: `cua-driver` paired-agent mode on comp2/comp3 — not a full agent stack.
- Require **explicit pairing + on-target consent** before capture or input.
- Provide **authenticated, versioned** remote framing compatible with the generated remote connection direction in #2512.
- Default to **deny remote**; prefer private networks (LAN / Tailscale / WireGuard); short-lived credentials; immediate revoke.
- Document and test **Windows interactive session** placement (not Session 0), macOS TCC identity, and Linux graphical session binding.
- Remain **harness-independent**: CLI, MCP, and SDK all work; Hermes is one consumer.
- Reuse RFC 2549 permission modes and bounded `SessionManifest` values instead
  of creating a parallel remote-scope permission system.
- Provide one official secure paired-host carrier while allowing an existing
  trusted host fabric to carry the same Driver envelopes.

## Non-goals

- Replacing Cua Cloud Fleet, Sandbox, or Lume lifecycle products.
- Adding shell, files, PTY, tunnels, snapshots, or VM lifecycle to Driver.
- Requiring any specific agent (Hermes or otherwise) on target machines.
- Mesh multi-hop routing or internet-wide unsolicited control.
- Auto-enabling control via unauthenticated LAN discovery.
- Becoming a general RDP/VNC replacement (pixel streaming product).
- Solving cross-machine agent memory, model routing, or multi-agent chat.
- Replacing RFC 2549's runtime ceiling or session authorization model.
- Requiring a harness that already has an authenticated host/node fabric to
  pair the same two machines a second time.

## Terminology

| Term | Meaning |
| --- | --- |
| **Controller** | Machine or process that holds paired credentials and opens `CuaDriver` clients for remote hosts (comp1). |
| **Target / paired host** | Physical machine running Driver in paired-agent accept mode (comp2, comp3). |
| **Host registry** | Controller-side list of paired hosts (ids, endpoints, metadata). Secrets prefer OS secure storage. |
| **Pairing** | Human-confirmed ceremony that mints host-scoped credentials bound to a controller identity. |
| **Pairing grant** | Target-local, revocable policy row binding a controller public key to an explicit set of allowed permission modes, an optional bounded manifest, and a lifetime. It is not a bearer token. |
| **Host attachment** | Authenticated remote connection backend yielding a `CuaDriver` for exactly one target desktop. |
| **Local attachment** | Existing local daemon or SDK-owned runtime path on the controller’s own OS session. |
| **Carrier** | The authenticated byte channel carrying generated Driver envelopes. The official paired carrier is specified here; trusted products may supply another conforming carrier. |

## Current state

Observable today:

- Local Driver automation is production-grade on macOS, Windows, and Linux via accessibility + input stacks.
- MCP clients typically speak **stdio** to `cua-driver mcp`, which proxies to a **machine-local** daemon socket/pipe.
- Multiple MCP clients may share one machine; they still share one desktop.
- Environment products (Sandbox, Lume, Fleet) own machine lifecycle; Driver owns desktop semantics — convergence is actively specified in #2512 / PR #2513 and runtime ownership in RFC 2549.
- There is **no** stable public “pair physical PC B and call `driver.click` from PC A” API in the open self-hosted driver path.
- Issue #1858 documents the Windows SSH Session 0 gap and proposes HTTP; it does not define pairing, generated contracts, or SDK parity.

## Proposal

### 1. Same public contract, new connection backend

Applications continue to use typed `CuaDriver` methods. Environments and self-hosted pairing only differ in **how the connection is established**:

```text
CuaDriver.connect_local()
CuaDriver.connect_host("comp2")          # paired physical host
# Fleet/Sandbox/Lume continue to supply environment-scoped drivers per #2512
```

Internally this is a `PairedHost` remote connection backend alongside Fleet/environment routes — **not** a second click/type schema.

Invariant:

> One runtime owns one interactive desktop. Multi-host = many attachments.

### 2. Roles and control flow

```text
comp1 (controller)                         comp2 (target)          comp3 (target)
─────────────────                          ──────────────          ──────────────
Agent harness
   │
   ▼
CuaDriver clients ── local ──► local desktop (optional)
   │
   ├── paired channel (auth) ─────────────► paired-agent runtime
   └── paired channel (auth) ──────────────────────────────────► paired-agent runtime
```

### 3. Pairing ceremony (human presence on target)

**Target prep** (interactive user session):

```bash
cua-driver target enable
cua-driver target pair --show   # QR and copy/paste payload; short TTL
```

**Controller**:

```bash
cua-driver hosts pair --payload <pairing-payload> --name comp2
cua-driver hosts list
```

Pairing is a host-administration operation. It must not be exposed as an MCP
tool, Driver action, or model-selectable SDK method. A local human opens the
pairing window on the target and confirms the controller identity, requested
permission modes, bounded manifest if any, and grant lifetime.

The v1 QR or copy/paste payload contains:

- protocol version and target endpoint;
- target public-key fingerprint; and
- a single-use, short-TTL secret with at least 128 bits of entropy.

The payload is not a lasting credential. A successful ceremony enrolls the
controller public key on the target and pins the target public key on the
controller. If a short human-typed code is added later, it must use a
password-authenticated key exchange such as
[SPAKE2+ (RFC 9383)](https://www.rfc-editor.org/rfc/rfc9383.html); an HMAC or
hash over a low-entropy code is not sufficient.

The target-local pairing grant binds at least:

- controller public-key fingerprint and target host id;
- an explicit set of allowed permission modes;
- an optional canonical bounded `SessionManifest` and its hash;
- protocol/capability version;
- grant lifetime (`once`, `until reboot`, or `until revoke`) and expiry; and
- a generation changed by edits or revocation.

Permission modes are a set, not a numeric maximum: `standard`, `bounded`, and
`unrestricted` have different semantics and are not assumed to be ordered.
The default new pairing allows `standard` only. `bounded` requires an explicit
manifest. `unrestricted` requires both the target's launch-time danger
acknowledgement and an explicit target-local pairing grant.

Revocation:

```bash
# on target
cua-driver target pair revoke --controller <id>
# on controller
cua-driver hosts remove comp2
```

Target revocation deletes authority, closes live connections, invalidates
sessions and handles, and fails the next call closed within one second.
Controller removal only forgets the local host record; it does not revoke the
target-side grant.

Pairing may authorize future attachments for its chosen lifetime. It does not
require a click on every reconnect. Every active attachment must instead show
a persistent target-local **“Cua is controlling this computer — Stop”**
indicator. Stop closes the attachment and revokes its active grants. Protected
operations in `standard` mode can still require additional target-local
consent.

### 4. Authorization composes with RFC 2549

Every action is authorized and enforced on the target:

```text
effective authority =
    target runtime authorization ceiling
  ∩ pairing grant
  ∩ immutable connection-bound session authorization
  ∩ target managed policy
  ∩ target user policy
  ∩ protected local consent and active resource grants
```

Each term is subtractive. No controller argument, public session id, bearer
value, reconnect token, or harness policy can widen authority. The target
authenticates the controller public key, resolves the pairing grant, creates
the immutable RFC 2549 session, and binds that session to the accepted
connection generation before accepting actions.

Mode behavior:

- `standard`: normal target-side Cua policy and protected-consent adapters
  apply.
- `bounded`: unattended work is limited to the exact approved
  `SessionManifest`.
- `unrestricted`: Cua runtime approval prompts are skipped only when the target
  runtime was launched with explicit danger acknowledgement and the pairing
  grant explicitly allows this mode. Managed/user policy, hard safety
  invariants, OS permissions, and resource ownership still apply.

The pairing grant must not create a “remote root shell” capability. Password
and OS permission dialogs keep existing local policy. In particular,
`browser_prepare(strategy=existing_profile)` in `standard` mode still requires
the protected consent provider and visible indicator on the **target**. The
controller or harness cannot render or satisfy that consent.

### 5. Transport and bind policy

**Source of truth:** the exact generated Driver request/result envelopes and
`DriverEnvelopeChannel` contract from RFC 2549 and #2512, not a handwritten
REST catalog. The paired-host implementation must supply authenticated
principal and connection-generation bindings to that contract.

**Official paired carrier:** TLS 1.3
([RFC 9846](https://www.rfc-editor.org/rfc/rfc9846.html)) with mutually
authenticated, pairing-pinned raw public keys
([RFC 7250](https://www.rfc-editor.org/rfc/rfc7250.html)) and a versioned ALPN
such as `cua-driver/1`. Pairing confirmation binds to the exact TLS connection
using the `tls-exporter` channel binding from
[RFC 9266](https://www.rfc-editor.org/rfc/rfc9266.html). The carrier may be a
length-prefixed stream, WebSocket, HTTP/2, or gRPC stream, but it carries the
same opaque envelopes and must not introduce per-tool RPC methods or
metadata-carried authority.

The pairing records pin both installation public keys; TLS authenticates them
on every connection. There is no reusable bearer credential for a harness to
copy, log, or accidentally expose.

Custom carriers remain valid for Fleet, OpenClaw-style node fabrics, and other
trusted gateways when they provide an authenticated principal, connection
generation, confidentiality, integrity, replay protection, deadlines, and
cancellation. They still terminate at the same target-side authorization path.
For example, a gateway with an existing authenticated controller-to-node route
may run Cua locally on the selected node rather than creating a second Cua
pairing. A carrier cannot bypass target policy or protected consent.

**Defaults:**

- Remote accept is **off** until paired-agent is enabled and a successful pair exists (or an explicit pairing listener with TTL).
- Prefer private overlay addresses (Tailscale/WireGuard/LAN).
- No happy path of `0.0.0.0` + static long-lived shared bearer token.
- Public exposure requires an extra explicit danger flag and still requires pairing-grade auth.
- Optional HTTP JSON adapter (spirit of #1858) only as a **downstream generated adapter**, disabled by default, same authz ceiling.
- LAN discovery, if added, is an unauthenticated reachability hint only. The
  controller must still verify the pairing-pinned target key.

### 5.1 Reconnect, replay, and interrupted actions

Every connection has a fresh generation. Reconnect invalidates element handles,
browser bindings, cursor/session leases, and other generation-scoped
references. The target re-evaluates the pairing grant on every request; edits,
expiry, and revocation terminate live sessions rather than silently changing
their authority.

Generated envelopes carry request identity. Implementations should deduplicate
an idempotency key when they can prove a stored completion result. They must
not claim exactly-once execution across crashes. If connection loss leaves the
completion of a click, keypress, type, or other mutation unknown, the
controller must not automatically repeat it: reconnect, observe fresh target
state, and let the harness decide the next action.

### 6. CLI / MCP sketch (names flexible)

```text
cua-driver target enable|disable|status
cua-driver target pair --show
cua-driver target pair list|revoke
cua-driver hosts list|show|rename|remove
cua-driver hosts pair --payload <payload> --name <name>
cua-driver call <tool> --host <name-or-id> ...
cua-driver mcp --host <name-or-id>
cua-driver doctor --host <name-or-id>
```

An MCP process is pinned to exactly one host for its lifetime. Pairing,
retargeting, and host selection are trusted launch-time operations, never
model-facing tools or per-call tool arguments. Operators start a second MCP
process to expose a second host. SOM element tokens and browser references are
host- and connection-generation-scoped and must never be applied across hosts.

The Rust, Python, and TypeScript SDKs expose a typed
`CuaDriver.connect_host(...)` (exact language spelling may differ) that returns
the same public Driver contract as local creation and connection.

### 7. Platform placement requirements

| Platform | Requirement |
| --- | --- |
| Windows | Paired agent in interactive session (1+), not Session 0; autostart via interactive logon task pattern. UIPI elevated-window limits unchanged. |
| macOS | Stable TCC-responsible identity (CuaDriver.app / signed service path per RFC 2549 macOS rules). |
| Linux | Correct user graphical session (`DISPLAY` / Wayland) and that session’s AT-SPI bus. |

### 8. Relationship to RFC 2549 (SDK-owned runtime)

- Local apps may use SDK-owned runtime or explicit daemon/service topologies per RFC 2549.
- A **target** paired-agent is an explicit long-lived **service topology** (accepts authenticated remote attachments).
- A **controller** may use a lightweight client connection without owning the remote desktop runtime.
- Authorization ceiling remains host-trusted configuration; agent-visible tool
  args cannot select a mode or widen authority beyond the pairing grant.
- Each remote attachment binds one immutable authorization session to the
  authenticated controller identity and connection generation.

### 9. Relationship to Fleet / Sandbox / Lume

| Environment | Channel creator | Lifecycle owner |
| --- | --- | --- |
| Local | OS user / app | User |
| Sandbox / Lume | Environment SDK | Environment product |
| Cua Cloud Fleet | Fleet claim/service | Fleet |
| **Paired physical host (this RFC)** | **Pairing ceremony + host registry** | **User / paired-agent** |

All yield the same typed `CuaDriver`.

### 10. Product and harness responsibilities

The security boundary stays in Cua; agent policy and orchestration stay in the
harness:

| Cua owns and enforces | Harness owns |
| --- | --- |
| Pairing ceremony and target-local consent | Which configured host a task should use |
| Installation identities, secure storage, peer authentication, and encrypted carrier | User-facing host picker, aliases, and task routing |
| Pairing grants, runtime ceiling, immutable session binding, policy intersection, and revocation | Additional approval rules and a stricter policy ceiling |
| Persistent target indicator and Stop | Agent loop, skill instructions, and action-selection ladder |
| Host/generation-scoped references and interrupted-action safety | Re-observing state and deciding what to do after an unknown completion |

A harness may narrow Cua authority but never widen it. It must not read,
persist, forward, or broker Cua private keys or pairing secrets.

Recommended integrations:

- Hermes, Pi, and custom applications use the typed SDK or a host-pinned MCP
  process.
- Claude Code and Codex standalone users configure
  `cua-driver mcp --host <name>`.
- OpenClaw-style gateways use their existing authenticated Gateway-to-node
  route and run Cua locally on the selected node. The official paired carrier
  is used only when the target is outside that trusted node fabric.
- Fleet and other managed products may supply a conforming custom carrier while
  preserving the same target authorization and envelope semantics.

## Alternatives considered

1. **Full agent install on every PC**  
   Works operationally; heavy; duplicates credentials; does not give one controller session multi-host tool routing. Remains a valid workaround, not Driver architecture.

2. **#1858 REST-only primary API**  
   Fast to prototype; becomes a second behavioral contract; easy to ship weak auth. Rejected as primary; optional later adapter only.

3. **Raw MCP over TCP without pairing**  
   Agents understand MCP, but open MCP is high risk and lacks host identity/session pin. Rejected without pairing + authz ceiling.

4. **RDP/VNC then local Driver on the viewer**  
   No native remote a11y; brittle. Rejected as primary design.

5. **Harness-specific remote control (e.g. only Hermes)**  
   Every harness reimplements insecure remote desktop control. Rejected; protocol belongs in Cua.

6. **Fleet-only remote**  
   Excellent for managed fleets; does not cover self-hosted home/lab multi-PC without cloud claims. Insufficient alone.

## Compatibility and migration

- Purely **additive**. Local MCP/stdio and existing daemon flows remain default.
- No break to current single-machine users.
- Capability negotiation: old controllers talking to new targets fail closed with upgrade errors; new controllers to old targets omit remote host features.
- Rollback: disable paired-agent / revoke all controllers; local behavior unchanged.
- Sequencing: land protocol + Phase A tools before promoting HTTP compat or full SOM parity.
- Pairing and host selection are additive administration commands. Existing
  CLI defaults, local SDK constructors, daemon sockets, and MCP stdio behavior
  remain unchanged.

## Security, privacy, and telemetry

### Security

- Default deny remote control.
- Pairing requires presence and consent on the target; the window is
  single-use and short-lived.
- Installation private keys use OS secure storage where available; pairing
  rows are integrity-protected, rotatable, and immediately revocable.
- Steady-state identity comes from the mutually authenticated peer keys, not a
  bearer token or caller-supplied controller id.
- No default unauthenticated non-loopback desktop endpoint (#1892 class).
- Version and ALPN negotiation fail closed without downgrade.
- Pairing grants, runtime ceilings, session modes, policies, and consent are
  intersected and enforced on the target.
- Unknown completion of a mutating action is never automatically retried.
- Target-local audit of pair/revoke and coarse action classes; avoid full keystroke logs by default.
- The active target displays a persistent, locally rendered Stop control.

### Privacy / telemetry

Align with #2512 telemetry rules:

**May record:** SDK/runtime versions, connection mode (`paired_host`), platform, operation id, latency, capability version, normalized result class, host id hash.

**Must not record:** screenshots, accessibility trees, visible or typed text,
clipboard/file contents, private keys, pairing payloads, TLS exporter values,
unhashed peer fingerprints, bounded manifest contents, raw envelopes, or other
secrets.

## Implementation plan

### Phase 0 — protocol and platform spikes

- Prove TLS 1.3 mutual raw-public-key interoperability in the selected Rust TLS
  stack on macOS, Windows, and Linux.
- Specify the pairing transcript, exporter-bound confirmation, ALPN, frame
  limits, deadlines, cancellation, and stable refusal codes.
- Specify OS key storage and registry integrity rules per platform.
- Threat-model pairing races, copied payloads, controller loss, target loss,
  downgrade, replay, revocation races, and unknown action completion.

**Gate:** two clean installations pair without trust-on-first-use; a relay or
wrong key fails; no reusable bearer credential exists.

### Phase A — smallest useful merge

- Controller host registry, target pairing registry, installation identities,
  and OS secure-storage hooks.
- Pair / revoke / list CLI.
- Official authenticated paired carrier using the exact generated remote
  envelopes.
- Target-side pairing-grant and RFC 2549 session authorization intersection.
- Persistent target indicator and Stop/revoke path.
- Authenticated remote channel for a **subset**: screenshot, list_windows,
  click, type_text, hotkey, scroll (coordinate path acceptable if SOM lags).
- Windows interactive autostart path documented and tested in CI or manual matrix.
- Threat-model docs + Tailscale quickstart.
- Doctor checks for paired host reachability/auth.

**Gate:** lab proof comp1→comp2 screenshot+click after pair; allowed mode and
manifest are enforced on the target; Stop/revoke fails closed; unauthenticated
port noise is rejected.

### Phase B — Driver parity

- SOM / AX remote.
- Structured action verdicts (effect / escalation / path) parity with local.
- Host-pinned MCP `--host`.
- Generated Python/TypeScript/Rust client methods for `connect_host`.
- Persistent indicator and agent-cursor behavior parity.
- Idempotency cache for safely deduplicable requests and reconnect recovery
  guidance for unknown mutating completions.

### Phase C — adapters and ops

- Optional HTTP compat adapter (addresses #1858 safely).
- Recording/trajectory host tags.
- Broader multi-controller policy controls.
- Perf budgets for WAN/tailnet latency.

## Test and acceptance plan

- [ ] Pairing succeeds only with target confirm; wrong, expired, replayed, or
  concurrently consumed payloads fail.
- [ ] Peer-key substitution, relay, ALPN downgrade, unauthenticated requests,
  and caller-supplied identity metadata are rejected.
- [ ] Pairing and retargeting are absent from model-facing MCP tools.
- [ ] Target revoke terminates live connections and refuses the next action
  within one second; controller removal alone does not claim revocation.
- [ ] `standard`, `bounded`, and explicitly acknowledged `unrestricted` behave
  according to RFC 2549; a pairing cannot widen the runtime ceiling.
- [ ] Bounded sessions enforce the exact target-approved `SessionManifest`.
- [ ] Existing-profile consent in `standard` mode is rendered and revocable on
  the target, never satisfied by the controller.
- [ ] Same scenario script passes local and paired remote for Phase A tools.
- [ ] Windows: agent not in Session 0 when interactive desktop is the target.
- [ ] macOS: TCC identity remains valid for paired-agent service path.
- [ ] Linux: correct user session display/AT-SPI.
- [ ] No divergent tool semantics vs local for implemented actions.
- [ ] Telemetry exclusions enforced in tests/review checklist.
- [ ] Docs cross-link Fleet/Sandbox remote backends: paired host is another backend.
- [ ] Element/SOM handles and browser bindings cannot be replayed across hosts
  or connection generations.
- [ ] Lost responses do not cause automatic repetition of unknown mutating
  actions.
- [ ] A conforming custom carrier supplies authenticated principal and
  connection binding but cannot bypass target authorization.

## Unresolved questions

1. Default policy: one controller per target or multiple independently paired
   controller rows?
2. Final naming: `target`, `paired-agent`, or `serve --remote-accept`?
3. Exact native indicator/Stop UX on each platform and whether it can reuse the
   agent-cursor status surface.
4. Exact public SDK spelling: `CuaDriver.connect_host(name)` or a
   connection-options constructor consistent with other backends?
5. Which stream carrier should implement the official TLS profile first:
   length-prefixed frames, WebSocket, HTTP/2, or gRPC?
6. Should persistent grants require an absolute expiry, or may a user
   explicitly choose `until revoke` with no expiry?

## Decision record

_Complete when review concludes. Summarize material feedback, accepted changes, rejected alternatives, remaining risks, and final disposition (accepted / declined / superseded)._

## Consumer note (non-normative)

Agent harnesses (for example Hermes Agent) should consume host registry + attach APIs rather than inventing parallel remote input stacks. A companion consumer discussion exists at [NousResearch/hermes-agent#71157](https://github.com/NousResearch/hermes-agent/issues/71157). That work is out of scope for this repository except insofar as stable CLI/SDK/MCP contracts are required.
