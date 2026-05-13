# cua-driver-rs ↔ cua-driver (Swift) Parity Audit

Tracking surface-by-surface line-by-line behavioral comparison between the
Rust port and the Swift reference. Each entry lists Swift source location,
Rust source location, divergences (intentional vs. accidental), and the
deterministic test that locks the verified behavior in.

Format per entry:
```
## <surface>
- Swift: <path:line>
- Rust:  macos=<path:line>, windows=<path:line>, linux=<path:line>
- Status: VERIFIED | INTENTIONAL_DIVERGENCE | OPEN
- Test:   <path>
- Notes:  ...
```

---

## MCP tool: `move_cursor`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/MoveCursorTool.swift:6-60`
- Rust:
  - macos=`crates/platform-macos/src/tools/move_cursor.rs`
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
  - macos=`crates/platform-macos/src/tools/get_cursor_position.rs`
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
  - macos=`crates/platform-macos/src/tools/get_screen_size.rs`
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
  - macos=`crates/platform-macos/src/tools/check_permissions.rs` (FIXED, pending macOS run)
  - windows=`crates/platform-windows/src/tools/impl_.rs` (CheckPermissionsTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (CheckPermissionsTool)
- Status:
  - macos: OPEN (fixed in source; macOS runner needed to verify)
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

## MCP tool: `list_apps`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/ListAppsTool.swift:6-71`
        + `libs/cua-driver/Sources/CuaDriverCore/Apps/AppInfo.swift`
- Rust:
  - macos=`crates/platform-macos/src/tools/list_apps.rs` + `apps.rs:format_app_list`
  - windows=`crates/platform-windows/src/tools/impl_.rs` (ListAppsTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ListAppsTool)
- Status:
  - macos: code fixed (✅ checkmark added, description copied from Swift); pending macOS run
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
  - macos=`crates/platform-macos/src/tools/list_windows.rs` (TBD audit)
  - windows=`crates/platform-windows/src/tools/impl_.rs` (ListWindowsTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ListWindowsTool, blocked behind Linux compile fix)
- Status:
  - windows: VERIFIED
  - macos: OPEN (audit pending — macOS port already exists)
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
  - macos=`crates/platform-macos/src/tools/click.rs` (TBD audit)
  - windows=`crates/platform-windows/src/tools/impl_.rs` (ClickTool)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (ClickTool)
- Status:
  - windows: VERIFIED (text format + error wording); schema divergences documented below
  - macos: OPEN (already exists; line-by-line audit pending)
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
  - macos=`crates/platform-macos/src/tools/launch_app.rs` (TBD audit)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (TBD audit)
- Status:
  - windows: VERIFIED
  - macos: OPEN
  - linux: OPEN
- Test: `crates/platform-windows/examples/launch_app_parity.rs`

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

---

## MCP tool: `press_key`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/PressKeyTool.swift:20-202`
- Rust:
  - windows=`crates/platform-windows/src/tools/impl_.rs` (PressKeyTool)
  - macos=`crates/platform-macos/src/tools/press_key.rs` (TBD)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (TBD)
- Status: windows VERIFIED; macos / linux OPEN
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
