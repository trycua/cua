# agentenv-cua

An [AgentEnv](https://github.com/scaleapi/agentenv-framework) plugin from Cua that runs computer-use tasks on macOS VMs.

> **Status: draft, in progress.** It's being built so that Cua Driver Bench runs on AgentEnv on macOS, the same way it already runs on Linux ([earakely-scale/agentenv-cua-driver-bench](https://github.com/earakely-scale/agentenv-cua-driver-bench)). Two pieces depend on AgentEnv core, which today composes Linux computers only. See [What's missing](#whats-missing).

## What's here

| Path | What it is |
|---|---|
| `src/agentenv_cua/lume/` | The `lume` sandbox provider. Each sandbox is a macOS VM cloned from a pinned image and booted headless with [lume](https://github.com/trycua/cua/tree/main/libs/lume). |
| `src/agentenv_cua/drivers/cua_driver_macos/` | [cua-driver](https://github.com/trycua/cua/tree/main/libs/cua-driver) 0.38.0 as a computer driver on macOS. It is the macOS counterpart of AgentEnv's Linux `cua-driver` driver. |
| `image/` | Builds the macOS bench image: `build.sh` on the Mac, `provision.sh` in the VM. |
| `smoke/conformance_smoke.py` | Boots one VM through the provider and runs AgentEnv's computer conformance probe against the driver. |
| `tests/` | Offline tests: a fake lume, and no daemon. |

### The `lume` provider

- **Boot.** A sandbox is a fresh clone of the base VM, booted headless.
- **Settings.** A computer's boot settings (driver settings, app settings and its control token) are written to a file only this user can read. The provider shares that file into the guest read-only, as `/Volumes/My Shared Files/agentenv/boot.json`. This is how a Linux disk form reads its NoCloud seed.
- **Ports.** The driver's port is reached at the VM's address on lume's NAT network.
- **Reset.** A reset deletes the clone and clones the base again under the same name and MAC address, so the sandbox's URL stays the same.
- **Capacity.** Apple's macOS license allows two macOS VMs per Mac. The provider won't start a VM while two already run on the host, whoever started them.
- **Agents.** It doesn't run agents' containers. Keep agents on a container provider such as `local`.

```toml
# .agentenv/config.toml
[sandbox.providers.lume.config]
base = "agentenv-macos-bench"   # a local lume VM, or image = "ghcr.io/<org>/<repo>:<tag>"
```

### The cua-driver macOS driver

- **Daemon.** cua-driver's daemon runs in the VM's GUI session from `CuaDriver.app` 0.38.0. That app's bundle (`com.trycua.driver`) holds the Accessibility and Screen Recording grants.
- **Tools.** Agents get what cua-driver's own MCP server serves at the pinned version: its instructions, its tools beside the `computer_*` tools, and its skill pack.
- **The `computer/v1` operations** are cua-driver's desktop-scope calls. They run in a cua-driver session of the framework's own.
- **Ready checks** cover three things:
  - the pinned executable's sha256;
  - the pinned tool list (`mcp.json`, pinned per OS: macOS lists different tools from Linux);
  - the TCC grants, checked after every reset, so a lost grant shows up as a harness error rather than an agent failure. A capture that works is the framework's own `capture` ready check.
- **First screenshot.** macOS 15 and later ask once, per app, whether it may capture the screen directly, even with Screen Recording granted. cua-driver skips captures (`direct_capture_consent_required`) until that answer is recorded, so the image records it ahead of time: an entry for `com.trycua.driver` in the user's `ScreenCaptureApprovals.plist`.
- **Display.** `display` in the provider config (default `1280x800`, the size the conformance fixtures need) is set on each clone with `lume set --display` before it boots. A guest otherwise boots in the mode its disk last had, so the driver's start script runs `set-display`, which picks the display's largest 1x mode and gives it the sRGB profile (screenshots then hold the pixels as drawn).
- **Pointer.** The computer has one pointer: the driver serialises pointer actions and delivers them in the foreground, since a headless computer has nobody whose pointer it could disturb.
- **Image extras.** `provision.sh` also puts a GNU-style `stat -c` and a `python3` with a current Tk first on the PATH of commands the driver runs (the conformance fixture is a Tk program written for Linux), and turns off the base image's login-item banner and Cua Spacesd window, which cover the top right of the screen and the whole screen.

Known gaps against the conformance probe on macOS: the fixture is X11-specific in four checks. `P6 typed text arrives as key events` expects Tk's X11 keysym name `comma` (Tk on macOS reports `,`), `P6 a chord selects all` and `P6 a repeated key` use Ctrl+/ (Tk on macOS selects all with Cmd+A), and `P7 scrolls ... each arrive` counts X11 wheel buttons 4 and 5 (Tk on macOS reports MouseWheel events). They need a platform-aware fixture in the probe, not changes here.

`mcp.json` is generated on a macOS VM with `pin_manifest.py`. `image/build.sh` does this when the folder has none.

## Build the image and smoke-test it

On an Apple-silicon Mac with lume, and with at most one macOS VM already running:

```bash
image/build.sh --base-image ghcr.io/trycua/macos:26-<pin> --name agentenv-macos-bench
python smoke/conformance_smoke.py --base agentenv-macos-bench --level full
```

## What's missing

1. **macOS computers in AgentEnv core.** A computer artifact is `os.type = "linux"` with a `container` or `disk` form today. A macOS computer needs `os.type = "macos"` and a prebuilt VM-image form that isn't composed, which the `lume` provider boots through `create_vm`. Until then, `env.toml`'s `install.macos` is documentation, and `image/provision.sh` does the install.
2. **The other bench arms' drivers:** agent-desktop, the browser-use macOS harness and Codex computer use.
3. **The bench's macOS apps as computer apps:** BenchLab, the sentinel, and the shared tasks' apps.

## Tests

```bash
pip install -e '.[test]'
pytest
```
