# cua-driver-rs ↔ cua-driver (Swift) Parity Audit

## Integration Test Suite

All API surfaces (CLI subcommands, stdio MCP protocol, daemon lifecycle) are
covered by `tests/integration/test_api_parity.py` — **111 tests per binary,
222 total**, parametrized so both binaries run the identical suite.

```bash
# Rust binary only
cd libs/cua-driver-rs/tests/integration
./run_tests.sh test_api_parity -v

# Both binaries side-by-side
./run_tests.sh --parity -v
```

### Known parity gaps (as of 2026-05-13, macOS)

| Gap | Rust | Swift |
|-----|------|-------|
| `type_text_chars` | ✅ | missing |
| `browser_eval` | ✅ | missing |
| `get_accessibility_tree` | ✅ | missing |
| `page` tool | ✅ (registered as of this commit) | ✅ |
| `--version` flag | ✅ (fixed in this commit) | ✅ |
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
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ListAppsTool)
- Status:
  - macOS: code fixed (✅ checkmark added, description copied from Swift); pending macOS run
  - windows: VERIFIED (text format + structured shape + `active` flag)
  - linux: OPEN (subagent currently fixing pre-existing compile errors)
- Test: `crates/platform-windows/examples/list_apps_parity.rs`

### Fixed divergences

1. **Windows: returned every OS process** (System, Registry, csrss, etc.)
   instead of "apps". Now filters to processes that own at least one
   visible top-level window — the closest analogue to Swift's
   `NSApplicationActivationPolicyRegular` filter (excludes background
   helpers / agents).
2. **Structured shape**: Swift returns `Output { apps: [AppInfo] }` with
   `AppInfo = {pid, bundle_id, name, running, active}`. Windows was
   returning `{ processes: [{pid, name}] }`. Now returns both keys —
   `apps` (Swift shape) for new callers, `processes` (legacy shape) as a
   transitional alias.
3. **Text format**: Swift starts with `"✅ Found N app(s): R running, I installed-not-running."`.
   macOS Rust was missing the `✅` prefix; Windows had a different
   wording entirely (`"Found N processes:"`). Both now match Swift 1:1.
4. **`active` flag** — Windows resolves it via `GetForegroundWindow` +
   `GetWindowThreadProcessId` (only one app is `active` at a time).
5. **Description** — copied verbatim from Swift on macOS; adapted with
   Windows-specific caveats on Windows (no bundle_id, no installed-apps
   enumeration yet).

### Known limitation (Windows / Linux)

Neither Rust port enumerates *installed but not running* apps yet — Swift
scans `/Applications`, `~/Applications`, `/System/Applications`, etc.
Windows would need to scan Start Menu shortcuts and the Registry
Uninstall keys; Linux would need to scan `.desktop` files in
`/usr/share/applications` and `~/.local/share/applications`. Marked as
follow-up work; the running-app view is still useful and matches Swift's
running subset exactly.

### Verified on Windows

`list_apps_parity.exe`: header line matches `"✅ Found N app(s): R
running, 0 installed-not-running."`; `structuredContent.apps` is an
array of `{pid, bundle_id (null on Windows), name, running, active}`;
at least one entry has `active: true` (the foreground app).

---

## MCP tool: `list_windows`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ListWindowsTool.swift:6-245`
- Rust:
  - macOS=`crates/platform-macos/src/tools/list_windows.rs` (TBD audit)
  - windows=`crates/platform-windows/src/tools/impl_.rs` (ListWindowsTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ListWindowsTool, blocked behind Linux compile fix)
- Status:
  - windows: VERIFIED
  - macOS: OPEN (audit pending — macOS port already exists)
  - linux: OPEN
- Test: `crates/platform-windows/examples/list_windows_parity.rs`

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
filters them out at the `EnumWindows` callback level (`IsWindowVisible &&
!IsIconic`).  The `on_screen_only` schema field is accepted but has no
effect.  Follow-up to refactor `list_windows` to return everything and
filter at the tool layer.

---

## MCP tool: `click`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ClickTool.swift:29-595`
- Rust:
  - macOS=`crates/platform-macos/src/tools/click.rs` (TBD audit)
  - windows=`crates/platform-windows/src/tools/impl_.rs` (ClickTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ClickTool)
