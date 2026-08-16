---
rfc: 3197
title: 'Cua Driver: OAuth and MCP gateway boundary'
authors:
  - 'outdog-hwh (gateway design exploration in PR #2063)'
  - Cua maintainers
created: 2026-08-16
last_updated: 2026-08-16
status: review
discussion: https://github.com/trycua/cua/issues/3197
rfc_pr: https://github.com/trycua/cua/pull/3198
implementation:
supersedes:
superseded_by:
---

# RFC 3197: Cua Driver: OAuth and MCP gateway boundary

## Summary

Cua Driver will expose one stable, standard MCP contract independent of how a
client reaches it. A separate, optional gateway will own public HTTPS, OAuth,
Dynamic Client Registration (DCR), tunnels, and provider-specific connection
adapters. The gateway will forward standard MCP messages to Driver without
moving OAuth state or provider policy into the Driver core.

Driver will keep one canonical tool inventory and response schema. It will not
rewrite `tools/list` for a named client. A provider-specific presentation, if
one is required, belongs to an explicit gateway adapter and must leave the
Driver-facing MCP contract unchanged.

## Motivation

Some remote MCP clients require OAuth discovery, client registration, and an
HTTPS endpoint before they connect to an MCP server. Cua Driver also supports
local stdio, local HTTP, direct SDK, private-worker, and daemon topologies. An
OAuth implementation inside the Driver executable would combine public-edge
identity and network policy with desktop-control runtime ownership.

That combination would make the Driver responsible for provider registration
rules, token storage, certificates, public origins, and tunnel lifecycle. It
would also encourage client-specific tool projections in the canonical MCP
server. Those responsibilities change on different schedules and need
different security review, packaging, and operational controls.

