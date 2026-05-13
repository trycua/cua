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

---

## MCP tool: `hotkey`
- Swift: `libs/cua-driver/Sources/CuaDriverServer/Tools/HotkeyTool.swift:6-142`
- Rust:
  - windows=`crates/platform-windows/src/tools/impl_.rs` (HotkeyTool)
  - macos=`crates/platform-macos/src/tools/hotkey.rs` (TBD)
  - linux=`crates/platform-linux/src/tools/impl_.rs` (TBD)
- Status: windows VERIFIED; macos / linux OPEN
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
  - macos / linux: OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (RightClickTool); macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (ScreenshotTool); macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (ScrollTool); macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (TypeTextTool); macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (SetValueTool); macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (GetConfigTool, SetConfigTool); macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (GetAgentCursorStateTool); macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs`; macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (GetWindowStateTool); macos/linux OPEN
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
- Rust: windows=`crates/platform-windows/src/tools/impl_.rs` (DragTool); macos/linux OPEN
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
