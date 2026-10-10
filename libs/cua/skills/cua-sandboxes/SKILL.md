---
name: cua-sandboxes
description: Create, use and clean up cua sandboxes (disposable Linux or macOS computers) locally or in the Cua cloud with the `cua` CLI or the cua SDK, and browse the web inside one. Use when a task needs an isolated machine to run code, test an app, drive a desktop GUI, browse or test a website, fill a web form, take a web page screenshot, or reproduce something without touching the user's own computer.
---

# cua sandboxes

A sandbox is a disposable computer: a container or VM on this machine or in
the Cua cloud. The `cua` CLI and the cua SDK (Python, TypeScript, Swift,
Kotlin, Rust) share one runtime and one list of sandboxes.

## Before you start

```bash
cua --version            # the Rust cua CLI
cua auth status          # the cloud needs `cua auth login` (or CUA_CLIENT_ID/CUA_CLIENT_SECRET)
cua config get default.on   # where sandboxes run without --on (local unless changed)
cua runtime doctor       # local runtimes (containers, gVisor, QEMU, Lume); read-only
```

Local sandboxes need no account. Cloud sandboxes are metered: always delete
them when done.

## Create

```bash
cua sb create linux --name dev                 # local (default): a gVisor container
cua sb create linux --kind vm --name dev       # local Linux VM (QEMU)
cua sb create macos --name mac                 # local macOS VM (Apple silicon, Lume)
cua sb create linux --on cloud --name dev      # the Cua cloud
cua sb create ghcr.io/org/image:tag --name x   # any OCI image, local or --on cloud
cua sb create --on direct:127.0.0.1:3211 --token TOKEN --name mine   # an existing machine running cua-spacesd
```

`--on` is where (`local`, `cloud`, `direct:<addr>`), `--kind` what
(`auto`, `container`, `vm`), `--runtime` which engine (`auto`; local `gvisor`,
`runc`, `qemu`, `lume`; cloud `gvisor`, `kubevirt`). An impossible combination
fails with `invalid placement` and lists the valid values. Local
runtimes are set up on first use (`cua runtime setup` does it explicitly).
Omit `--name` for a generated name. `cua sb ls` prints each sandbox's ref
(`local:dev`, `cloud:dev`, `direct:host:port`); every NAME argument takes a
ref or a bare name that is unique across locations (`--local` / `--cloud`
narrow it). Add `--json` to any command for
machine-readable output. `exec`, `shell`, `cp`, screenshots and GUI control
go through cua-spacesd, so use an image that ships it (the `linux` alias
does). Plain images still get lifecycle, `logs` and `port-forward`.

## Use

```bash
cua sb ls                          # every location (--local or --cloud filters)
cua sb exec dev -- uname -a        # run a command
cua sb shell dev                   # interactive shell
cua sb screenshot dev              # save a screenshot
cua sb vnc dev                     # open the desktop in a browser
```

For GUI work, select the sandbox once and use `cua do` (see the
gui-automation skill):

```bash
cua do switch dev
cua do screenshot
cua do click 400 300
```

## Clean up

```bash
cua sb rm dev --force              # delete
cua fleet pools gc                 # remove idle auto-managed cloud capacity
cua sb suspend dev                 # or pause it and `cua sb resume dev` later
```

## From code (Python)

```python
import asyncio, cua

async def main():
    c = cua.embedded()
    sb = await c.sandboxes().create(cua.SandboxCreateOptions(
        on="local",
        image="ghcr.io/trycua/linux:24.04", name="dev"))
    env = await sb.spacesd(None)
    out = await env.run(cua.SpacesdCommand(program="uname", args=["-a"]))
    print(out.stdout.decode())
    await sb.delete()

asyncio.run(main())
```

The TypeScript, Swift and Kotlin bindings expose the same objects
(`Cua`, `sandboxes()`, `Sandbox.spacesd()`).

## From the cua MCP server

When the `cua` MCP server is connected, prefer its tools: `images_list`
(which images exist), `sandbox_list`, `sandbox_create`, `sandbox_get`,
`sandbox_delete`, then the `computer_*` tools (screenshot, click, type,
shell, file) against the sandbox.

## Browse the web

Browse inside a sandbox, never in the user's own browser. Everything below
is `cua` MCP tool calls; no other browser tool or setup is needed.

1. Pick an image: `images_list {"browser": true}` (the canonical
   `ghcr.io/trycua/linux:24.04` ships Chromium).
2. Create it with a browser: `sandbox_create {"browser": true, "url":
   "https://example.com"}` (add `"on": "cloud"` for the cloud). It returns
   `id`, `session` and `browser.target_id` / `browser.tab_id`.
3. Drive the browser with `call_tool {"space": <id>, "tool": <tool>,
   "arguments": {"session", "target_id", "tab_id", ...}}`:
   - read: `get_browser_state {"snapshot_format": "semantic_v2"}` returns
     an outline and refs (`p1:0`); add `"include_screenshot": true` for an
     image;
   - `browser_navigate {"url"}`, `browser_click {"ref", "delivery_mode":
     "foreground"}`, `browser_type {"ref", "text", "replace": true}` (text
     `"\n"` with `"mode": "keystrokes"` presses Enter), `browser_pointer`
     (hover, scroll, drag);
   - read again after every page change: refs are per snapshot.
   To test a local app, run it in the sandbox (`space_write`, `space_bash`)
   and open `http://127.0.0.1:<port>`.
4. Logged-in sites: only when the user asks. `teleport_browser_session`
   with the exact sites first returns what would move and a consent
   requirement; show it to the user and continue only after they approve.
   Never move a session without that approval.
5. Clean up: `sandbox_delete {"name": <id>}`.

From a terminal: `cua images ls --browser`, `cua sb create --browser
--open https://example.com`, `cua sb view <id>` to watch.

## Rules

- Never run destructive commands on the user's own machine when a sandbox
  would do.
- Delete cloud sandboxes you created as soon as the task ends.
- Report the sandbox name you used so the user can inspect or remove it.
