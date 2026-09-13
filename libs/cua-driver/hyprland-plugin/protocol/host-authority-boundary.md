# Driver admission and compositor isolation

This implementation note follows the scope correction in
[RFC #3550](https://github.com/trycua/cua/issues/3550#issuecomment-5556178980).
The v3 implementation is a candidate pending native certification and review,
not a release announcement. The default plugin build remains discovery-only.

## One permission system

Driver's common tool registry admits every action before invoking the Linux
adapter. Standard and explicitly acknowledged unrestricted mode without a
manifest do not require per-window protected grants. Bounded mode requires an
approved capability manifest. Manifests, managed policy, user policy, and
shared hard invariants remain binding in every applicable mode.

The plugin does not add an Omarchy-specific permission mode, approval panel,
per-window prompt, indicator-owned authorization, or signing service. Existing
Driver activity and lifecycle behavior still applies. A cached native
connection, public session label, environment switch, or target token never
bypasses the common registry on a later call.

The earlier disconnected `isolated_input_authority` prototype has been removed.
It required a separate trusted host but was not called by any runtime or
adapter. The integrated boundary uses the existing dispatch, policy, and
lifecycle paths instead.

## Local trust and per-action binding

An explicitly enabled plugin accepts the trusted desktop account over private
same-UID sockets. Driver also attests the socket against its Wayland compositor
connection. These checks prevent cross-account access and accidental routing
to another compositor; they do not prove human consent or sandbox arbitrary
native code running as the same user.

The Linux adapter checks the qualified application package on every action
before allocating or using a lane. Eligibility is a compatibility check, not
an authorization boundary. The initial candidate scope is native Calc
`libreoffice-fresh 26.2.5-3` and Inkscape `1.4.4-6`, subject to per-operation
native qualification. Unknown packages refuse. The plugin separately checks
the live native surface, geometry, primary/other-agent conflicts, desktop
availability, and canonical US keymap.

Production [input v3](cua-input-v3.md) has separate endpoints from discovery v2
and signed experiment 0. Each private connection claims one compositor-owned
lane. Every admitted action performs a fresh `TARGET` for one exact operation
and a maximum five-second technical lifetime. Complete dispatch consumes that
binding. Driver does not negotiate down to the experiment or primary seat.

## Cancellation and recovery

The registry supplies a runtime-private lifecycle identity. Independent Driver
processes claim independent compositor lanes; matching public labels cannot
transfer ownership. The current candidate supports at most two lanes.

Session end signals interrupt in-flight adapter waits. Closing the owned
connection cancels that lane and frees its reservation. Runtime teardown also
cleans connections that never acquired an overlay. Plugin disable, session
transitions, stale targets, keymap changes, and primary-target conflicts revoke
affected work. Cleanup releases synthetic state only; it does not undo app
effects. A stalled compositor cannot promise bounded cleanup latency.

These are adapter lifecycle semantics, not an additional MCP cancellation
feature. Direct stdio MCP processes one request at a time and ignores
notifications, including `notifications/cancelled`. An `end_session` request or
stdin EOF is handled after the current request returns; neither interrupts that
request immediately. SDK invocation cancellation, runtime process termination,
and plugin-socket EOF are distinct paths and need separate evidence. The
process-termination proof does not certify MCP cancellation or session end.

Do not replay canceled, partial, or unknown actions. A later new call may
acquire a fresh target after common permission, lifecycle, identity, geometry,
compatibility, and conflict checks. Config disable/re-enable preserves
session-lifetime seats; replacing the plugin requires a desktop restart.

## Evidence and remaining gates

Focused common registry coverage is available from `libs/cua-driver/rust`:

```bash
cargo test -p cua-driver-core background_input_ --lib --locked
```

These tests exercise normal promptless modes and manifest allow/deny decisions
on repeated calls before adapter invocation. They are not native delivery or
continuous foreground-isolation evidence. The adapter tests additionally
cover distinct protocol negotiation, exact operation binding, compositor lane
allocation, and owned-session cleanup.

The [production proof harness](../tests/production-proof.md) uses two direct
Driver MCP processes with no signer. Native acceptance still requires saved
application outputs, continuous primary-input and held-state oracles, a
warp-and-return negative control, cancellation/fault coverage, and exact-source
provenance. Test both an instrumented candidate and an uninstrumented package.
Keep unproven and refused cells explicit.

Review and merge the implementation chain, then publish and verify Driver and
compatible plugin artifacts before building the final Fleet image. Source
candidates can be tested in disposable Fleet instances before release; that is
pre-release validation, not a final supported image.
