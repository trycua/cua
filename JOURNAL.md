# macOS cua-driver-rs Parity Sprint — JOURNAL

**Started:** 2026-05-26
**Branch:** `feat/macos-parity-and-harnesses` (local only — no push)
**Base:** `origin/main` @ `f51ce806` (latest as of branch creation)
**Goal:** align cua-driver-rs/crates/platform-macos to parity with platform-windows (recently refactored by Francesco), build Mac-app-specific test harnesses, verify everything works end-to-end.
**Scope:** ONLY `libs/cua-driver/rust/crates/platform-macos`. **Ignore `libs/cua-driver/swift/`.**

---

## Initial state snapshot

| Metric | platform-macos | platform-windows |
|---|---|---|
| Source files (.rs) | 57 | 23 |
| Example binaries (.rs) | **0** | 33 |

**Read:** macOS has *more* implementation files than Windows but *zero* example/parity binaries. So the implementation exists but is unverified at the per-tool level. That mirrors the Linux gap I documented earlier this week.

## Recent Windows-side work to reconcile against

From `git log --oneline origin/main`:

| PR | Commit | What |
|---|---|---|
| #1698 | f51ce806 | feat(test-harness): WPF + WinUI3 deterministic test apps + Rust integration tests |
| #1696 | 876d48d2 | fix(windows)(capture): size PrintWindow buffer to GetWindowRect, not GetClientRect |
| #1694 | c4c84710 | chore: delete dead ScreenshotTool / ScreenshotCompatTool after #1692 |
| #1692 | 892edd47 | feat(windows): agent-cursor z-order + background-dispatch hardening |
| #1690 | 8137a3d7 | feat(windows): suppress UWP self-foreground during UIA Invoke / Expand / Toggle (mine, merged yesterday) |

**#1698 is the load-bearing reference**: Francesco built **deterministic test apps + Rust integration tests** for WPF + WinUI3 on Windows. That's the pattern I need to mirror for macOS (deterministic Cocoa / SwiftUI / Catalyst test apps + Rust integration tests).

**#1692 is the architecture commit**: agent-cursor z-order + background-dispatch hardening. macOS likely needs equivalents.

## Sprint plan

