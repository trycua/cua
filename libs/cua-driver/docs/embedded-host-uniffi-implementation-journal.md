# Embedded Host + UniFFI Implementation Journal

Status: local implementation complete; replacement PR and cross-platform CI pending

## Objective

Make embedded hosting a daemon lifecycle mode shared by both Cua Driver client
surfaces:

- application SDKs connect through the Rust-backed UniFFI `CuaDriver`;
- agent runtimes launch the ordinary `cua-driver mcp` proxy;
- both routes reach the same host-owned daemon and therefore the same tools,
  policy, sessions, and capture-scope behavior.

## Source and attribution

- Base: `origin/main` at `b8a0f32a06c75225ba24ebb5ab14f6507fa90d15`.
- Source contribution: PR #2400 by Zane Chee (`injaneity`) at
  `e2ccf711508daf1cbe2c3f9c162c29d2169770dd`.
- All seven source commits were carried with `git cherry-pick -x`; the replacement PR
  must say `Salvaged from #2400` and retain the contributor's authorship.

## Completion contract

1. Rust owns process startup, readiness, authorization configuration, parent
   liveness, stop/restart, endpoint ownership, and compatibility metadata.
2. Generated Python and TypeScript UniFFI bindings expose that host.
3. The public SDK client remains `CuaDriver` in standalone and embedded modes.
4. Agents use their existing MCP clients with a returned command/args/env
   descriptor; no language-specific MCP client is introduced.
5. Embedded and standalone modes have the same tool contract and session
   semantics.
6. Lifecycle, security, packaging, and SDK/MCP parity tests pass before the
   replacement PR supersedes #2400.

## Decisions

- `EmbeddedCuaDriverHost` names the owner; it is not a second SDK client.
- A per-start generation identifies the endpoint owner and prevents stale
  cleanup from unlinking a replacement endpoint.
- The daemon receives a dedicated parent-liveness stdin pipe. EOF is an
  operating-system-backed death signal on Unix and Windows and avoids PID-reuse
  races.
- Readiness requires a daemon metadata handshake, not an open socket alone.
- Authorization is a typed launch option. Ambient `CUA_DRIVER_*` variables are
  not inherited by an embedded child.
- `/embedded` may provide ergonomic adapters, but canonical lifecycle behavior
  belongs to Rust and is generated into Python and TypeScript.
- Python wheels, the public npm package, and native npm platform packages ship
  from the same Cua Driver tag and exact version. Release validation rejects
  source version drift before registry publication.

## Validation matrix

| Daemon mode | SDK route | Agent route |
| --- | --- | --- |
| Standalone | UniFFI SDK to default/explicit endpoint | `cua-driver mcp` |
| Embedded | UniFFI SDK to host connection endpoint | returned MCP launch descriptor |

Each cell must prove the same contract metadata and representative session/tool
behavior. A mixed-client test must use SDK and MCP concurrently against one
embedded daemon.

## Progress

- [x] Fetched latest `origin/main` and PR #2400.
- [x] Created `agent/cua-driver-embedded-host` from latest main.
- [x] Cherry-picked all seven contributor commits with `-x` attribution.
- [x] Freeze API and supported platform contract.
- [x] Implement daemon metadata handshake and parent-liveness shutdown.
- [x] Implement Rust embedded host and generated bindings.
- [x] Replace handwritten TypeScript lifecycle with a thin generated adapter.
- [x] Add Python and agent-runtime examples.
- [x] Add version-locked PyPI/npm release assembly.
- [x] Complete local lifecycle, SDK/MCP parity, generated-binding, package,
      version-lock, formatting, and sensitive-data validation.
- [ ] Complete Linux, macOS, and Windows CI validation.
- [ ] Publish replacement draft PR and supersede #2400 after parity proof.

## Local evidence

- Rust SDK: 10 unit tests passed, including cancelled-stop recovery.
- Embedded runtime: 5 lifecycle and dual-client integration tests passed.
- Unix daemon ownership: 2 privacy and replacement-socket cleanup tests passed.
- Python: 2 generated UniFFI loader and embedded-host tests passed.
- TypeScript: 3 generated-loader, embedded-host, and `/embedded` identity tests
  passed; `npm audit` reported zero vulnerabilities.
- The npm release assembler produced one public SDK archive plus all six
  platform-native archives at the same version.
- Contract generation and UniFFI generation both passed their deterministic
  `--check` modes.
- The release-version validator confirmed Rust, Python, TypeScript, docs, and
  release metadata all agree on `0.10.0`.
- Full-workspace `-D warnings` remains blocked by pre-existing Clippy findings
  outside this change; ordinary Rust tests and formatting pass.
