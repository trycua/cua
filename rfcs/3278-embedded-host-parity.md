---
rfc: 3278
title: 'Complete Embedded Host Parity for Managed Integrations'
authors:
  - injaneity
created: 2026-08-20
last_updated: 2026-08-20
status: accepted
discussion: https://github.com/trycua/cua/issues/3278
rfc_pr: https://github.com/trycua/cua/pull/3279
implementation:
  - https://github.com/trycua/cua/pull/3280
  - https://github.com/trycua/cua/pull/3281
  - https://github.com/trycua/cua/pull/3282
  - https://github.com/trycua/cua/pull/3283
supersedes:
superseded_by:
---

# RFC 3278: Complete Embedded Host Parity for Managed Integrations

## Summary

Cua Driver will complete the released `EmbeddedCuaDriverHost` compatibility
adapter so managed integrations can delegate private-daemon lifecycle without
losing daemon presentation policy, startup diagnostics, or control over binary
distribution. The adapter will gain additive typed overlay and diagnostics
surfaces. Host-only SDK artifacts may omit the driver executable, but callers
using them must supply an absolute executable from the exact matching Cua
Driver component release. Existing packages, defaults, lifecycle behavior, and
exact contract validation remain unchanged.

## Motivation

`EmbeddedCuaDriverHost` already owns the sensitive and failure-prone parts of a
private daemon lifecycle:

- private endpoint preparation and ownership checks;
- child startup and parent-liveness binding;
- readiness and metadata validation;
- exact SDK, daemon, capability, and MCP contract checks;
- canonical MCP connection construction; and
- orderly shutdown, forced termination, and endpoint cleanup.

A downstream integration can still need a parallel process manager because the
adapter cannot express all released host policy or diagnostics. In particular,
serve-side overlay policy is not represented, and startup stderr can only be
inherited or discarded. The Python package also includes an exact driver
executable even when a managed application already distributes one. Using the
separately installed executable avoids running the duplicate, but it leaves
package size and independently updated versions as an undocumented integration
boundary. The Node package already separates its native SDK payload and does
not bundle the driver executable, so it needs no parallel host-only package.

This prevents downstream applications such as Hermes Agent from replacing
their private-daemon lifecycle code with a thin policy adapter. Cua should own
Cua lifecycle mechanics, while the embedding product should continue to own
its permission mode, capability manifest, presentation policy, session
lifetime, and platform identity.

This RFC is a narrow follow-up to [RFC 2549](2549-cua-driver-sdk-owned-runtime.md).
It completes the released service compatibility adapter. It does not displace
RFC 2549's direct runtime as the preferred application boundary or redefine its
private-worker topology.

## Goals

- Add a typed, default-preserving way to disable the embedded daemon's cursor
  overlay.
- Let a host retrieve bounded, structured evidence after embedded startup
  fails.
- Offer a host-only Python artifact without a bundled driver executable.
- Preserve exact contract validation for externally supplied executables.
- Make version mismatches deterministic and actionable before a host exposes an
  MCP connection.
- Preserve all existing package names, constructors, defaults, generated
  bindings, and bundled-binary workflows.
- Give downstream applications enough parity to delete duplicate socket,
  readiness, MCP, shutdown, and cleanup code.

## Non-goals

- Relax exact SDK and daemon contract matching.
- Search `PATH`, silently download a driver, or select a global daemon.
- Add arbitrary serve arguments or unrestricted environment passthrough.
- Change direct-runtime behavior or make daemon embedding the default SDK path.
- Transfer macOS TCC ownership from an embedding application to
  `CuaDriver.app`.
- Define a downstream product's permission modes, manifests, approval UX, or
  update policy.
- Remove the executable from the existing Cua Driver Python package.
- Rename or duplicate the existing Node package, which already omits the driver
  executable.
- Promise that two independently updated component releases remain compatible.

## Terminology

**Embedded host**
: The released `EmbeddedCuaDriverHost` object that supervises a private
  `cua-driver serve --embedded` child and returns a validated MCP connection.

**Bundled artifact**
: The existing Python Cua Driver package containing generated bindings, its
  native SDK library, and the matching driver executable.

**Host-only artifact**
: A proposed Python package containing generated bindings and the native SDK
  library, but no driver executable.

**External executable**
: An absolute driver path supplied by trusted host code rather than resolved
  from the SDK package.

**Exact match**
: Agreement on the contract, tools-list schema, capability, and MCP protocol
  versions validated by the embedded host. Component semantic versions remain
  diagnostic context rather than a substitute for those checks.

## Current state

The embedded host record in
[`embedded.rs`](../libs/cua-driver/rust/crates/cua-driver-sdk/src/embedded.rs)
accepts binary, endpoint, timeout, permission, manifest, environment, and stderr
inheritance options. It constructs the `serve` arguments internally, so a host
cannot safely append `--no-overlay`.

