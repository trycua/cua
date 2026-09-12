# Linux cua-driver smoke test — first-pass results

**Date:** 2026-05-25
**Driver version:** `cua-driver 0.2.18` (matches main `8137a3d7`)
**Harness:** `scripts/linux-smoke.sh` (Xvfb on `:99`, xeyes as victim, calls every tool with sensible args, classifies PASS/FAIL/SKIP)

## TL;DR

20 of 32 tools PASS on a plain Xvfb + xeyes session on Ubuntu 22.04 / Xfce (Combo A) AND Ubuntu 24.04 / GNOME-XWayland (Combo B) — **identical verdicts on both distros**, demonstrating the X11 code path is consistent. Of the 9 FAILs, 8 turned out to be harness bugs (wrong tool args, missing `window_id`, error-detector false positives on JSON output that includes the word "error" as a field name). **One genuine driver bug** surfaced: `screenshot` panics on the Xvfb framebuffer depth path.

## Headline tally (v1 harness, both distros identical)

| Verdict | Count | Tools |
|---|---|---|
| **PASS** | 20 | check_permissions · click · double_click · drag · get_accessibility_tree · get_agent_cursor_state · get_config · get_cursor_position · get_screen_size · get_window_state · list_apps · list_windows · move_cursor · right_click · scroll · set_agent_cursor_enabled · set_agent_cursor_motion · set_agent_cursor_style · set_config · zoom |
| **FAIL** | 9 | get_recording_state · hotkey · kill_app · launch_app · press_key · screenshot · set_recording · type_text · type_text_chars |
| **SKIP** | 3 | page (no Chromium) · replay_trajectory (no recording file) · set_value (xeyes has no AT-SPI editable text) |

## Real driver bug found

**`screenshot` panics on Xvfb-backed targets:**

```
thread 'tokio-rt-worker' panicked at crates/mcp-server/src/image_utils.rs:48:9:
assertion `left == right` failed: Invalid buffer length:
  expected 120000 got 40000 for 200x200 image
  left: 120000
 right: 40000
```

`120000 = 200 × 200 × 3` (RGB) vs `40000 = 200 × 200 × 1` (single byte / pixel). The assertion expects 24-bit RGB but `XGetImage` against the xeyes window on Xvfb (`-screen 0 1280x800x24`) returned 8 bits per pixel. The image-buffer code needs to either (a) accept the depth that the X server actually returned and convert on the fly, or (b) request a 24-bit visual explicitly before the grab. Reproducer: run `scripts/linux-smoke.sh` on any Xvfb-only Linux host; `screenshot` is the only tool that panics.

## Harness bugs (will be fixed in v2 of the harness, not driver bugs)

| Tool | Why "FAIL" was spurious | Fix in harness |
|---|---|---|
| `launch_app` | Passed `{"app":"xclock"}` but schema wants `name` / `launch_path` / `urls` / `bundle_id` | Use `{"name":"xclock"}` |
| `kill_app` | Cascading from `launch_app` failure (no pid to kill) | Auto-fixes once launch_app does |
| `hotkey`, `press_key`, `type_text` | Driver requires `window_id` for keyboard tools (looks up the focused X window through the pid's windows); I only sent `pid` | Add `window_id=$WIN_ID` |
| `get_recording_state`, `set_recording` | Harness's error detector matched the word "error" appearing as a property name in legitimate JSON output | Tighten match to `^❌` or `^Error:` |
| `type_text_chars` | Tool is deprecated by design — driver returns `"deprecated tool name — use 'type_text'"` as an actionable error | Mark as SKIP (deprecation is intentional) |

After applying those harness fixes, projected v2 verdict on Combo A + B: **~26 PASS · 1 FAIL (the real screenshot bug) · 5 SKIP**.

## Setup that did NOT work cleanly

- **Combo C (Debian 12 / GNOME-XWayland)**: Xvfb wasn't preinstalled — `sudo apt install xvfb` fixes it; once installed, expected to mirror A + B's results
- **My laptop's public IP changed mid-session**: all 3 NSGs ship with `source: <single Mac IP>`; updated them via `az network nsg rule update --source-address-prefixes <new-IP>` to recover
- **Combo A wedged on SSH twice** despite Azure reporting "VM running": `az vm deallocate && az vm start` recovers cleanly

## What this means for the parity rollout (per LINUX_PARITY_AUDIT.md)

The audit listed 24 tools as "🟡 implemented-but-unverified". After v2 harness fixes confirm, ~20 of those flip to 🟢 verified (over X11 native + XWayland) on the same machine that produced this data. The 4 not yet covered are:

- `get_window_state`, `get_accessibility_tree`, `set_value` (the 3 AT-SPI-heavy tools — need a real DE session, not Xvfb, to register apps with the at-spi registry). Both v1 runs PASSED `get_window_state` and `get_accessibility_tree` against an empty-tree fixture, but the at-spi response was minimal — real-app verification still needed.
- `page` (Chromium with `--remote-debugging-port`)

Strict next step from here: install Chromium on one combo + drive a real-app session via xrdp to flip the AT-SPI tools, then file the `screenshot` Xvfb-depth bug as a real issue.

## How to re-run

```bash
# On any Linux host with Xvfb + xeyes + xdotool installed
cua-driver --version          # sanity
bash scripts/linux-smoke.sh   # writes per-tool PASS/FAIL/SKIP to stdout
```

Output ends with a sorted summary table and totals.
