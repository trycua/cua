# cua-driver-rs ↔ cua-driver (Swift) Parity Audit

## Integration Test Suite

All API surfaces (CLI subcommands, stdio MCP protocol, daemon lifecycle) are
covered by `tests/integration/test_api_parity.py` — **111 tests per binary,
222 total**, parametrized so both binaries run the identical suite.

```bash
# Rust binary only
cd libs/cua-driver/rust/tests/integration
./run_tests.sh test_api_parity -v

# Both binaries side-by-side
./run_tests.sh --parity -v
```

### Known parity gaps (as of 2026-05-13, macOS)

| Gap | Rust | Swift |
|-----|------|-------|
| `type_text_chars` | ✅ | missing |
| `get_accessibility_tree` | ✅ | missing |
| `page` tool | ✅ (cross-platform: Apple-Events on macOS, UIA+CDP on Windows, AT-SPI+CDP on Linux) | ✅ (macOS only) |
| `--version` flag | ✅ | ✅ |
| `call check_permissions` JSON | ✅ JSON | human-readable text |
| `call screenshot` (no window_id) | ✅ full-display default | error (requires window_id) |

---

Tracking surface-by-surface line-by-line behavioral comparison between the
Rust port and the Swift reference. Each entry lists Swift source location,
Rust source location, divergences (intentional vs. accidental), and the
deterministic test that locks the verified behavior in.

Format per entry:
```
## <surface>
- Swift: <path:line>
- Rust:  macOS=<path:line>, windows=<path:line>, linux=<path:line>
- Status: VERIFIED | INTENTIONAL_DIVERGENCE | OPEN
- Test:   <path>
- Notes:  ...
```

---

