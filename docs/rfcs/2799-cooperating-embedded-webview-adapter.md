---
title: Authenticated adapter for cooperating embedded webviews
authors:
  - railapex
created: 2026-11-23
last_updated: 2026-11-23
status: review
discussion: https://github.com/trycua/cua/issues/2799
rfc_pr: https://github.com/trycua/cua/pull/8571
implementation:
  - https://github.com/railapex/webview-adapter-example
supersedes:
superseded_by:
---

# RFC: Authenticated adapter for cooperating embedded webviews

## Summary

Define an opt-in local protocol through which a native host exposes bounded
metadata and navigation for the exact embedded webview owned by one accessible
host control. Cua Driver verifies the native owner, transport peer,
process-start identity, pane identity, and generations before it uses the
route. Hosts that do not opt in keep today's refusal.

## Motivation

Native accessibility identifies a host control. Browser debugging protocols
identify a renderer target. In a process that contains several embedded
webviews, URL, title, geometry, creation order, process order, and target order
do not prove which control owns which renderer. Two sibling panes showing the
same URL are indistinguishable from the outside.

The host already holds both halves of the join: the native control and the
engine-specific webview object. Without a route the host vouches for, Cua must
refuse exact embedded-webview actions. Top-level browser binding does not solve
same-URL sibling panes.

## Goals

- Address one embedded webview through the stable ID of its native host
  control.
- Support late attachment, dynamic panes, same-URL siblings, navigation,
  webview recreation, and host restart.
- Authenticate the local peer and reject PID reuse or a stale process identity.
- Start with bounded metadata reads and navigation, with explicit version and
  capability checks.
- Reject stale reads and mutations through pane and document generations.
- Leave room for a platform-neutral contract without claiming engines that are
  not implemented.

## Non-goals

- Infer ownership from URL, title, geometry, enumeration order, or process
  ancestry.
- Accept caller-provided JavaScript.
- Expose cookies, storage, network interception, arbitrary targets, or
  profile-wide debugger access.
- Put a filesystem path, socket path, browser target ID, or secret in
  accessibility metadata.
- Claim that Cua can independently prove how an authenticated host routes
  inside its own process.
- Add semantic-tree, click, type, or scroll operations in the first slice.
- Add the broader element query API proposed in closed PR #1993.

## Terminology

- **Pane**: one embedded webview together with the native control that hosts
  it.
- **Host**: the process that owns the pane's native window and the webview
  object.
- **Adapter identity**: an opaque value the host publishes on the pane's
  accessible control. It names the adapter instance and carries no path,
  endpoint, target, or secret.
- **Pane instance and pane generation**: the pane's identity, and a counter the
  host advances whenever the webview object behind the pane is replaced.
- **Document generation**: a counter the host advances on each committed
  navigation in a pane.

## Current state

- [`BROWSER.md`](../../libs/cua-driver/rust/Skills/cua-driver/BROWSER.md) lists
  WebView2, WKWebView, WebKitGTK, Tauri, and Electron surfaces that cannot be
  correlated to a page target as native fallback cases. Its support table gives
  WebView2, Tauri, and other embedded webviews "Native AX/PX fallback unless an
  exact route is reported", and says host/renderer correlation may refuse.
- On Windows, `browser_endpoints_for_pid` in
  `libs/cua-driver/rust/crates/platform-windows/src/browser_platform.rs` lets a
  native embedded host use a DevTools endpoint owned by its classified
  `msedgewebview2.exe` descendants. That discovers DevTools endpoints, and
  `select_unique_owned_endpoint` refuses with `browser_binding_ambiguous` when
  more than one endpoint qualifies, before any page target is chosen. Neither
  step says which native control owns which renderer target.
- `WindowElement` in `cua-driver-contract` has no platform control ID. RFC
  #2798 proposes that field; this RFC consumes it and does not add it.

## Proposal

### Discovery

The host opts into a versioned local adapter. The accessible node of each
pane's host control publishes a stable control ID (the #2798 field) and the
opaque adapter identity. Cua Driver resolves exactly one native accessibility
node outside web content for the selector. Zero or several matches refuse.

### Attestation

Before it sends a request, the driver proves, in order:

1. the PID and window that own the resolved native node;
2. that the transport's server process is that same PID;
3. through `hello`, the protocol version, the adapter identity it read from the
   node, the host PID, and the host's process start time. A mismatch on any of
   them refuses, which rejects PID reuse and an adapter left over from an
   earlier process.

### Methods

The first slice has four methods:

- `hello`: protocol version, capabilities, adapter identity, host PID, and
  process-start identity.
- `list_panes`: bounded metadata for live panes, for late attachment and
  restart recovery.
