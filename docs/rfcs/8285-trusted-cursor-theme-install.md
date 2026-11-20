---
title: Trusted SDK installation of compiled cursor themes
authors:
  - darkness22s
created: 2026-11-20
last_updated: 2026-11-20
status: review
discussion: https://github.com/trycua/cua/issues/8285
rfc_pr:
implementation:
  - https://github.com/trycua/cua/issues/8285
supersedes:
superseded_by:
---

# RFC: Trusted SDK installation of compiled cursor themes

## Summary

Add an explicit, trusted embedding-host API for installing a bounded compiled `.cua-theme` artifact into the current user's Cua Driver theme store. This enables desktop applications to offer a user-initiated theme import flow while keeping theme installation out of agent-facing tools and remote-session APIs. This RFC proposes accepting compiled artifacts only; theme authoring and compilation remain in the existing CLI workflow.

## Motivation

Cua Driver currently exposes agent cursor theme selection through the SDK, but installation and authoring are local CLI operations. An application embedding the SDK cannot provide an in-app import flow without requiring a separate CLI binary or duplicating Cua's validation and theme-store logic. The trusted host should be able to install a theme the user selected without granting that authority to the agent.

## Goals

- Provide a cross-platform SDK method for a trusted host application to install a compiled `.cua-theme` artifact supplied by the user.
- Reuse the existing bounded artifact validation and atomic theme-store installation.
- Return the installed theme ID and safe metadata needed by a host-side theme picker.
- Keep the operation absent from agent-facing tool inventories, generic MCP, and remote receivers.

## Non-goals

- Accept arbitrary PNG, SVG, or Lottie files in this SDK method.
- Compile themes or change the `.cua-theme` artifact format.
- Allow agents to install, replace, or remove themes.
- Add a general-purpose filesystem API to the driver.
- Change theme rendering or selection semantics.

## Terminology

- **Compiled theme artifact**: a bounded `.cua-theme` file validated by Cua Driver's existing artifact decoder.
- **Trusted host operation**: an SDK method invoked directly by the embedding application, outside the generic tool/MCP dispatch path.
- **Agent operation**: a driver tool callable through the agent-facing tool inventory or remote receiver.

## Current state

`cursor-overlay` already validates, decodes, installs, lists, and removes compiled artifacts in the user's theme store. The authoring/installation functions are behind the `theme-authoring` feature and are consumed by the `cursor-theme-cli` sidecar. `CuaDriver` exposes `set_agent_cursor_theme`, which selects an installed theme, and the generic tool inventory does not expose theme installation. The TypeScript SDK distributes the Rust-backed driver but does not expose the CLI's local authoring workflow.

## Proposal

Add an additive trusted-host method to the Cua Driver SDK that accepts artifact bytes (not an arbitrary filesystem path). It should:

1. enforce an explicit maximum input size before decoding;
2. validate and decode using the existing compiled-artifact parser;
3. atomically install through the existing current-user theme-store implementation;
4. return the stable theme ID and only safe display metadata (for example, a display name if the artifact schema provides one); and
5. produce structured validation, duplicate, and I/O errors without logging or returning artifact contents or user-specific paths.

Keep the method outside the generic `desktop_tool_methods!` list and agent tool schema. Do not route it through `callTool`, MCP, remote receivers, or serialized agent sessions. Agent sessions continue to select only an installed theme ID using `set_agent_cursor_theme`. An embedding application must invoke installation only in response to an explicit user action. Preserve the existing CLI path and store format.

The SDK method should be exposed consistently to supported bindings through the repository's normal UniFFI generation and package release process. Cross-platform store resolution must remain owned by `cursor-overlay`; callers must not invent platform-specific paths.

## Alternatives considered

- **Spawn the CLI from each embedding application.** This requires a separately installed or bundled compiler/installer binary and adds process, path, and distribution handling to every host.
- **Expose installation as an agent tool.** This would grant untrusted agent input an installation capability and violate the existing boundary between selecting trusted styles and importing user-chosen content.
- **Let every host application implement its own installer.** This duplicates validation, size limits, atomic writes, and storage policy and risks divergent platform behavior.
- **Accept arbitrary image or Lottie source bytes in the SDK.** This expands parsing and rendering inputs, format policy, and attack surface; the existing compiled artifact is a narrower first contract.

## Compatibility and migration

The API is additive; existing SDK methods, tool schemas, and stored themes remain unchanged. Hosts must call the new method only when running a version that supports it. The existing CLI remains available for compiling source themes and for theme administration. If an implementation cannot safely install an artifact, it must leave existing themes untouched and return a structured error. Rollback is to remove or disable the additive API while preserving artifacts already installed in the established store.

## Security, privacy, and telemetry

The host application, not the agent, controls the import action. The operation accepts only bounded compiled data, reuses canonical validation, and never executes theme content. It installs atomically into the current user's store and does not permit arbitrary output paths. No artifact bytes, source filenames, theme contents, or user-specific filesystem paths may be logged or emitted in telemetry. The feature must not add installation authority to the agent, MCP, or remote receiver.

## Implementation plan

1. Add and test the trusted install operation in the common Rust driver/SDK boundary without adding it to generic tool dispatch.
2. Generate and drift-check supported SDK bindings and document the host-only trust boundary.
3. Verify artifact validation, error mapping, store behavior, and relevant platform adapters on macOS, Windows, and Linux.
4. Integrate host applications after a release containing the new API is available; each host must keep upload UX and user confirmation local to the host.

## Test and acceptance plan

- Hermetic tests cover valid installs, invalid headers/payloads, oversized input, duplicate IDs, atomic replacement/refusal behavior, and returned metadata.
- A test proves the method is not present in the generic tool inventory and cannot be invoked through agent or remote dispatch.
- Run focused Rust tests and the repository's binding-generation drift checks for each supported public binding.
- Verify theme-store path resolution and installation behavior on macOS, Windows, and Linux using the repository's desktop test guidance; document any concrete OS/compositor limitation.
- Run the applicable repository CI checks before the implementation PR is ready.

## Unresolved questions

- Should replacing an already-installed custom theme with the same ID be refused or require a separate explicit host confirmation API?
- Which artifact metadata is safe and useful to return to host applications?
- Should removal remain CLI-only for the first version?

## Decision record

Pending maintainer review in [issue #8285](https://github.com/trycua/cua/issues/8285). No implementation should begin until the maintainer decision required by the RFC process is recorded.
