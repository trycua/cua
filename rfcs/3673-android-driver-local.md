---
title: Local Android backend for Cua Driver
authors:
  - f-trycua
created: 2026-09-08
last_updated: 2026-09-08
status: review
discussion: https://github.com/trycua/cua/issues/3673
---

# Local Android backend for Cua Driver

## Decision under review

Implement a local experimental Android runtime and Kotlin client SDK, with
explicit `cua-driver --device SERIAL` host routing and a phone-local client.
There is no OS command namespace. Existing unqualified desktop commands retain
their behavior. The maintainer selected this prototype; stable API adoption
remains under review and is not implied by the experiment.

## Architecture

An explicitly started development runtime runs under Android's authorized shell
identity. It owns one virtual display, an ImageReader capture path, session
lifecycle, admission, and action evidence. Host requests arrive through ADB and
an on-device client; Android app clients use authenticated local IPC. Local
socket peer credentials identify callers and the runtime; a session ID is never
a credential. The first app client is admitted by installed signing identity.
The same runtime executes both routes. No control TCP listener is exposed.

Use Kotlin for Android framework access. The initial wire extension is
`cua.android.v0`: request ID, operation, session ID, and strictly checked params;
responses include status, structured data/error, and execution evidence.
Requested/actual delivery and effect vocabulary follows the shared Driver
contract. This extension does not add Android to existing closed desktop enums
or claim the complete desktop ToolRegistry authorization contract is implemented.

## Boundaries

Only an explicitly installed and started helper can exercise shell-granted
capabilities. Ordinary APK installation does not grant control of other apps.
Caller authentication, app allowlists, session generations, current target
placement, snapshot geometry, bounded operations, and expiry are enforced by
the runtime. Unknown fields and unsupported operations refuse rather than
falling back to the main display or another transport. Capture excludes secure
content as required by Android. Logs must omit screenshots and typed content
unless an operator explicitly requests test evidence.

## Initial slice and remaining work

The first slice proves create/launch/capture/tap/stop with two synthetic apps.
Use an expiring session and serialized bounded operations; publish lifecycle
limits. Native event acceptance is an unverifiable effect until independent
fixture evidence confirms the result. Raw text, accessibility refs, concurrent
IME isolation, physical phone support, an autonomous agent loop, and general
third-party app qualification remain separate acceptance gates. The production
SDK should use typed operations and an app lifecycle owner; a synchronous local
IPC client is an initial transport slice, not that complete public SDK.

## Validation

Build reproducible APKs and host bridge. Check request rejection, caller denial,
stale handles, target placement, bounded input, frame dimensions, main/target
fixture state, and cleanup. Repeat through an on-device app. Preserve desktop
dispatch with focused host tests; do not replace canonical desktop certification
with Android smoke evidence. No stable release or merge is authorized by a
passing experiment alone.

## Alternatives and unresolved decisions

A new `android` CLI verb was rejected. A framework fork, full desktop daemon port,
and multiple public display streams are deferred. Validate virtual-display flags,
focus/IME behavior, app/service lifetime, Binder versus native client transport,
shared schema generation, and preview frame IPC cost before declaring stable
capabilities. Runtime reconnection must not imply exactly-once effects.
