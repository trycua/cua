# agentenv-cua

An [AgentEnv](https://github.com/scaleapi/agentenv-framework) plugin from Cua that runs computer-use tasks on macOS VMs.

> **Status: draft, in progress.** It's being built so that Cua Driver Bench runs on AgentEnv on macOS, the same way it already runs on Linux ([earakely-scale/agentenv-cua-driver-bench](https://github.com/earakely-scale/agentenv-cua-driver-bench)). Two pieces depend on AgentEnv core, which today composes Linux computers only. See [What's missing](#whats-missing).

## What's here

| Path | What it is |
|---|---|
| `src/agentenv_cua/lume/` | The `lume` sandbox provider. Each sandbox is a macOS VM cloned from a pinned image and booted headless with [lume](https://github.com/trycua/cua/tree/main/libs/lume). |
| `src/agentenv_cua/drivers/cua_driver_macos/` | [cua-driver](https://github.com/trycua/cua/tree/main/libs/cua-driver) 0.38.0 as a computer driver on macOS. It is the macOS counterpart of AgentEnv's Linux `cua-driver` driver. |
| `src/agentenv_cua/macos-drivers/` | The other three bench arms' drivers, one folder each: [agent-desktop](#the-other-arms-drivers), the [browser-use macOS harness](#the-other-arms-drivers) and [Codex Computer Use](#codex-computer-use-needs-a-chatgpt-login). A bundle (`agent_env.bundles`), so `agent-env` finds them as `@local/agentenv-cua/macos-drivers/<name>`. |
| `src/agentenv_cua/macos-apps/` | The bench's macOS apps as computer apps: [BenchLab, the sentinel and the shared tasks cdb-s02 to s04](#the-bench-apps-on-macos), and the sentinel's arming step as an artifact. Also a bundle. |
| `image/` | Builds the macOS bench image: `build.sh` on the Mac, `provision.sh` in the VM; then `build-arms.sh` and `provision-arms.sh` add the other arms and the apps. |
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
  - the TCC grants. These include a capture that actually works, and they're checked after every reset, so a lost grant shows up as a harness error rather than an agent failure.

`mcp.json` is generated on a macOS VM with `pin_manifest.py`. `image/build.sh` does this when the folder has none.

### The other arms' drivers

Each driver has the same shape as the cua-driver one: a pinned version, ready checks that fail with a reason, a pinned tool
manifest, and the neutral `computer_*` tools. Every arm runs the same agent (Claude Code with Sonnet 5.5); only the tools differ.

| Driver | Wraps | Pin | Tools the agent sees | Instructions |
|---|---|---|---|---|
| `agent-desktop` | [vercel-labs/agent-desktop](https://github.com/vercel-labs/agent-desktop), Apache-2.0, a CLI with no MCP server | 1.0.2, `aarch64-apple-darwin` (archive and executable sha256 in `driver.py`) | 32 tools, one for each snapshot, action, app and wait command (`tools.json`) | its own skill, as the binary embeds it |
| `macos-harness` | [browser-use/macos-harness](https://github.com/browser-use/macos-harness), MIT, eleven shell commands | 0.2.1 from PyPI (wheel sha256 in `driver.py`; the skill's sha256 too) | its eleven tools (`tools.json`, written by `pin_manifest.py` from the harness's own table); a read's screenshot comes back as an image | its own skill |
| `codex-cu` | OpenAI's `unified-computer-use` plugin, the `cua_repl` MCP server | ChatGPT app 26.930.51102 (zip and executable sha256 in `driver.py`), `mcp.json` | `js` and `js_reset`, as the plugin enables them, with the computer surface only | none, as OpenAI ships it |

- **The neutral `computer_*` helper.** AgentEnv wants the seven `computer_*` operations from every driver, for recordings, the
  conformance probe and readiness. `macos_computer.py` serves them from `/usr/sbin/screencapture` and Quartz events, which no
  arm owns. An env hides them from its agent with its `[tools]` list, so each arm is scored on its own tools only. The file is
  the same in each driver's folder, since a driver installs from its own folder; a test checks that.
- **Ready checks** cover the pinned executable or version, the pinned tools (a wrapped command missing from the binary's own help,
  or a tool list that drifted from the pin, is named), and the TCC grants, which are checked after every reset.
- **To move one to another version,** bump its pin in `driver.py` and `requirements.txt`, regenerate its manifest
  (`python driver.py --write-manifest` for agent-desktop, `python pin_manifest.py` for the harness, and for Codex the
  `mcp.json` from the plugin's server on a VM with the app), and read the diff.

#### Codex Computer Use needs a ChatGPT login

Sky runs the Codex CLI of the pinned ChatGPT app, which has to be logged in with a **ChatGPT account** inside the VM. That is a
precondition no driver can meet, and no credential or app ships in this repository or in a public image:

1. Build the image with `--arms codex-cu` (`image/build-arms.sh`). It installs the pinned app and nothing else.
2. In the VM, as its session user, run `/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex login`,
   then open the ChatGPT app once so it installs its plugin.
3. Check `codex login status`. The driver checks the same, every time, and fails the ready check "ChatGPT login" with these
   instructions until it holds. A computer that isn't ready is a `harness_error`, never a score.

An image with the login is private. Anyone reproducing the arm needs an account of their own.

### The bench apps on macOS

The bench's own macOS sources, from the public Bench v2 in this repository (`libs/cua-driver-bench`), as computer apps. Each
app's `files.json` lists the files it takes at one pinned commit, by URL and sha256.

| App | What it is | Settings and data |
|---|---|---|
| `benchlab` | BenchLab, the AppKit test bench of MB-09 to MB-11 and IR-01 to IR-04. The install builds `BenchLab.app` from its three Swift sources, ad hoc signed, with the bench's own flags (`build_app.sh`). | `mode` and `seed`; data `~/.benchlab` (`state.json`, `events.jsonl`) |
| `bench-sentinel` | BenchSentinel, the window that plays the user and logs what disturbs them. | `condition`: `foreground` (the witness) or `background` (the "User activity" window at the bench's frame); data `~/.bench-sentinel/sentinel.jsonl` |
| `cdb-s02`, `cdb-s03`, `cdb-s04` | The shared tasks' windows (the lighthouse-desk, Ledger Lantern and Rillwatch/Margin Lantern Electron apps, LibreOffice Calc, GnuCash, and Chrome for Testing on the Node archive service), opened as each task's `platform/launch.macos.json` says and placed at the bench's bounds. | data: the task's workspace, `~/cdb-s0N`, built by the task's `reset/setup.py` and checked by its `reset/verify.py` |

- **Arming the sentinel.** `artifacts/sentinel-macos/run.py` is the `run_code` step that arms or disarms it around the agent
  (`SIGUSR1` activates it, `SIGUSR2` arms or disarms it, and the log says when it has).
- **Window placement** uses Cua Driver's `set_window_frame` in the image's `CuaDriver.app`, before the agent starts, whichever
  arm runs, as the bench's own runner placed windows. BenchLab places itself.
- **LibreOffice and GnuCash** are `/Applications` installs of the image: the app's `apps_installed` ready check names any that is
  missing.
- **The grader** and the task files (prompts and seeds) live in the bundle that runs them, not here.

## Build the image and smoke-test it

On an Apple-silicon Mac with lume, and with at most one macOS VM already running:

```bash
image/build.sh --base-image ghcr.io/trycua/macos:26-<pin> --name agentenv-macos-bench
python smoke/conformance_smoke.py --base agentenv-macos-bench --level full
```

To add the other arms and the bench's apps to that image (leave `codex-cu` out of a public one):

```bash
image/build-arms.sh --base-vm agentenv-macos-bench --name agentenv-macos-bench-arms --arms agent-desktop,macos-harness
```

`provision-arms.sh` also writes one `start-driver.sh`, which starts the driver whose name the boot settings carry (`ENVIRONMENT_NAME`)
and the apps those settings name.

## What's missing

1. **macOS computers in AgentEnv core.** A computer artifact is `os.type = "linux"` with a `container` or `disk` form today. A macOS computer needs `os.type = "macos"` and a prebuilt VM-image form that isn't composed, which the `lume` provider boots through `create_vm`. Until then, `env.toml`'s `install.macos` is documentation, and `image/provision.sh` does the install.
2. **`install.macos` for the other drivers and the apps.** Core composes `linux` installs only and refuses a `[install.macos]` table
   in an env, so the new envs carry none; `image/provision-arms.sh` and `image/install_app.py` do what it would do. The app envs
   also name `computer = "macos-26"` and `driver = "cua-driver-macos"`, placeholders for the macOS computer and driver env ids
   that core, or a plugin stopgap, will give.
3. **Verification on a macOS VM.** Everything under `macos-drivers/` and `macos-apps/` is covered offline (fakes for the CLIs, the MCP
   server, Cua Driver and the log). What only a VM can show: that each wrapped CLI answers under the image's TCC rows, the Codex
   login and plugin, the apps building and opening at their bounds, Chrome for Testing reading its managed policies, and the
   sentinel arming. The ready checks above are where each failure shows.

## Tests

```bash
pip install -e '.[test]'
pytest
```