- Status:
  - windows: VERIFIED (text format + error wording); schema divergences documented below
  - macOS: OPEN (already exists; line-by-line audit pending)
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
  - windows: VERIFIED
  - macOS: VERIFIED (full focus-steal contract — see [Focus-steal prevention](#focus-steal-prevention))
  - linux: OPEN
- Tests:
  - windows=`crates/platform-windows/examples/launch_app_parity.rs`
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
   has no bundle-identifier concept). Cross-platform callers can use
   the same field name.
5. **`additional_arguments`** — honored, passed as `lpParameters` to
   `ShellExecuteExW`.
6. **Window-resolution retry** — ports Swift's 5-attempt 100/200ms
   retry to absorb Win32 window-creation lag after `ShellExecuteEx`
   returns.
7. **Error wording** — `"Provide either bundle_id or name to identify
   the app to launch."` matches Swift's `errorResult` text.
8. **`active: false`** — hardcoded; `SW_SHOWNOACTIVATE` is the
   Windows-equivalent of Swift's background-launch invariant.
9. **Description** — multi-paragraph port from Swift with explicit
   Windows-specific notes (path takes precedence; bundle_id alias).

### Intentional Rust-only fields accepted (no-op)

- `electron_debugging_port`, `webkit_inspector_port`,
  `creates_new_application_instance` — Swift-specific.  Accepted in
  the schema so cross-platform callers can pass them; currently
  no-ops on Windows. Documented as follow-up.

### Verified on Windows

`launch_app_parity.exe` launches Notepad:
- Header `"✅ Launched notepad.exe (pid 30612) in background."` ✓
- `structuredContent.pid` is the actual ShellExecuteEx pid ✓
- `bundle_id: null`, `running: true`, `active: false` ✓
- Notepad killed on test exit. ✓

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
  - macOS=`crates/platform-macos/src/tools/press_key.rs` (TBD)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (TBD)
- Status: windows VERIFIED; macOS / linux OPEN
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
  - macOS=`crates/platform-macos/src/tools/hotkey.rs` (TBD)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (TBD)
- Status: windows VERIFIED; macOS / linux OPEN
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
2. **Default JPEG quality** — was 85, Swift defaults 95. Now 95.
3. **Description** — multi-paragraph port from Swift adapted to Windows
   (BitBlt + PrintWindow transport; no permission gate needed).
4. **`idempotent`** — was `true`; Swift uses `false` (a fresh pixel grab
   every call). Now matches Swift.

### Intentional Rust-only

- **Optional `window_id`** — Swift requires it; Windows allows omission
  for whole-display capture (`screenshot_display_bytes`).  Useful
  Windows-only convenience that Swift can't easily provide because
  macOS Screen Recording requires per-window grants.  Schema accepts
  both shapes; description explains.

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
- Rust: `libs/cua-driver-rs/crates/cua-driver/src/cli.rs::run_list_tools`
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
- Rust: `libs/cua-driver-rs/crates/cua-driver/src/cli.rs::run_describe`
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

## CLI subcommands: `status` + `stop`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/ServeCommand.swift:368-470`
- Rust: `libs/cua-driver-rs/crates/cua-driver/src/serve.rs::run_status_cmd, run_stop_cmd`
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
- Rust: `libs/cua-driver-rs/crates/cua-driver/src/serve.rs` (both pipe variants)
  + `libs/cua-driver-rs/crates/mcp-server/src/tool.rs::ToolRegistry::invoke`
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
- Rust: `libs/cua-driver-rs/crates/cua-driver/src/cli.rs::run_mcp_config`
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
- Rust: `libs/cua-driver-rs/crates/cua-driver/src/cli.rs::run_update_cmd`
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

## CLI subcommand: `dump-docs`
- Swift: `libs/cua-driver/Sources/CuaDriverCLI/DumpDocsCommand.swift`
- Rust: `libs/cua-driver-rs/crates/cua-driver/src/cli.rs::run_dump_docs_with_type`
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

## Focus-steal prevention

Cross-cutting infrastructure (not an MCP tool) used by `launch_app` today
and slated for use by `click`/`hotkey`/AX-action tools when those are
ported to Rust macOS. Catches apps that self-activate during launch
(Chrome, Electron, Safari) and re-activates the prior frontmost app
before the user perceives the steal.

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

- **No `FocusGuard.withFocusSuppressed` analogue yet** — Swift's
  per-AX-action wrapper. The Rust `click` AX path doesn't exist yet, so
  there's nothing to wrap. Port when the AX action path lands.
- **No `WindowChangeDetector` analogue yet** — same reason; the Rust port
  has no snapshot/detect cycle to wrap. Port when WindowChangeDetector
  lands.
- **Janitor uses `tokio::sync::watch`** — Swift uses Task cancellation;
  Rust's tokio idiom is the watch-channel select pattern. Behavior is
  identical: idle dispatcher → janitor sleeps; new entry → janitor
  wakes; map drains → janitor exits and waits for the next add.