## MCP tool: `move_cursor`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/MoveCursorTool.swift:6-60`
- Rust:
  - macOS=`crates/platform-macos/src/tools/move_cursor.rs`
  - windows=`crates/platform-windows/src/tools/impl_.rs` (MoveCursorTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (MoveCursorTool)
- Status: INTENTIONAL_DIVERGENCE (semantic) + VERIFIED (overlay behavior)
- Test:  `crates/platform-windows/examples/cursor_visibility.rs`

### Intentional semantic divergence

Swift's `move_cursor` calls `CGWarpMouseCursorPosition` — it warps the
**real OS cursor** instantly. The Rust port repurposes the same tool name
to drive the **agent overlay** (animated, non-warping arrow) instead.

Rationale: the entire premise of `cua-driver-rs` is background automation
that never steals focus and never moves the user's physical mouse. Porting
the Swift cursor-warp behavior would directly violate Swift's own
focus-guard / no-cursor-warp invariants enforced elsewhere (click,
type_text, etc.). The Rust port treats `move_cursor` as "show the agent's
attention" — visual only.

Consequences:
- Schema accepts an extra optional `cursor_id: string` (multi-cursor support
  doesn't exist in Swift).
- Schema accepts floats (`number`) for `x`/`y`; Swift only accepts integers.
  Rust's looser type accepts every Swift-valid integer plus fractional pixel
  targets used by HiDPI flows.
- Response text uses `Agent cursor '<id>' moved to (X.X, Y.Y).` instead of
  `✅ Moved cursor to (X, Y).` — the Swift wording would be misleading
  given the different semantics.

### Cross-platform consistency (verified)

All three Rust platforms now send:
```
OverlayCommand::MoveTo { x, y, end_heading_radians: FRAC_PI_4 }
```

`FRAC_PI_4` (π/4) matches Swift `AgentCursor.animateAndWait(endAngleDegrees:
45)` so the overlay arrow always settles pointing upper-left. Linux was
previously sending `0.0` (left-pointing); fixed in this commit.

The deterministic test (`cursor_visibility.rs`) drives the live daemon via
the named pipe, sets a magenta gradient, sends `move_cursor`, and polls
screenshots until the cursor centroid settles. Asserts the final centroid
is within 100 px of the requested target and that ≥50 magenta pixels are
rendered. Hard 4 s timeout. Verified on Windows; should run on macOS/Linux
once the daemon pipe is exposed there (macOS uses Unix socket, Linux uses
Unix socket).

---

## MCP tool: `get_cursor_position`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/GetCursorPositionTool.swift:6-37`
- Rust:
  - macOS=`crates/platform-macos/src/tools/get_cursor_position.rs`
  - windows=`crates/platform-windows/src/tools/impl_.rs` (GetCursorPositionTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (GetCursorPositionTool)
- Status: VERIFIED
- Test:  `crates/platform-windows/examples/get_cursor_position_parity.rs`

### Fixed divergences

1. **Response text format** — Swift returns `"✅ Cursor at (X, Y)"`; Rust on
   every platform was returning `"Cursor: (X, Y)"` (no checkmark, wrong
   word). All three platforms now match Swift exactly.
2. **macOS coord type** — Swift truncates to `Int(pos.x)`; macOS Rust was
   returning floats formatted `"({x:.1}, {y:.1})"`. Now truncates to
   integers like Swift, consistent with Windows/Linux Rust.
3. **Description text** — was inconsistent across Rust platforms. All
   three now use Swift's wording: `"Return the current mouse cursor
   position in screen points (origin top-left)."`.

### Intentional additions (Rust-only)

- `structuredContent: { x: int, y: int }` is included alongside the text
  response. Swift returns text only. This is a backwards-compatible MCP
  enrichment — tools that read structured content get integers; tools
  that read text get Swift's exact format. The test asserts both views
  agree and both agree with the platform's native `GetCursorPos` call
  within ±5 px.

### Underlying API per platform

| Platform | Swift                              | Rust                                       |
|----------|------------------------------------|--------------------------------------------|
| macOS    | `CGEvent(source: nil).location`    | `CGEvent::new(CGEventSource(HIDSystemState)).location()` |
| Windows  | n/a                                | `GetCursorPos` (Win32)                     |
| Linux    | n/a                                | `xproto::query_pointer` on the root window |

All three return screen-coordinate space, top-left origin, matching the
documented Swift behavior.

---

## MCP tool: `get_screen_size`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/GetScreenSizeTool.swift:6-46`
        + `libs/cua-driver/Sources/CuaDriverCore/Capture/ScreenInfo.swift`
- Rust:
  - macOS=`crates/platform-macos/src/tools/get_screen_size.rs`
  - windows=`crates/platform-windows/src/tools/impl_.rs` (GetScreenSizeTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (GetScreenSizeTool)
- Status: VERIFIED
- Test:  `crates/platform-windows/examples/get_screen_size_parity.rs`

### Fixed divergences

1. **Missing `scale_factor`** — Swift's whole reason for this tool is to
   expose the backing scale factor (Retina = 2.0). All three Rust
   platforms were dropping it. Now all three return it:
   - macOS: `NSScreen.mainScreen.backingScaleFactor` via `objc2-app-kit`.
   - Windows: `GetDpiForSystem() / 96.0`.
   - Linux: hardcoded `1.0` (X11 has no per-monitor scale; recorded as a
     known limitation rather than silently dropping the field).
2. **Text format** — was `"Screen: W×H"` (no checkmark, no scale, ×
   unicode); now `"✅ Main display: WxH points @ Sx"` matching Swift.
3. **Error case** — macOS now returns `ToolResult::error("No main display
   detected.")` when `NSScreen.mainScreen` is nil, matching Swift's
   `isError: true` response.
4. **Description** — standardized to Swift's wording across all three.
5. **`structuredContent.scale_factor` key** is snake_case to match
   Swift's `ScreenSize.CodingKeys.scaleFactor = "scale_factor"`.

### Verified on Windows

`get_screen_size_parity.exe` round-trips the pipe and asserts:
- Text matches `"✅ Main display: 1680x1050 points @ 1x"` exactly.
- `structuredContent.width / height / scale_factor` agree with the text.
- All three agree with native `GetSystemMetrics` + `GetDpiForSystem` /96.

### Intentional Rust-only

`structuredContent: { width, height, scale_factor }` is included alongside
the text response.  Swift returns text only.  Same rationale as
`get_cursor_position` — backwards-compatible MCP enrichment, text format
still matches Swift exactly.

---

## MCP tool: `check_permissions`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/CheckPermissionsTool.swift:6-59`
        + `libs/cua-driver/Sources/CuaDriverCore/Permissions/Permissions.swift`
- Rust:
  - macOS=`crates/platform-macos/src/tools/check_permissions.rs` (FIXED, pending macOS run)
  - windows=`crates/platform-windows/src/tools/impl_.rs` (CheckPermissionsTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (CheckPermissionsTool)
- Status:
  - macOS: OPEN (fixed in source; macOS runner needed to verify)
  - windows / linux: INTENTIONAL_DIVERGENCE
- Test: TODO macOS — needs a macOS machine or CI runner to drive the daemon
  and assert the text format + structured response.

### Fixed divergences (macOS)

1. **Schema** — added `prompt: boolean` parameter matching Swift's
   `inputSchema.properties.prompt`.
2. **Default behavior** — Swift defaults `prompt: true` and raises
   the macOS Accessibility + Screen Recording TCC prompts via
   `AXIsProcessTrustedWithOptions({"AXTrustedCheckOptionPrompt": true})`
   and `CGRequestScreenCaptureAccess()`.  Rust previously never prompted.
   Both APIs are now wired up via `link(name = "ApplicationServices"|"CoreGraphics")`.
3. **Text format** — was `"Accessibility API: ✅ granted\nScreen Recording: ✅ granted"`;
   now matches Swift exactly: `"✅ Accessibility: granted.\n✅ Screen Recording: granted."`.
4. **Annotation** — `read_only: false` (was `true`) matching Swift's
   `readOnlyHint: false` — the default path can mutate state by raising a dialog.
5. **Description** — copied from Swift verbatim.
6. **Screen Recording probe** — Swift uses `SCShareableContent.excludingDesktopWindows`
   (ScreenCaptureKit), which is hard to call from Rust without large bindings.
   Approximation: `CGPreflightScreenCaptureAccess()` (preflight C API) with
   fallback to the existing window-enumeration heuristic.  May report
   false negatives for some subprocess-launched apps (same caveat that
   prompted Swift to switch to SCShareableContent).  Documented as a
   known approximation.

### Windows / Linux intentional divergence

`check_permissions` on macOS is fundamentally about TCC (Apple's per-app
permission database).  Neither Windows nor Linux have an equivalent.  The
Rust ports retain the tool name and the read-only-status structure, but
return platform-specific content:

- **Windows** reports process elevation (admin vs standard) and confirms
  PostMessage + UIA work without elevated rights.  The whole premise of
  cua-driver-rs on Windows is that background automation needs no
  special permissions, so the tool's role is to *confirm* that.
- **Linux** reports X11 display reachability, D-Bus session presence,
  and XSendEvent availability — the inputs the X11 backends actually
  need to work.

These divergences are intentional and not fixable without making the tool
no-op on Windows/Linux (worse UX than returning the platform-specific
status).

---

## Startup flow: permissions gate (`serve`)
- Swift: `libs/cua-driver/Sources/CuaDriverCore/Permissions/PermissionsGate.swift`
        (SwiftUI panel + AppKit window + 1 Hz polling)
- Rust:
  - macos=`crates/platform-macos/src/permissions/gate.rs`
    (CLI banner + auto-open System Settings + 1 Hz polling)
  - windows / linux: N/A — TCC is a macOS concept; the `--no-permissions-gate`
    flag is accepted and silently ignored on those platforms for CLI uniformity.
- Status:
  - macos: PORTED with intentional UX divergence (CLI, not SwiftUI)
  - windows / linux: N/A

### Intentional UX divergence

Swift surfaces a branded SwiftUI window on first launch.  The Rust port
ships a terminal-driven banner instead.  Rationale:

1. cua-driver-rs already drives an AppKit run loop on the main thread
   for the cursor overlay; bolting on a second window invites
   main-thread deadlocks.
2. The Rust binary's primary deployment shape is the daemon under
   `cua-driver serve` from a shell (Claude Code, Cursor, Codex), which
   already has a terminal attached.
3. Headless / CI use cases need an opt-out; a CLI flow with
   `--no-permissions-gate` + `CUA_DRIVER_RS_PERMISSIONS_GATE=0` is the
   straight-line approach.  Replicating Swift's window only to suppress
   it under headless would be more code with no UX upside.

The CLI gate still preserves the substantive Swift behaviours:

- Lists exactly which TCC grants are missing (Accessibility / Screen
  Recording), with the same rationale strings the SwiftUI panel uses.
- Opens both `x-apple.systempreferences:` URLs at once so the user can
  grant both in a single Settings visit.  (Swift's "chain to next pane
  when one flips green" trick is unnecessary when both panes are
  pre-opened.)
- Polls at 1 Hz, identical cadence to the Swift `Timer`.
- Auto-continues startup the moment all required grants are green.

### Opt-out signals

| Signal                                  | Effect      |
| --------------------------------------- | ----------- |
| `--no-permissions-gate` flag            | gate skipped |
| `CUA_DRIVER_RS_PERMISSIONS_GATE=0`      | gate skipped |
| `CUA_DRIVER_RS_PERMISSIONS_GATE=false`  | gate skipped |
| `CUA_DRIVER_RS_PERMISSIONS_GATE=no`     | gate skipped |
| `CUA_DRIVER_RS_PERMISSIONS_GATE=off`    | gate skipped |
| any other env value                     | gate active  |

Default deadline is 10 minutes; on timeout the gate logs an error and
`serve` continues to start, mirroring the Swift "user closed the
panel" path (individual tool calls then fail with the underlying TCC
error).

A native `NSAlert` via objc2 is tracked as a follow-up if the
terminal-only flow proves insufficient; the CLI is the MVP.

---

## MCP tool: `list_apps`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ListAppsTool.swift:6-71`
        + `libs/cua-driver/Sources/CuaDriverCore/Apps/AppInfo.swift`
- Rust:
  - macOS=`crates/platform-macos/src/tools/list_apps.rs` + `apps.rs:format_app_list`
  - windows=`crates/platform-windows/src/tools/impl_.rs` (ListAppsTool)
           + `crates/platform-windows/src/win32/installed_apps.rs`
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ListAppsTool)
         + `crates/platform-linux/src/installed_apps.rs`
- Status:
  - macOS: VERIFIED — unified shape + installed-app scan + running merge
  - windows: VERIFIED live — Win11 24H2 in Session 0 returns ~150 apps
    (Start-Menu .lnk + WinRT PackageManager UWP), cold call ~370ms
    (80ms Start-Menu, 320ms UWP), warm call sub-100ms. All entries
    expose the unified shape (`pid`, `name`, `bundle_id`, `kind`,
    `launch_path`, `last_used`, `windows`, `running`, `active`).
    Validated against `list_apps_parity` example.
  - linux: code ready (cross-target check pending Linux host)
- Test: `tests/integration/test_api_parity.py::RustParityTests::test_call_list_apps_*`
        + `crates/platform-windows/examples/list_apps_parity.rs`

### Unified response shape (cross-platform)

Single flat array. Every entry carries the same fields on every
platform; values that don't apply to a given platform are `null`.

```jsonc
{
  "apps": [
    {
      "pid": 47291,                         // 0 when running=false
      "name": "Visual Studio Code",
      "bundle_id": "com.microsoft.VSCode",  // macOS bundle id, Win32 .exe path, or Linux desktop file id
      "running": true,
      "active": false,                       // true for the system-frontmost app (only one at a time)
      "kind": "desktop",                    // "desktop" | "uwp" | null
      "launch_path": "/Applications/Visual Studio Code.app",
      "last_used": "2026-05-15T12:34:56Z", // RFC3339, null if unreadable
      "windows": []                          // reserved — kept cheap; query list_windows for per-window state
    }
  ]
}
```

### Per-platform enumeration

- **macOS**: running set from `NSWorkspace` (via the existing
  AppleScript bridge), installed set from a filesystem scan of
  `/Applications`, `/Applications/Utilities`, `/System/Applications`,
  `/System/Applications/Utilities`, `~/Applications`. Bundle metadata
  read from each `.app/Contents/Info.plist` via `plutil`. Merged by
  bundle id; `launch_path` and `last_used` (bundle mtime) are
  backfilled onto running entries when the bundle id matches.
- **Windows**: running set from visible top-level windows
  (`EnumWindows` → owner pids) + `CreateToolhelp32Snapshot` for the
  pid→exe table. Installed set is the union of Start-Menu
  `.lnk` shortcuts (resolved via `IShellLinkW::GetPath`) and WinRT
  `Management::Deployment::PackageManager::FindPackagesWithPackageTypes(Main)`.
  Merged by exe basename. UWP entries carry
  `launch_path = "shell:appsFolder\\{PackageFamilyName}!App"`.
- **Linux**: running set from `/proc/<pid>/status` + `cmdline`.
  Installed set from XDG `.desktop` files in `$XDG_DATA_HOME/applications`
  and each `$XDG_DATA_DIRS` entry's `applications/` subdir. Entries
  with `NoDisplay=true`, `Hidden=true`, or `Type!=Application` are
  filtered. Merged by exe basename (after stripping `env`-style
  prefixes and `Exec=` field codes).

### Backwards compatibility

The unified shape is **additive**. `pid`, `name`, `bundle_id`,
`running`, `active` keep their pre-change positions and types.
The new keys (`launch_path`, `kind`, `last_used`, `windows`) are
new. The legacy `processes` key (Linux/Windows) remains as a thin
alias over the running subset so older callers that read the old
running-only shape keep working.

### Verification recipes

**macOS** (verified live):
```bash
cargo build --release -p cua-driver
./target/release/cua-driver call list_apps | python3 -c "
import json, sys
d = json.load(sys.stdin)
running = [a for a in d['apps'] if a['running']]
installed = [a for a in d['apps'] if not a['running']]
print(f'{len(d[\"apps\"])} total: {len(running)} running, {len(installed)} installed-only')
for a in d['apps'][:1]:
    for k in ('pid','name','bundle_id','running','active','kind','launch_path','last_used','windows'):
        assert k in a, f'missing field {k!r}'
print('shape OK')
"
```

**Windows** (cross-target check on macOS host succeeds; live run on a Windows host):
```powershell
cargo build --release -p cua-driver
.\target\release\cua-driver.exe call list_apps | ConvertFrom-Json | ForEach-Object {
    $_.apps | Group-Object kind | Format-Table Name, Count
}
```
Expect at least one `desktop` group (Start-Menu hits) and on Win10+
at least one `uwp` group (WinRT packages). Every entry should have
a `launch_path` that's either an absolute `.exe` path or
`shell:appsFolder\...`.

**Linux**:
```bash
cargo build --release -p cua-driver
./target/release/cua-driver call list_apps | jq '.apps | group_by(.running) | map({running: .[0].running, n: length})'
```
Expect two groups: one with `running: true` (live processes that
matched a `.desktop` launcher), one with `running: false` (the
installed-but-not-running tail).

---

## MCP tool: `list_windows`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ListWindowsTool.swift:6-245`
- Rust:
  - macOS=`crates/platform-macos/src/tools/list_windows.rs` (TBD audit)
  - windows=`crates/platform-windows/src/tools/impl_.rs` (ListWindowsTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ListWindowsTool, blocked behind Linux compile fix)
- Status:
  - windows: VERIFIED (EnumWindows-first enumeration; UIA contributes missing HWNDs only)
  - macOS: OPEN (audit pending — macOS port already exists)
  - linux: OPEN
- Test: `crates/platform-windows/examples/list_windows_parity.rs`

### Enumeration source (Windows)

`crate::win32::list_windows` walks `EnumWindows` first — that's the Win32
window manager's canonical top-to-bottom z-order, so it's the authoritative
source for both window membership and ordering. It then asks UI Automation
for any top-level windows EnumWindows missed
(`AutomationElement::RootElement.FindAll(TreeScope::Children, TrueCondition)`,
filtered to `IsOffscreen == false` + non-empty `GetWindowTextW`) and appends
those at the end of the merged list, deduped by HWND. UIA elements contribute
their `NativeWindowHandle` as the canonical HWND so downstream code keyed on
`(pid, window_id)` keeps working unchanged.

Why UIA at all: modern apps (WebView2-hosted Notepad, packaged-UWP frames,
some Electron containers) sometimes hide their visible window inside a host
HWND that `EnumWindows` either skips or surfaces with the wrong title/bounds.
UIA's desktop-children walk surfaces the real interactable window — but only
when EnumWindows didn't.

Why EnumWindows-first (not UIA-first): UIA's `FindAll(TreeScope::Children)`
gives no z-order guarantee; using it as the primary source would produce
non-deterministic ordering. EnumWindows order IS the Win32 z-order.

`filter_pid` is applied to the merged list, so a UWP app's pid that
previously returned empty now returns its real window.

### Fixed divergences (Windows)

Swift returns per-record: `{window_id, pid, app_name, title, bounds {x,y,width,height}, layer, z_index, is_on_screen, on_current_space?, space_ids?}`
plus top-level `{windows, current_space_id}`. Windows Rust was returning
a flat `{window_id, pid, title, x, y, width, height}`. Now matches Swift:

1. **`app_name`** — populated by joining against the process table
   (`crate::win32::list_processes`).
2. **`bounds: {x, y, width, height}`** — was flat `x`, `y`, `width`,
   `height` siblings of `title`.
3. **`layer: 0`** — Swift filters to layer-0 (normal windows); Windows
   has no layer concept so hard-coded 0.
4. **`z_index`** — derived from `EnumWindows` order (top-to-bottom),
   inverted so higher = closer to front per Swift convention.
5. **`is_on_screen: true`** — currently always true because the Win32
   `list_windows` source filter only returns `IsWindowVisible && !IsIconic`
   windows.  See limitation below.
6. **Top-level `current_space_id: null`** — Windows has no Spaces.
7. **`on_current_space` / `space_ids` omitted** — matching Swift's
   else-branch when SkyLight SPIs are unavailable; the text header
   explicitly says so.
8. **Text format** — header now reads `"✅ Found N window(s) across M
   app(s); X on-screen. (SkyLight Space SPIs unavailable — ...)"`.
   Per-record line: `"- {app_name} (pid {pid}) {"title"|(no title)} [window_id: {id}]{[off-screen]?}"`.
9. **Pid-filter warning** — when a pid filter returns zero windows,
   the response is `"⚠️ No windows found for pid X. ..."` with a hint
   about the current frontmost app, matching Swift's warning.
10. **Description** — copied from Swift with Windows-specific caveats
    about Spaces.

### Legacy alias

`structuredContent._legacy_windows` keeps the old flat shape (no `app_name`,
flat `x/y/width/height`) for any pre-existing callers; remove once they migrate.

### Known limitation

Windows Rust does **not** yet enumerate **off-screen / minimized windows**.
Swift's default (`on_screen_only: false`) returns them; Windows currently
filters them out at both enumeration sources (UIA's `IsOffscreen` flag and
`EnumWindows`'s `IsWindowVisible && !IsIconic` callback). The
`on_screen_only` schema field is accepted but has no effect. Follow-up to
refactor `list_windows` to return everything and filter at the tool layer.

---

## MCP tool: `click`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ClickTool.swift:29-595`
- Rust:
  - macOS=`crates/platform-macos/src/tools/click.rs` (focus-suppression wrap VERIFIED; full audit pending)
  - windows=`crates/platform-windows/src/tools/impl_.rs` (ClickTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ClickTool)
- Status:
  - windows: VERIFIED (text format + error wording); schema divergences documented below
  - macOS: VERIFIED (focus-suppression wrap — see [Per-action focus suppression](#per-action-focus-suppression)); full line-by-line audit pending
  - linux: OPEN
- Test: `crates/platform-windows/examples/click_parity.rs`

### Fixed (Windows)

1. **Text format — pixel path**: was `"✅ Clicked at (X.X, Y.Y) × N."`;
   now `"✅ Posted click to pid X."`, `"✅ Posted double-click to pid X."`,
   `"✅ Posted triple-click to pid X."` matching Swift's `performPixelClick`.
2. **Text format — element path**: was `"✅ Clicked element [N] at screen (X,Y)."`;
   now `"✅ Performed {action} on [N] (screen (X,Y))."` matching Swift's
   `"✅ Performed AXPress on [N] {role} \"{title}\"."` shape (UIA has no
   readily-available role/name on the cached element, so we emit
   element_index + screen coords for traceability; future work: populate
   the UIA cache with name/control-type and include them).
3. **Missing-target error wording** — Swift: `"Provide element_index or
   (x, y) to address the click target."` — Windows previously said
   `"Provide element_index or (x + y). pid is always required."` Now
   matches Swift verbatim.
4. **Description** — multi-paragraph, ported from Swift, with explicit
   notes about Windows-only fields and missing fields.

### Intentional Rust-only schema divergences

Swift's `click` takes `{action: enum, modifier: [string], debug_image_out: string}`;
Windows's `click` takes `{button: enum}` instead.  Rationale:

- **`button: left|right|middle`** — Windows convenience.  Swift exposes
  right-click as a separate `right_click` tool (which we also have as
  `right_click`), so `button: right` overlaps that tool.  Windows
  keeps both shapes; the standalone tool matches Swift's per-tool
  decomposition while `button` gives single-call flexibility.
- **`action: enum`** — AX-specific (AXPress / AXShowMenu / AXPick /
  AXConfirm / AXCancel / AXOpen).  UIA has no clean 1:1 mapping (Invoke
  pattern handles most cases; ShowMenu doesn't exist as a UIA pattern).
  Not exposed on Windows yet; a future Windows port could map a subset
  to UIA patterns (Invoke ≈ press, but show_menu has no analogue).
- **`modifier: [string]`** — not yet implemented on Windows; UIA's
  background-click via PostMessage doesn't propagate modifier-key state
  cleanly (would require synthesizing `WM_KEYDOWN(VK_CONTROL)` first).
  Follow-up.
- **`debug_image_out`** — also follow-up.

### Verified on Windows

`click_parity.exe` against Chrome (pid 62156, window_id 4464038):
- Missing-target: `"Provide element_index or (x, y) to address the click target."` ✓
- Pixel click: `"✅ Posted click to pid 62156."` ✓
- Double-click (`count: 2`): `"✅ Posted double-click to pid 62156."` ✓

---

## MCP tool: `launch_app`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/LaunchAppTool.swift:6-490`
- Rust:
  - windows=`crates/platform-windows/src/tools/impl_.rs` (LaunchAppTool)
  - macOS=`crates/platform-macos/src/tools/launch_app.rs` (full focus-steal contract)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (TBD audit)
- Status:
  - windows: VERIFIED for Win32 path; UWP path requires interactive session
    (returns descriptive error in Session 0 — never hangs). Win32 launches
    use `ShellExecuteExW` + `SW_SHOWNOACTIVATE` (no focus steal, matches
    macOS oapp) AND now schedule a best-effort polling
    `GetForegroundWindow`/`SetForegroundWindow` restore (≤3s, 100ms cadence)
    that flips the user's prior foreground back if the spawned app
    activates. URLs-only invocations skip the restore (the user explicitly
    asked the default browser to come up with that page). UWP launches use
    `IApplicationActivationManager` + best-effort `GetForegroundWindow`
    snapshot/restore (best-effort because `SetForegroundWindow` is subject
    to Windows' foreground-lock restrictions — visual confirmation in
    Session 1+ recommended).
  - macOS: VERIFIED (full focus-steal contract — see [Focus-steal prevention](#focus-steal-prevention))
  - linux: OPEN
- Tests:
  - windows=`crates/platform-windows/examples/launch_app_parity.rs` (accepts
    Session-0 fast-fail for UWP path the same way it accepts "not installed")
  - macOS=`tests/integration/test_focus_steal_parity.py` + `crates/platform-macos/src/focus_steal.rs` (Rust unit tests)

### Fixed (Windows)

1. **pid capture** — was using `ShellExecuteW` which returns only an
   HINSTANCE that's useless for pid lookup.  Now uses
   `ShellExecuteExW` with `SEE_MASK_NOCLOSEPROCESS` so we read the
   spawned process handle and call `GetProcessId` → real pid in the
   response, matching Swift's `AppLauncher.launch.info.pid`.
2. **Structured response** — was missing entirely.  Now returns
   `{pid, bundle_id, name, running, active, windows}` matching Swift's
   `LaunchResult` shape exactly.  `bundle_id` is null on Windows.
3. **Text format** — was `"Launched 'X' (no focus steal)."`; now
   `"✅ Launched <name> (pid <N>) in background."` + a `Windows:` block
   listing per-window `"- <title|(no title)> [window_id: ID]"` lines
   and a `→ Call get_window_state(...)` hint, matching Swift verbatim.
4. **`bundle_id` parameter** — accepted as an alias for `name` (Windows
   has no bundle-identifier concept), with one extra meaning on Win11:
   when the value matches an AUMID (`{PackageFamilyName}!{ApplicationId}`,
   contains `!`), `launch_app` routes through
   `IApplicationActivationManager::ActivateApplication` and returns
   the real packaged-app pid. Without this routing, Win11 launches of
   built-in apps (Notepad, Calculator, Paint, …) return the pid of
   the ~7 KB System32 stub that exits within milliseconds — useless
   for `list_windows`, `get_window_state`, etc.
4a. **`name` → packaged-app lookup (Win11)** — `name` is now first
    resolved against `shell:AppsFolder` (the Start Menu's "all apps"
    index, cached for the lifetime of the driver process). On a hit
    the lookup yields an AUMID and goes through the packaged path
    (see #4); on a miss it falls back to `ShellExecuteExW`'s PATH
    search. `.exe` suffix is stripped before matching so `"notepad"`
    and `"notepad.exe"` both resolve to the packaged Notepad.
4b. **`aumid` parameter** — optional explicit AUMID; cleaner than
    overloading `bundle_id` when the caller has the AUMID in hand.
    Takes precedence over `bundle_id`/`name`.
5. **`additional_arguments`** — honored, passed as `lpParameters` to
   `ShellExecuteExW` (Win32 path) or as the activation `arguments`
   string to `ActivateApplication` (packaged path).
6. **Window-resolution retry** — ports Swift's 5-attempt 100/200ms
   retry to absorb Win32 window-creation lag after `ShellExecuteEx`
   returns.
7. **Error wording** — `"Provide either bundle_id or name to identify
   the app to launch."` matches Swift's `errorResult` text.
8. **`active: false`** — hardcoded; `SW_SHOWNOACTIVATE` is the
   Windows-equivalent of Swift's background-launch invariant.
9. **Description** — multi-paragraph port from Swift with explicit
   Windows-specific notes (path takes precedence; bundle_id alias).
10. **Polling foreground-restore for the Win32 path** — mirrors the macOS
    `FocusRestoreGuard`. `LaunchAppTool::invoke` captures
    `GetForegroundWindow()` before the launch dispatch and, for the
    `ShellExecuteExW` branch, spawns a tokio task that polls every 100ms
    (up to 3s) for "the spawned app actually grabbed foreground" and then
    flips the prior HWND back via `SetForegroundWindow`. The UWP/AUMID
    branch keeps its existing synchronous restore in
    `launch_uwp::restore_foreground_best_effort` — the polling task is
    gated on the Win32 branch to avoid double-restoring.

    URLs-only invocations (`{urls: [...]}` with no app-identifying field)
    skip the restore: the user asked for that page to come up in the
    default browser, restoring would hide it. The decision is a pure
    function (`should_restore_foreground_after_launch`) with unit
    coverage in `launch_focus_restore_decision_tests`.

    `SetForegroundWindow` from non-UIAccess processes is restricted by
    Windows' foreground-lock; failures are logged at `tracing::trace!`
    and not surfaced — the launch itself already succeeded.

### Intentional Rust-only fields accepted (no-op)

- `electron_debugging_port`, `webkit_inspector_port`,
  `creates_new_application_instance` — Swift-specific.  Accepted in
  the schema so cross-platform callers can pass them; currently
  no-ops on Windows. Documented as follow-up.

### Verified on Windows

`launch_app_parity.exe` launches Notepad:
- Header `"✅ Launched notepad.exe (pid 30612) in background."` ✓
- `structuredContent.pid` is the actual ShellExecuteEx pid (Win10)
  or the real packaged-app pid (Win11, after AppsFolder lookup) ✓
- `bundle_id: null` on Win10 / unpackaged Notepad; AUMID string on
  Win11 packaged Notepad ✓
- `running: true`, `active: false` ✓
- Notepad killed on test exit. ✓
- On Win11 the parity binary additionally exercises an explicit-AUMID
  launch via `bundle_id="Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"`
  and asserts `bundle_id` round-trips identically. ✓

### Fixed (macOS)

1. **No more shell-out** — `apps::launch_app`, `launch_app_by_name`, and
   `launch_with_urls_*` no longer `Command::new("open")`. All paths now
   call `apps::nsworkspace::open_application` /
   `open_urls_with_application` directly via objc2-app-kit's
   `NSWorkspace.openApplication(at:configuration:completionHandler:)`
   (no-URL) and `open(_:withApplicationAt:configuration:)` (URL-handoff)
   variants. Matches Swift `AppLauncher.swift:106-131` byte-for-byte in
   the launch semantics.
2. **`activates = false` + `addsToRecentItems = false`** — set on every
   launch via `NSWorkspaceOpenConfiguration`. Mirrors Swift
   `AppLauncher.swift:65-66`.
3. **`oapp` AppleEvent descriptor on the no-URL path** — hand-rolled
   `extern_methods!` block in `apps/nsworkspace.rs` binds
   `NSAppleEventDescriptor.init(eventClass:eventID:targetDescriptor:returnID:transactionID:)`
   (not exposed by objc2-foundation 0.2.2). Without this, cold-launches
   of state-restored apps (Calculator-class) are windowless. Matches
   Swift `AppLauncher.swift:85-103`.
4. **3-phase focus-steal wrap** — `LaunchAppTool::invoke` captures the
   prior frontmost pid, arms a wildcard suppression entry, launches,
   swaps to a targeted entry (with overlap, not drop-then-begin, to
   avoid the hoang17 PR #1521 race), sleeps 500ms, drops the lease,
   and belt-and-braces re-activates the prior frontmost if the target
   is still on top. Matches Swift `LaunchAppTool.swift:181-281`.
5. **Direct pid from completion handler** — the `NSRunningApplication`
   returned by `openApplication` is used directly; no `list_running_apps`
   scan-and-match race. Matches Swift.
6. **Window-resolution retry** — same 5-attempt 100ms retry as before;
   unchanged.

### Verified on macOS

`tests/integration/test_focus_steal_parity.py` covers:
- Passive app launch (Calculator) — frontmost unchanged ✓
- Self-activating app launch (Safari) — frontmost restored within 1.5s ✓
- Launch with `urls=["about:blank"]` — frontmost preserved ✓
- Cold launch creates a window (verifies the `oapp` AppleEvent) ✓
- Back-to-back launches don't leak suppression state across calls ✓
- 5s deadline reaper evicts leaked entries ✓ (Rust unit test
  `focus_steal::tests::deadline_reaps_leaked_entry`)

---

## MCP tool: `press_key`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/PressKeyTool.swift:20-202`
- Rust:
  - windows=`crates/platform-windows/src/tools/impl_.rs` (PressKeyTool)
  - macOS=`crates/platform-macos/src/tools/press_key.rs` (focus-suppression wrap VERIFIED; full audit pending)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (TBD)
- Status: windows VERIFIED; macOS VERIFIED (focus-suppression wrap — see [Per-action focus suppression](#per-action-focus-suppression)); linux OPEN
- Test: `crates/platform-windows/examples/press_key_parity.rs`

### Fixed (Windows)

1. **Text format** — was `"Pressed key 'KEY'."`; now matches Swift verbatim
   `"✅ Pressed KEY on pid X."` (no quotes around key, pid included).
2. **Error wording** —
   - Missing pid: `"Missing required integer field pid."` (was generic)
   - Missing key: `"Missing required string field key."` (was generic)
   - element_index without window_id: `"window_id is required when
     element_index is used — the element_index cache is scoped per
     (pid, window_id). Pass the same window_id you used in
     get_window_state."` (was previously not validated; added the
     same Swift guard).
3. **Schema** — added `element_index` field (accepted; currently no-op
   on Windows since UIA SetFocus isn't wired up yet — documented).
4. **Description** — multi-paragraph port from Swift, including the
   full key vocabulary list.

### Intentional Rust-only (Windows)

- `element_index` is accepted but currently no-op (Swift focuses the
  element via `AXSetAttribute(kAXFocused, true)` first; Windows UIA
  `IUIAutomationElement::SetFocus` exists but is not yet wired up
  here).  Documented as a follow-up; using `press_key` without
  `element_index` already works for scroll keys / shortcuts on the
  already-focused element.

### Verified on Windows

`press_key_parity.exe` against Chrome:
- Missing-key error: `"Missing required string field key."` ✓
- Element-without-window error: matches Swift's wording ✓
- End key: `"✅ Pressed end on pid 85676."` ✓

---

## MCP tool: `hotkey`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/HotkeyTool.swift:6-142`
- Rust:
  - windows=`crates/platform-windows/src/tools/impl_.rs` (HotkeyTool)
  - macOS=`crates/platform-macos/src/tools/hotkey.rs` (focus-suppression wrap VERIFIED; full audit pending)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (TBD)
- Status: windows VERIFIED; macOS VERIFIED (focus-suppression wrap — see [Per-action focus suppression](#per-action-focus-suppression)); linux OPEN
- Test: `crates/platform-windows/examples/hotkey_parity.rs`

### Fixed (Windows)

1. **Text format** — was `"Pressed CTRL+C on pid X."` (no checkmark,
   uppercase mods only); now `"✅ Pressed K1+K2+... on pid X."`
   matching Swift's `keys.joined(separator: "+")` of the full keys
   array as the caller supplied it (preserves case and order).
2. **Error wording** —
   - Missing pid: schema-layer catches first; tool layer falls back to
     `"Missing required integer field pid."` (Swift's wording).
   - Missing keys: `"Missing required array field keys."` (was
     a Rust-specific "Provide 'keys' array..." fallback).
   - Non-string elements: `"keys must be a non-empty array of strings."`.
3. **Description** — multi-paragraph port from Swift, dropping the
   macOS-specific FocusWithoutRaise / NSMenu mechanics that have no
   Windows analogue.

### Verified on Windows

`hotkey_parity.exe` against Chrome:
- Missing-pid / missing-keys errors ✓
- `ctrl+end` → `"✅ Pressed ctrl+end on pid 85676."` ✓

---

## MCP tool: `double_click`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/DoubleClickTool.swift:28-327`
- Rust:
  - windows=`crates/platform-windows/src/tools/impl_.rs` (DoubleClickTool)
  - macOS / linux: OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/double_click_parity.rs`

### Fixed (Windows)

1. **Text format pixel path** — was `"✅ Double-clicked at (X.X, Y.Y)."`;
   now `"✅ Posted double-click to pid X at window-pixel (a, b) → screen-point (c, d)."`
   matching Swift verbatim.
2. **Text format element path** — was `"✅ Double-clicked element [N] at screen (X,Y)."`;
   now `"✅ Posted double-click to [N] at screen-point (X, Y)."` matching
   Swift's pixel-fallback element wording.  (UIA role/title placeholder
   pending element-cache enrichment.)
3. **Validation guards added** — ports Swift's full set:
   - `"Provide both x and y together, not just one."`
   - `"Provide either element_index or (x, y), not both."`
   - `"Provide element_index or (x, y) to address the double-click target."`
   - `"window_id is required when element_index is used — the element_index
      cache is scoped per (pid, window_id). Pass the same window_id you used
      in get_window_state."`
4. **Error wording for missing pid** — `"Missing required integer field pid."`
   matches Swift (schema-layer catches this first; tool-layer fallback uses
   Swift wording).
5. **`modifier` schema field** — accepted for parity (no-op on Windows;
   PostMessage doesn't propagate modifier-key state — same caveat as
   `click`).
6. **Multi-paragraph description** ported from Swift with Windows-specific
   notes (no AXOpen analogue; element path always falls back to pixel
   double-click — user-visible behavior matches Swift's AXOpen-not-advertised
   fallback).

### Verified on Windows

`double_click_parity.exe` against Chrome:
- 4 error paths (missing-target, partial-xy, both-modes,
  element-without-window) ✓
- Pixel double-click: `"✅ Posted double-click to pid 85676 at
  window-pixel (300, 300) → screen-point (462, 456)."` ✓

---

## MCP tool: `right_click`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/RightClickTool.swift:27-324`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (RightClickTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/right_click_parity.rs`

### Fixed (Windows)
1. **Pixel-path text** — was `"✅ Right-clicked at (X.X, Y.Y)."`; now
   `"✅ Posted right-click to pid X at window-pixel (a, b) → screen-point (c, d)."`
   matching Swift.
2. **Element-path text** — was `"✅ Right-clicked element [N] at screen (X,Y)."`;
   now `"✅ Shown menu for [N] (screen (X, Y))."` matching Swift's
   AXShowMenu text (UIA role/title placeholder pending element-cache enrichment).
3. **Validation** — ports Swift's 5 guards verbatim (missing pid,
   partial xy, both modes, missing target, element-without-window).
4. **Description** — multi-paragraph port from Swift with Windows-
   specific caveats (no AXShowMenu analogue wired up; element path
   falls through to pixel recipe — matches Swift's
   AXShowMenu-not-advertised fallback).

### Verified on Windows
`right_click_parity.exe` against Chrome:
- 4 error paths ✓
- Pixel right-click: `"✅ Posted right-click to pid 62156 at window-pixel
  (300, 300) → screen-point (301, 368)."` ✓

---

## MCP tool: `screenshot`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ScreenshotTool.swift:5-170`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (ScreenshotTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/screenshot_parity.rs`

### Fixed (Windows)
1. **Text format** — was `"Screenshot (window): WxH png."`; now matches
   Swift verbatim: `"✅ Window screenshot — WxH png [window_id: ID]"`
   (em-dash, checkmark, window-id suffix). Display fallback uses
   `"✅ Display screenshot — WxH png"` (Rust-only, see intentional below).
2. **Description** — multi-paragraph port from Swift adapted to Windows
   (BitBlt + PrintWindow transport; no permission gate needed).
3. **`idempotent`** — was `true`; Swift uses `false` (a fresh pixel grab
   every call). Now matches Swift.
4. **`max_image_dimension` default** — was 0 (no cap), Swift uses 1568.
   Now 1568 on all 3 Rust platforms (matches
   `CuaDriverConfig.defaultMaxImageDimension`). The 0 default was
   producing 10MB screenshots on the Windows VM; 1568 caps the long
   edge before encoding.

### Intentional Rust-only

- **Optional `window_id`** — Swift requires it; Windows allows omission
  for whole-display capture (`screenshot_display_bytes`).  Useful
  Windows-only convenience that Swift can't easily provide because
  macOS Screen Recording requires per-window grants.  Schema accepts
  both shapes; description explains.
- **Default `format`** — Swift defaults `png`; all 3 Rust platforms now
  default `jpeg`. Rationale: agents typically want compact images for
  vision-model context windows; PNG is lossless but multi-MB on screen
  content. Schema still accepts both; callers wanting PNG pass
  `{"format":"png"}`. Swift may follow; tracked as a follow-up parity
  question.
- **Default JPEG `quality`** — Swift defaults 95; Rust defaults 85
  (already Linux's default and the macOS Claude-Code-compat tool's
  default). 85 is the typical sweet spot for screen content. Diverges
  from Swift only when both sides actually emit JPEG.

### Verified on Windows

`screenshot_parity.exe`:
- Window screenshot: text matches `"✅ Window screenshot — 1087x644 png
  [window_id: 4464038]"` ✓
- Image content block present ✓
- structuredContent has `width`, `height`, `format` ✓
- JPEG format: `mimeType: image/jpeg`, `format: "jpeg"` ✓

---

## MCP tool: `scroll`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ScrollTool.swift:23-211`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (ScrollTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/scroll_parity.rs`

### Fixed (Windows)
1. **Text format** — was `"Scrolled DIR Nx GRAN."`; now matches Swift
   shape `"✅ Scrolled pid X DIR via Nx SB_LINEDOWN message(s)."` (Swift uses key
   names; Rust uses Win32 SB_* constants since the actual transport is
   WM_VSCROLL/WM_HSCROLL, not keystrokes — text reflects mechanism).
2. **Error wording** — `"Missing required integer field pid."` /
   `"Missing required string field direction."` matches Swift.
3. **Element-without-window guard** added — same wording as Swift.
4. **Description** — multi-paragraph port from Swift with Windows-
   specific note about WM_VSCROLL/WM_HSCROLL vs Swift's keystroke
   approach.
5. **`element_index`** — accepted; currently no-op on Windows (UIA
   SetFocus not wired up).

### Intentional Rust-only

- **Transport**: Windows uses `WM_VSCROLL`/`WM_HSCROLL` messages with
  `SB_LINE*`/`SB_PAGE*` codes. Swift uses synthesized keystrokes
  (PageDown/arrow keys) via auth-signed SLEventPostToPid because
  CGEventCreateScrollWheelEvent2 is dropped by Chromium on macOS.
  Windows doesn't have that constraint; scroll-message events work
  reliably background.

### Verified on Windows

`scroll_parity.exe` against Chrome:
- Missing-direction error ✓
- Element-without-window error ✓
- Page-down × 2: `"✅ Scrolled pid 62156 down via 2× SB_PAGEDOWN message(s)."` ✓

---

## MCP tool: `type_text`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/TypeTextTool.swift:13-225`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (TypeTextTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/type_text_parity.rs`

### Fixed (Windows)
1. **Text format** — was `"Typed N character(s)."`; now matches Swift's
   CGEvent-fallback shape `"✅ Typed N char(s) on pid X via PostMessage (Yms delay)."`
   (Swift says `CGEvent`; Windows says `PostMessage` since that's the
   actual transport).
2. **Error wording** — `"Missing required integer field pid."` /
   `"Missing required string field text."` matches Swift.
3. **Element-without-window guard** added with Swift's wording.
4. **`delay_ms` field** added (0–200, default 30, matches Swift).
5. **Description** — multi-paragraph port from Swift adapted to the
   PostMessage transport.

### Intentional Rust-only

- **No AX-fast-path attempt**: Swift tries `AXSetAttribute(kAXSelectedText)`
  first for bulk insert, falls back to CGEvent when AX rejects.  Windows
  always takes the character-by-character path (no UIA equivalent of
  AXSelectedText wired up yet).  User-visible behavior matches Swift's
  fallback path, just without the fast-path optimization.
- **`element_index`** accepted; no-op on Windows (no UIA SetFocus path).

### Verified on Windows
`type_text_parity.exe` against Chrome:
- Missing-text error ✓
- Element-without-window error ✓
- 5-char type: `"✅ Typed 5 char(s) on pid 62156 via PostMessage (10ms delay)."` ✓

---

## MCP tool: `set_value`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/SetValueTool.swift:8-336`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (SetValueTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/set_value_parity.rs`

### Fixed (Windows)
1. **Text format** — was `"Set value of element N."`; now matches Swift's
   default-write shape `"✅ Set AXValue on [N] (UIA ValuePattern)."` (UIA
   role/title placeholder pending element-cache enrichment).
2. **`idempotent: true`** — was `false`; Swift uses `true` (setting same
   value twice is idempotent).
3. **Error wording** — Swift's "Missing required integer fields pid,
   window_id, and element_index." for the all-ints-missing case;
   schema-layer wording for individual missing fields.
4. **Description** — multi-paragraph port from Swift adapted to UIA
   ValuePattern transport, with note about Swift's AXPopUpButton
   special-case (UIA SelectionPattern analogue not yet wired up).

### Intentional Rust-only

- **No AXPopUpButton special-case** yet — Swift specifically iterates
  AX children to AXPress the matching option (bypassing the native
  popup menu).  UIA has `IUIAutomationSelectionPattern` /
  `IUIAutomationSelectionItemPattern` which would be the equivalent;
  documented as a follow-up.  For now `set_value` on a combo box
  delegates to `IUIAutomationValuePattern::SetValue` which works on
  most native ComboBoxes.

### Verified on Windows
`set_value_parity.exe`:
- Missing-value error (schema layer): mentions `value` + `required` ✓
- Cache-miss error: `"Element 99999 not in cache."` ✓

---

## MCP tools: `get_config` + `set_config`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/GetConfigTool.swift:13-79`
  + `libs/cua-driver/Sources/CuaDriverServer/Tools/SetConfigTool.swift:25-167`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (GetConfigTool, SetConfigTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/config_parity.rs`

### Fixed (Windows)
1. **get_config text** — was `"cua-driver-rs configuration"`; now matches
   Swift's `"✅ <pretty JSON>"` format with `schema_version`, `version`,
   `platform`, `capture_mode`, `max_image_dimension`.
2. **set_config schema** — now accepts Swift's `{key: <dotted-path>, value: <json>}`
   shape AND keeps the legacy per-field shape.  Unknown keys return
   `"Unknown config key 'X'. Known: capture_mode, max_image_dimension."`.
3. **set_config response** — was `"Config updated: ..."`; now echoes the
   full updated config in the same pretty-JSON `"✅ {...}"` format as
   `get_config`, matching Swift's "echo full config after write" pattern.
4. **Descriptions** — both ported from Swift with Windows-specific
   schema notes.

### Intentional Rust-only

- **No `agent_cursor.*` subtree** in the Windows config struct.  Swift
  exposes `agent_cursor.enabled` + `agent_cursor.motion.*` (start_handle,
  end_handle, arc_size, arc_flow, spring) as persistent config; Windows
  currently has only `capture_mode` and `max_image_dimension`.  Cursor
  config lives separately in `cursor-overlay` crate config and is set
  via CLI flags, not this tool yet.  Documented as a follow-up.

### Verified on Windows
`config_parity.exe`:
- get_config: `"✅ {<pretty JSON>}"` with all keys ✓
- set_config {key, value}: capture_mode=vision applied ✓
- set_config legacy: capture_mode=som + max_image_dimension=1024 applied ✓
- Unknown key error ✓
- Missing-input error ✓

---

## MCP tool: `get_agent_cursor_state`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/GetAgentCursorStateTool.swift:9-68`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (GetAgentCursorStateTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/get_agent_cursor_state_parity.rs`

### Fixed (Windows)
1. **Text format** — was `"N cursor instance(s)."`; now matches Swift's
   single-line camelCase output:
   `"✅ cursor: enabled=true startHandle=0.3 endHandle=0.3 arcSize=0.25
   arcFlow=0 spring=0.72 glideDurationMs=0 dwellAfterClickMs=80 idleHideMs=20000"`
2. **`current_motion()`** helper added to `overlay.rs` — mirrors macOS
   `current_motion()` so the tool can snapshot live motion values.
3. **Description** — ported from Swift verbatim.

### Intentional Rust-only

- **Multi-cursor `cursors` array** — Rust supports multiple cursor
  instances via `CursorRegistry`; Swift has a single `AgentCursor.shared`.
  Included in `structuredContent` as an extra field; text format keeps
  Swift's single-cursor vocabulary.

### Verified on Windows
`get_agent_cursor_state_parity.exe`:
- Text: `"✅ cursor: enabled=true startHandle=0.3 endHandle=0.3 arcSize=0.25
  arcFlow=0 spring=0.72 glideDurationMs=0 dwellAfterClickMs=80 idleHideMs=20000"` ✓
- structuredContent has all 9 fields + `cursors` array ✓

---

## MCP tools: `set_agent_cursor_enabled` + `set_agent_cursor_motion`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/SetAgentCursorEnabledTool.swift:8-85`
  + `libs/cua-driver/Sources/CuaDriverServer/Tools/SetAgentCursorMotionTool.swift:11-187`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs`; macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/agent_cursor_setters_parity.rs`

### Fixed (Windows)
1. **set_agent_cursor_enabled text** — was `"Agent cursor 'default' enabled."`;
   now matches Swift verbatim `"✅ Agent cursor enabled."` (or `"disabled"`).
2. **set_agent_cursor_enabled error** — was `"Missing required parameter: enabled"`;
   now Swift's `"Missing required boolean field \`enabled\`."`.
3. **set_agent_cursor_motion was silently dropping all motion knobs** —
   tool accepted them in schema but only forwarded appearance fields to
   the cursor registry.  Now applies each motion knob via
   `MotionConfig::with_overrides()` and sends `OverlayCommand::SetMotion`
   to the live render state — matches Swift's
   `AgentCursor.shared.defaultMotionOptions = opts`.
4. **set_agent_cursor_motion text** — was `"Cursor 'default' config updated."`;
   now matches Swift's `"✅ cursor motion: startHandle=X endHandle=Y arcSize=Z arcFlow=W spring=S glideDurationMs=N dwellAfterClickMs=N idleHideMs=N"`.
5. **Number coercion** — JSON ints (`{"glide_duration_ms": 500}`) now
   coerced to f64 instead of silently ignored (mirrors Swift's `number()`).
6. **Descriptions** — both ported from Swift; appearance/motion split
   documented (Rust splits appearance into the separate
   `set_agent_cursor_style` tool, matching Swift's
   SetAgentCursorStyleTool surface).

### Intentional Rust-only
- **`cursor_id`** parameter for both tools — selects an instance from
  the Rust-only multi-cursor registry.  Swift has a single
  `AgentCursor.shared`.

### Verified on Windows
`agent_cursor_setters_parity.exe`:
- Missing-enabled error ✓
- `enabled: true` → `"✅ Agent cursor enabled."` ✓
- `enabled: false` → `"✅ Agent cursor disabled."` ✓
- Motion update: `"✅ cursor motion: startHandle=0.4 endHandle=0.3 arcSize=0.3
  arcFlow=0 spring=0.8 glideDurationMs=500 dwellAfterClickMs=80 idleHideMs=20000"` ✓

---

## MCP tools: `get_recording_state` + `set_recording`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/GetRecordingStateTool.swift:11-72`
  + `libs/cua-driver/Sources/CuaDriverServer/Tools/SetRecordingTool.swift:12-160`
- Rust: shared `crates/mcp-server/src/recording_tools.rs` (cross-platform)
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/recording_parity.rs`

### Fixed
1. **get_recording_state text** — was `"recording: enabled  output_dir=X  next_turn=N"`
   (double-space-separated); now Swift's `"✅ recording: enabled output_dir=X next_turn=N"`
   single-space. Disabled case: `"✅ recording: disabled"`.
2. **set_recording text** — was `"Recording enabled → X"` (Unicode arrow);
   now Swift's `"✅ Recording enabled -> X"` (ASCII arrow). Disabled:
   `"✅ Recording disabled."`.
3. **Error wording** — Swift's `"Missing required boolean field \`enabled\`."`
   and `"`output_dir` is required when enabling recording."`.
4. **`idempotent: true`** — was `false`; Swift uses `true`.
5. **Descriptions** — both ported from Swift verbatim with full turn-folder
   layout details.

### Verified on Windows
`recording_parity.exe`:
- `"✅ recording: disabled"` ✓
- Missing-enabled error ✓
- Missing-output_dir error ✓
- Enable: `"✅ Recording enabled -> <path>"` ✓
- `"✅ recording: enabled output_dir=<path> next_turn=1"` ✓
- Disable: `"✅ Recording disabled."` ✓

---

## CLI subcommand: `list-tools`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/CallCommand.swift:298-322`
- Rust: `libs/cua-driver/rust/crates/cua-driver/src/cli.rs::run_list_tools`
- Status: VERIFIED
- Test: `crates/platform-windows/examples/list_tools_parity.rs`

### Fixed
**Sort order** — Swift sorts tools alphabetically by name
(`tools.sorted(by: { $0.name < $1.name })`). Rust was iterating in
registration order (HashMap-backed Vec). Now sorts alphabetically
before printing, matching Swift's output byte-for-byte modulo per-tool
description content.

### Verified on Windows
`list_tools_parity.exe`:
- Names sorted ascending ✓
- All 23 core tools present (click, double_click, right_click, type_text,
  press_key, hotkey, scroll, screenshot, list_apps, list_windows,
  get_cursor_position, get_screen_size, launch_app, move_cursor,
  set_agent_cursor_enabled, set_agent_cursor_motion,
  get_agent_cursor_state, set_value, get_config, set_config,
  check_permissions, get_recording_state, set_recording) ✓
- Line shape `name: <first sentence>` or just `name` ✓

---

## CLI subcommand: `describe`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/CallCommand.swift:324-366`
  + `printUnknownTool:552`
- Rust: `libs/cua-driver/rust/crates/cua-driver/src/cli.rs::run_describe`
- Status: VERIFIED
- Test: `crates/platform-windows/examples/describe_parity.rs`

### Fixed
**Unknown-tool listing order** — Swift sorts available tools
alphabetically in the error stderr block. Rust was using
`tool_names()` which is registration order. Now sorts to match.
Exit code 64 (EX_USAGE) and known-tool output shape (`name: X`
+ `description:` + `input_schema:` pretty JSON) already matched
Swift.

### Verified on Windows
`describe_parity.exe`:
- `describe click`: stdout starts with `"name: click\n"`, contains
  `"description:"` + pretty-printed `"input_schema:"` ✓
- `describe this_tool_does_not_exist`: exit code 64, stderr lists
  31 tools alphabetically with `"Unknown tool:"` + `"Available tools:"`
  preamble ✓

---

## MCP tool: `get_window_state`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/GetWindowStateTool.swift:5-end`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (GetWindowStateTool); macOS/linux OPEN
- Status: windows VERIFIED (error wording + validation); response shape already verified
- Test: `crates/platform-windows/examples/get_window_state_parity.rs`

### Fixed (Windows)
1. **Error wording** — Swift's verbatim messages now used:
   - `"Missing required integer field pid."`
   - `"Missing required integer field window_id. Use `list_windows` to enumerate the target app's windows, or read `launch_app`'s `windows` array."`
2. **Window-belongs-to-pid validation** — Swift hard-errors when the
   window_id doesn't belong to pid. Windows now validates the same way:
   - Window doesn't exist anywhere: `"No window with window_id N exists.
     Call list_windows({pid: P}) for candidates."`
   - Window exists under a different pid: `"window_id N belongs to pid Q,
     not pid P. Call list_windows({pid: P}) to get this pid's own windows."`
3. **`idempotent: false`** — was `true`; Swift uses `false` (each call
   is a fresh snapshot).
4. **Description** — multi-paragraph port from Swift covering invariant,
   `capture_mode` semantics, `query` filter behavior, and Windows-specific
   notes (UIA instead of AX, no Spaces, no `javascript` / `screenshot_out_file`
   fields).

### Intentional Rust-only schema absences
- **`javascript`** — macOS-only AppleScript hook for Chromium/Safari.
- **`screenshot_out_file`** — could be added later; not currently
  implemented on Windows.

### Verified on Windows
`get_window_state_parity.exe`:
- Missing-pid error ✓
- Missing-window_id error (with Swift's full hint) ✓
- Mismatched pid+window_id: `"window_id N belongs to pid Q, not pid P..."` ✓
- Bogus window_id: `"No window with window_id N exists..."` ✓

---

## MCP tool: `drag`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/DragTool.swift:21-327`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (DragTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/drag_parity.rs`

### Fixed (Windows)
1. **Text format** — was `"✅ Posted drag (BUTTON) to pid P from (x,y) → (x,y) in Nms / Ssteps."`;
   now Swift's `"✅ Posted drag[ (BUTTON button)] to pid P from window-pixel (a, b) → (c, d), screen (e, f) → (g, h) in Nms / Ssteps."`
   (window-pixel + screen-coord pair; button suffix only for non-left).
2. **Missing-coordinates error** — was `"Missing: from_y"` (one field at a
   time); now Swift's `"from_x, from_y, to_x, and to_y are all required
   (window-local pixels)."` even if only one is missing.
3. **Missing-pid error** — `"Missing required integer field pid."`
   matches Swift.

### Intentional Rust-only
- Schema unchanged; all Swift fields supported. No divergences in shape.

### Verified on Windows
`drag_parity.exe`:
- Missing-coordinates error ✓
- Real drag: `"✅ Posted drag to pid 62156 from window-pixel (100, 100) → (102, 102), screen (101, 168) → (103, 170) in 50ms / 2 steps."` ✓

---

## MCP tool: `replay_trajectory`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ReplayTrajectoryTool.swift:18-end`
- Rust: shared `crates/mcp-server/src/recording_tools.rs` (cross-platform)
- Status: VERIFIED
- Test: `crates/platform-windows/examples/replay_trajectory_parity.rs`

### Fixed
1. **Error wording** — was `"Missing required parameter: dir"`; now Swift's
   `"Missing required string field \`dir\`."`. Empty-string `dir` now also
   rejected (was silently passing the empty path through).
2. **`open_world: false`** — was `true`; Swift uses `false` (replays only
   recorded actions, no fresh world interactions).
3. **Description** — multi-paragraph port from Swift covering caveats
   (element-index actions fail on replay; read-only tools not recorded;
   recording-during-replay deliberate).

### Verified on Windows
`replay_trajectory_parity.exe`:
- Empty-dir: `"Missing required string field \`dir\`."` ✓
- Nonexistent dir: `"Trajectory directory does not exist: <path>"` ✓

---

## CLI subcommand: `mcp` (TCC auto-relaunch / daemon proxy)
- Swift:
  - `libs/cua-driver/Sources/CuaDriverCLI/CuaDriverCommand.swift` —
    `MCPCommand`, `shouldUseDaemonProxy`, `runViaDaemonProxy`,
    `launchDaemonViaOpen`, `waitForDaemon`.
  - `libs/cua-driver/Sources/CuaDriverCLI/BundleHelpers.swift` —
    `isExecutableInsideCuaDriverApp()`.
  - `libs/cua-driver/Sources/CuaDriverServer/CuaDriverMCPServer.swift` —
    `makeProxy` (the actor that re-implements `ListTools` /
    `CallTool` over the daemon UDS).
- Rust:
  - `libs/cua-driver/rust/crates/cua-driver/src/bundle.rs` —
    `is_executable_inside_cuadriver_app`,
    `parent_is_not_launchd`, `is_env_truthy`.
  - `libs/cua-driver/rust/crates/cua-driver/src/cli.rs` —
    `should_use_daemon_proxy`, `launch_daemon_and_wait`,
    `run_mcp_via_daemon_proxy`.
  - `libs/cua-driver/rust/crates/cua-driver/src/proxy.rs` —
    `run_proxy` (the stdio loop forwarding `tools/list` and
    `tools/call` through the daemon socket).
  - `libs/cua-driver/rust/scripts/CuaDriverBundle/Contents/Info.plist` —
    skeleton (Info.plist + empty MacOS/) that CD assembles into the
    release-tarball `CuaDriver.app` that the auto-relaunch path lands
    in. Stored under a non-`.app` directory so LaunchServices on
    developer machines doesn't surface a ghost entry alongside the
    real install.
  - `libs/cua-driver/rust/scripts/install.sh` — drops the bundle to
    `/Applications/CuaDriver.app` and symlinks the bin into it.
- Status: implemented on macOS (issue #1525); smoke-tested manually
  before merge.

### Why this exists
When `cua-driver-rs mcp` is invoked from an IDE terminal (Claude
Code, Cursor, VS Code, Warp), macOS attributes the spawned process
to the parent terminal's TCC responsibility chain — *not* to
`com.trycua.driver`. AX probes against the process silently
fail because the user granted Accessibility to the bundle, not to
the IDE terminal. The Swift driver hit the same pathology and fixed
it in PR #1479; the Rust port hit it on the macOS GA flip path and
fixed it here. See issue #1525 for the full background.

### Bundle id divergence (intentional)
Swift `CuaDriver.app` → `com.trycua.driver`.
Rust `CuaDriver.app` → `com.trycua.driver`.
The two bundles coexist on disk and in TCC; a user can grant
Accessibility + Screen Recording to each independently. The Rust
port has its own bundle name + identifier so:
  - `open -n -g -a CuaDriverRs --args serve` never accidentally
    relaunches into the Swift bundle (and vice versa).
  - TCC grants are per-cdhash, so granting one doesn't carry into
    the other — users explicitly opt in to each binary.

### Escape hatches
- `--no-daemon-relaunch` flag — same flag Swift exposes.
- `CUA_DRIVER_RS_MCP_NO_RELAUNCH=1` env var — Rust-specific name
  (Swift uses `CUA_DRIVER_MCP_NO_RELAUNCH`).
- `--socket <path>` flag — override the daemon UDS path used by the
  proxy.
- `CUA_DRIVER_RS_MCP_FORCE_PROXY=1` env var (Rust-only) — force
  proxy mode without the bundle-context check. Useful when wrapping
  the binary in a custom .app, or for manual smoke-testing of the
  proxy path against a daemon you've already started by hand. Skips
  the `open -a` step entirely; caller must supply a daemon on
  `--socket`.

### Daemon protocol divergence
The daemon's `list` method now returns full `ToolDef`
(`input_schema` + annotation hints), not just `{name, description}`.
The proxy uses this to build a complete `tools/list` from one
round-trip instead of N+1 list+describe calls. Backwards compatible:
older clients that only read name/description still work.

### Manual smoke test (macOS)
1. `cua-driver serve --socket /tmp/test.sock &`
2. `CUA_DRIVER_RS_MCP_FORCE_PROXY=1 cua-driver mcp --socket /tmp/test.sock`
3. From an MCP client, run the standard initialize → tools/list →
   tools/call get_screen_size handshake. Expect identical envelope
   shape to the in-process path. Concretely:

   `tools/list` response (the daemon caches and returns it once at
   proxy startup — same shape as the in-process server's `tools/list`):

   ```json
   {
     "jsonrpc": "2.0",
     "id": 1,
     "result": {
       "tools": [
         { "name": "check_permissions",       "description": "…", "inputSchema": {…}, "annotations": {…} },
         { "name": "click",                   "description": "…", "inputSchema": {…}, "annotations": {…} },
         { "name": "double_click",            "…": "…" },
         { "name": "drag",                    "…": "…" },
         { "name": "get_accessibility_tree",  "…": "…" },
         { "name": "get_config",              "…": "…" },
         { "name": "get_cursor_position",     "…": "…" },
         { "name": "get_recording_state",     "…": "…" },
         { "name": "get_screen_size",         "…": "…" },
         { "name": "get_window_state",        "…": "…" },
         { "name": "hotkey",                  "…": "…" },
         { "name": "launch_app",              "…": "…" },
         { "name": "list_apps",               "…": "…" },
         { "name": "list_windows",            "…": "…" },
         { "name": "page",                    "…": "…" },
         { "name": "press_key",               "…": "…" },
         { "name": "replay_trajectory",       "…": "…" },
         { "name": "right_click",             "…": "…" },
         { "name": "screenshot",              "…": "…" },
         { "name": "scroll",                  "…": "…" },
         { "name": "set_config",              "…": "…" },
         { "name": "set_recording",           "…": "…" },
         { "name": "set_value",               "…": "…" },
         { "name": "type_text",               "…": "…" },
         { "name": "zoom",                    "…": "…" }
         // …plus the agent_cursor.* family when overlay is enabled.
         // For an exact snapshot run: `cua-driver list-tools`
       ]
     }
   }
   ```

   `tools/call get_screen_size` request + response:

   ```json
   // → stdin
   {"jsonrpc":"2.0","id":2,"method":"tools/call",
    "params":{"name":"get_screen_size","arguments":{}}}

   // ← stdout
   {"jsonrpc":"2.0","id":2,"result":{
     "content":[{"type":"text","text":"{\"width\":1920,\"height\":1080}"}],
     "structuredContent":{"width":1920,"height":1080},
     "isError":false
   }}
   ```

   The `result` envelope is identical to the in-process path —
   structuredContent + text mirror, no proxy-specific wrapping.

4. Without spawning the daemon first, repeat step 2. Expect
   non-zero exit and a "daemon not reachable" diagnostic on stderr
   (the fail-fast contract that matches Swift `makeProxy`). Exact
   stderr text emitted by `main.rs`'s proxy-error branch (wrapping
   `proxy::run_proxy`'s pre-check):

   ```
   cua-driver-rs: cua-driver-rs daemon not reachable on /tmp/test.sock. Start it with `open -n -g -a CuaDriverRs --args serve` and retry.
   ```

   Process exits with status `1` before reading any MCP request on
   stdin. With `CUA_DRIVER_RS_MCP_FORCE_PROXY=1` set, `cli.rs`'s
   `run_mcp_via_daemon_proxy` emits the more specific:

   ```
   cua-driver-rs: CUA_DRIVER_RS_MCP_FORCE_PROXY=1 but no daemon listening on /tmp/test.sock. Start one with `cua-driver serve --socket /tmp/test.sock` and retry.
   ```

---

## CLI subcommands: `status` + `stop`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/ServeCommand.swift:368-470`
- Rust: `libs/cua-driver/rust/crates/cua-driver/src/serve.rs::run_status_cmd, run_stop_cmd`
- Status: VERIFIED
- Test: `crates/platform-windows/examples/daemon_lifecycle_parity.rs`

### Fixed
**`stop` silent on success** — Rust was printing `"cua-driver daemon stopped."`
on stdout after a successful stop.  Swift's `stop` exits silently with
status 0.  Now matches Swift byte-for-byte.

### Already correct
- `status` output: `"cua-driver daemon is running\n  socket: <path>\n  pid: <N>\n"` ✓
- `status` exit code: 0 when running, 1 when not ✓
- `stop` exit code: 0 when ran, 1 when no daemon ✓
- Error wording on stderr: `"cua-driver daemon is not running"` ✓

### Verified on Windows
`daemon_lifecycle_parity.exe`:
- `status` running: 3-line output ✓
- `stop` running: silent stdout (was printing extra line) ✓
- `status` not-running: exit 1 + stderr message ✓
- `stop` not-running: exit 1 + stderr message ✓

---

## MCP tool alias: `type_text_chars` → `type_text`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/ToolRegistry.swift:55-70`
- Rust: `libs/cua-driver/rust/crates/cua-driver/src/serve.rs` (both pipe variants)
  + `libs/cua-driver/rust/crates/mcp-server/src/tool.rs::ToolRegistry::invoke`
- Status: VERIFIED
- Test: `crates/platform-windows/examples/type_text_chars_alias_parity.rs`

### Fixed
Swift treats `type_text_chars` as a **deprecated alias** for `type_text`:
the aliased name is NOT in `tools/list`, but invoking it with the old name
works (resolves to `type_text`) AND emits a stderr deprecation warning.
Rust was previously registering `type_text_chars` as a fully-fledged
separate tool with its own description and a different text format.

Changes:
- Remove `TypeTextCharsTool` from the registration in
  `platform-windows/src/tools/impl_.rs::build_registry` (the struct is
  kept in the crate for now via a no-op binding to avoid the dead-code
  warning during incremental cleanup).
- `serve.rs`: both `"call"` dispatch sites (Unix-socket + Windows-pipe
  variants) now resolve `type_text_chars` → `type_text` before the
  registry lookup, with the stderr deprecation message.
- `mcp-server/src/tool.rs::ToolRegistry::invoke`: also resolves the
  alias as a defense-in-depth for direct in-process callers.

### Verified on Windows
`type_text_chars_alias_parity.exe`:
- `tools/list` does NOT contain `"type_text_chars"` ✓
- Invoking `type_text_chars` with a 1-char text resolves to `type_text`'s
  response: `"✅ Typed 1 char(s) on pid 62156 via PostMessage (30ms delay)."` ✓

---

## MCP tool: `set_agent_cursor_style`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/SetAgentCursorStyleTool.swift:10-111`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (SetAgentCursorStyleTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/set_agent_cursor_style_parity.rs`

### Fixed (Windows)
1. **Text format** — was `"cursor style: gradient_colors=[X, Y] bloom_color=Z image_path=(unchanged)"`
   (always lists all three fields with `(unchanged)`/`(reverted)` placeholders,
   space after comma). Now Swift's exact wording: `"✅ cursor style: gradient_colors=[X,Y] bloom_color=Z"`
   (omit fields not provided; no space after comma; `image_path` only when set).
2. **Revert-all case** — was `"cursor style: gradient_colors=(unchanged) bloom_color=(unchanged) image_path=(unchanged)"`;
   now `"✅ cursor style: reverted to default"` matching Swift's `parts.isEmpty` branch.

### Verified on Windows
`set_agent_cursor_style_parity.exe`:
- Set gradient + bloom: `"✅ cursor style: gradient_colors=[#FF6B6B,#FF8E53] bloom_color=#A855F7"` ✓
- Revert all: `"✅ cursor style: reverted to default"` ✓

---

## MCP tool: `zoom`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ZoomTool.swift:12-end`
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (ZoomTool); macOS/linux OPEN
- Status: windows VERIFIED
- Test: `crates/platform-windows/examples/zoom_parity.rs`

### Fixed (Windows)
1. **Text format** — was `"Zoom (X,Y)–(X,Y) → WxH px JPEG."`; now Swift's
   verbatim multi-line message starting `"✅ Zoomed region captured at
   native resolution."` with the `from_zoom=true` integration hint.
2. **Error wording**: `"Missing required integer field pid."` /
   `"Missing required integer field window_id."` /
   `"Missing required region coordinates (x1, y1, x2, y2)."` /
   `"Invalid region: x2 must be > x1 and y2 must be > y1."` /
   `"Zoom region too wide: N px > 500 px max. ..."` — all match Swift.
3. **Schema** — `pid` now also required (Swift requires it).
4. **`idempotent: false`** — was `true`; Swift uses `false`.

### Intentional Rust-only
- **`window_id` required** — Swift uses pid+frontmost-window; Windows
  uses HWND directly since there's no clean Win32 analogue without an
  explicit HWND.

### Verified on Windows
`zoom_parity.exe`: invalid-region, too-wide, and real-zoom paths all OK.

---

## CLI subcommand: `mcp-config`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/CuaDriverCommand.swift:37-150`
- Rust: `libs/cua-driver/rust/crates/cua-driver/src/cli.rs::run_mcp_config`
- Status: VERIFIED (no fixes needed — already matched Swift)
- Test: `crates/platform-windows/examples/mcp_config_parity.rs`

### Already correct
- Default (no `--client`): generic mcpServers JSON snippet ✓
- `--client claude`: `claude mcp add --transport stdio cua-driver -- <bin> mcp` ✓
- `--client codex`: `codex mcp add cua-driver -- <bin> mcp` ✓
- `--client cursor`: JSON with `type: stdio` ✓
- `--client openclaw`: `openclaw mcp set ...` ✓
- `--client opencode`: opencode.json snippet with type=local ✓
- `--client hermes`: YAML snippet ✓
- `--client pi`: shell-tool fallback message ✓
- Unknown client: exit 2 + stderr message ✓

### Verified on Windows
`mcp_config_parity.exe` runs all 8 client variants + the unknown-client
error path against the in-process binary, asserting each output contains
the right needles. All pass on first try.

---

## CLI subcommand: `update`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/CuaDriverCommand.swift:638-686`
- Rust: `libs/cua-driver/rust/crates/cua-driver/src/cli.rs::run_update_cmd`
- Status: VERIFIED (bug fix)
- Test: `crates/platform-windows/examples/update_parity.rs`

### Fixed — REAL BUG
Rust was looking for release tag prefix `cua-driver-v` (Swift's prefix)
when fetching the latest version from `trycua/cua`. That would match the
Swift `cua-driver-v0.1.9` release and report it as an available upgrade
for the Rust port — confusing users into installing the WRONG binary.

Now uses prefix `cua-driver-rs-v` (Rust port's actual tag prefix).

### Verified on Windows
`update_parity.exe`:
- Always-present lines: `Current version:` and `Checking for updates…` ✓
- Outcome is one of: `Already up to date.`, `New version available: <v>`,
  or `Could not reach GitHub` ✓
- After the fix, the Swift `cua-driver-v0.1.9` tag is no longer mis-
  matched as a Rust-port release.

---

## CLI subcommand: `doctor`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/DoctorCommand.swift` (legacy-cleanup only — single
  `Nothing to clean — install is up to date.` codepath, no diagnostics)
- Rust: `libs/cua-driver/rust/crates/cua-driver/src/doctor.rs` +
        `libs/cua-driver/rust/crates/cua-driver/src/cli.rs::run_doctor_cmd`
- Status: INTENTIONAL_DIVERGENCE (Rust adds full diagnostic surface)
- Test: `crates/cua-driver/src/doctor.rs` `#[cfg(test)] mod tests`
        (5 unit tests: text rendering, JSON rendering, status-tag mapping,
        cross-platform probe smoke path)

### Behavior

Probe-runner that emits a structured report. Each probe is one line
tagged `[ok]`, `[warn]`, or `[err]` so the output is grep-friendly.
`--json` switches to a machine-readable shape for scripting (also
suppresses the update-available banner so JSON output stays parseable).

**Cross-platform probes:**
- `binary` — version + `<arch>-<os>` target triple
- `install dir` — `current_exe()` resolved through symlinks
- `home dir` — `~/.cua-driver-rs` existence + cached release-dir count
- `telemetry` — env-var opt-out state + install-id file presence
  (presence only — UUID value never read)

**Windows probes:**
- `interactive session` — `ProcessIdToSessionId(GetCurrentProcessId())`.
  Session 0 → `[warn]` with explicit guidance ("re-run from an
  interactive logon — RDP, console, or a scheduled task in the user's
  session"). Sessions ≥1 → `OpenWindowStationW(WinSta0)` +
  `GetForegroundWindow()` confirmation probe → `[ok]` when both succeed.
- `UI Automation` — `CoCreateInstance(CUIAutomation)` succeeds → `[ok]`,
  else `[err]`.
- `EnumWindows visible` — top-level visible-window count. When zero
  *and* Session 0 was warned above, the probe appends a "consistent
  with Session 0" detail so the two findings read as one.

**Linux probes:**
- `display server` — `DISPLAY` / `WAYLAND_DISPLAY` matrix (X11 only,
  Wayland only with XWayland hint, both set, neither set).
- `X11 connection` — quick handshake via `platform_linux::x11::list_windows(None)`.
- `AT-SPI` — `AT_SPI_BUS` env var, fallback to `gdbus introspect
  --session --dest org.a11y.Bus`.

**macOS probes:**
- `legacy LaunchAgent` — opportunistic removal of
  `~/Library/LaunchAgents/com.trycua.cua_driver_updater.plist` (preserves
  the old DoctorCommand cleanup behavior, now as a structured probe).
- `legacy update script` — opportunistic removal of
  `/usr/local/bin/cua-driver-update` (root-owned path gets a `[warn]`
  with the `sudo rm` command).
- `TCC + cdhash report` — pointer to `cua-driver diagnose` for the
  full bundle / signature / TCC dump.

### Exit code

`0` when every probe is `[ok]` or `[warn]`. Non-zero only when at
least one `[err]` probe failed (e.g. `current_exe()` returned an error,
or `CoCreateInstance(CUIAutomation)` failed on Windows). Warnings
deliberately do not fail the run because misconfigured environments
are sometimes the expected state — CI invocations of `doctor` to
render the report still expect exit 0.

### Why the divergence from Swift

The Swift port's `DoctorCommand` only handles legacy install-bit
cleanup on macOS. The Rust port runs on Windows + Linux as well, where
the analogous misconfigurations (Session 0 on Windows, missing
`DISPLAY` on Linux, no AT-SPI on Linux) are *the* source of "the tools
are broken" reports. Folding diagnostics into `doctor` mirrors what
users instinctively try first when something silently returns empty
arrays.

---

## CLI subcommand: `dump-docs`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/DumpDocsCommand.swift`
- Rust: `libs/cua-driver/rust/crates/cua-driver/src/cli.rs::run_dump_docs_with_type`
- Status: VERIFIED (with caveat about CLI extraction)
- Test: `crates/platform-windows/examples/dump_docs_parity.rs`

### Fixed
1. **Top-level JSON shape** — was `{mcp_tools: [...]}`; now matches Swift's
   `CombinedDocs` shape: `--type=all` → `{cli: {...}, mcp: {...}}`,
   `--type=mcp` → `{version, tools: [...]}`, `--type=cli` → CLI section.
2. **`--type` flag** added (`cli` | `mcp` | `all`, default `all`) matching
   Swift's flag.
3. **`version` field** added to the MCP section matching Swift's
   `MCPDocumentation`.

### Intentional Rust-only
- **CLI section is a stub** — Swift extracts via swift-argument-parser
  introspection; Rust uses hand-rolled arg matching in `cli.rs` so there's
  no equivalent introspection. Stub returns `{version, commands: [],
  _note: "..."}` directing users to `--help` for CLI docs. Full CLI
  introspection would require either clap migration or a parallel
  hand-maintained doc table.
- **Extra MCP fields** — `read_only`, `destructive`, `idempotent` per
  tool. Swift only emits `name`, `description`, `input_schema`. Rust keeps
  the extras as a documented enrichment.

### Verified on Windows
`dump_docs_parity.exe`:
- Default (`--type=all`): `{cli, mcp}` with 30 tools ✓
- `--type=mcp`: top-level `{version, tools}` ✓
- `--type=cli`: stub section ✓
- `--pretty`: multi-line JSON (991 lines) ✓

---

## Installer post-install hints

Not an MCP tool — an installer-text contract. The hint text printed at
the end of every cua-driver-rs install (Try-it / agent skill pack / MCP
setup per client / docs link) is sourced from a single shared file:

- **Shared text**: `libs/cua-driver/scripts/post-install-hints.txt`
  with `{{BINARY}}` placeholder.
- **Renderers**: each of the 4 Rust installers reads the .txt, swaps
  `{{BINARY}}` for the installed binary path, prints it, then appends
  an OS-specific autostart hint inline:
    - `libs/cua-driver/scripts/_install-rust.sh` — `curl` from
      raw.githubusercontent.com (remote install path) + bash `sed`.
    - `libs/cua-driver/scripts/install.ps1` — `Invoke-WebRequest` from
      raw.githubusercontent.com + PowerShell `-replace`.
    - `libs/cua-driver/rust/scripts/install-local.sh` — direct disk read
      from `../cua-driver/scripts/post-install-hints.txt` + `sed`.
    - `libs/cua-driver/rust/scripts/install-local.ps1` — direct disk read
      from `..\cua-driver\scripts\post-install-hints.txt` + `-replace`.

If the .txt is unreachable (network failure on remote installs, repo
layout change on local), each installer falls back to a one-line
essentials string so the user always gets enough to recover.

**Why not a CLI subcommand**: an earlier draft of this work added
`cua-driver post-install` to the Rust binary and had all 4 installers
delegate via `& $installedBinary post-install`. Reverted — the
chicken-and-egg risk (failed binary install = no hints either) made
the .txt approach the safer choice. The .txt has no runtime
dependency; even a totally broken binary install still prints hints.

**Why OS-specific hints stay inline**: each script targets one OS
(install.ps1 = Windows; install-local.sh on macOS vs Linux is the
only branching case). The OS-specific block is 4-6 lines, naturally
fits in the script that targets that OS, and is the only part that
would need conditional rendering in a single-file design.

**Status**: VERIFIED on macOS via `bash libs/cua-driver/rust/scripts/install-local.sh`
end-to-end. Windows VM verification pending.

---

## Focus-steal prevention

Cross-cutting infrastructure (not an MCP tool) used by `launch_app` and
by the 7 action tools (`click`, `type_text`, `hotkey`, `press_key`,
`drag`, `scroll`, `set_value`) via the
[Per-action focus suppression](#per-action-focus-suppression) wrap.
Catches apps that self-activate during launch (Chrome, Electron,
Safari) or as a side-effect of an action (Safari opening a new tab in
response to AXPress, autocomplete pulling itself forward), and
re-activates the prior frontmost app before the user perceives the
steal.

- Swift: `libs/cua-driver/Sources/CuaDriverCore/Focus/SystemFocusStealPreventer.swift`
- Swift hardening: open PR [#1521](https://github.com/trycua/cua/pull/1521)
  (4-layer leak prevention: closure scope, RAII lease, 5s monotonic
  deadline, 1s janitor)
- Rust: `crates/platform-macos/src/focus_steal.rs`
- Status: macOS VERIFIED. windows/linux N/A (no equivalent OS focus-steal
  surface area).
- Tests:
  - Rust unit: `crates/platform-macos/src/focus_steal.rs` `#[cfg(test)] mod tests`
    — dispatcher add/remove/match, deadline reap, janitor start/stop
    lifecycle.
  - Integration: `tests/integration/test_focus_steal_parity.py` — runs
    against both Swift and Rust binaries with `expectedFailure` on the
    Swift Safari-URL case for the known Cryptex+`oapp` LaunchServices bug.

### Design

- Process-wide singleton (`OnceLock<Arc<FocusStealPreventer>>`) — mirrors
  Swift's `AppStateRegistry.systemFocusStealPreventer`.
- One `NSWorkspaceDidActivateApplicationNotification` observer, registered
  on a **fresh background `NSOperationQueue`** (not `mainQueue`). This is
  load-bearing: the binary's `Call` and `--no-overlay` Serve modes don't
  run an NSApplication main-thread run loop, so an observer on `mainQueue`
  would silently no-op. Background queue means the block fires regardless
  of run-loop state.
- Dispatcher: `Mutex<HashMap<Uuid, SuppressionEntry>>` where each entry is
  `(target_pid: Option<i32>, restore_to: i32, deadline: Instant, origin)`.
  `target_pid = None` is the wildcard (catches any activation other than
  `restore_to`).
- API:
  - Closure: `with_suppression(target_pid, restore_to, origin, async fn)`.
  - RAII: `begin_suppression(...) -> SuppressionLease`, `Drop` calls
    `end_suppression` synchronously.
- Deadline reaper: each entry stamped `Instant::now() + 5s`. Pruned on
  every observer fire and by a 1s tokio interval task gated by a
  `watch::Sender` (start on first-add, stop on last-remove). Mirrors
  PR #1521's deadline + janitor layers; RAII (`SuppressionLease`)
  subsumes Swift's `withSuppression` (layer 1) + `SuppressionLease`
  (layer 2) into one Rust idiom.
- Restore: when a notification matches an entry, the observer block
  resolves `NSRunningApplication::runningApplicationWithProcessIdentifier(restore_to)`
  and calls `activate(options:[])`. `-[NSRunningApplication activate:]`
  is documented thread-safe — no main-thread hop needed.

### Intentional simplifications vs Swift

- **`FocusGuard.withFocusSuppressed` ships layer 3 only** — see
  [Per-action focus suppression](#per-action-focus-suppression). The
  reactive suppressor is wired up across the 7 action tools; the
  enablement (`AXManualAccessibility`/`AXEnhancedUserInterface`) and
  synthetic-focus (`AXFocused`/`AXMain` write+restore) layers are
  deferred because the AX assertion + attribute-write plumbing isn't
  yet ported. Empirically the layer-3 guard combined with
  `WindowChangeDetector`'s wildcard catches the majority of
  side-effects on real-world workflows.
- **`WindowChangeDetector` ported and wired** — see
  [Per-action focus suppression](#per-action-focus-suppression).
- **Janitor uses `tokio::sync::watch`** — Swift uses Task cancellation;
  Rust's tokio idiom is the watch-channel select pattern. Behavior is
  identical: idle dispatcher → janitor sleeps; new entry → janitor
  wakes; map drains → janitor exits and waits for the next add.

---

## Per-action focus suppression

Per-action wrap around the 7 macOS action tools (`click`, `type_text`,
`hotkey`, `press_key`, `drag`, `scroll`, `set_value`) that catches
side-effect side-effects of dispatching an action on a backgrounded
app — Safari opening a new tab in response to AXPress, a "Sign In"
button opening a sheet, an autocomplete popover floating into view,
etc. Mirrors Swift's `WindowChangeDetector` + `FocusGuard` cross-cutting
helpers wired into ClickTool / TypeTextTool / SetValueTool.

- Swift:
  - `libs/cua-driver/Sources/CuaDriverServer/Tools/WindowChangeDetector.swift`
  - `libs/cua-driver/Sources/CuaDriverCore/Focus/FocusGuard.swift`
- Rust:
  - `crates/platform-macos/src/window_change_detector.rs`
  - `crates/platform-macos/src/focus_guard.rs`
- Status: macOS VERIFIED (action tools wired up; result-suffix wording
  matches Swift verbatim). windows/linux N/A (different focus model).
- Tests:
  - Rust unit: `window_change_detector::tests` (8 cases — diff +
    result_suffix branches) and `focus_guard::tests` (4 cases —
    arm/skip lifecycle).
  - Integration: existing `tests/integration/test_focus_steal_parity.py`
    + `tests/integration/test_api_parity.py` — confirmed no regression
    after the action-tool wiring (identical pass/fail to main).

### Snapshot → action → detect cycle

Each wrapped action follows the same shape:

```rust
let prior = apps::frontmost_pid();
let snapshot = WindowChangeDetector::snapshot();         // arms wildcard
let result = focus_guard::with_focus_suppressed(
    Some(target_pid), prior, "<tool>.<origin>",
    || async { do_action(...).await }
).await;
let changes = snapshot.detect_async().await;             // drops wildcard
// append changes.result_suffix() to success text
```

Two suppression entries are armed in series:

1. **Wildcard** (armed in `snapshot()`, dropped in `detect()`) —
   `target_pid = None`, `restore_to = current frontmost`. Catches any
   activation other than the prior frontmost during the full
   snapshot → action → detect window. Mirrors Swift's
   `WindowChangeDetector.snapshot()` wildcard.
2. **Targeted** (armed by `FocusGuard::with_focus_suppressed` around
   the action call) — `target_pid = Some(action_pid)`,
   `restore_to = prior frontmost`. Catches a target-self-activation
   triggered by the AX call itself. Skipped when target ==
   frontmost (no point fighting ourselves). 50ms post-action settle
   before the lease drops, giving any in-flight reflex activation
   time to be observed.

### Result-suffix verbatim

The `Changes.result_suffix()` wording matches Swift's
`WindowChangeDetector.Changes.resultSuffix`:

- New windows: `"\n\n🪟 Action opened new window(s): App (\"Title\")."`
  (multiple windows grouped by app, titles in quotes, joined with a
  `,` (comma followed by a space); multiple apps joined with a `;`
  (semicolon followed by a space); alphabetical by app name).
- Foreground-only change (no new windows): `"\n\n🔀 Action caused a
  different app to become frontmost."`.
- No change: empty string.

MCP callers can string-match on either lead emoji to detect a
side-effect without per-binary special-casing.

### Intentional simplifications vs Swift

- **FocusGuard layers 1+2 (enablement, synthetic-focus) deferred** —
  Swift's `AXManualAccessibility` / `AXEnhancedUserInterface`
  assertion and `AXFocused`/`AXMain` write+restore aren't ported yet.
  The Rust port ships only layer 3 (reactive suppressor). Empirically
  the wildcard + targeted reactive pair catches the majority of
  side-effects; layers 1+2 are a follow-up when the AX assertion
  plumbing lands.
- **Pure-function diff exposed for tests** — `Snapshot::diff` is
  `pub(crate)` so unit tests can pin down opened/closed semantics
  without driving the live `CGWindowList` enumerator. Swift tests
  the same way via `currentWindowIds()` indirection.
- **`detect_async()` runs on `spawn_blocking`** — Swift's poll loop
  uses `Task.sleep`; Rust's blocking `std::thread::sleep` is cheap to
  offload via `tokio::task::spawn_blocking` and keeps the runtime
  responsive to other in-flight work.

---

## Telemetry (PostHog)

- Swift: `libs/cua-driver/Sources/CuaDriverCore/Telemetry/TelemetryClient.swift`
- Rust:  `crates/cua-driver/src/telemetry.rs`
- Status: VERIFIED (events emit on entry-point dispatch + install)
- Test:  `crates/cua-driver/src/telemetry.rs` `#[cfg(test)] mod tests`
         (8 unit tests: env parsing, opt-out default, CI detection,
         payload shape, payload-key collision, install-id idempotent
         persistence, ISO-8601 format, arch mapping)

### Endpoint + event names (identical to Swift)

- POST `https://eu.i.posthog.com/capture/`
- API key: `phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14` (public —
  ingest-only, can't read events)
- Events: `cua_driver_install`, `cua_driver_mcp`, `cua_driver_serve`,
  `cua_driver_stop`, `cua_driver_status`, `cua_driver_list_tools`,
  `cua_driver_describe`, `cua_driver_recording`, `cua_driver_config`,
  `cua_driver_mcp_config`, `cua_driver_dump_docs`, `cua_driver_update`,
  `cua_driver_doctor`, `cua_driver_diagnose`,
  `cua_driver_api_<tool>` (per-tool `call` invocations).

Keeping the endpoint + names identical means Rust + Swift events land
in the same PostHog project; `$lib = "cua-driver-rs"` vs
`"cua-driver-swift"` is the only signal to split them.

### Payload shape

Each event sends:

| Key | Value | Source |
|-----|-------|--------|
| `cua_driver_version` | CARGO_PKG_VERSION (e.g. `"0.1.3"`) | build-time |
| `os` | `"macos"` / `"linux"` / `"windows"` | `std::env::consts::OS` |
| `os_version` | OS-reported version string | `sw_vers -productVersion` / `/etc/os-release` / `cmd /c ver` |
| `arch` | `"arm64"` / `"x86_64"` (aarch64 → arm64) | `std::env::consts::ARCH` |
| `is_ci` | bool | env-var probe (see below) |
| `$lib` | `"cua-driver-rs"` | hard-coded |
| `$lib_version` | CARGO_PKG_VERSION | build-time |

CI-environment detection probes the same vars Swift does: `CI`,
`CONTINUOUS_INTEGRATION`, `GITHUB_ACTIONS`, `GITLAB_CI`, `JENKINS_URL`,
`CIRCLECI`.

### Privacy posture (what we DO NOT send)

Verified by unit test `build_payload_contains_required_keys` which
asserts none of `$user`, `username`, `home_dir`, `cwd`, `argv` ever
appear in a serialized payload:

- No usernames or `$USER` / `$HOME`
- No file paths (cwd, executable path, tool args, screenshot paths)
- No tool arguments — `call <tool>` reports only the tool name as
  `cua_driver_api_<tool>`
- No user-typed content (text, key sequences, URLs)
- No window titles, application names, or coordinates

### Opt-out

Set `CUA_DRIVER_RS_TELEMETRY_ENABLED=false` (or `0`, `no`, `off`) to
disable ALL telemetry from the binary. Unset defaults to enabled
(matches Swift's persisted-flag default of `true`).

The **only** path that ignores the opt-out is `capture_install()`,
which fires the one-shot `cua_driver_install` ping from `install.sh`'s
post-install hook. Rationale: an opt-out user is still a counted
install in the adoption metric; every subsequent event from the binary
respects the flag normally. Guarded by `~/.cua-driver-rs/.installation_recorded`
so re-running `install.sh` doesn't re-fire it.

### Independence from Swift install

The Rust port is deliberately partitioned from the Swift port at the
filesystem + env-var layer:

| Layer | Swift | Rust |
|-------|-------|------|
| Install dir | `~/.cua-driver/` | `~/.cua-driver-rs/` |
| Install UUID | `~/.cua-driver/.telemetry_id` | `~/.cua-driver-rs/.telemetry_id` |
| Install marker | `~/.cua-driver/.installation_recorded` | `~/.cua-driver-rs/.installation_recorded` |
| Opt-out env var | `CUA_DRIVER_TELEMETRY_ENABLED` | `CUA_DRIVER_RS_TELEMETRY_ENABLED` |

This means:
- A machine with both installed shows up as two distinct adoption events
  (we can count Rust adoption separately from Swift).
- A user who opts out of one port stays opted-in for the other unless
  they set both env vars.
- The Rust port can ship telemetry changes without invalidating Swift's
  install UUID (no shared on-disk state).

### HTTP client

`ureq` v3 with default features (rustls + gzip + json). Single POST,
3-second timeout, fire-and-forget. Sent from `tokio::task::spawn_blocking`
when a runtime is live (MCP server, serve daemon), else from a
short-lived OS thread (synchronous CLI subcommands like `list-tools`).

Network errors, timeouts, and 4xx/5xx responses are logged via
`tracing::debug!(target: "cua_driver::telemetry", …)` only — never
surfaced to stdout/stderr unless `CUA_DRIVER_RS_TELEMETRY_DEBUG=true`.

### Intentional divergences from Swift

- **No persisted config flag.** Swift falls back to a YAML
  `telemetryEnabled` setting via `ConfigStore.loadSync()`. Rust honors
  only the env var. The Rust port has no `ConfigStore` analogue yet, and
  YAGNI suggests waiting until someone files a request.
- **No `GUI_LAUNCH` emission.** Swift fires `cua_driver_gui_launch` when
  the binary is launched bare (Finder / Dock double-click). Rust has no
  GUI surface yet, so the constant is reserved but unused.
- **`is_ci` uses env-var probing only.** Same probe list as Swift; no
  extra Rust-specific signals.

## Startup flow: update-available banner (`mcp` / `serve` / `doctor`)

- Swift: not present (the Swift port has no analogous banner today)
- Rust:  `crates/cua-driver/src/version_check.rs`
- Status: INTENTIONAL_ADDITION (Rust-only)
- Test:   `crates/cua-driver/src/version_check.rs` `#[cfg(test)] mod tests`
          (22 unit tests: semver edge cases, cache round-trip in a
          tempdir, dismissal persistence, 20-hour refresh threshold,
          env-var + config opt-out, JSON release-list filtering,
          banner format, ISO-8601 timestamp)

### Behavior

On the long-running interactive entry points (`mcp`, `serve`,
`doctor`) the binary kicks off a background HTTP check against
`https://api.github.com/repos/trycua/cua/releases?per_page=40`,
filters to the `cua-driver-rs-v*` tag prefix, and prints a two-line
banner to **stderr** if the highest non-draft non-prerelease release
is strictly newer than `CARGO_PKG_VERSION` and the user hasn't
previously dismissed it:

```text
✨ cua-driver v0.1.4 is available (you have v0.1.3).
   Update with: cua-driver update
   Release notes: https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.1.4
```

The check runs on `tokio::task::spawn_blocking` when a runtime is
live, else a short-lived OS thread, so the daemon's start-up path
is never delayed by network latency.

### Cache

The latest-version answer is cached on disk at
`~/.cua-driver-rs/version_check.json`:

```json
{
  "last_checked_unix": 1700000000,
  "last_checked_at": "2023-11-14T22:13:20Z",
  "latest_version": "0.1.4",
  "dismissed_versions": []
}
```

Refreshed only when the cached `last_checked_unix` is more than 20
hours old, bounding outbound requests to roughly one per machine per
day even on a hot reload loop. Failed HTTP fetches fall back to the
cached value (better an old banner than none on a brief network blip).

### Skipped contexts

Only `mcp`, `serve`, and `doctor` call `maybe_announce_update()`.
The following entry points are **NOT** instrumented because they're
routinely piped from scripts and a banner would corrupt their
parseable output:

- `--version` / `-V`
- `list-tools`
- `describe <tool>`
- `call <tool>`
- `dump-docs`
- `mcp-config`
- `update`, `stop`, `status`, `recording`, `config`, `diagnose`,
  `telemetry install-event`

### Opt-out (three layers, any one disables the check)

1. **Env var** `CUA_DRIVER_RS_UPDATE_CHECK=false` (also `0`, `no`,
   `off`; case-insensitive) — single-invocation off.
2. **Config flag** `update_check_enabled = false` in
   `~/.cua-driver/config.json` — persistent off. Set via
   `cua-driver config set update_check_enabled false`.
3. **Pre-release build auto-skip** — any `CARGO_PKG_VERSION` that
   carries a semver pre-release suffix (`-dev`, `-rc.1`, `-beta`)
   short-circuits the check entirely. There is no matching published
   release for a source / development build to recommend.

### HTTP client

`ureq` v3 with `Accept: application/vnd.github+json` and a
`cua-driver-rs/<version>` user-agent. 4-second timeout. Single GET,
fire-and-forget — the response body is read into `serde_json::Value`
on the background task, never touching the foreground startup path.

Network errors, timeouts, 4xx/5xx responses, and JSON-parse failures
are logged via `tracing::debug!(target: "cua_driver::version_check",
…)` only — never surfaced to stderr.

### Shared with `cua-driver update`

`run_update_cmd` calls into the same
`version_check::fetch_latest_version()` and `version_check::is_newer()`
helpers so the proactive banner and the manual subcommand agree on
tag-filtering rules and semver compare semantics. This also removes
the prior shell-out to `curl` from `cli::run_update_cmd`.

### Dismissal API

`version_check::dismiss_version(&str)` appends a version string to
the `dismissed_versions` list on disk. No call site in the current
binary (banner is informational only today); kept public so a future
interactive prompt path (TUI helper, GUI extra) can persist the
"skip until next version" choice without re-implementing the cache
layer.

## Installer: layout + lifecycle

This section covers the cross-platform Rust installer's runtime
behaviors: how it picks a version, where it lands files, and how it
keeps the on-disk state bounded across repeated upgrades.

### Version-resolution chain (`env > baked > API`)

Both Rust installers (`scripts/install.sh`, `scripts/install.ps1`)
resolve the release tag in the same priority order as the Swift
`cua-driver` installer:

1. **Explicit env pin** —
   `CUA_DRIVER_RS_VERSION` (`$env:CUA_DRIVER_RS_VERSION` on Windows).
2. **Baked-in default** —
   a sentinel-block-wrapped constant carried in the install script
   itself, updated by the CD `Bake version into install scripts` step
   after each `cua-driver-rs-v*` tag push. Matches the Swift driver's
   `CUA_DRIVER_BAKED_VERSION` shape.
3. **GitHub Releases API fallback** —
   only consulted when the env override is absent *and* the baked
   line hasn't been updated yet (dev branches, pre-release checkouts).

#### Why this exists

The Swift installer adopted this pattern after we hit two failure
modes the API-only chain couldn't dodge:

- **API outages / rate limits** — unauthenticated GitHub Releases is
  capped at 60 req/hr per source IP. A shared egress IP (CI runner,
  corporate NAT, dev laptop on a busy office network) can exhaust
  that and turn every `curl … | bash` install into a hard failure.
- **Tag-prefix pagination drift** — the repo ships both
  `cua-driver-v*` (Swift) and `cua-driver-rs-v*` (Rust) tags, plus
  unrelated tags from other libs. As release cadence grows, the
  first page of `/releases?per_page=N` is no longer guaranteed to
  include any matching tag, and a single-page fetch will silently
  resolve nothing.

Baking the version turns both of those into non-issues for the
common-case install (curl-against-main / irm-against-main). The API
path is still exercised by dev installs from un-baked branches, so
the pagination fix in `install.ps1` (commit `3425af0b`) stays
valuable; `install.sh` keeps its single-page fetch for now, which
is a follow-up candidate once the baked default is shipped (it is
fallback-only and not hit by the default curl-from-main path).

#### Sentinel-block markers (kept byte-identical across both scripts)

```bash
# ~~~ BAKED_VERSION: auto-updated by CD workflow after each release — do not edit ~~~
CUA_DRIVER_RS_BAKED_VERSION="<version>"
# ~~~ END_BAKED_VERSION ~~~
```

The PowerShell variant swaps the bash assignment for
`$Script:CuaDriverRsBakedVersion = "<version>"` but reuses the same
marker comments. The CD step's `sed` patterns key on the assignment
line, not the markers, so the markers are a human cue only.

#### CD wiring

`.github/workflows/cd-rust-cua-driver.yml` runs a `Bake version into
install scripts` step at the end of the `release` job after each
`cua-driver-rs-v*` tag push. It runs on `ubuntu-latest` (GNU sed)
and the equivalent Swift step runs on `macos-15` (BSD sed), so the
two workflows use slightly different `sed -i` syntax:

| Workflow | Runner | `sed -i` form |
|---|---|---|
| `cd-rust-cua-driver.yml` (this) | `ubuntu-latest` | `sed -i 's/.../.../`  (GNU) |
| `cd-swift-cua-driver.yml` | `macos-15` | `sed -i '' 's/.../.../`  (BSD) |

Both push the rewritten files back to `main` using a GitHub App
token (`RELEASE_APP_ID` + `RELEASE_APP_PRIVATE_KEY`) so the push
bypasses the "Changes must be made through a pull request" ruleset
on `main` — the default `GITHUB_TOKEN` (github-actions[bot]) is
rejected by that ruleset.

The commit author is `trycua-release[bot]` and the message is
`chore(cua-driver-rs): bake version <V> into install scripts [skip ci]`
(the `[skip ci]` suppresses the recursive CD trigger from the
bake-push hitting `main`).

### Per-version release-dir GC (`CUA_DRIVER_RS_KEEP_VERSIONS`)

Each install drops the binary into a fresh
`$HOME_DIR/packages/releases/<version>-<target>/` directory and
retargets `current` at it. Old per-version dirs are kept on disk so
rollback is `ln -sfn` / junction-retarget away with no re-download.
Disk usage grows ~15 MB per upgrade, so the installer runs a
post-install GC pass to trim oldest dirs back to a configurable cap.

#### Defaults + override

- Default: **keep 5** most recent per-version dirs per target triple.
- Override: `CUA_DRIVER_RS_KEEP_VERSIONS=<N>` (env). `<N>` is any
  non-negative integer; `0` disables GC entirely (legacy behavior —
  retains every version forever). Non-integer or negative values fall
  back to the default with a `warning:` log.

#### Invariants

1. **Per-target** — only dirs whose name suffix matches the current
   platform's `${TARGET}` triple are eligible to prune. A multi-arch
   dev with both `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`
   under one `$HOME_DIR` (rare but possible — e.g. shared home over
   NFS) has each target's history GC'd independently.
2. **Active install is always preserved** — even if the dir that
   `current` resolves to is older than the keep window (e.g. user
   rolled back to an old version). Worst-case post-GC dir count is
   `keep + 1`; common-case is exactly `keep`.
3. **Runs after the atomic swap** — `prune_old_releases` (sh) /
   `Invoke-OldReleasesGc` (ps1) is invoked only after `current` has
   been retargeted at the new install, so the about-to-be-active
   version is never a deletion candidate.
4. **macOS path unchanged** — the macOS `/Applications/CuaDriver.app`
   install is an in-place replacement (no per-version directory
   accumulation), so the GC pass is a no-op there by construction
   (the Darwin branch never enters the versioned-dirs install path).

#### Implementation shape

- `install.sh` — `prune_old_releases` uses `ls -dt "$RELEASES_DIR"/*/`
  for mtime-sorted candidates, filters by `*-$TARGET`, skips the dir
  that `readlink "$CURRENT_LINK"` resolves to, and `xargs -0 rm -rf`s
  the excess past the keep window.
- `install.ps1` — `Invoke-OldReleasesGc` uses
  `Get-ChildItem -Directory | Where-Object Name -like "*-$target" |
  Sort-Object LastWriteTime -Descending`, resolves
  `Get-JunctionTarget $CurrentDir` to find and exempt the active
  install, and `Remove-Item -Recurse -Force`s the excess.

#### Verification recipes

**Linux** (pin three versions, observe GC):

```bash
# Install three pinned versions in sequence. Each one drops a new
# per-version dir under ~/.cua-driver-rs/packages/releases/ and
# retargets `current`.
CUA_DRIVER_RS_VERSION=0.1.4 bash install.sh
CUA_DRIVER_RS_VERSION=0.2.0 bash install.sh
CUA_DRIVER_RS_VERSION=0.2.1 bash install.sh
ls ~/.cua-driver-rs/packages/releases/
# → 0.1.4-…, 0.2.0-…, 0.2.1-…  (3 dirs, GC saw ≤ default-5, no-op)

# Force keep=2 — GC trims to 2 newest, current always preserved.
CUA_DRIVER_RS_VERSION=0.2.1 CUA_DRIVER_RS_KEEP_VERSIONS=2 bash install.sh
ls ~/.cua-driver-rs/packages/releases/
# → 0.2.0-…, 0.2.1-…  (0.1.4 pruned)

# keep=0 — GC disabled.
CUA_DRIVER_RS_VERSION=0.2.1 CUA_DRIVER_RS_KEEP_VERSIONS=0 bash install.sh
# (logs "version GC disabled", retains all on-disk dirs)
```

**Windows** (equivalent in PowerShell):

```powershell
$env:CUA_DRIVER_RS_VERSION = "0.1.4"; irm <url>/install.ps1 | iex
$env:CUA_DRIVER_RS_VERSION = "0.2.0"; irm <url>/install.ps1 | iex
$env:CUA_DRIVER_RS_VERSION = "0.2.1"; irm <url>/install.ps1 | iex
Get-ChildItem ~\.cua-driver-rs\packages\releases\
# → 3 dirs

$env:CUA_DRIVER_RS_KEEP_VERSIONS = "2"
$env:CUA_DRIVER_RS_VERSION = "0.2.1"; irm <url>/install.ps1 | iex
Get-ChildItem ~\.cua-driver-rs\packages\releases\
# → 2 dirs (0.1.4 pruned)
```

### Per-host concurrent-install lockfile

Two installs running at the same time (user clicks the one-liner while
a CI script is also installing; two terminals; cron-driven reinstall
racing a manual one) can race on the atomic `current` swap and leave
the visible binary pointing at a partially-populated release dir.
Serialize installs per `$HOME_DIR` with a process-level mutex.

#### Primitive

| Platform | Mutex |
|---|---|
| Linux (and Linux-via-WSL) | `mkdir $HOME_DIR/packages/.install.lock.d` — atomic on POSIX, no `flock` dependency. First install wins; concurrent attempts get `EEXIST` and poll. |
| Windows | `System.IO.FileStream` opened on `$HomeDir\install.lock` with `FileShare::None`. Windows kernel rejects a second open until the first handle closes, so the open call itself is the acquisition. |

Both primitives are unprivileged — no admin / sudo / Developer Mode.

#### Wait + stale-detection UX

- **Wait** — polls every 1s, prints `another cua-driver-rs install is
  already in progress (lock at <path>); waiting...` exactly once.
- **Stale threshold** — 600 seconds. Named constant in both scripts
  (`LOCK_STALE_AFTER_SECONDS` / `$Script:LockStaleAfterSeconds`), not
  a magic number, so future tuning is grep-able.
- **Force-release** — after `LOCK_STALE_AFTER_SECONDS` of waiting we
  log `lock appears stale (>600s), forcing release` and `rm -rf` /
  `Remove-Item` the lock entry, then retry. The alternative (hang
  forever) leaves users wedged with no obvious recovery path.

#### Lock-info stamp

After acquiring, the installer writes a tiny info blob into the lock
so a user investigating a stuck install can see who holds it:

```bash
$ cat ~/.cua-driver-rs/packages/.install.lock.d/info
pid=43210
started=2026-05-17T09:14:22Z
argv=install.sh
```

```powershell
PS> Get-Content $env:USERPROFILE\.cua-driver-rs\install.lock
pid=43210
started=2026-05-17T09:14:22.123Z
invocation=install.ps1 -Release latest
```

#### Release on every exit path

Both scripts release the lock unconditionally:

- `install.sh` — `trap cleanup_on_exit EXIT` plus per-signal traps
  for `INT` and `TERM` that release then re-raise so the exit code
  reflects the signal.
- `install.ps1` — top-level `try { ... } finally { Release-InstallLock }`
  wrapping the whole Main block. PowerShell's `finally` fires on
  normal exit, exceptions, `exit`, and `Ctrl-C` (pipeline-stop).

A half-finished install with a held lock would wedge every subsequent
install for the full 600s stale window, so the cleanup wiring is
non-optional.

#### Verification recipes

**Linux** — concurrent contention:

```bash
# Shell 1
bash install.sh           # holds the lock, takes ~20s

# Shell 2 (kick off while shell 1 is still running)
bash install.sh
# → "another cua-driver-rs install is already in progress (lock at
#    ~/.cua-driver-rs/packages/.install.lock.d); waiting..."
# (blocks until shell 1 finishes, then proceeds normally)
```

**Linux** — stale lock recovery:

```bash
# Simulate a dead holder.
mkdir ~/.cua-driver-rs/packages/.install.lock.d
echo "pid=99999" > ~/.cua-driver-rs/packages/.install.lock.d/info

bash install.sh
# → "another cua-driver-rs install is already in progress…; waiting..."
# (waits 600s, then:)
# → "lock appears stale (>600s), forcing release"
# (proceeds normally)
```

**Windows** — equivalent in two PowerShell windows. The stale-recovery
test in PowerShell is `New-Item -Path $env:USERPROFILE\.cua-driver-rs\install.lock`
plus `Remove-Item` to clear after; same 600s wait.
