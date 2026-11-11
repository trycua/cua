---
name: verify-cua-driver
description: Verify the Cua Driver daemon and CLI/MCP desktop tools against real fixture apps after driver changes - Linux X11/Wayland on a disposable desktop, macOS/AppKit in a disposable Lume guest - using AX-first actions, side-effect checks, and retained evidence.
---

# Verify Cua Driver

Read [the feature map](features/README.md) first. This verifies the daemon's desktop-tool surface, not the bench, and does not certify every mapped entry point by exercising one convenient path. Use only a disposable desktop: the foreground cells intentionally move focus and input.

## Pick the lane

| Change touches | Lane | Entry point |
| --- | --- | --- |
| Linux platform code, shared core/contract/SDK, this skill | Linux X11, plus Wayland for capture/input routes | `scripts/verify-linux.sh x11\|wayland` (below) |
| `platform-macos`, AppKit/SwiftUI/TextEdit harnesses, `scripts/ci/macos/**`, `tests/runners/macos-lume/**` | macOS Lume guest | `scripts/verify-macos.sh` ([macOS lane](macos-lume.md)) |
| Shared code with macOS behavior | both | both |

Run every lane the change touches that the host can run, and name the lanes you skip in the report. A change for one compositor (KWin, GNOME, niri) also needs a session of that compositor. X11 and Sway results don't cover it, so name it as skipped when you can't start one. Launch through Cleanup below cover Linux. The macOS lane keeps the same proof standards; its wrapper owns the worker VM, preflight and evidence collection.

## Launch

Run from the repository root. Use the CI toolchain (Rust 1.97.1, Node 22) and install the Linux dependencies from `.github/workflows/e2e-rust-linux.yml`. Electron needs a writable `/dev/shm` (mode 1777). A root-owned 755 one fails preflight before any cell runs:

```bash
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  xvfb dbus-x11 at-spi2-core openbox picom python3-gi gir1.2-gtk-3.0 \
  libgtk-3-dev clang pkg-config libdbus-1-dev libpipewire-0.3-dev \
  libspa-0.2-dev libei-dev libx11-dev libxi-dev libxtst-dev libxext-dev \
  libwebkit2gtk-4.1-dev libssl-dev libxdo-dev \
  libayatana-appindicator3-dev librsvg2-dev ffmpeg jq x11-utils

CUA_E2E_RETRY_ONCE=0 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11
```

The helper prints an absolute `Evidence:` directory under `artifacts/cua-driver/verify/run-XXXXXXXX`. Each invocation creates a new directory; never reuse an older canonical artifact directory. The canonical runner builds the release driver once and stages only Electron using `tests/fixtures/build/linux.sh --only electron`. Reuse that build with:

```bash
timeout 1260 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build
```

The X11 recipe follows CI: `xvfb-run -a --server-args='-screen 0 1920x1080x24' dbus-run-session`, openbox, then picom with the xrender backend. `at-spi2-core` is D-Bus activated, not bootstrapped by another accessibility launcher. Bounded probes wait for the WM and `org.a11y.Bus.GetAddress`. Testkit creates private daemon sockets and empty isolated HOME/XDG state, then owns the daemon, MCP proxy, fixture, and sentinel processes. Do not attach to your installed daemon.

The runner first executes `canonical_e2e_environment_is_ready`, then only `shared_web_action_matrix_is_state_verified`. The default substring filter runs **two** cells: `linux-electron-left-click-ax-background` and its `-typed-sdk` sibling. Both must deliver, not skip or refuse. `READY … daemon usable … AX and journal marker present` means a successful daemon tool path, a discovered real window, and `WEB_HARNESS_MARKER_v1` in both AX and the fixture's read-only event journal. Socket existence alone is not ready. The helper completes the action and tears down automatically; it is not an interactive server launcher.

Nested Wayland uses the existing CI wrapper, with its default runner and the same focused lane:

```bash
timeout 1260 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh wayland --no-build
```