The same implementation clears the child environment and restores only a fixed
allowlist. That security boundary is intentional. The focused fix in
[#3277](https://github.com/trycua/cua/pull/3277) preserves the driver's two
supported telemetry settings without opening generic passthrough.

For stderr, `inherit_stderr: true` writes directly to the parent's stream and
`false` sends output to null. Neither mode lets an application attach a bounded
startup explanation to its own structured diagnostics.

The Python package build in
[`pyproject.toml`](../libs/cua-driver/python/pyproject.toml) publishes both the
native SDK library and executable. This guarantees that package helpers can
select a matching executable, but managed applications that already distribute
Cua Driver carry another copy. Node release packaging already publishes only
the native SDK and runtime in platform packages, so no Node packaging change is
required.

The embedded readiness handshake already rejects exact contract mismatches.
When a caller supplies an independently updated executable, that refusal occurs
after process spawn and is reported as a general incompatible-daemon reason.
There is no distribution contract telling a host how to keep a host-only SDK
and executable aligned.

## Proposal

### 1. Add typed daemon presentation options

Add `no_overlay: bool` to `EmbeddedDriverHostOptions` with a generated default
of `false`. When true, the embedded host adds `--no-overlay` to the owned
`serve` command.

This remains a typed option rather than a generic argument list. The SDK keeps
control over embedded mode, endpoint, parent liveness, host identity,
authorization, manifests, and dangerous acknowledgements. Unknown or future
serve arguments require their own contract review.

The default preserves current behavior. The option applies to every supported
platform, while the daemon remains responsible for whether an overlay facility
exists on that platform.

### 2. Return bounded startup diagnostics

Add an opt-in diagnostics capture option and a generated diagnostics record.
The record must be available after `start()` fails and include only:

- the lifecycle phase;
- child exit status when known;
- whether output was truncated; and
- a bounded stderr tail.

Expose the record through `last_diagnostics()` so released lifecycle error
variants remain unchanged.

Capture is disabled by default. Existing stderr inheritance behavior remains
unchanged. When both inheritance and capture are requested, the implementation
tees stderr to the parent while retaining only the bounded tail. Capture must
never block child shutdown, retain unbounded output, or enter telemetry.

The tail retains the final 65,536 stderr bytes and then uses lossy UTF-8
conversion. The record marks truncation when earlier bytes were discarded.

### 3. Keep exact compatibility for external executables

The SDK continues to require an absolute executable path and the existing exact
metadata agreement. A host-only artifact does not search `PATH`, choose the
newest installation, silently download a release, or accept a protocol range.
Trusted host code owns executable provenance and update coordination.

Before returning an MCP connection, mismatch diagnostics must identify expected
and observed values for:

- component version when available;
- contract version;
- tools-list schema version;
- capability version; and
- MCP protocol version.

The daemon metadata handshake remains authoritative because semantic version
alone does not prove generated-contract parity. The implementation does not add a semantic-version preflight; the daemon
metadata handshake remains the single compatibility authority.

A host-only package and external executable are supported only when they come
from the same Cua Driver component release. Applications that update the
executable independently must update the host package as one operation or keep
their current lifecycle implementation.

### 4. Publish host-only SDK artifacts

Publish an additional Python distribution containing:

- generated language bindings;
- the native SDK library needed by those bindings; and
- package metadata identifying the Cua Driver component release.

They omit the driver executable and require the application to pass an
absolute matching executable path. Existing distributions keep their bundled
executables and remain the default for ordinary SDK installation.

Publish the artifact as `cua-driver-host` on PyPI. It must not shadow the
existing command entry point, claim to be pure Python, or make a host-only
installation appear runnable without an external executable.

Package tests must inspect built wheels rather than source directories. Release
automation publishes host-only and bundled artifacts from the same candidate
SHA and component version.

### 5. Preserve platform ownership

These changes do not alter process responsibility:

- on macOS, the signed embedding application must create the host directly so
  the child remains in its TCC responsibility chain;
- on Windows, the embedding application must run in the interactive user
  session; and
- on Linux, the host supplies the intended display and user-session
  environment through the existing safe allowlist.

The SDK does not launch through `CuaDriver.app` or an unrelated broker for this
embedded topology. A downstream Python backend cannot claim a desktop
application's identity merely by setting `host_bundle_id`.

## Alternatives considered

### Keep downstream process managers

This preserves every application-specific option and package arrangement. It
also duplicates endpoint safety, readiness, protocol, MCP, shutdown, and
cleanup behavior. That duplication is the problem this RFC addresses.

### Add arbitrary serve arguments

A generic argument array would close presentation gaps quickly. It could also
override endpoint, host identity, authorization, parent-liveness, or other
invariants that the SDK must own. Typed additive options keep the embedding
contract reviewable.

### Always disable overlays in embedded mode

Some embedding applications use the Cua cursor indicator. Changing the default
would be observable and unnecessary. A default-false option preserves current
behavior.

### Return unbounded child logs

Unbounded capture can consume host memory and expose unrelated process output.
A bounded, opt-in tail provides startup evidence without becoming a logging
transport.

### Keep bundling the executable everywhere

This is the safest default because the package owns a matching binary. It is
retained for existing packages. It does not serve managed products that already
distribute Cua Driver and want one executable installation.

### Accept a semantic-version or protocol range

Range compatibility reduces release coupling but can combine authorization,
schema, or lifecycle behavior that was never certified together. Exact
metadata agreement remains the safer contract. A future range requires its own
compatibility RFC and cross-version evidence.

### Launch through `CuaDriver.app`

That gives a stable Cua-owned macOS identity, but it changes the embedded host's
permission owner and lifecycle topology. Standalone service mode already owns
that use case. Embedded mode remains host-owned.

## Compatibility and migration

Delivery is additive:

1. Existing packages gain the default-false presentation field through
   regenerated bindings.
2. Existing callers continue inheriting or discarding stderr exactly as today
   unless they opt into diagnostics.
3. Mismatch errors become more structured without accepting combinations that
   are currently refused.
4. The new host-only package ships alongside, not instead of, the bundled
   Python package.

Downstream applications can migrate behind their current lifecycle interface:
construct options, start the SDK host, consume the returned MCP command,
arguments, and environment unchanged, then stop the host during session
cleanup. They can roll back to their process manager without changing the
installed daemon or public MCP tool contract.

No existing constructor, record field, package export, executable path helper,
or command entry point is removed. Generated bindings must retain source
compatibility according to each language's supported package policy.

## Security, privacy, and telemetry

The embedded host remains trusted host code. Agent-visible input cannot select
an executable, permission mode, manifest approval, overlay policy, diagnostic
mode, or lifecycle operation.

The fixed environment allowlist remains in force. New host requirements use
typed fields rather than generic arguments or environment passthrough. Existing
endpoint ownership, daemon PID, embedded-mode, host-identity, parent-liveness,
and exact contract checks remain mandatory.

An external executable is code execution selected by the application. The SDK
requires an absolute path but cannot make an arbitrary caller-selected binary
trustworthy. Host-only package documentation must require the application to
verify and distribute the executable through its normal trusted update path.

Captured stderr is local, opt-in, and strictly bounded. It may contain local
paths or platform diagnostics, so applications must treat it as diagnostic
data rather than telemetry. Cua telemetry must never include captured stderr,
environment values, capability manifests, screenshots, accessibility content,
typed text, clipboard content, browser data, or local file contents.

Permission ownership remains unchanged. In particular, `host_bundle_id` is a
protocol assertion checked against the daemon; it does not grant or transfer
macOS TCC identity.

## Implementation plan

### PR 1: Embedded daemon presentation

- Add the additive `no_overlay` option with a generated default.
- Pass the exact serve-side flag only when enabled.
- Regenerate Python and TypeScript bindings and signature fixtures.
- Test default and enabled serve arguments on portable lanes.

### PR 2: Bounded startup diagnostics

- Add the accepted diagnostics option and result shape.
- Implement non-blocking bounded stderr collection and optional tee behavior.
- Preserve current inheritance defaults and Windows no-console behavior.
- Test startup exit, timeout, truncation, concurrent stop, and cleanup.

### PR 3: External executable mismatch evidence

- Return accepted structured expected and observed contract fields.
- Keep the metadata handshake authoritative without adding semantic-version
  preflight.
- Test matching, mismatching, early-exit, and endpoint-substitution cases.

### PR 4: Host-only Python package

- Add the accepted Python distribution name.
- Exclude executables while retaining the native SDK library and generated
  bindings.
- Publish from the same release candidate as bundled artifacts.
- Test archive contents and launch against an explicitly supplied matching
  fixture.

Each pull request remains independently revertible. PR 4 depends on PR 3. PRs
1 and 2 do not need to stack on unrelated implementation branches.

## Test and acceptance plan

The RFC is complete when:

- Rust unit tests prove default and enabled presentation arguments;
- generated Rust, Python, and TypeScript contracts remain synchronized;
- previous supported package signatures remain source compatible;
- diagnostics tests prove the byte cap, truncation marker, optional tee,
  startup timeout, early exit, and cleanup behavior;
- diagnostics are absent from Cua telemetry fixtures;
- mismatch tests expose expected and observed contract fields and never return
  an MCP connection;
- host-only wheels contain the native SDK and omit the driver executable;
- host-only package smoke tests launch a matching explicit fixture and reject a
  mismatching fixture;
- macOS, Windows, and Linux compile and portable contract lanes pass; and
- existing bundled-package and embedded-host lifecycle tests remain green.

This work changes no desktop action semantics, so the full interactive desktop
matrix is not required unless an implementation diff reaches platform action,
overlay rendering, input, capture, or permission code. If it does, the affected
canonical lane must run on the exact candidate SHA.

## Unresolved questions

- Which package-signature fixtures best prove additive record fields for every
  supported language version?

## Decision record

Accepted by maintainer direction on 2026-08-20. Review selected a
`last_diagnostics()` accessor, a final 65,536-byte stderr tail with lossy UTF-8
conversion, tee behavior when inheritance and capture are both enabled,
the `cua-driver-host` package name, and the existing daemon metadata handshake
as the sole compatibility authority. Repository inspection during
implementation confirmed that Node already omits the driver executable, so no
new npm package is needed. Exact contract
matching, typed options, current defaults, and host-owned platform identity
remain mandatory. Implementation will proceed as linked draft pull requests;
the RFC must merge before or with the first implementation pull request.