- `snapshot`: committed metadata for one pane: pane instance, pane generation,
  document generation, URL, and optional title.
- `navigate`: a bounded URL plus the expected pane and document generations.

A method outside the negotiated capabilities fails explicitly. `navigate` runs
through the exact webview object the host holds. Success means the host
accepted the request; the driver treats navigation as complete only after it
observes the next committed document generation through `snapshot`.

### Windows transport (prototype)

The prototype uses an owner-local named pipe. Its parameters are inputs to
review, not settled protocol limits: requests and responses are newline-framed
JSON capped below 64 KiB with five-second I/O timeouts, the host caps the pane
inventory at 256 entries and URLs at 4096 bytes, and the pipe's access list
admits only the user that owns the host.

### Other platforms

A later design could use XPC or a user-only Unix-domain socket on macOS and a
user-only runtime-directory socket on Linux. Neither platform claims support
until it has equivalent peer and process-start checks, and until then the
driver reports the route as unsupported there.

### Driver surface

The issue thread leaves the driver side to build: selector resolution, binding
a logical tab, and routing later browser tool calls through the driver. One
possible shape, for review: browser tools that bind a tab gain a route kind for
cooperating hosts, and a bound logical tab records the pane instance and both
generations so every later call through it checks them. Which refusal codes
carry the failures is open.

## Trust boundary

Cua can prove that it speaks to the live process that owns the selected native
control. It cannot prove that the authenticated host routed an operation to the
correct internal webview. That is the same trust Cua already extends to the
host's accessibility provider. Generations detect stale routing; they do not
remove this boundary.

## Alternatives considered

- **Host-written JSON manifests**: weak lifecycle, late-attach, and stale-file
  behavior, and they disclose paths.
- **Raw CDP endpoints**: broad authority, Chromium-only, and the native-control
  join stays unsolved.
- **WebView2 debugger discovery**: finds targets but does not carry the host
  control's accessibility identity. This is what main does today.
- **URL, title, geometry, or enumeration matching**: mutable and ambiguous,
  above all for same-URL siblings.
- **Caller script injection**: more authority than the first slice needs, and
  engine-specific.

## Compatibility and migration

The adapter is optional and off by default. Native and browser automation paths
are unchanged. A host that does not opt in keeps the current exact-ownership
refusal. Capabilities are negotiated, and unsupported methods fail without
approximation. The protocol carries an explicit version so later methods are
additive.

## Security, privacy, and telemetry

The transport is local and owner-scoped. The route binds the native
accessibility node, window owner, pipe server PID, process start time, adapter
identity, pane identity, and generations. A wrong peer, PID reuse, duplicate
native IDs, stale generations, an invalid URL scheme, malformed frames, and
oversized frames fail closed. Accessibility metadata carries no path, endpoint,
target ID, or secret. No adapter traffic or page metadata goes to telemetry.

## Implementation plan

1. Land #2798 so `WindowElement` carries the platform control ID.
2. Settle the protocol's owner (see the first unresolved question) and publish
   its schema with version and capability fields.
3. Windows driver client: node resolution, pipe peer and process-start checks,
   `hello` and `list_panes`, behind the route kind and off for hosts that do
   not advertise an adapter.
4. `snapshot` and `navigate` with generation checks, and logical-tab binding.
5. Update `BROWSER.md` and the support table with the exact route and its
   refusals.

The split above is a proposal for review. Every step leaves the
non-cooperating path unchanged.

## Test and acceptance plan

- Contract tests for the schema, frame caps, timeouts, and each refusal: wrong
  peer, PID reuse, stale adapter identity, duplicate native ID, stale pane or
  document generation, closed pane, unsupported method, invalid scheme,
  oversized and malformed frames. Each asserts the exact refusal code.
- A Windows desktop E2E against a two-pane fixture host such as
  [railapex/webview-adapter-example](https://github.com/railapex/webview-adapter-example).
  That example is a standalone host and PowerShell client: it has no driver
  side and did not launch remote or different-user clients, so it is a fixture,
  not evidence of driver routing. The E2E covers
  late attach, distinct binding for same-URL siblings, navigation of one pane
  with the sibling's URL and document generation unchanged, recovery after
  webview recreation and host restart.
- A regression check that a host without an adapter still refuses as it does
  today.

## Unresolved questions

- Should Cua own this contract, or consume a small protocol crate maintained
  with host adapters?
- Is metadata plus navigation the right first slice?
- Should the first implementation stay Windows-only until macOS and Linux have
  equivalent peer attestation?
- Should the host advertise its adapter identity on the accessible control, or
  should Cua derive the route from a host capability registered some other way?

## Decision record

Pending review.