Install the compositor dependencies from `.github/workflows/e2e-rust-linux-wayland.yml` before this lane. `scripts/ci/linux/run-rust-e2e-wayland.sh` provisions its runtime, Sway and AT-SPI; do not duplicate its bootstrap. On Debian the AT-SPI launcher is off `PATH`: set `CUA_AT_SPI_BUS_LAUNCHER=/usr/libexec/at-spi-bus-launcher`. Compositor and AT-SPI logs honor the unique artifact directory. Wayland is unverified here; X11 results do not certify its capture or input routes.

## Doctor

Read-only diagnostics run inside the same session before driving:

- `gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.a11y.Bus.GetAddress` probes the accessibility bus; `at-spi-address.txt` retains its address.
- `libs/cua-driver/rust/target/release/cua-driver doctor --json` writes `doctor.json`; no `--fix`. Inspect warnings even if the exit status is zero. Linux `health_report` checks the display and AT-SPI availability.
- `libs/cua-driver/rust/target/release/cua-driver --version` writes `version.txt`.
- Preflight calls `get_config`, `health_report`, and `check_permissions` on its owned daemon. Its recording directory retains `health_report.json`, `check_permissions.json`, and `daemon.json` (PID, socket, owner UID, mode, version/build config). Preflight asserts source version and optional requested SHA, Unix socket type, same-user ownership and no other-user access. It also proves fixture AX, capture and video readiness.

A permissions report is not authorization coverage: canonical testkit explicitly enables unrestricted GUI operations and bypasses interactive approvals. Health warnings must be interpreted alongside preflight, not silently called healthy.

## Drive

Reuse `libs/cua-driver/rust/crates/cua-driver-e2e/tests/cross_platform_behavior_test.rs`. It launches the real Electron fixture from `libs/cua-driver/tests/fixtures/apps/cross-platform/electron`, discovers its new PID/window, and obtains fresh snapshot-bound handles. The actual MCP tool sequence is:

```rust
let windows = fixture.driver.call("list_windows", serde_json::json!({}));
let pre = snapshot(fixture);
let args = action_target_args(fixture, &pre, "border-click-target", "ax", "background");
let response = fixture.driver.call("click", args);
```

`action_target_args` sends `pid`, `window_id`, `delivery_mode: "background"`, and the discovered `element_token` from `require_element`. Do not paste numeric handles or reuse a token after another snapshot. The fixture journal must independently report `last_action=left_click`; an accepted response alone is insufficient. The typed SDK sibling additionally exercises stale/mismatched handle refusals before a valid click.

Use the commands in the feature map to select other user journeys. Prefer AX handles; use existing `action_target_args` pixel conversion only for pixel coverage, not guessed screen coordinates. The same holds when you drive a real app with the installed driver: on macOS a foreground pixel click can miss a sheet or a web-view menu row that an `element_token` press hits, on the host as well as in a guest. A missed pixel click is not an app bug until the AX press also fails. Do not use DOM setters, fixture control endpoints, or direct journal writes to make target state pass. The fixture journal observes effects; the real driver performs the action.

## Evidence

Keep the printed unique directory. It contains `launch.log`, doctor/version output, preflight logs, `results.jsonl`, `cases.jsonl`, `environment.jsonl`, and `summary.md`. Each existing per-cell recording directory contains:

- `tool-calls.jsonl`: actual MCP requests and full response envelopes, including transport errors/timeouts. Image/base64 payloads are replaced by encoded byte length and a hash; PNGs remain separate. Typed SDK traffic is not MCP transcript traffic.
- `trajectory.json`, `recording.mp4`, and action `turn-*` directories with `action.json`, `before.png`, `after.png` from the product recording mechanism.
- `fixture-state.json`: the independent fixture journal after the action, also saved during unwinding.
- `sentinel-events.jsonl`: foreground heartbeat, focus and input journal.
- `desktop-observation.txt`: native before/after focus, z-order, pointer, transient focus journal, unsupported oracles and violations. Result errors retain these diagnostics too.