[PR #2063](https://github.com/trycua/cua/pull/2063), authored by
[@outdog-hwh](https://github.com/outdog-hwh), demonstrates the user need and
explores OAuth discovery, DCR, protected MCP forwarding, lifecycle limits, and
connector compatibility. This RFC preserves that design credit. It proposes a
different component boundary before the implementation ships. Any later work
that adapts material code from PR #2063 must also preserve its contributor
authorship under the repository contribution policy.

## Goals

- Give local and remote clients one versioned, standard MCP contract for Cua
  Driver tools, errors, lifecycle, and capability negotiation.
- Keep the Driver core and typed SDK independent of OAuth providers, TLS,
  public URL discovery, and tunnel products.
- Define a separate optional gateway that can authenticate remote MCP clients
  and forward their requests over an authenticated Driver connection.
- Bind a remote principal and MCP connection to trusted Driver permission and
  session context without trusting agent-supplied identifiers.
- Require a trusted local user to approve remote desktop-control access and
  keep protected actions subject to Driver's canonical authorization path.
- Define revocation, expiry, rate limits, audit boundaries, compatibility, and
  acceptance evidence before the gateway ships.
- Keep provider-specific compatibility code isolated and replaceable.

## Non-goals

- Select the gateway's final repository, package name, binary name, or release
  owner in this draft.
- Standardize one OAuth provider, tunnel vendor, identity provider, or hosted
  control plane.
- Make an OAuth grant sufficient authorization for every Driver action.
- Let an MCP client choose a Driver permission mode, runtime ceiling, grant,
  or trusted session by supplying metadata.
- Change existing `cua-driver mcp`, MCP HTTP, SDK, CLI, daemon, or private-worker
  defaults as part of the first gateway release.
- Add a client-specific tool inventory to the Driver contract.
- Describe a vulnerability or an exploit against current code.

## Terminology

**Driver MCP boundary**
: The versioned standard MCP server contract exposed by the Cua Driver adapter.
It includes MCP initialization, capability negotiation, tool discovery, tool
calls, structured errors, cancellation, and connection lifecycle.

**Gateway**
: An optional component that accepts remote MCP client connections, enforces
public-edge authentication and network policy, and forwards standard MCP
messages to an authenticated Driver endpoint.

**Provider adapter**
: Gateway code for a provider's discovery, registration, redirect, or
connection requirements. A provider adapter does not become part of the
Driver contract.

**Remote principal**
: The gateway-authenticated user, account, or workload identity attached to a
connection. Agent-controlled MCP parameters cannot define this identity.

**Driver connection lease**
: An opaque, expiring, revocable binding issued by a trusted local host. It
connects a gateway-authenticated principal and one MCP connection to a
Driver authorization context and lifecycle namespace.

**Local approval**
: A decision made through a trusted local Cua surface that identifies the
requesting gateway, remote principal, requested capability scope, Driver
instance, and expiry. A remote OAuth page alone is not local approval.

## Current state

Cua Driver already exposes MCP over stdio and HTTP. The HTTP adapter is in
[`mcp_http.rs`](../libs/cua-driver/rust/crates/cua-driver/src/mcp_http.rs), and
the command and transport selection are in
[`cli.rs`](../libs/cua-driver/rust/crates/cua-driver/src/cli.rs). The canonical
contract defines one MCP protocol and tool-list shape in
[`cua-driver-contract`](../libs/cua-driver/rust/crates/cua-driver-contract/src/lib.rs).

[RFC 2549](2549-cua-driver-sdk-owned-runtime.md) makes the typed SDK and native
runtime transport-free. It places MCP and HTTP above the SDK, requires one
canonical authorization path, and prevents transport metadata or public
session IDs from selecting authority.
[RFC 3007](3007-cua-driver-lifecycle-sessions.md) binds lifecycle sessions to
trusted ownership namespaces and authenticated transport leases rather than
treating public session names as credentials.

PR #2063 adds an experimental OAuth front door inside the `cua-driver` binary.
It also explores a client-oriented `tools/list` view. The pull request is useful
design evidence, but its process and ownership boundary is not the target
architecture in this proposal.

## Proposal

### 1. Freeze a standard Driver MCP boundary

The Driver MCP adapter will expose one canonical contract across supported
stdio and HTTP transports:

- one supported MCP protocol-version negotiation policy;
- one tool name, input schema, output schema, and error model per operation;
- standard initialization, cancellation, progress, and connection lifecycle;
- the same authorization result for the same trusted context and operation;
  and
- transport-neutral correlation and session semantics.

The Driver will not inspect a client brand to add, remove, rename, or reshape
tools. It will not embed provider discovery metadata in tool descriptions. Its
`tools/list` result remains the canonical inventory generated from the Driver
contract.

The gateway consumes the Driver endpoint as an ordinary MCP client. A gateway
release must declare the Driver MCP contract versions it supports and fail with
a structured compatibility error when negotiation has no common version.

### 2. Put the public edge in an optional gateway

The target dependency and process boundary is:

```text
remote MCP client
        |
        | HTTPS and OAuth
        v
optional OAuth/MCP gateway
        |
        | standard MCP over an authenticated local or private connection
        v
Cua Driver MCP adapter
        |
        v
typed SDK and native runtime
```

The gateway owns:

- OAuth protected-resource and authorization-server discovery;
- authorization-code flows, proof-key requirements, token issuance, and token
  validation;
- DCR when enabled by local policy;
- redirect and public-origin validation;
- TLS termination and certificate policy;
- tunnel creation, health, rotation, and teardown when a tunnel is used;
- provider-specific discovery, registration, and connection adapters;
- public-edge request limits and abuse controls;
- OAuth client, authorization code, token, and approval records; and
- removal of public bearer credentials before forwarding a request to Driver.

The Driver owns:

- the standard MCP protocol and canonical tool inventory;
- the typed operation contract and runtime dispatch;
- desktop permissions, managed policy, user policy, hard invariants, and
  protected-resource authorization;
- lifecycle sessions, resource bindings, cancellation, and cleanup; and
- platform behavior for macOS, Windows, X11, and supported Wayland sessions.

The gateway must not import private Driver core modules or call platform
adapters directly. It uses a released MCP or typed SDK boundary.

### 3. Keep provider compatibility outside Driver

A provider adapter may handle differences in OAuth discovery, registration,
redirect rules, or transport setup. If a client cannot consume the canonical
`tools/list` response, any presentation shim must be an explicit, versioned
gateway adapter. It must be disabled by default, identify its target contract,
and preserve a one-to-one mapping to canonical Driver operations.

No provider adapter may change the meaning of a tool call, bypass Driver
authorization, invent a second operation, or cause Driver to return a different
inventory based on client identity. Compatibility tests must compare the
adapter's projection with the canonical inventory and reject unmapped tools or
schemas.

### 4. Propagate trusted identity and permission context

OAuth establishes who may connect to the gateway. Driver still decides what an
admitted connection may do.

For each accepted MCP connection, the gateway authenticates the remote
principal and asks a trusted local host for a Driver connection lease. The host
binds the lease to:

- the gateway instance and authenticated local channel;
- the remote principal and OAuth client;
- the Driver runtime generation;
- an immutable effective authorization context under the runtime ceiling;
- a lifecycle-session ownership namespace;
- an issued-at time, expiry, and revocation handle; and
- the approved public origin and provider adapter, when relevant.

The binding is opaque to the remote client. A bearer token, MCP session ID,
client name, request field, environment value, or reconnect label cannot
create, replace, or widen it. The gateway forwards only the trusted binding and
the standard MCP request. Driver validates the binding on its authenticated
connection before admitting work.

One remote MCP connection receives one default lifecycle lease. Reconnection
may resume it only through authenticated proof held by the gateway and within
the approved expiry. Closing, expiring, or revoking the connection ends or
invalidates its lifecycle resources according to the Driver session contract.

### 5. Require trusted local approval

The first remote connection, a material scope increase, a new public origin,
or a new gateway identity requires approval through a trusted local Cua
surface. The approval must show enough information for the user to distinguish
the gateway, remote principal, client, requested capability scope, and expiry.

OAuth consent cannot stand in for approval of desktop control. Driver continues
to evaluate each protected action through its canonical authorization stack.
Existing grants remain bounded by their resource, target, duration, and policy.
A model-visible tool call cannot approve itself.

Installations may support a managed pre-approval policy, but the trusted local
administrator must configure it outside the agent-visible MCP channel. The
policy must retain an audit record and a revocation path.

### 6. Bound credentials and public traffic

The gateway must support immediate revocation of clients, tokens, approvals,
connection leases, and all access for one gateway instance. Revocation must
block new calls and cause in-flight work to follow Driver's documented
cancellation and unknown-completion rules.

Authorization codes, access tokens, registrations, approvals, and connection
leases require explicit maximum lifetimes. Defaults must be finite and tested.
Refresh or renewal must recheck the principal, client, local approval, runtime
generation, and policy. A restart must not silently restore expired authority.

The gateway must apply bounded request sizes, connection counts, registration
attempts, token attempts, and tool-call rates. Limits should distinguish a
principal and client where identity is known. Public source address may be a
secondary signal, but it cannot replace authenticated identity.

Secrets and bearer credentials must not enter Driver arguments, MCP payloads,
tool results, logs, traces, crash reports, or telemetry. Gateway storage must
use operating-system protections appropriate to its deployment and atomic,
recoverable updates. The gateway forwards a minimum set of headers and strips
public credentials before the Driver hop.

### 7. Keep telemetry at the owning boundary

Gateway telemetry may record content-free protocol outcomes, provider adapter,
coarse error category, rate-limit result, and latency. Driver telemetry may
record its existing content-free transport, session, authorization, and tool
outcomes. Neither component may record tokens, authorization codes, secrets,
redirect query values, tool arguments, screenshots, typed text, or stable
third-party account identifiers.

Correlation across gateway and Driver requires an ephemeral request ID. It
must not carry authority and must expire with the operational retention window.

## Alternatives considered

### Put OAuth and DCR in `cua-driver`

This is the shape explored in PR #2063. It has a simple installation path, but
it makes the Driver release responsible for public-edge identity, token state,
TLS, tunnels, and provider changes. It also expands a desktop-control binary's
network-facing surface. The proposal keeps the exploration and moves those
responsibilities to a separately owned component.

### Add OAuth behavior to every Driver transport

Stdio and private local connections do not need a public authorization server.
Adding OAuth concepts to each transport would produce different public
contracts and duplicate lifecycle rules. One optional gateway can serve remote
clients while local transports remain unchanged.

### Let each provider maintain a Driver fork or tool inventory

Provider-specific Driver forks would split tool names, schemas, errors, and
authorization behavior. A provider adapter at the gateway edge can absorb
connection differences while the Driver contract stays canonical.

### Use one hosted gateway for all deployments

A hosted service could reduce local setup, but it would add a mandatory remote
trust and data path. This RFC permits a hosted implementation only if it meets
the same identity, approval, transport-security, revocation, and privacy
contract. It does not require one.

### Treat the OAuth token as the Driver permission grant

That would let an external protocol artifact select desktop authority and
would collapse client authentication into action approval. The proposal binds
the authenticated principal to a separately issued Driver authorization
context under local policy.

## Compatibility and migration

Delivery is additive. Existing `cua-driver mcp`, MCP HTTP, CLI, SDK, daemon,
and private-worker commands retain their names, defaults, schemas, permission
identity, and lifecycle behavior. The first gateway release is optional and
uses a supported Driver boundary.

PR #2063 remains linked as design and implementation exploration. It should
not merge its in-core OAuth subcommand as the target architecture. Maintainers
may salvage tested protocol logic into the gateway with contributor authorship
preserved. Driver-side changes should be limited to standard contract fixes or
the trusted connection binding needed by all transports.

Migration has four gates:

1. Freeze the current Driver MCP contract as fixtures, including initialization,
   `tools/list`, tool calls, errors, cancellation, and session lifecycle.
2. Define and implement the authenticated Driver connection lease without
   changing existing local defaults.
3. Ship the gateway as an opt-in package with one reference local deployment
   and provider-neutral conformance tests.
4. Add provider adapters independently, with the canonical boundary tests run
   against each adapter.

Rollback disables or removes the gateway and its public route. It does not
roll back Driver authorization or restore an unauthenticated public endpoint.
Gateway and Driver releases must publish a compatibility range so either
component can refuse an unsupported pairing before accepting actions.

## Security, privacy, and telemetry

The gateway is a public-edge security component. Its threat model and review
must cover client registration, redirect and origin binding, token and code
lifecycle, replay resistance, public request bounds, storage, transport
security, tunnel ownership, and credential removal at the Driver hop. This
public RFC states the required properties and does not describe defects or
exploit procedures.

Driver remains the final enforcement point for desktop actions. It validates a
trusted connection binding, applies its runtime ceiling and effective
authorization context, checks managed and user policy, honors platform
permissions, and records content-free outcomes. Gateway authentication cannot
weaken these checks.

Local approval records and OAuth state are sensitive. Access must follow least
privilege, storage must be scoped to the gateway identity, and revocation must
be available without starting a remote MCP session. Logs and telemetry follow
the exclusions in Proposal section 7.

## Implementation plan

1. Specify the Driver MCP compatibility fixture and authenticated connection
   lease at the contract layer. Keep existing callers on their current trusted
   compatibility context.
2. Add a reference gateway package with OAuth discovery, policy-controlled
   DCR, token lifecycle, TLS configuration, public request limits, and a
   standard MCP upstream client.
3. Add the trusted local approval surface and bind approvals to gateway,
   principal, client, scope, origin, runtime generation, and expiry.
4. Add revocation and shutdown paths for tokens, clients, approvals, leases,
   tunnels, and MCP lifecycle resources.
5. Add provider adapters behind explicit configuration and conformance tests.
6. Publish version compatibility, setup, migration, rollback, and security
   documentation before marking the RFC completed.

Each increment needs its own reviewable pull request and rollback gate. The RFC
must be accepted before an implementation pull request is made ready.

## Test and acceptance plan

The RFC is complete only when the following evidence passes at the final
candidate revisions:

- Protocol fixtures prove that direct stdio, direct HTTP, and gateway-backed
  clients negotiate the supported MCP version and receive the same canonical
  tool names, schemas, results, and structured errors.
- Driver tests prove that client identity cannot alter `tools/list` and that
  provider adapter code is absent from Driver core and contract crates.
- Identity tests prove that bearer tokens, public session IDs, MCP metadata,
  reconnect labels, and tool arguments cannot select or widen Driver authority.
- Approval tests cover first connection, denied approval, scope increase,
  origin or gateway change, expiry, managed pre-approval, and revocation.
- Lifecycle tests cover disconnect, authenticated reconnect, runtime restart,
  gateway restart, idle expiry, cancellation, and unknown completion.
- OAuth conformance tests cover discovery, registration policy, authorization,
  token validation, audience and resource binding, expiry, renewal, and
  revocation without recording secrets in test output.
- Public-edge tests cover TLS policy, tunnel teardown, request and connection
  bounds, malformed input, storage recovery, and credential stripping at the
  Driver hop.
- Provider adapter tests prove a one-to-one mapping to the canonical Driver
  inventory and reject incompatible projections.
- Privacy tests scan logs, telemetry, traces, crash fixtures, and Driver-bound
  requests for prohibited credentials and sensitive content.
- Supported macOS, Windows, X11, and Wayland lanes prove that gateway use does
  not change Driver process identity, OS permission attribution, action
  authorization, or cleanup behavior. Any platform limit returns a documented
  structured error.
- Compatibility fixtures prove that existing CLI, SDK, stdio MCP, HTTP MCP,
  daemon, and private-worker workflows remain unchanged when the gateway is not
  installed or enabled.
- Documentation identifies the owner of every process, credential, listener,
  tunnel, approval, lifecycle lease, shutdown path, and upgrade boundary.

## Unresolved questions

- Which repository and team own the gateway, its security response, and its
  release schedule?
- Is the gateway a separate binary in this monorepo, a separate repository, a
  plugin, or more than one package for local and hosted deployments?
- Which component owns the trusted local approval UI on each platform?
- What is the minimum Driver connection-lease contract needed by both local and
  hosted gateways without exposing provider concepts to Driver?
- Which OAuth profile and MCP transport versions form the first supported
  conformance target?
- Is DCR enabled by default for local deployments, or must every installation
  opt in through trusted policy?
- May the first release include any provider-specific tool presentation shim,
  or should it ship only when every target accepts the canonical inventory?
- Which secret store and recovery model is required for local, server, and
  hosted packaging?
- Who owns tunnel binaries, updates, certificates, domain configuration, health
  checks, and teardown?
- What default and maximum TTLs and rate limits balance reconnect usability
  with bounded remote access?
- How will gateway and Driver version ranges be published and enforced across
  independent releases?

## Decision record

Pending review in [issue #3197](https://github.com/trycua/cua/issues/3197). The
RFC document is [pull request #3198](https://github.com/trycua/cua/pull/3198).
