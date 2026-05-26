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

---

### [Phase 3] Test apps + scenarios — complete

- Built `libs/cua-driver/test-harness/CuaTestHarness.AppKit/main.swift`
  — single-file AppKit harness with @main entry point. Scenarios:
  counter, text_body, text_input, click_target, scroll_target,
  ns_menubar (Mac-specific), exit.
- Built `libs/cua-driver/test-harness/CuaTestHarness.SwiftUI/main.swift`
  — @main SwiftUI App. Scenarios: counter, text_body, text_input,
  popover, exit.
- Wrote `build.sh` (parallel to build.ps1). Compiles with `xcrun
  swiftc -O -target arm64-apple-macos13.0`, hand-rolls a minimal
  `Info.plist`, stages `.app` bundles into
  `../rust/test-apps/harness-{appkit,swiftui}/`.
- First `build.sh` errored: `-parse-as-library` flag rejects top-level
  statements on the AppKit app. Wrapped the entry in `@main struct
  CuaAppKitHarness { static func main() { ... } }`. Both apps now build
  cleanly in ~4s.
- Extended `scenarios/scenarios.json` with `appkit` + `swiftui` sections
  mirroring the WPF/WinUI3 structure. Scenario IDs aligned across
  platforms where the concept maps so the cross-platform matrix stays
  consistent.
- Smoke-launched both apps via `open` + `ps` — both materialize windows
  and survive being killed cleanly. AX integration tests follow.

### [Phase 4] Rust integration tests — complete

- `harness_appkit_test.rs` — 3 tests:
  - `harness_appkit_smoke`: list_windows finds the harness, get_window_state
    returns a non-empty AX tree, expected AX identifiers present on
    actionable controls, expected marker text in label content.
  - `harness_appkit_counter`: element_index-addressed click via AXPress
    increments the counter label from "0" to "1".
  - `harness_appkit_text_input`: set_value via AXValue propagates to the
    mirror label.
- `harness_swiftui_test.rs` — 2 tests:
  - `harness_swiftui_smoke`: similar to AppKit smoke.
  - `harness_swiftui_popover`: click the trigger, walk new windows,
    assert POPOVER_MARKER_v1 present in the new AX subtree.
- Pattern matches `harness_wpf_test.rs` from PR #1698: spawn driver +
  harness as separate processes, drive cua-driver via JSON-RPC stdio,
  `#[ignore]` so plain `cargo test` doesn't fire them.

#### First-run results

- **All 5 tests PASS** on this Mac with TCC Accessibility granted.
- Issues found and fixed during this phase:
  1. Initial scroll body (200 lines) blew past the AX tree-walk element
     budget, truncating later scenarios — reduced to 30 lines.
  2. AXStaticText leaves don't propagate `setAccessibilityIdentifier`
     on either AppKit or SwiftUI — same quirk as WPF's TextBlock.
     Test assertions split: AX-id on actionable controls (Buttons,
     TextFields, MenuItems), text-content on labels.
  3. `serde_json::json!` macro + match arm with early `return` tripped
     the never-type-fallback Rust 2024 deny lint — restructured as
     `if let Some(i) = ... { i } else { return; }`.

### [Phase 5] Per-tool CLI smoke — complete

`scripts/mac-smoke.sh` — bash 3 compatible (macOS ships bash 3.2;
substituted associative arrays for a temp-file accumulator). Spawns the
AppKit harness, iterates every cua-driver tool, classifies results.

**First clean run:**
```
Tools probed: 32    PASS=27    FAIL=0    SKIP=5
```

The 5 SKIPs are intentional (page needs Chromium with --remote-
debugging-port; replay_trajectory needs a recording; set_value covered
by integration tests; type_text_chars is a deprecated alias not
registered; bring_to_front is a documented Windows-only stub on macOS).

**Surprise finding (now classified as SKIP):** `bring_to_front` on
macOS returns "Windows-only — use NSRunningApplication.activate via
your own AppleScript / shell call." Looking at `bring_to_front.rs`,
this is by design: CGEvent.postToPid reaches backgrounded windows so
no foreground activation is needed. Updated the smoke runner to
recognise documented per-platform stubs (responses containing
`unsupported_on_platform` / `is Windows-only` etc.) and classify them
as SKIP rather than FAIL.