The runner uses `cua-e2e-report --require-video` and ffprobe validation. A passing summary must include delivered click rows and playable owned videos. Inspect action, response, before/after screenshots and fixture side effect together. For background input, require a live heartbeat, no leaked input/blur, stable native focus and pointer, no target raise, and no unsupported oracle. A refusal is a valid refusal test, **not** proof of successful delivery. Failed setup must remain a failure, not be replaced by a screenshot from another run.

## Cleanup

Testkit's child reaper owns daemon/fixture groups; the sentinel owns its own group. Scope exit reaps them. The X11 wrapper traps exit and kills/waits only the openbox and picom PIDs it captured with `$!`; xvfb-run and dbus-run-session own their sessions. The Wayland wrapper owns its compositor and bus PIDs. Never use process-name matching or kill another desktop.

After session teardown, the helper invokes `scripts/check-evidence.sh` on the printed directory, even if the desktop command failed. The checker requires complete current-attempt evidence for every cell declared in `cases.jsonl`, so a `CUA_E2E_CELL_FILTER` run is checked against the cells it selected, and checks `kill -0` against recorded daemon PIDs and discovered `CuaTestHarness` window PIDs, without killing them. Malformed extraction, zero recorded PIDs or any surviving PID fails verification, not permission to kill by name. Do not delete the evidence directory. A timeout or crash may leave incomplete evidence; inspect the launch log and recorded PIDs before retrying.

## Helpers

The shipped executables are:

```bash
.cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11
.cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build
.cursor/skills/verify-cua-driver/scripts/verify-linux.sh wayland --no-build
.cursor/skills/verify-cua-driver/scripts/verify-macos.sh doctor
.cursor/skills/verify-cua-driver/scripts/verify-macos.sh --source /abs/clean/checkout focus <cell>
.cursor/skills/verify-cua-driver/scripts/verify-macos.sh --source /abs/clean/checkout full
```

It reuses `scripts/ci/linux/run-rust-e2e.sh` through `CUA_E2E_INTERNAL_LANE=verify` and `CUA_E2E_ARTIFACT_DIR`; the runner is responsible for strict fixtures, preflight, recordings and report validation. Each desktop run is bounded to 1200 seconds internally. `verify-macos.sh` calls the Lume host's installed `lume-e2e` wrapper, which runs `run-all.sh` unchanged in the guest. No bench code or second harness is involved.

## macOS

Follow [the macOS lane](macos-lume.md). It runs the canonical `libs/cua-driver/tests/runners/macos-lume/run-all.sh` in a disposable clone of a granted Lume seed, through `scripts/verify-macos.sh`. Don't run AppKit E2E on a host desktop.

## Windows: unverified

Windows needs an interactive desktop, native UIA and named-pipe transport; testkit uses Job Objects for ownership. Use its existing CI recipe and recording paths. Linux's unrestricted testkit mode does not prove its production permission paths.

## Execution status and maintenance

The Linux X11 proof runs in CI: `.github/workflows/e2e-rust-linux.yml`, job "Linux / verification skill proof" (`verify-skill`), on skill and driver pull requests or `workflow_dispatch` with `lane=verify`, with retries off and a 25-minute bound. Its first recorded pass is [run 42399619203](https://github.com/trycua/cua/actions/runs/42399619203), and its `verify-cua-driver-evidence` artifact keeps the evidence even on failure. When the host can't run X11, cite that job at the PR head instead of skipping the lane. See [execution instructions and stand-ins](execution.md). Claim proof only after both click cells deliver and all required evidence survives cleanup; setup logs alone do not prove delivery or owned-process teardown.

## Verdict

A FAIL names a defect the diff introduces. Before you report one, rerun the failing check at the base. A failure main has too, with the same message in code the diff doesn't touch, is a note on the verdict unless the diff claims to fix it. A failure that is only something a merge-first PR adds (for example a command that lands in that PR) stays a failure at the tested head. Name that PR, and call it a merge-order dependency rather than a defect.

Use `/maintain-verification-skill` when tools, fixture markers, CI lanes or supported input routes change.
