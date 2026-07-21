# Cua Driver

Background computer-use driver for any agents. Speaks MCP over stdio; drives native macOS apps without stealing focus.

**[Documentation](https://cua.ai/docs/cua-driver)** - Installation, guides, and API reference.

## Repository Layout

| Path                            | Purpose                                                                |
| ------------------------------- | ---------------------------------------------------------------------- |
| `rust/`                         | Cargo workspace for the driver daemon, platform crates, and Rust tests |
| `python/`                       | Python SDK, bundled-binary wrapper, and package tests                   |
| `contract/`                     | Experimental generated SDK contract and fixtures                       |
| `typescript/`                   | Generated TypeScript SDK                                                |
| `tests/fixtures/`               | Source-built GUI harness apps and shared fixtures                      |
| `rust/crates/cua-driver/tests/` | Rust integration tests for the driver and GUI harnesses                |
| `scripts/`                      | Install, uninstall, local build, and VM sync helpers                   |
| `docs/`                         | Small repo-local specs that are not part of the hosted docs site       |

Start with `rust/README.md`, `rust/crates/cua-driver/tests/README.md`, and
`tests/fixtures/README.md` when changing driver behavior or tests.

The experimental contract-first SDK prototype is documented in
[`contract/README.md`](contract/README.md). Its checked-in Python SDK surface
and TypeScript SDK are generated from the Rust contract crate and communicate
over the public `cua-driver mcp` stdio transport.

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
- **Embedded:** have the macOS app that owns the grants spawn `cua-driver serve --embedded` directly, wait for its private socket, then spawn `cua-driver mcp --embedded --socket <path>` as the stdio proxy. The daemon stays in the app's responsibility chain and inherits its grants. A gateway, terminal, or unrelated helper must not spawn the daemon on the app's behalf.

Directly spawning a raw `cua-driver serve` outside `CuaDriver.app` without embedded mode is unsupported: it has no stable bundle identity for TCC attribution. Do not grant permissions to arbitrary binary paths or rely on that configuration in production. See [`rust/Skills/cua-driver/EMBEDDING.md`](rust/Skills/cua-driver/EMBEDDING.md) for the embedding contract and examples.

## Publishing the agent skill to ClawHub

The canonical skill source is `rust/Skills/cua-driver`. It is published as one
cross-platform ClawHub skill at `@cua/driver`; the bundle includes the
macOS, Windows, and Linux documents. Direct installs through `cua-driver skills
install` still keep only the host OS document unless `--all-platforms` is used.

ClawHub releases have their own explicit license boundary. The repository stays
under MIT, while every skill copy published through ClawHub is distributed
under MIT-0. Before a release, retain an internal record that Cua AI has the
right to distribute every bundled file under MIT-0.

Pull requests that change the skill run a publish dry-run with the pinned
ClawHub CLI. A real release is available only through the
`ClawHub: cua-driver skill` workflow's manual dispatch. The publish job requires
all of the following:

1. Dispatch the workflow from `main`.
2. Enter a version that matches both `rust/Cargo.toml` and the `version` field
   in `rust/Skills/cua-driver/SKILL.md`.
3. Confirm the MIT-0 rights check in the workflow form.
4. Configure a repository Actions secret named `CLAWHUB_TOKEN` for a publisher
   that can release under the selected owner. The default owner is `cua`.

The workflow pins the ClawHub CLI, records the source repository, commit, ref,
and path, and uploads the JSON publish result as an Actions artifact.

After publishing, inspect and scan the exact version, then install it into an
empty work directory:

```bash
npx --yes clawhub@0.23.1 inspect @cua/driver --version 0.8.3 --files
npx --yes clawhub@0.23.1 scan --slug driver --version 0.8.3 --update
npx --yes clawhub@0.23.1 --workdir /tmp/cua-driver-clawhub-smoke \
  install @cua/driver
```

Confirm that `MACOS.md`, `WINDOWS.md`, and `LINUX.md` are present, run
`cua-driver doctor`, and perform a read-only `list_apps` call through OpenClaw.
If a release is faulty, publish the last known-good content as a new patch
version. Do not delete the current latest version before a replacement exists.
