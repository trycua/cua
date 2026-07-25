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
| [RFC 2549](2549-cua-driver-sdk-owned-runtime.md) | Runtime ownership and adapters. Remote paired hosts are an **adapter/connection**, not a second tool contract. |
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

## Non-goals

- Replacing Cua Cloud Fleet, Sandbox, or Lume lifecycle products.
- Adding shell, files, PTY, tunnels, snapshots, or VM lifecycle to Driver.
- Requiring any specific agent (Hermes or otherwise) on target machines.
- Mesh multi-hop routing or internet-wide unsolicited control.
- Auto-enabling control via unauthenticated LAN discovery.
- Becoming a general RDP/VNC replacement (pixel streaming product).
- Solving cross-machine agent memory, model routing, or multi-agent chat.

## Terminology

| Term | Meaning |
| --- | --- |
| **Controller** | Machine or process that holds paired credentials and opens `CuaDriver` clients for remote hosts (comp1). |
| **Target / paired host** | Physical machine running Driver in paired-agent accept mode (comp2, comp3). |
| **Host registry** | Controller-side list of paired hosts (ids, endpoints, metadata). Secrets prefer OS secure storage. |
| **Pairing** | Human-confirmed ceremony that mints host-scoped credentials bound to a controller identity. |
| **Host attachment** | Authenticated remote connection backend yielding a `CuaDriver` for exactly one target desktop. |
| **Local attachment** | Existing local daemon or SDK-owned runtime path on the controller’s own OS session. |

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
cua-driver paired-agent enable
cua-driver pair --show          # short code and/or QR; short TTL
```

**Controller**:

```bash
cua-driver hosts pair --code ABCD-2345 --name comp2
cua-driver hosts list
```

**Target consent UI or TTY confirm** must show controller identity and requested scopes before minting credentials.

Credentials bind at least:

- controller device id,
- host id,
- scopes,
- protocol/capability version,
- expiry / refresh policy.

Revocation:

```bash
# on target
cua-driver pair revoke --controller <id>
# on controller
cua-driver hosts remove comp2
```

Revoke fails closed on the next call within a short bound (seconds, not login cycles).

### 4. Scopes

Pair-time selectable minimum set:

| Scope | Allows |
| --- | --- |
| `capture` | screenshots / window bitmaps |
| `ax_read` | accessibility / SOM |
| `input` | click, type, key, drag, scroll |
| `focus` | focus/raise behaviors that are more disruptive |
| `browser_prepare` | existing elevated browser-profile flows |

Default grant: `capture + ax_read + input`.

Driver must not grow a “remote root shell” scope. Password and OS permission dialogs keep existing local policy (hard blocks + agent policy).

### 5. Transport and bind policy

**Source of truth:** generated Driver remote envelopes / connection framing from the Rust contract (#2512 direction), not a handwritten permanent REST catalog.

**Carrier:** implementation-defined (for example authenticated WebSocket or HTTP/2 stream). Carrier is not the product contract.

**Defaults:**

- Remote accept is **off** until paired-agent is enabled and a successful pair exists (or an explicit pairing listener with TTL).
- Prefer private overlay addresses (Tailscale/WireGuard/LAN).
- No happy path of `0.0.0.0` + static long-lived shared bearer token.
- Public exposure requires an extra explicit danger flag and still requires pairing-grade auth.
- Optional HTTP JSON adapter (spirit of #1858) only as a **downstream generated adapter**, disabled by default, same authz ceiling.

### 6. CLI / MCP sketch (names flexible)

```text
cua-driver paired-agent enable|disable|status
cua-driver pair --show
cua-driver pair revoke --controller <id>|--all
cua-driver hosts list|show|rename|remove
cua-driver call <tool> --host <name-or-id> ...
cua-driver mcp --host <name-or-id>
cua-driver doctor --host <name-or-id>
```

MCP session should be **host-pinned** (preferred for agents) or accept an explicit host argument with hard errors if omitted when multiple hosts exist. SOM element tokens are **per host** and must never be applied across hosts.

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
- Authorization ceiling remains host-trusted configuration; agent-visible tool args cannot widen scopes beyond the paired credential.

### 9. Relationship to Fleet / Sandbox / Lume

| Environment | Channel creator | Lifecycle owner |
| --- | --- | --- |
| Local | OS user / app | User |
| Sandbox / Lume | Environment SDK | Environment product |
| Cua Cloud Fleet | Fleet claim/service | Fleet |
| **Paired physical host (this RFC)** | **Pairing ceremony + host registry** | **User / paired-agent** |

All yield the same typed `CuaDriver`.

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

## Security, privacy, and telemetry

### Security

- Default deny remote control.
- Pairing requires presence/consent on the target.
- High-entropy credentials; prefer OS secure storage; rotatable; revocable.
- No default unauthenticated non-loopback desktop endpoint (#1892 class).
- Version negotiation fail closed.
- Define non-idempotent input semantics under retry (dedupe or documented at-least-once hazards).
- Target-local audit of pair/revoke and coarse action classes; avoid full keystroke logs by default.

### Privacy / telemetry

Align with #2512 telemetry rules:

**May record:** SDK/runtime versions, connection mode (`paired_host`), platform, operation id, latency, capability version, normalized result class, host id hash.

**Must not record:** screenshots, accessibility trees, visible or typed text, clipboard/file contents, tokens, raw envelopes, precise secrets.

## Implementation plan

### Phase A — smallest useful merge

- Controller host registry + secret storage hooks.
- Pair / revoke / list CLI.
- Authenticated remote channel for a **subset**: screenshot, list_windows, click, type_text, hotkey, scroll (coordinate path acceptable if SOM lags).
- Windows interactive autostart path documented and tested in CI or manual matrix.
- Threat-model docs + Tailscale quickstart.
- Doctor checks for paired host reachability/auth.

**Gate:** lab proof comp1→comp2 screenshot+click after pair; revoke fails closed; unauthenticated port noise rejected.

### Phase B — Driver parity

- SOM / AX remote.
- Structured action verdicts (effect / escalation / path) parity with local.
- Session pin + MCP `--host`.
- Generated Python/TypeScript/Rust client methods for `connect_host`.
- Agent cursor policy remote or explicit documented degradation.

### Phase C — adapters and ops

- Optional HTTP compat adapter (addresses #1858 safely).
- Recording/trajectory host tags.
- Broader multi-controller policy controls.
- Perf budgets for WAN/tailnet latency.

## Test and acceptance plan

- [ ] Pairing succeeds only with target confirm; wrong/expired code fails.
- [ ] Revoke causes controller failure closed quickly.
- [ ] Unauthenticated requests rejected.
- [ ] Same scenario script passes local and paired remote for Phase A tools.
- [ ] Windows: agent not in Session 0 when interactive desktop is the target.
- [ ] macOS: TCC identity remains valid for paired-agent service path.
- [ ] Linux: correct user session display/AT-SPI.
- [ ] No divergent tool semantics vs local for implemented actions.
- [ ] Telemetry exclusions enforced in tests/review checklist.
- [ ] Docs cross-link Fleet/Sandbox remote backends: paired host is another backend.
- [ ] Element/SOM handles cannot be replayed across hosts.

## Unresolved questions

1. Should paired-host remote use the **exact** generated envelopes as Fleet remote (#2512), with only a different credential issuer? (**Recommendation: yes.**)
2. QR pairing in Phase A, or code+confirm only?
3. Default policy: single controller per host vs multi-controller?
4. Naming: `paired-agent` vs `serve --remote-accept` vs `host-agent`?
5. How much agent cursor overlay is required in v1 remotely?
6. Exact public SDK shape: `CuaDriver.connect_host(name)` vs environment-style accessor?
7. Interaction details with RFC 2549 one-direct-runtime-per-process on the **target** service process (service is the runtime owner — confirm).

## Decision record

_Complete when review concludes. Summarize material feedback, accepted changes, rejected alternatives, remaining risks, and final disposition (accepted / declined / superseded)._

## Consumer note (non-normative)

Agent harnesses (for example Hermes Agent) should consume host registry + attach APIs rather than inventing parallel remote input stacks. A companion consumer discussion exists at [NousResearch/hermes-agent#71157](https://github.com/NousResearch/hermes-agent/issues/71157). That work is out of scope for this repository except insofar as stable CLI/SDK/MCP contracts are required.
