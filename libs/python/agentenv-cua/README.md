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
  - the TCC grants, checked after every reset, so a lost grant shows up as a harness error rather than an agent failure. A capture that works is the framework's own `capture` ready check.
- **First screenshot.** macOS 15 and later ask once, per app, whether it may capture the screen directly, even with Screen Recording granted. cua-driver skips captures (`direct_capture_consent_required`) until that answer is recorded, so the image records it ahead of time: an entry for `com.trycua.driver` in the user's `ScreenCaptureApprovals.plist`.
- **Display.** `display` in the provider config (default `1280x800`, the size the conformance fixtures need) is set on each clone with `lume set --display` before it boots. A guest otherwise boots in the mode its disk last had, so the driver's start script runs `set-display`, which picks the display's largest 1x mode and gives it the sRGB profile (screenshots then hold the pixels as drawn).
- **Pointer.** The computer has one pointer: the driver serialises pointer actions and delivers them in the foreground, since a headless computer has nobody whose pointer it could disturb.
- **Image extras.** `provision.sh` also puts a GNU-style `stat -c` and a `python3` with a current Tk first on the PATH of commands the driver runs (the conformance fixture is a Tk program written for Linux), and turns off the base image's login-item banner and Cua Spacesd window, which cover the top right of the screen and the whole screen.

Known gaps against the conformance probe on macOS: the fixture is X11-specific in four checks. `P6 typed text arrives as key events` expects Tk's X11 keysym name `comma` (Tk on macOS reports `,`), `P6 a chord selects all` and `P6 a repeated key` use Ctrl+/ (Tk on macOS selects all with Cmd+A), and `P7 scrolls ... each arrive` counts X11 wheel buttons 4 and 5 (Tk on macOS reports MouseWheel events). They need a platform-aware fixture in the probe, not changes here.

`mcp.json` is generated on a macOS VM with `pin_manifest.py`. `image/build.sh` does this when the folder has none.

### The other arms' drivers

Each driver has the same shape as the cua-driver one: a pinned version, ready checks that fail with a reason, a pinned tool
manifest, and the neutral `computer_*` tools. Every arm runs the same agent (Claude Code with Sonnet 5.5); only the tools differ.

| Driver | Wraps | Pin | Tools the agent sees | Instructions |
|---|---|---|---|---|
| `agent-desktop` | [vercel-labs/agent-desktop](https://github.com/vercel-labs/agent-desktop), Apache-2.0, a CLI with no MCP server | 1.0.2, `aarch64-apple-darwin` (archive and executable sha256 in `driver.py`) | 67 tools, one for each leaf command of the CLI, derived from its command tree and named `desktop_<command>` (`tools.json` pins them) | its own skill, as the binary embeds it |
| `macos-harness` | [browser-use/macos-harness](https://github.com/browser-use/macos-harness), MIT, eleven shell commands | 0.2.1 from PyPI (wheel sha256 in `driver.py`; the skill's sha256 too) | its eleven tools (`tools.json`, written by `pin_manifest.py` from the harness's own table); a read's screenshot comes back as an image | its own skill |
| `codex-cu` | OpenAI's `unified-computer-use` plugin, the `cua_repl` MCP server | ChatGPT app 26.930.51102 (zip and executable sha256 in `driver.py`), `mcp.json` | `js` and `js_reset`, as the plugin enables them, with the computer surface only | none, as OpenAI ships it |

- **agent-desktop's tools are derived, not listed.** agent-desktop has no schema export, so the driver walks the CLI's `--help`
  tree (`desktop_catalog.py`) and makes one tool of every leaf command: `desktop_snapshot`, `desktop_click`,
  `desktop_double_click`, `desktop_skills_get`, `desktop_cursor_overlay_enable` and so on (subcommands are joined with `_`). Each
  argument and flag is an input property, with its type, whether it's required, its enum, its default and its help text. A call
  runs the binary as a child process with the mapped argv (flags as `--flag=value`, then `--` and the positionals) and returns its
  JSON envelope as printed; a failure adds the exit code and stderr, and a call that outlives its limit (its own timeout argument
  plus 30 s, else 60 s) is killed and answers with an error envelope. The derived catalog is cached under the binary's sha256, and
  the ready check fails when it differs from `tools.json`, so a row can't run on tools that changed under it.
  The options every command shares are the driver's: logging, trace files and session selection aren't tools' arguments, and
  `headed` and the wait flags appear only on commands that act on a ref or name them. Flags for every call (`--headed` to
  let the commands that need physical input run) go in `AGENT_DESKTOP_GLOBAL_FLAGS`. To pin another release, set
  `AGENT_DESKTOP_BIN` to its binary and run `python driver.py --write-manifest`, then update the pins it prints and read the
  diff of `tools.json`. The tests derive the tools from recorded `--help` output of 1.0.2 (`tests/fixtures`) and need no Mac.
  Needs a macOS VM to check: that the derived list matches the binary on the image (the ready check does it), the TCC grants, and real
  actions.
- **The neutral `computer_*` helper.** AgentEnv wants the seven `computer_*` operations from every driver, for recordings, the
  conformance probe and readiness. `macos_computer.py` serves them from `/usr/sbin/screencapture` and Quartz events, which no
  arm owns. An env hides them from its agent with its `[tools]` list, so each arm is scored on its own tools only. The file is
  the same in each driver's folder, since a driver installs from its own folder; a test checks that.
- **Ready checks** cover the pinned executable or version, the pinned tools (for agent-desktop, the tools derived from the binary's own help,
  and a derived list that differs from the pin is named), and the TCC grants, which are checked after every reset.
- **To move one to another version,** bump its pin in `driver.py` and `requirements.txt`, regenerate its manifest
  (`AGENT_DESKTOP_BIN=<new binary> python driver.py --write-manifest` for agent-desktop, `python pin_manifest.py` for the harness, and for Codex the
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
