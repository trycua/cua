# Cua Spaces from OpenClaw

Drive Cua Spaces from the OpenClaw Control UI: provision a throwaway macOS Space
on your own Mac, teleport a logged-in app into it, and let an agent work in there
instead of on your live desktop.

This targets the common OpenClaw setup: **the Gateway runs on the same Mac as
your logged-in apps** (the `openclaw.ai/install.sh` + `openclaw onboard
--install-daemon` path, Control UI on `127.0.0.1:18789`). Because the Gateway is
local, the MCP server stays **stdio** (no remote transport, no tunnel), and
`teleport_app` reads the right machine's apps by construction.

Spaces are additive. Nothing here changes how your existing OpenClaw setup works;
it adds one MCP server.

## Setup (one time)

```bash
~/cua/apps/cua-spaces/scripts/openclaw/install-openclaw-mcp.sh
openclaw gateway restart
```

That is it. The script merges a `mcp.servers["cua-spaces"]` entry that runs
`cua daemon mcp` (stdio) into `~/.openclaw/openclaw.json`, resolving an
absolute path to the `cua` CLI at install time (a launchd Gateway has a bare
`PATH`). It is idempotent, backs the config up to `openclaw.json.bak`, and
leaves every other key alone. `--print` shows the fragment without writing;
`--remove` unregisters.

`cua daemon mcp` shares the Spaces registry (`~/.cua/spaces.json`) with the Cua
Spaces app and the `cua` CLI, so a Space added in one shows up in the others.

Confirm the Gateway picked it up:

```bash
openclaw mcp list
```

### Prerequisites

- **The `cua` CLI** (`cargo build --manifest-path libs/cua/Cargo.toml -p cua-cli --release`,
  or on `PATH`; `CUA_BIN` overrides).
- **Cua Spaces app running** for the host-side windows. It owns the loopback
  control server that draws picture-in-picture, viewer and streamed windows
  (`~/.cua/spaces-control.json`).
- **Cua Cloud credentials** (`cua auth login`) for cloud Spaces. Local Spaces do
  not need them.
- **For local macOS Spaces, a golden VM** (see `scripts/golden/`).
- **Unity licensing**: a Space that runs the Unity Editor consumes a seat from
  your account's per-machine Personal seat pool. `delete_space` returns the
  seat; abandoning Spaces without deleting them will exhaust it.

## The demo

Paste into the Control UI:

> teleport my unity hub to a new lume cua-space, open the biome-tiles scene and
> blender, add a hoverboard and spawn it next to the player

The tool path that serves it: `create_space` (`on: "local"`) → `teleport_manifest` →
`teleport_app` → `space_bash` / `call_tool`, optionally `show_space_pip` to
watch it happen. `delete_space` when you are done.

## macOS permissions: why this does not bite us

Gateway-spawned MCP processes do **not** inherit the terminal's Accessibility or
Screen-Recording grants. That does not affect Cua Spaces:

1. **Guest GUI work happens in the Space.** Capture goes through
   cua-spacesd *inside* the Space, and input through the cua-driver it
   delegates to; their TCC grants are baked into the golden image.
2. **Host GUI work is delegated, not performed.** `show_space_pip`,
   `hide_space_pip`, `open_space_viewer` and `stream_space_window` POST over
   loopback to the Cua Spaces app, a normal GUI app with its own TCC identity.
3. **Teleport reads plain files, not protected ones.** App profile data is
   ordinary user-owned files; a process with no Full Disk Access produced
   complete `unity-hub` and `chrome` manifests.

**Consent for teleport.** `teleport_app` needs `acknowledge_sensitive: true`
whenever the selection holds a sensitive item, and without `include` only the
provider's default-checked set moves (never everything). Sensitive items may
also raise a biometric prompt on the host, which a launchd-spawned process may
have no GUI session to show; the Cua Spaces app's allow-list (Settings) covers
apps you want to teleport unattended.

## Limitations

- **MCP config changes need a Gateway restart** (unless the build supports hot
  reload). Everything per-Space is handled inside the tools: spacesd tokens
  live in `~/.cua/spaces-credentials.json` (mode 0600) and are never registered
  in the Gateway config.
- **A Space can never live inside an OpenClaw sandbox.** Apple Virtualization
  does not nest; Spaces are siblings of the sandbox on the host.
- **Two concurrent macOS VMs.** macOS caps concurrent Virtualization guests at
  2, and the golden counts while running.

## Not verified

OpenClaw was not installed on the machine this was written on. The config
shape (`mcp.servers.<name>` with `command` / `args` / `transport` / `enabled` /
timeouts / `env`) follows current OpenClaw docs; if registration does not take,
MCP-for-Unity's alternate shape
(`plugins.entries["openclaw-mcp-bridge"].config.servers`) is the next thing to
try. The end-to-end demo prompt has not been run through an actual Gateway.
