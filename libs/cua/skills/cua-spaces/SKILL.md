---
name: cua-spaces
description: Work inside cua Spaces through the cua MCP server. A Space is a remote or local computer the user can watch; you can run commands in it, move files in and out, show its desktop or a single window on the user's screen, start coding agents inside it, teleport a signed-in app session into it, and share the host's network with it. Use when the user mentions Spaces, asks you to do work "in a Space", or wants a task isolated but visible.
---

# cua Spaces

Spaces are exposed as MCP tools by the `cua` MCP server (`cua mcp`). If the
tools below are missing, ask the user to run `cua agents setup` or
`cua auth login`.

## Get a Space

| Tool | Use |
|---|---|
| `list_spaces` | See the Spaces you can use. Start here. |
| `create_space` | A new Space: `on` = `local` (free, the user's machine), `cloud` (metered), or `host:<name>` (one of the user's machines set up with `cua host setup`); `reuse: true` returns a reachable one first. |
| `add_space` / `remove_space` | Register an existing machine by URL and token, or forget one without deleting it. |
| `delete_space` | Delete a Space's sandbox and stop metering. |
| `stop_space` / `start_space` | Turn a local Space (or one your machines provide) off and back on: suspended or stopped, as `power` in `list_spaces` says. |

Prefer an existing Space. A cloud Space costs money: delete the ones you
created when the task ends.

## Work in it

| Tool | Use |
|---|---|
| `space_bash` | Run a shell command (`space`, `command`, optional `timeout`). |
| `space_write` | Write text to a file. |
| `upload` / `download` | Copy files or folders in and out. |
| `send_file` | Drop a host file into the Space's Downloads (sha256 verified). |
| `list_tools` / `call_tool` | Reach MCP services running inside the Space. |

## Show it to the user

| Tool | Use |
|---|---|
| `open_space_viewer` | Open the full desktop viewer. |
| `show_space_pip` / `hide_space_pip` | Picture-in-picture on the user's screen. |
| `list_space_windows` / `stream_space_window` | Show one window from the Space. |
| `stream_endpoint` | Get a ticketed media URL for the desktop or a window. |

## Agents, apps and network

| Tool | Use |
|---|---|
| `agent_capabilities` | Which agent harnesses the Space supports. |
| `agent_start` | Start an agent run (`space`, `agent`, `prompt`). |
| `agent_status` / `agent_message` / `agent_stop` / `agent_list` | Follow, steer and stop runs. |
| `teleport_manifest` | Preview what moving an app session would copy. |
| `teleport_app` | Move a signed-in app session (tabs, profile) into the Space. |
| `hotspot_start` / `hotspot_status` / `hotspot_stop` | Share the host's network with a Space. |

## Set up a host over ssh (Tailscale or LAN)

To host Spaces on a machine you reach only over ssh (a spare Mac mini on the
user's Tailscale, or the LAN):

1. ssh into it (Tailscale is only the ssh transport, the same as any LAN or
   internet address).
2. Run the install script non-interactively:
   `curl -fsSL https://cua.ai/install.sh | sh -s -- -y --select spaces,host --no-onboarding`.
3. Sign in on the mini: `cua auth login --remote` prints a device code and a
   URL. Show both to the user and have them approve it (on another signed-in
   device or at the URL).
4. `cua host setup --profile spare --name "<name>"` hosts Spaces only (no
   desktop sharing), through the cua.ai relay.
5. Tell the user: the mini needs an active GUI login session (cua-spacesd
   runs as a LaunchAgent in the Aqua session, so it won't start at the
   login window; auto-login is recommended for a headless mini), and it
   needs a one-time grant of Screen Recording and Accessibility to
   cua-spacesd in System Settings.
6. From the laptop: `create_space(on="host:<name>", count=2)`, or
   `cua spaces create macos:26 --on "<name>" --count 2`.

`host:<name>` only resolves machines set up with `cua host setup` (they
register with the relay). `direct:<addr>` attaches one Space that is already
running at that address (`add_space`) and cannot host new ones.

## Rules

- Run `teleport_manifest` and tell the user what will move before
  `teleport_app`; sensitive items need their explicit acknowledgement.
- Only start a hotspot when the Space needs the host's network (VPN,
  intranet), and stop it afterwards.
- Name the Space you used in your answer.
