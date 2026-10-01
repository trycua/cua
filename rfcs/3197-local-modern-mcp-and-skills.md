---
title: Local modern MCP and embedded skills
authors:
  - f-trycua
created: 2026-09-07
last_updated: 2026-09-07
status: review
discussion: https://github.com/trycua/cua/issues/3197
rfc_pr: https://github.com/trycua/cua/pull/3609
implementation:
  - https://github.com/trycua/cua/pull/3609
supersedes:
superseded_by:
---

# RFC: Local modern MCP and embedded skills

## Summary

Add MCP `2026-07-28` to the existing Rust stdio server while retaining legacy
initialization. Serve the canonical bundled Cua Driver skill through the
Skills extension and base Resources methods. This is the local protocol
increment of [#3197](https://github.com/trycua/cua/issues/3197), selected for
implementation in [#3609](https://github.com/trycua/cua/pull/3609). It does not
resolve the optional OAuth gateway decisions in
[#3198](https://github.com/trycua/cua/pull/3198).

## Motivation

Modern MCP clients negotiate each request instead of requiring `initialize`.
The previous Driver adapter only supports initialization-based clients.
Separately, agents need the Driver workflow instructions and companion files
without requiring a second tool server or an installation into host skill
directories.

## Goals

- Serve modern and legacy stdio clients through the canonical Rust runtime.
- Support discovery, strict per-request metadata, and modern result envelopes.
- Expose one complete, verifiable skill pack with bounded lazy reads.
- Preserve Driver authorization, session ownership, and unknown-outcome retry
  guidance across direct and daemon-backed stdio paths.

## Non-goals

- OAuth, public HTTP, tunnels, remote desktop grants, or gateway ownership.
- Modern HTTP application handles and cross-connection lifecycle.
- Client-side skill activation, consent storage, or script execution.
- Arbitrary installed skills, directory traversal, dynamic skills, archives,
  optional directory listing, subscriptions, tasks, or server-initiated requests.
- Replacing the installer integrity work in
  [#2295](https://github.com/trycua/cua/issues/2295).

## Terminology

**Modern MCP** means protocol version `2026-07-28`, not the JSON-RPC version or
an SDK major version. **Legacy** means initialization-based MCP. **Skill
serving** provides bytes and metadata; the client separately controls loading,
activation, and permission decisions.

## Current state

The shared [dispatcher](../libs/cua-driver/rust/crates/cua-driver-core/src/server.rs)
and [stdio adapter](../libs/cua-driver/rust/crates/cua-driver/src/proxy.rs) own MCP
translation. The SDK and daemon own execution. The
[HTTP adapter](../libs/cua-driver/rust/crates/cua-driver/src/mcp_http.rs) binds
session ownership to each TCP connection. The eight-file
[canonical skill](../libs/cua-driver/rust/Skills/cua-driver/SKILL.md) is already
versioned with Driver releases.

## Proposal

Pin implementation and conformance fixtures to the
[base specification](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/e76e9c572c6f2bfcb730357101acc90f2f802e02/schema/2026-07-28)
and [SEP-2640](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/d6b31a03504c15677d49b922b6b6ace0ef65728d/seps/2640-skills-extension.md).
The SEP text is accepted, but its upstream pull request remains open. Describe
support as the pinned proposal implementation until publication is finalized.

### Stdio protocol

A legacy `initialize` selects legacy behavior for metadata-free requests on
that stdio connection. Modern requests carry both
`params._meta["io.modelcontextprotocol/protocolVersion"]` and
`params._meta["io.modelcontextprotocol/clientCapabilities"]`. Capabilities are
never inferred from a prior request. A modern request may arrive without any
initialization. Missing or malformed metadata fails before tool invocation.

`server/discover` reports the modern version and supported capabilities.
Modern successful results include `resultType: "complete"`; discovery and
list/read results include conservative `ttlMs: 0` and `cacheScope: "private"`
hints where specified. Unsupported versions return `-32022` with the requested
and supported versions. Metadata stays outside tool arguments and does not
establish a principal or a Driver session.

Existing stdio process/control ownership remains the application lifetime.
Disconnect invalidates that owner as before. Reconnection does not replay
actions or revive an old owner's sessions. A lost action response remains an
unknown outcome requiring verification.

### Embedded skills

Embed the existing eight Markdown files into the Rust core at build time.
Construct one immutable catalog containing the full YAML frontmatter and each
file's raw-byte SHA-256 digest and byte length. A single static catalog page
enumerates every file; reads use exact URI matching rather than filesystem
resolution. No runtime network, home-directory reads, symlinks, or extraction
are involved.

Publish `skill://cua-driver/SKILL.md` through `skills/list` and `skills/get`.
Expose all eight files through `resources/list` and `resources/read`.
Declare `resources` and the `io.modelcontextprotocol/skills` extension in both
legacy initialization and modern discovery. Do not declare optional directory
reads or other unimplemented capabilities. Both direct and daemon-backed
stdio serve the pack embedded in the MCP endpoint binary. The pack version
identifies that endpoint, not an attestation of the executing daemon's build.
Mixed-version daemons remain subject to the existing SDK contract checks and
explicit output-schema compatibility check. Clients must use the advertised
tool inventory; operators should update the endpoint and daemon together.

The URI is stable; the complete digest set identifies the content revision.
The host must verify bytes against that set and handle changed-content consent
according to the pinned SEP. Serving or reading a skill grants no authority to
execute code, broaden policy, or activate a skill in the host.

### HTTP boundary

Keep the authenticated loopback HTTP endpoint on the legacy protocol. Reject
modern version metadata, modern version headers, and modern discovery before
dispatch, with a diagnostic directing callers to stdio. HTTP must not silently
advertise a modern profile whose application identity still depends on one TCP
connection. Modern HTTP remains a later increment of the existing RFC.
The rejection uses HTTP 400 and legacy `-32600`, rather than the modern
`-32022` negotiation signal, so dual-era clients may fall back to initialization.

## Alternatives considered

A modern-only server would break deployed legacy clients. A second TypeScript
execution facade, as explored in [#2824](https://github.com/trycua/cua/pull/2824),
would require equivalent runtime authority and lifecycle behavior. This
proposal instead exposes instructions from the canonical Rust endpoint.
Dynamic reads of installed packs would couple serving to mutable local files
and installer provenance; embedding gives the initial server a bounded source.

## Compatibility and migration

Keep existing tool signatures and the legacy initialization version. Select
modern behavior through explicit request metadata. Installed skill directories
and plugin generation are unchanged. Advertise only the transport/profile
actually implemented; a base MCP client is not assumed to implement Skills.
Rollback uses the prior Driver release and its legacy protocol configuration.

## Security, privacy, and telemetry

The manifest proves consistency with served bytes, not trustworthiness of a
server. Keep host consent and execution approval separate. Exact resource
allowlists reject paths, encoded aliases, unknown schemes, query strings, and
foreign origins. Preserve the existing bounded telemetry policy: do not record
arbitrary request metadata, skill bodies, or client capability payloads.

## Implementation plan

1. Add shared version validation, discovery, and result-envelope helpers.
2. Add immutable skill metadata and exact resource reads.
3. Integrate direct/proxy stdio and reject unsupported modern HTTP requests.
4. Add protocol fixtures, real-process coverage, documentation, and client proof.

## Test and acceptance plan

Require tests for metadata errors before execution, unsupported versions,
legacy negotiation, modern discovery and result envelopes, direct/proxy parity,
raw-byte hashes and sizes, full frontmatter equality, complete manifests, and
rejected resource aliases. Verify a real modern stdio client can discover the
server, enumerate skills, retrieve a skill, and read its resources. Report
native host activation separately from raw protocol interoperability.

Ordinary cross-platform CI must pass. Canonical desktop certification belongs
to the stable landing candidate. The macOS VM capacity blocker remains an
environment prerequisite; prior source-PR certification is not certification
of this new implementation.

## Unresolved questions

- Final upstream publication and any subsequent SEP-2640 changes.
- Which host versions support native Skills activation and consent.
- Trusted application handles and client isolation for modern HTTP.
- Whether to serve only the host platform documents in a later pack revision.
- A daemon/endpoint skill-pack attestation for mixed-version deployment.

## Decision record

The maintainer selected this local implementation increment on 2026-09-07.
The proposal and implementation remain in draft review. Selection does not
declare the broader gateway RFC accepted or claim the acceptance tests passed.
