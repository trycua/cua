# Browser Engine Support Plan

This is an architecture explanation and implementation roadmap. It records why
the typed browser surface supports an engine only after it can preserve exact
targeting, approval, and background-delivery guarantees. It is not a support
matrix; the public support contract remains the source of truth for accepted
browser and platform combinations.

## Current Boundary

The current browser engine is CDP-native. Core owns a `CdpConnection`, binds a
native window to an exact Chromium target, and re-proves process, window,
endpoint, generation, and tab identity before mutation. `BrowserPlatform`
abstracts operating-system identity and endpoint ownership, not the browser
wire protocol.

This boundary is appropriate for Chromium but cannot truthfully represent
Firefox or Safari by renaming CDP concepts. Unsupported engines therefore
return `browser_route_unavailable` with bounded `engine_family`, `product`,
`required_protocol`, and `limitation` detail before native-window or endpoint
probing.

## Firefox

Firefox's Remote Agent implements WebDriver BiDi. Mozilla documents that it is
started with `--remote-debugging-port`, accepts loopback connections, and has no
other enablement mechanism. An ordinary Firefox process that was not launched
with the flag cannot become attachable without a restart. See Mozilla's
[Remote Agent security model](https://firefox-source-docs.mozilla.org/remote/Security.html).

Consequences:

- Cua Driver must not edit the profile, restart Firefox, or claim that a
  desktop-input fallback is typed browser attachment.
- Existing-profile attachment to an already-running ordinary Firefox profile
  remains a structured refusal.
- A driver-owned isolated Firefox process is feasible later, but it needs a
  WebDriver BiDi transport and capability store rather than a CDP adapter.
- A Firefox capability must bind the BiDi session, browser process generation,
  native window, browsing context, and Cua session, then re-prove all of them
  after reconnect.

## Safari

Safari automation uses `safaridriver` and WebDriver. Apple requires the host to
enable remote automation and describes additional isolation between automation
sessions and normal browsing data. See [Enable WebDriver on macOS](https://developer.apple.com/documentation/safari-developer-tools/macos-enabling-webdriver)
and [About WebDriver for Safari](https://developer.apple.com/documentation/webkit/about-webdriver-for-safari).

Consequences:

- Safari does not expose the loopback CDP endpoint used by the current binding
  and grant model.
- Cua Driver must not present a new WebDriver automation window as attachment
  to the person's existing Safari profile.
- A future Safari engine needs an explicit WebDriver session contract,
  Safari-owned approval state, native window correlation, and session-specific
  privacy rules.
- Existing-profile attachment remains a structured refusal until a real Safari
  route can prove those properties without copying or restarting the profile.

## Protocol-Neutral Core

A future multi-engine implementation should introduce a protocol boundary
above CDP rather than expand `BrowserPlatform`:

1. `BrowserTransport` owns connection, generation, and reconnect behavior.
2. `BrowserContextId` replaces CDP target and session identifiers in the public
   capability store.
3. Engine adapters provide snapshot, navigation, typing, pointer, dialog,
   upload, download, and lifecycle operations with explicit capability flags.
4. Native binding remains a separate proof joined to the engine context before
   every mutation.
5. Approval artifacts name the engine, exact process/window generation, and
   requested profile posture.

No adapter may silently emulate a missing engine action with foreground desktop
input. It either meets the typed action contract or returns a stable refusal.

## Release Acceptance

An engine becomes supported only when representative real-browser rows prove:

- setup or launch follows the advertised profile contract;
- exact window and browsing-context binding, including ambiguous-window cases;
- reconnect generation invalidation and stale capability refusal;
- external page-state mutation for every advertised action;
- background focus, z-order or occlusion, cursor, and no-leaked-input guards;
- non-mutating structured refusals for unavailable actions and environments;
- redacted screenshots, trajectories, logs, and telemetry; and
- playable video evidence at the exact source commit.
