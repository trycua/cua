---
name: verify-cua-spaces
description: Verify Cua Spaces on Linux the way a user drives a Space - register a running cua-spacesd with the source-built `cua`, open it in the HTML5 viewer, act on it with `cua do`, `cua spacesd` and `cua mcp` - with retained evidence. Use after changes to libs/cua-spacesd, the Spaces CLI/SDK (libs/cua) or the HTML5 viewer.
---

# Verify Cua Spaces

A **Space** is any machine running **cua-spacesd** (port 3211 in an image; a random loopback port here). Users reach it through the `cua` CLI (`cua spaces`, `cua sb view`, `cua do`, `cua spacesd`, `cua mcp`), the HTML5 viewer that cua-spacesd serves at `/viewer/`, and the desktop apps (Tauri, Electron, SwiftUI) that sit on the same SDK. This skill drives the CLI and the viewer against a throwaway Linux desktop. The macOS app has its own lane, [Cua Spaces Local on a real Mac](macos-app.md). Read [the feature map](features/README.md) before picking a journey.

Everything here is source-built: `libs/cua-spacesd` (the daemon and the `cua-spacesd-x11-pad` input-echo fixture) and `libs/cua` (`cua-spaces-cli`, the Spaces build of `cua`; the apps ship it as `cua`). Never drive a cua-spacesd, cua daemon or `~/.cua` you did not start: every run gets its own display, session bus, audio server, port, token and cua home.

## Launch

Run from the repository root. Linux only. Needs Rust (pinned by the workspaces), `protoc`, Node 20+, Chromium, `Xvfb`, `openbox`, `dbus-daemon`, `curl`, `ss`, `python3`, and for audio `pipewire`, `wireplumber`, `pipewire-pulse` (Debian/Ubuntu: the build packages in `.github/workflows/ci-cua-spacesd.yml` plus `xvfb openbox dbus chromium pipewire wireplumber pipewire-pulse pulseaudio-utils`).

```bash
H=.cursor/skills/verify-cua-spaces/scripts/spaces-desk.sh
$H build                      # once: release cua-spacesd, cua-spacesd-x11-pad, cua-spaces-cli (~20 min cold, ~9 GB of target/)
$H up                         # or: $H up --no-audio
```

`up` starts, in this order and each in its own session: Xvfb on a free display (1280x800), a private D-Bus session bus, PipeWire + WirePlumber + pipewire-pulse (skipped with `--no-audio` or when not installed), openbox, the x11-pad fixture filling the screen with a red/green/blue/white grid, and cua-spacesd on a free loopback port with a fresh token and data dir (`--quic-port 0`). It then registers the Space with `cua spaces add 127.0.0.1:<port> --name verify` in an isolated `CUA_HOME`.

Ready means usable, not just listening: `up` waits for `GET /health` = 204 **and** `SystemService/Health` = `HEALTH_STATUS_SERVING` with every component (`desktop` is not serving until a window manager runs), then for the Space registration and the pad's `ready` line. It prints:

```
RUN=/tmp/cua-spaces-verify.XXXXXX
EVIDENCE=<repo>/artifacts/cua-spaces/verify/run-XXXXXXXX
READY space=direct:127.0.0.1:<port> display=:<n> audio=pipewire|none
```

Keep `RUN`; every other command takes it. Teardown is `$H down "$RUN"` (see Cleanup).

## Doctor

```bash
$H doctor "$RUN"
```

Read-only. Checks that every process `up` recorded is alive, that the port is owned by our cua-spacesd pid, prints the daemon version and `cua spacesd caps`, confirms the Space is registered in our cua home, then runs `cua-spacesd doctor --target <url>` and keeps `doctor.txt`/`doctor.json`/`doctor/screenshot.display.png` in the evidence dir. Exit 0 means worth driving. Expected warnings on this stand-in: `auth.token_file` (no `/run/cua/env-token` outside an image), `stream.quic` (QUIC is off), and `driver.health_report` AT-SPI unreachable. Any `[fail]`, a dead pid or a foreign port owner means stop and `down`.

## Drive

Three harnesses, all against the same Space:

- **CLI:** `$H cua "$RUN" -- <args>` runs the source-built `cua <args>` with the run's isolated `HOME`/`CUA_HOME`/`DISPLAY`, and writes the command, stdout, stderr and exit code to `cli-NNN.txt` in the evidence dir. The Space is `direct:127.0.0.1:<port>` (from `$RUN/state.env`, `SPACE_ID`) and is named `verify`.
- **Viewer:** `$H viewer "$RUN" <name> [extra-fragment]` gets a link from `cua sb view <SPACE_ID> --no-open` (a real viewer ticket), opens it in headless Chromium through `scripts/drive-viewer.mjs`, waits for `#app[data-status=streaming]` and a decoded frame, clicks the white (bottom-right) grid cell and types `ok`, then requires the pad journal to show that click and those keys. Toolbar buttons have stable `aria-label`s (`Send keys`, `Upload files`, `Share a folder`, `Sound on`, `Microphone off`, `Stats`, ...); `window.__cuaViewer.stats()` reports frames and audio packets.
- **MCP:** pipe JSON-RPC lines to `cua mcp --embedded` with the run's environment (recipe in [features/mcp.md](features/mcp.md)).

The x11-pad journal (`pad.jsonl`) is the independent side-effect oracle for input: one JSON line per `button`/`key`/`focus_in` the X server delivered to the fixture window.

## Evidence

Everything lands in the printed `EVIDENCE` dir under `artifacts/` (gitignored): `state.env`, `pids.txt`, each process log (`cua-spacesd.log`, `xvfb.log`, ...), `spaces-add.log`, `doctor.*`, `cli-NNN.txt` transcripts, `viewer-<name>.json` (steps, stats, redacted link), `viewer-<name>.png`, `viewer-<name>.aria.txt`, and `pad.jsonl`.

Proof standards:

- Drive the real user path: `cua` commands, the viewer page a `cua sb view` link opens, MCP over stdio. Not `cargo test`, not direct gRPC calls when a CLI verb exists, not `window.__cuaViewer.input()` (that hook exists for tests and skips the canvas).
- Capture the action and the resulting state: the CLI transcript plus a second read (`cua spacesd cp` back, `cua do screenshot`, `pad.jsonl`), the viewer screenshot plus the pad journal.
- Side effects count more than messages: a file read back with the same sha256, a pad event at the expected coordinates, a registry entry in `cua spaces ls --json`.
- Viewer tickets are credentials: `drive-viewer.mjs` redacts them; do not paste links into reports.

## Cleanup

```bash
$H down "$RUN"
```

Kills only the pids `up` recorded (newest first, TERM then KILL) and deletes `RUN` (cua home, token, data dir, Chromium's playwright-core). It never kills by name and never touches another display. The evidence dir survives; `down` prints its path. Run `down` after every failed attempt too. To reclaim disk afterwards, delete `libs/cua-spacesd/target` and `libs/cua/target` yourself.

## Helpers

- `scripts/spaces-desk.sh` (`build | up [--no-audio] | doctor RUN | cua RUN -- ARGS | viewer RUN NAME [FRAGMENT] | index RUN | down RUN`; no arguments prints usage).
- `scripts/drive-viewer.mjs LINK OUT PAD_LOG` (called by `viewer`; needs `PW_DIR` with `playwright-core` and `CHROMIUM`; `viewer` installs `playwright-core@1.63.0` into `RUN` the first time).

## Unverified here

- The Tauri (`apps/cua-spaces`) and Electron (`apps/cua-spaces-desktop`) apps. The SwiftUI app, its notch and its first run go through [the macOS app lane](macos-app.md). The web UI's demo mode (`apps/cua-spaces-web`, `pnpm dev`) is fixtures only.
- macOS and Windows cua-spacesd (ScreenCaptureKit/TCC grants, WASAPI, scheduled task). Linux X11 only; Wayland/Hyprland capture is not exercised.
- Creating Spaces (`cua spaces create`, `create_space`): needs a container runtime or VM image pulls; this skill attaches to a cua-spacesd it started instead.
- Sign-in, the relay (`cua-spacesd join`, `relay:<id>` Spaces), sharing and account features: need a Cua account and the hosted relay.
- QUIC media, hardware encoders, GPU: QUIC is disabled here; no GPU.

Use `/maintain-verification-skill` to keep the map honest as the CLI verbs, viewer handles or launch steps change.
