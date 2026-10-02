---
name: cua-spaces
description: Work in cua Spaces through the cua MCP server. A Space is a local or remote computer the user can watch: run commands, move files, use its screen and browser, run coding agents, teleport a signed-in session, share it. Use when the user mentions Spaces or wants work done in an isolated but visible computer.
---

# cua Spaces

The `cua` MCP server (`cua mcp`) exposes Spaces. If its tools are missing, ask the user to run `cua agents setup`.

## Tools

| Tool | Use |
|---|---|
| `list_spaces` | What exists. Start here and prefer an existing Space. |
| `create_space` | New Space. `on`: `local` (free), `cloud` (metered), `host:<machine>`. `reuse: true` returns a reachable one. `images` lists images. |
| `space` | `start`, `stop`, `delete` (what you created), `forget`, `add` (`url`, `token`). |
| `space_bash`, `space_files` | Run a command; `write` text, `upload`, `download`, `send` a file. |
| `computer`, `window` | The Space's screen: `screenshot` first (coordinates are its pixels), then click, type, key, scroll; windows, apps, accessibility tree. |
| `list_tools`, `call_tool` | Tools of services inside the Space: the desktop driver and any MCP server it runs. |
| `open_browser` | A throwaway browser; then use the browser tools through `call_tool`. |
| `agent` | Run a coding agent in the Space: `start`, then `status`, `events`, `message`, `stop`. |
| `volume` | The user's shared files (see the cua-volume skill). |
| `teleport`, `request_site_login` | Bring the user's signed-in session or saved password into a Space. |
| `share_space` | Share a Space with someone. |
| `show_space` | FOR THE HUMAN: put the Space on their screen. Only when asked. |
| `more` | Rare tools: cloud accounts, persistent agents, routines, hotspot, relay, volume storage. No name lists them. |

## Approval

Actions that touch the user's own machines, cloud account, secrets, files or network ask the user for Touch ID. `approvals` shows which. If a call returns `approval_denied`, stop and tell the user what you wanted to do; never retry another way. Teleport, site logins and sharing always ask: the first call returns a `request_id`, the user approves in Cua, then call again with it.

## Rules

- Delete the Spaces you created when done; cloud ones cost money.
- Preview with `teleport` `manifest` and tell the user what moves before `app` or `browser`.
- Start a hotspot (`more`) only when the Space needs the host's network; stop it after.
- Name the Space you used in your answer.

## Host over ssh

For a machine reached only by ssh: ssh in, run `curl -fsSL https://cua.ai/install.sh | sh -s -- -y --select spaces,host --no-onboarding`, run `cua auth login --remote` and show the user the code and URL to approve, then `cua host setup --profile spare --name "<name>"`. The machine needs a GUI login session and a one-time Screen Recording and Accessibility grant to cua-spacesd. Then `create_space(on="host:<name>", count=2)`.