1. **Phase 0 (now)**: branch + journal — DONE
2. **Phase 1 (next ~1h)**: full parity audit. Per-tool table: macOS impl status vs Windows impl status, file mapping, gaps
3. **Phase 2 (~2h)**: port any Windows fixes that have macOS equivalents (PR #1692 background-dispatch hardening is the obvious one; check #1696 for analog)
4. **Phase 3 (~2h)**: build Mac test apps + integration test pattern mirroring PR #1698
5. **Phase 4 (~1h)**: per-tool smoke harness (mac-smoke.sh equivalent to linux-smoke.sh)
6. **Phase 5 (~30min)**: build + run on this Mac, fix what breaks
7. **Phase 6 (~30min)**: commit everything to local branch, final journal entry

If I overrun any phase by >2x, I'll cut scope and document.

## Pre-flight decisions (no user to ask)

- **Branch:** local only, NO push to origin per instruction
- **Existing cua-driver install:** will leave installed for now; uninstall only if it interferes with TCC / building
- **Commit cadence:** journal entry + commit after each phase
- **When stuck:** document the decision + chosen default + tradeoff in journal, keep moving
- **Test apps:** prefer pure Rust over Swift wrappers where possible, but Cocoa apps may need a minimal Swift/ObjC harness — okay since the SwiftUI part is a test fixture, not the cua-driver-swift project

---

## Phase entries below (timestamped)

### [Phase 0] Setup — 2026-05-26 (start)
- Created branch `feat/macos-parity-and-harnesses` off `origin/main` @ f51ce806
- Wrote this JOURNAL.md
- About to start Phase 1 (audit)

### [Phase 1] Audit — complete

**Crate structure parity:**
- macOS: 57 source files, 0 example/parity binaries
- Windows: 23 source files, 33 example/parity binaries
- macOS uses **per-tool-file** layout (`tools/click.rs`, `tools/drag.rs`, etc.); Windows uses one big `tools/impl_.rs`. Different conventions, both functional.

**Tool registration parity** (`tools/mod.rs` macOS vs `tools/impl_.rs::build_registry` Windows):

| Tool | macOS | Windows | Notes |
|---|---|---|---|
| list_apps / list_windows / get_window_state | ✅ | ✅ | parity |
| launch_app / kill_app / bring_to_front | ✅ | ✅ | parity |
| debug_window_info | ❌ | ✅ | intentional Windows-only |
| click / double_click / right_click / drag | ✅ | ✅ | parity |
| type_text / press_key / hotkey | ✅ | ✅ | parity |
| set_value / scroll / zoom | ✅ | ✅ | parity |
| get_screen_size / get_cursor_position / move_cursor | ✅ | ✅ | parity |
| 4 cursor_tools | ✅ | ✅ | parity |
| check_permissions / get_config / set_config / get_accessibility_tree | ✅ | ✅ | parity |
| page (cross-platform; per-OS backend) | ✅ MacOsPageBackend | ✅ WindowsPageBackend | parity |
| recording tools | ✅ via register_recording_tools | ✅ via register_recording_tools | parity |
| **`type_text_chars`** | **❌ DIVERGENCE: registered as own tool** | ✅ alias-only via mcp-server | **parity item to fix** |
| screenshot / screenshot_compat | removed (per code comment) | removed (per code comment) | parity (intentional) |

**Recent Windows-side changes I need to reconcile against:**

| PR | What | macOS state |
|---|---|---|
| #1690 | UWP self-foreground bypass via `EnableWindow` | Windows-only (UWP/XAML doesn't exist on macOS). No macOS analog needed. |
| #1692 | agent-cursor z-order (`ZOrderEnforcer` trait, cross-platform) + Windows-specific input dispatch hardening | **Cross-platform parts already in macOS** (PR touched `platform-macos/src/cursor/overlay.rs` + `bring_to_front.rs` + `tools/mod.rs`). Need to verify the macOS `ZOrderEnforcer` impl is real, not stub. |
| #1696 | PrintWindow buffer sizing | Windows-only (uses GetClientRect/GetWindowRect APIs). No macOS analog. |
| #1698 | WPF + WinUI3 deterministic test apps + Rust integration tests | **Windows-only.** This is the pattern to mirror for macOS. |

**Build status on this Mac:**
- Had to install Rust (rustup → cargo 1.95.0 + rustc 1.95.0). Was missing.
- `cargo check -p cua-driver` exits 0 — 2 dead-code warnings, no errors.
- Release build kicked off in background, monitoring.

### [Phase 2] Plan — refined

Given the audit, the work splits into 3 parallel tracks:

1. **Parity fix:** unregister `type_text_chars` on macOS (mirror Windows alias-only handling) — small change, ~10 min.
2. **macOS test-harness apps** under `libs/cua-driver/test-harness/`:
   - `CuaTestHarness.AppKit/` — minimal AppKit Cocoa app, single-file Swift
   - `CuaTestHarness.SwiftUI/` — minimal SwiftUI app
   - extend `scenarios/scenarios.json` with `appkit` + `swiftui` sections
   - `build.sh` (parallels `build.ps1`) that compiles with `swiftc` and stages `.app` bundles into `rust/test-apps/harness-{appkit,swiftui}/`
3. **Rust integration tests** under `crates/cua-driver/tests/`:
   - `harness_appkit_test.rs` and `harness_swiftui_test.rs`
   - JSON-RPC against cua-driver MCP server, same shape as `harness_wpf_test.rs`
   - `#[ignore]` so they only run under `cargo test --ignored`

Decisions:
- Use **swiftc + manual .app bundle** (no Xcode .xcodeproj, no SPM). Simpler, builds in seconds, easier to read.
- Use **AX identifiers** on NSView/SwiftUI views — macOS equivalent of WPF AutomationId.
- Keep the scenarios.json scenario IDs aligned with WPF/WinUI3 where the concept maps (`counter`, `text_body`, `text_input`, `click_target`, `scroll_target`, `exit`) so the matrix is consistent across platforms.
- Macos-specific scenarios to add: `nstoolbar` (NSToolbar item enumeration), `nsmenubar` (top menubar — uniquely Mac).

Starting Phase 3 (apps) next.
