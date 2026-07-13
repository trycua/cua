# Cua Driver

Background computer-use driver for any agents. Speaks MCP over stdio; drives native macOS apps without stealing focus.

**[Documentation](https://cua.ai/docs/cua-driver)** - Installation, guides, and API reference.

## Repository Layout

| Path | Purpose |
| --- | --- |
| `rust/` | Cargo workspace for the driver daemon, platform crates, and Rust tests |
| `python/` | Python package wrapper and package tests |
| `tests/fixtures/` | Source-built GUI harness apps and shared fixtures |
| `rust/crates/cua-driver/tests/` | Rust integration tests for the driver and GUI harnesses |
| `scripts/` | Install, uninstall, local build, and VM sync helpers |
| `docs/` | Small repo-local specs that are not part of the hosted docs site |

Start with `rust/README.md`, `rust/crates/cua-driver/tests/README.md`, and
`tests/fixtures/README.md` when changing driver behavior or tests.

Contributor documentation:

- `docs/test-matrix.md` maps unit and canonical harness E2E suites.
- `docs/action-support.md` is the empirical platform behavior ledger.
- `docs/test-harnesses-guide.md` explains fixture and runner ownership.
- `docs/linux-desktop-validation.md` covers representative Linux sessions.
- `docs/linux-support-completion-plan.md` preserves the historical Linux plan.

## Claude Code computer-use compatibility

Standard Claude Code MCP registration:

```bash
claude mcp add --transport stdio cua-driver -- cua-driver mcp
```

If you want Claude Code's vision/computer-use-style flow to ground on CuaDriver window screenshots, register the compatibility mode:

```bash
claude mcp add --transport stdio cua-computer-use -- cua-driver mcp --claude-code-computer-use-compat
```

This keeps CuaDriver's normal MCP tools and changes only `screenshot`, which requires `pid` and `window_id` and captures that window only.

Use MCP for this Claude Code vision/computer-use-style path. CLI screenshots still work as CuaDriver calls, but they do not expose the `mcp__cua-computer-use__screenshot` tool name that Claude Code appears to use as the image-grounding cue.

## macOS process identity and permissions

macOS attributes Accessibility and Screen Recording grants to a responsible app identity, not simply to an executable path. Use one of these supported launch modes:

- **Standalone:** install `CuaDriver.app`, grant permissions to it, and start its daemon with `open -n -g -a CuaDriver --args serve`. The installed `cua-driver mcp` CLI may proxy through this daemon automatically.
- **Embedded:** have the macOS app that owns the grants spawn the driver directly with `CUA_DRIVER_EMBEDDED=1` (or `--embedded`). This keeps the driver in that app's responsibility chain, so it inherits the app's grants. A gateway, terminal, or unrelated helper must not spawn it on the app's behalf.

Directly spawning a raw `cua-driver` binary outside `CuaDriver.app` without embedded mode is unsupported: it has no stable bundle identity for TCC attribution. Do not grant permissions to arbitrary binary paths or rely on that configuration in production. See [`rust/Skills/cua-driver/EMBEDDING.md`](rust/Skills/cua-driver/EMBEDDING.md) for the embedding contract and examples.
