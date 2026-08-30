# Cua Driver plugin

This plugin connects supported coding agents to a locally installed Cua Driver
over MCP and supplies the cross-platform Cua Driver agent skill.

## Prerequisite

Install Cua Driver on the computer whose desktop the agent will control, then
complete the operating-system permission setup described in the
[installation guide](https://cua.ai/docs/how-to-guides/driver/install).

Verify the installation before enabling the plugin:

```bash
cua-driver --version
cua-driver doctor
```

The plugin does not install or update the native `cua-driver` executable.

## What the plugin provides

- A local stdio MCP server configuration that runs `cua-driver mcp`
- The Cua Driver skill and its macOS, Windows, Linux, browser, recording, and
  embedding references
- Manifests for Claude Code, Grok Build, and Codex CLI

The plugin uses the bundled MCP server and does not fall back to shell commands
or another computer-use provider when that server is unavailable.

Start a new agent session after installing or updating the plugin. Begin with a
read-only prompt such as:

```text
Use $cua-driver to list the visible applications and windows. Do not interact
with them yet.
```

The skill requires fresh state before actions and fresh verification after
actions. Consequential operations still require the user's authorization.

## Source and license

The canonical skill source lives in
[`libs/cua-driver/rust/Skills/cua-driver`](../../rust/Skills/cua-driver). The
plugin package is generated from that source and is released under the
repository's MIT license.
