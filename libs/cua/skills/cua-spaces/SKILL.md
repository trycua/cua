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
| `create_space` | A new Space: `on` = `local` (free, the user's machine), `aws`, `gcp` or `modal` (the user's own cloud account, billed there), or `host:<name>` (one of the user's machines set up with `cua host setup`); `reuse: true` returns a reachable one first. |
| `add_space` / `remove_space` | Register an existing machine by URL and token, or forget one without deleting it. |
| `delete_space` | Delete a Space's sandbox and stop its cost. |
| `stop_space` / `start_space` | Turn a local Space (or one your machines provide) off and back on: suspended or stopped, as `power` in `list_spaces` says. |

Prefer an existing Space. A Space in the user's own cloud account costs money: delete the ones you
created when the task ends.

## Work in it

| Tool | Use |
|---|---|
| `space_bash` | Run a shell command (`space`, `command`, optional `timeout`). For anything under a minute. |
| `space_job` | Run a long command in the background (`action`: `start`, `status`, `logs`, `cancel`, `list`). See "Long work" below. |
| `space_write` | Write text to a file. |
| `upload` / `download` | Copy files or folders in and out. |
| `send_file` | Drop a host file into the Space's Downloads (sha256 verified). |
| `list_tools` / `call_tool` | Reach MCP services running inside the Space. |

## Long work

Builds, test suites and waits on CI take minutes. Do not hold a `space_bash`
call open for them: a job started with `space_job` keeps running when you stop,
disconnect or the Space's daemon restarts, and it can wake you when it ends.

Long work (a build, a test suite, a wait on CI): never sit inside one call. Start it with space_job (action start, notify true, a name of a few words), post one line in the thread saying what you started, then end your turn. When it ends the thread gets `JOB <id> <state> exit=<n> ...` with the end of the log, and you receive it as your next task; read more with space_job logs (offset: the next_offset you were given). If you cannot be woken, poll with space_job logs and follow_ms instead of sleeping in space_bash.

- `space_job {action: "start", command, name, notify: true}` returns a job id
  at once. `cwd`, `env` and `timeout` (seconds) are optional.
- `space_job {action: "logs", job, offset}` reads the log from a byte offset
  and returns `next_offset`; `follow_ms` (at most 25000) waits for output.
  Without `offset` you get the end of the log. `status` is the state plus the
  last few KB. `list` shows the Space's jobs. `cancel` stops the whole process
  tree.
- `state` is `running`, `exited` (with `exit_code`), `cancelled`, `timed_out`
  or `lost` (the Space restarted and the job died with it).
- To wait for something that is not your own command, use `until` instead of
  `command`: `{file: path}`, `{port: 5432}`, `{command: "test -f x"}` or
  `{pr_checks: "owner/repo#123"}` (the pull request's required checks, through
  `gh` signed in inside the Space). The job ends when the condition holds, so
  `notify: true` wakes you at that moment.

## Working with other agents

Working with other agents (a parent Space and its child Spaces, or any agents in one Space): 1. Before you start, thread_read and note what others have claimed. 2. Post `CLAIM <what> | files: <paths or branch> | until: <when>` for what you will change. Do not edit files or branches another agent's live CLAIM covers: @mention the owner and wait for the reply. 3. A claim is live until its `until` passes or its owner posts DONE or RELEASE; once it has expired you may take it, and you say so in the thread. 4. Report with one structured line: `DONE <what> | <result> | next: <step>` or `BLOCKED <what> | needs: <thing> | from @who`. 5. Keep messages short, one @mention per request, and never your own name.

Example, for any task: `CLAIM refactor the parser | files: src/parser/** on
branch parser-refactor | until: 16:00` ... `DONE refactor the parser | merged
into the branch, tests pass | next: docs`. A parent Space's agent that starts
agents in child Spaces gives each one the parent thread's join link (see
`thread_join_code_create`), so the whole group reads and posts in one place.

## Show it to the user

| Tool | Use |
|---|---|
| `open_space_viewer` | Open the full desktop viewer. |
| `show_space_pip` / `hide_space_pip` | Picture-in-picture on the user's screen. |
| `list_space_windows` / `stream_space_window` | Show one window of a Space in your own Space's Stream section (`space=<child>`; `into` defaults to the Space that made it; `into=desktop` for the user's desktop). To drive a child you made, pass `space=<child>` to `space_bash` and `call_tool`. |
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
4. `cua host setup --share spaces --name "<name>"` hosts Spaces only (no
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

## Links to things you ran

Any link to something you ran (preview, dashboard, file): use `cua share`, never localhost; open it once to check it works before sending.

The `share` tool (or `cua share 3000`, `cua share space:dev:3000`, `cua share ./report.html`) returns a link that opens on the user's other signed-in devices, phone included. It checks the link loads and reports `check.ok`; do not send a link whose check failed. Links are private by default; use `access: "public"` with a `ttl` only when the user wants anyone to open it. `share_list` and `share_stop` manage them.