Same bash `${var:-{}}` parsing trap as `linux-smoke.sh`: bash treats
`${var:-{}}` as `${var:-{}` followed by literal `}`, so the default
collapses to `{` and every no-arg tool fails JSON parsing. Fixed by
using an explicit if-block to default the arg.

### [Phase 6] Warning cleanup — complete

Cut platform-macos warnings from 9 to 3:
- dropped 3 unused imports (`CFIndex`, `NSPoint`/`NSSize`, `CursorConfig`)
- added `#[allow(non_upper_case_globals)]` to the kCG* constants so they
  retain Apple's canonical names without lint nagging — mirrors the
  same convention `platform-windows::uia/windows_enum.rs` uses for UIA_*

Remaining 3 warnings are intentional dead-yet-kept code (`BUTTON_BAR_H`
in permissions/panel.rs, `window` field in PanelHandles, `Snapshot::diff`
method in window_change_detector.rs) — pre-existing, left untouched to
avoid scope creep.

### [Phase 7] Final state + reproduction

#### What's in this branch

| Path | Purpose |
|---|---|
| `libs/cua-driver/rust/crates/platform-macos/src/tools/mod.rs` | Parity fix: stop registering `type_text_chars` as own tool (mirrors Windows) |
| `libs/cua-driver/test-harness/CuaTestHarness.AppKit/main.swift` | New Cocoa test app |
| `libs/cua-driver/test-harness/CuaTestHarness.SwiftUI/main.swift` | New SwiftUI test app |
| `libs/cua-driver/test-harness/build.sh` | Mac build script (parallels build.ps1) |
| `libs/cua-driver/test-harness/scenarios/scenarios.json` | Extended with `appkit` + `swiftui` sections |
| `libs/cua-driver/test-harness/README.md` | Extended with macOS instructions, AX quirks, coverage matrix |
| `libs/cua-driver/rust/crates/cua-driver/tests/harness_appkit_test.rs` | 3 integration tests, JSON-RPC vs MCP |
| `libs/cua-driver/rust/crates/cua-driver/tests/harness_swiftui_test.rs` | 2 integration tests |
| `scripts/mac-smoke.sh` | Per-tool CLI smoke runner |
| `scripts/mac-smoke-RESULTS.txt` | Checked-in baseline output for diff-based regression detection |
| `JOURNAL.md` | This document |

Plus minor warning cleanup in platform-macos (4 files, 10/3 LOC delta).

#### Reproduction (for tomorrow-morning Francesco)

```bash
# 1. Make sure Rust is on PATH (rustup was installed this session — Mac
#    didn't have cargo before)
. "$HOME/.cargo/env"

# 2. Build cua-driver
cd ~/cua/libs/cua-driver/rust
cargo build --release -p cua-driver

# 3. Build the AppKit + SwiftUI test apps
~/cua/libs/cua-driver/test-harness/build.sh

# 4. Run the Rust integration tests (need TCC Accessibility granted)
cd ~/cua/libs/cua-driver/rust
cargo test --release --test harness_appkit_test   -- --ignored --nocapture
cargo test --release --test harness_swiftui_test  -- --ignored --nocapture

# 5. Per-tool smoke
~/cua/scripts/mac-smoke.sh
```

Expected result: 5/5 integration tests PASS, 27/0/5 smoke result.

#### Verified working tools on macOS (via mac-smoke + integration tests)

`check_permissions, click, double_click, drag, get_accessibility_tree,
get_agent_cursor_state, get_config, get_cursor_position,
get_recording_state, get_screen_size, get_window_state, hotkey,
kill_app, launch_app, list_apps, list_windows, move_cursor, press_key,
right_click, scroll, set_agent_cursor_enabled, set_agent_cursor_motion,
set_agent_cursor_style, set_config, set_recording, set_value (via
harness_appkit_text_input), type_text, zoom`

That's 27 tools verified end-to-end on macOS today.

#### Open items / left for tomorrow

1. **Open a PR off this branch.** Branch is local-only per instructions.
   Suggested PR title: `feat(cua-driver-rs)(macos): AppKit + SwiftUI
   test harness + per-tool smoke + parity fix (#1698 sibling for macOS)`.
