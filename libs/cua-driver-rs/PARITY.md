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
