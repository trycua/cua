---
name: cua-sandboxes
description: Create, use and clean up cua sandboxes (disposable Linux or macOS computers), locally or in the Cua cloud, with the `cua` CLI, the SDK or the cua MCP server, and browse the web inside one. Use when a task needs an isolated machine to run code, test an app, drive a GUI, browse or test a website, or reproduce something away from the user's own computer.
---

# cua sandboxes

A sandbox is a disposable container or VM, local or in the Cua cloud. Local needs no account; cloud is metered, so delete what you create. Every sandbox is also a Space (see the cua-spaces skill).

## With the cua MCP server

Prefer these tools when connected: `images` (which images exist), `create_space`, `list_spaces`, `space_bash`, `space_files`, `computer`, `window`, `space` `delete`.

## With the CLI

```bash
cua sb create linux --name dev                # local gVisor container
cua sb create linux --kind vm --name dev      # local QEMU VM
cua sb create macos --name mac                # local macOS VM (Apple silicon)
cua sb create linux --on cloud --name dev     # Cua cloud
cua sb create ghcr.io/org/image:tag --name x  # any OCI image
cua sb ls ; cua sb exec dev -- uname -a ; cua sb screenshot dev ; cua sb view dev
cua sb rm dev --force                         # also: suspend / resume
```

`--on` local, cloud or `direct:<addr>`; `--kind` auto, container or vm; `--runtime` auto, gvisor, runc, qemu, lume, kubevirt. An impossible combination fails and lists the valid ones. `exec`, `shell`, `cp`, screenshots and GUI control need an image with cua-spacesd (the `linux` alias has it). Names take a ref (`local:dev`, `cloud:dev`) or a bare name unique across locations; `--json` for machine output. Setup checks: `cua auth status`, `cua runtime doctor`. For GUI work see the gui-automation skill (`cua do`).

## From code

```python
import asyncio, cua

async def main():
    c = cua.embedded()
    sb = await c.sandboxes().create(cua.SandboxCreateOptions(
        on="local", image="ghcr.io/trycua/linux:24.04", name="dev"))
    env = await sb.spacesd(None)
    print((await env.run(cua.SpacesdCommand(program="uname", args=["-a"]))).stdout.decode())
    await sb.delete()

asyncio.run(main())
```

TypeScript, Swift and Kotlin expose the same objects.

## Browse the web

Browse in a sandbox, never in the user's own browser.

1. `open_browser {"url": "https://example.com"}` creates a Linux Space with Chromium and returns `space`, `session`, `target_id`, `tab_id` (pass `space` to use an existing one).
2. Drive it with `call_tool {"space", "tool", "arguments": {"session", "target_id", "tab_id", ...}}`: `get_browser_state` (outline and refs like `p1:0`), `browser_navigate {url}`, `browser_click {ref}`, `browser_type {ref, text}`, `browser_pointer`. Refs are per snapshot: read again after every page change.
3. To test a local app, run it in the Space (`space_files` `write`, `space_bash`) and open `http://127.0.0.1:<port>`.
4. Signed-in sites only when the user asks: `teleport` with `action: "browser"` and the exact `sites`. The user approves in Cua; then call again with the `request_id`.
5. Delete the Space when done.

CLI equivalents: `cua images ls --browser`, `cua sb create --browser --open <url>`, `cua sb view <id>`.

## Rules

- Prefer a sandbox to running risky commands on the user's machine.
- Delete cloud sandboxes you created as soon as the task ends, and say which sandbox you used.