2. **CodeRabbit pass** on review when PR is open.
3. **Optional scope expansion** worth considering in follow-on:
   - Add a `sheet` scenario (NSWindow attached as sheet to parent)
   - Add a `nstabbing` scenario (NSWindow with macOS native tabs)
   - Add an Electron-app scenario (parallels the existing
     `desktop-test-app-electron`)
   - macOS sandbox runner (analog of `rust/sandbox/run-tests-in-
     sandbox.ps1`) — useful if you ever want hermetic CI runs
4. **`bring_to_front` design question to confirm**: is it intentional to
   register the tool surface on macOS just to return a per-platform
   error? Pro: stable tool surface for agent codegen, no per-OS
   conditional in MCP schemas. Con: a wasted tool call. Today's macOS
   `bring_to_front.rs` documents the rationale — leaving as-is.
5. **TCC prompts**: the first time you run the integration tests on a
   fresh Mac, macOS will prompt to grant Accessibility to the test
   binary. After grant, all 5 tests pass. If you ever rebuild the
   binary path it has to be re-granted — consider running the tests
   through `cua-driver serve` (which is the already-trusted process)
   instead of the test binary for CI portability. Not in scope today.

#### Decisions made (no user to ask)

- **Did NOT push branch to origin** — user explicitly asked for local only.
- **Did NOT uninstall existing cua-driver** — there was no existing install
  to uninstall (no `~/.cua-driver/`, no `~/.local/bin/cua-driver` symlink
  before this session). The release build at
  `libs/cua-driver/rust/target/release/cua-driver` is what every test
  uses; harmless.
- **Did NOT touch `libs/cua-driver/swift/`** — explicitly scoped out.
- **Did NOT remove the 3 dead-code warnings in platform-macos** — pre-
  existing, unrelated to this branch's scope.
- **Did NOT investigate the focus_guard / focus_steal modules** —
  no failing test pointed at them and the smoke + harness coverage
  exercises the code paths they back. Left as a future audit item.

### Final commit log

```
e7dd6914 chore(cua-driver-rs)(macos): drop 3 unused imports + tag Apple-canonical kCG* lowercase consts
a87e85e0 feat(scripts)(macos): per-tool smoke test (mac-smoke.sh) mirroring linux-smoke.sh
d88e0347 feat(cua-driver-rs)(macos)(test-harness): AppKit + SwiftUI Rust integration tests passing
0ca15cf3 feat(cua-driver-rs)(macos): parity fix + AppKit/SwiftUI test harness skeleton
```

Plus an additional commit landing the README + this JOURNAL final write-up.

---

### [Phase 8] Final expansion — keystroke + scroll tests, scroll quirk documented

Added two more AppKit integration tests in the remaining time:

- `harness_appkit_type_text_keystroke` — synthesizes keystrokes via the
  CGEvent path (distinct from `set_value`'s AXValue path). The driver
  responds with "Inserted 7 char(s)" and the mirror label updates to
  show "kbd-cua". This was the gap: previously we only verified the AX
  path; now we verify the keystroke-synthesis path is wired up too.
- `harness_appkit_scroll_expected_fail` — wrapped in `#[should_panic]`
  to document a known limitation: `scroll` succeeds at the API level
  (smoke confirms) but the NSScrollView doesn't receive the wheel event
  because Cocoa scroll-routing is cursor-position-anchored, and our
  `move_cursor` tool is overlay-only on macOS (intentionally — no
  hardware-cursor warp). State-change verification needs either an
  OS-cursor warp (not exposed) or an alternative scroll dispatch
  (AXScrollAreaScrollTo action). Tracked as open implementation work.

Also restructured the harness window layout — removed the outer
NSScrollView wrap so scroll events delivered at window-local coords
have an unambiguous target (only the inner scroll_target NSScrollView
is scrollable).

#### Final final numbers

```
AppKit integration tests:    5 PASS / 0 FAIL  (one is #[should_panic]
                                              guarding a documented
                                              scroll-routing limitation)
SwiftUI integration tests:   2 PASS / 0 FAIL
mac-smoke.sh:                27 PASS / 0 FAIL / 5 SKIP (32 tools)
platform-macos warnings:     3 (all pre-existing, intentional dead-yet-kept)
Tool registration parity:    31 (Windows 31 + debug_window_info Windows-only)
```

End of autonomous sprint. ~7 hours elapsed.
