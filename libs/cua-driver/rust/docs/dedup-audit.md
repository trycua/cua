# cua-driver-rs cross-platform code-duplication audit

**Date:** 2026-05-23 evening (overnight session)
**Branch:** `cross-platform-dedup-audit` (off `main @ 7436ba89`)
**Approach:** scan `crates/platform-{macos,windows,linux}` for function signatures + struct definitions that appear in 2+ platforms with the same shape, rank by ROI (lines saved vs refactoring risk).

## Existing shared crates

Two cross-platform crates already in the workspace:

| Crate | What it owns | Notes |
|---|---|---|
| `mcp-server` | Tool trait, ToolDef, page tool + PageBackend trait, shared CDP helper, MCP protocol structs | PR #1666 added the cross-platform `page` tool. Right home for tool-side shared logic. |
| `cursor-overlay` | CursorConfig / CursorInstanceConfig / CursorRegistry / Palette / MotionConfig / Shape / PathPlanner / Bezier / `rotate_toward` / `crop_png_to_jpeg` | PR #1662 already extracted `rotate_toward`. Right home for cursor-overlay shared primitives. |

## Duplication candidates ranked by ROI

### 🥇 #1 — capture.rs image-processing helpers (high ROI, low risk)

**The duplication:**

| Function | macOS | Windows | Linux |
|---|---|---|---|
| `png_bytes_to_jpeg(png_bytes, quality)` | ✓ 24 lines | ✓ 13 lines | ✓ 13 lines |
| `resize_png_if_needed(png_bytes, max_dim)` | ✓ 45 lines | ✓ 22 lines | ✓ 20 lines |
| `crosshair_png_bytes(png_bytes, cx, cy)` | ✓ 10 lines | ✓ 34 lines | ✓ varies |
| `write_crosshair_png(...)` (macOS only — wraps `crosshair_png_bytes` + writes to disk) | ✓ | — | — |
| `png_dimensions(data)` | ✓ 12 lines | ✓ 8 lines | ✓ 9 lines |
| `write_uncompressed_png(rgba, w, h)` | (uses CGImage) | ✓ 44 lines | ✓ 20 lines |
| `write_png_chunk(out, name, data)` | (CGImage) | ✓ 8 lines | ✓ 6 lines |
| `zlib_store(data)` | (CGImage) | ✓ 23 lines | ✓ 18 lines |
| `adler32(data)` | (CGImage) | ✓ 9 lines | ✓ 5 lines |
| `crc32_ieee(name, data)` | (CGImage) | ✓ ~25 lines | ? |

**What stays per-platform:**
- `screenshot_window_bytes(window_id_or_hwnd_or_xid) -> Vec<u8>` — calls CGImage on macOS / BitBlt+PrintWindow on Windows / XGetImage on Linux
- `screenshot_display_bytes() -> Vec<u8>` — same

**Estimated savings:** ~400 lines across 3 platforms. Pure refactor with zero behavioural change. The `image` crate is already a workspace dependency (used by `resize_png_if_needed`'s lanczos resampler).

**Extraction target:** new module `mcp_server::image_utils` (in `crates/mcp-server/src/image_utils.rs`). All callers already depend on mcp-server.

**Risk:** very low — pure functions, no platform-specific deps, no FFI.

### 🥈 #2 — overlay.rs RenderState + render pipeline (highest absolute ROI, medium risk)

**The duplication:**

| Item | macOS overlay.rs | Windows overlay.rs | Linux overlay.rs |
|---|---|---|---|
| `struct RenderState { ... }` | 42 fields | 24 fields | 22 fields |
| `struct Spring { ox, oy, vx, vy }` | identical | identical | identical |
| `impl RenderState::new(cfg)` | ~25 lines | ~25 lines | ~25 lines |
| `impl RenderState::tick(dt)` | ~50 lines (path planner + spring physics) | ~50 lines (same) | ~50 lines (same) |
| `impl RenderState::apply_command(cmd)` | ~80 lines (OverlayCommand match arms) | ~80 lines (same) | ~80 lines (same) |
| `fn render_frame(rs) -> Pixmap` | ~135 lines tiny_skia | ~95 lines (same logic, fewer features) | ~78 lines (same) |
| `fn draw_default_arrow(...)` | ~175 lines tiny_skia path | ~63 lines (same path, simpler) | ~52 lines (same) |
| `fn rotate_toward` | ✓ already extracted to cursor-overlay | ✓ already extracted | ✓ already extracted |

**What stays per-platform:**
- The window-creation + message-loop / runloop: AppKit on macOS, Win32 message loop on Windows, X11 / Wayland on Linux
- `dispatch_set_layer_contents` (macOS) / `update_layered_window` (Windows) / `paint_x11` (Linux) — platform-specific paint plumbing

**Estimated savings:** ~1800 lines across 3 platforms (~600 per platform). This is the **biggest absolute win** in the codebase.

**Extraction target:** `cursor-overlay` crate. Add `RenderState` + `Spring` + `tick` + `apply_command` + `render_frame` + `draw_default_arrow` to `cursor-overlay::lib`. Each platform's `overlay.rs` becomes a thin "platform paint loop" wrapper that:
1. Owns the platform Window / Layer / X11Window resource
2. Owns the per-tick paint dispatch (CGImage / DIB / XImage)
3. Calls `cursor_overlay::RenderState::tick(dt)` → `cursor_overlay::render_frame(rs)` → platform paint

**Risk:** medium. The `RenderState` field sets differ slightly per platform (virt_x/y/w/h on Windows are Win32-DIP-specific; macOS uses CGFloat in NSScreen coords). Need to either:
- Generalise `RenderState` with platform-agnostic `virt_bounds: (f64, f64, f64, f64)` and let each platform convert
- OR split into `core: RenderStateCore` (shared) + `platform: WindowsRenderState { core, virt_w_dip }` (specific)

**Suggested approach:** the split — `RenderStateCore` shared, per-platform wrapper structs that hold the platform-specific extras.

### 🥉 #3 — element_index caches (medium ROI, medium risk)

**The duplication:**

| Module | Size | Pattern |
|---|---|---|
| `platform-macos/src/ax/cache.rs` | 82 lines | Per-pid LRU keyed on `(pid, window_id)` holding `Vec<AXElement>` |
| `platform-windows/src/uia/cache.rs` | 78 lines | Same pattern, holds `Vec<IUIAutomationElement>` (refcounted) |
| `platform-linux/src/atspi/cache.rs` | 45 lines | Same pattern, holds AT-SPI accessibles |

**Estimated savings:** ~150 lines across 3 platforms.

**Extraction target:** `mcp-server::element_cache` — generic `ElementCache<T>` over the platform-specific element type. Each platform inserts its own T.

**Risk:** medium — the cache's lifetime semantics differ (macOS AXUIElementRef is CFType-refcounted; Windows IUIAutomationElement is COM-refcounted; Linux uses raw pointers). Generic-T cache handles this cleanly with `Drop` on T.

### #4 — Tool argument-parsing boilerplate (low ROI per tool, but many tools)

Every tool in every platform repeats:

```rust
let pid = match args.get("pid").and_then(|v| v.as_i64()) {
    Some(p) => p as i32,
    None    => return ToolResult::error("Missing required integer field pid."),
};
let window_id = ...;
let element_index = ...;
```

**The duplication:** ~30 tools × 3 platforms × ~5 fields per tool = ~450 small repetitions, often differing by 1-2 chars in the error message.

**Estimated savings:** ~600 lines, but each one is 2-4 lines so individually small.

**Extraction target:** `mcp-server::tool_args` helper:
```rust
pub trait ArgsExt {
    fn pid(&self) -> Result<i32, ToolResult>;
    fn window_id(&self) -> Result<u32, ToolResult>;
    fn element_index(&self) -> Result<Option<u64>, ToolResult>;
    fn javascript(&self) -> Result<String, ToolResult>;
    // etc.
}
```

**Risk:** very low — pure refactor of argument parsing. Per-platform tools opt into the helper. The CodeRabbit fix on PR #1666 (`i32::try_from` instead of `as i32`) becomes free for every caller.

### #5 — list_apps / list_windows shape (low ROI, high risk)

Each platform implements its own `list_apps` / `list_windows`. The RESPONSE shape (the JSON fields cua-driver returns) is normalized via the `WindowInfo` / `AppInfo` structs — but those structs themselves are duplicated per platform crate.

**Estimated savings:** ~50 lines for the struct definitions; doesn't dedupe the actual `list_*` functions.

**Risk:** high — `WindowInfo` on Windows carries an `hwnd: u64`, on macOS a `window_id: u32`, on Linux an `xid: u64`. The field semantics overlap but the types differ; a unified struct needs a typed wrapper.

**Recommendation:** **skip** this one unless the response shape changes warrant it. The boilerplate is small relative to the win.

### #6 — Tool description strings (zero ROI)

Each platform's tool has its own description string (already verified in PR #1666). These are intentionally per-platform — they describe per-platform behaviour (Windows uses UIA, macOS uses AX, Linux uses AT-SPI). Don't try to dedupe.

## Recommended PR sequence

All five candidates landed on `cross-platform-dedup-audit` (PR #1670).

| PR | What | Estimated | Actual | Status |
|---|---|---|---|---|
| #A | `mcp-server::image_utils` — capture.rs PNG/JPEG/crosshair/resize helpers | ~400 | **−399** | ✓ shipped (`9c998916` + `921dcdc9`) |
| #1b | `cursor-overlay::Spring` — physics struct | small | small | ✓ shipped (`f10a3258`) |
| #B | `cursor-overlay::render_state` — `RenderStateCore` + `tick` + `apply_command` + `render_frame` + `draw_default_arrow` | ~1800 | **−947 across overlays / −211 net** | ✓ shipped (`e0336893`) |
| #C | `mcp-server::tool_args` — `ArgsExt` helper trait | ~600 | **−125 consumers + trait/tests** | ✓ shipped (`f1c0cca1` + `3824b0d7`) |
| #2 | `mcp-server::element_cache` — generic `ElementCacheCore<K, S>` | ~150 | **+14 net** | ✓ shipped (`f798e3ad`) |

## What the audit got wrong

Worth recording for future audits — three places the original estimates were off:

- **#B "byte-for-byte identical tick/apply_command" was wrong.** macOS uses Swift-reference hardcoded constants (peakSpeed=900, springK=400, springC=17, overshoot=0.8) while Windows/Linux use runtime `MotionConfig`. macOS's `MoveTo` and `ClickPulse` have sentinel-snap behaviour the others don't. The shared core resolves this with two tick variants (`tick_motion` vs `tick_swift_constants`) and a parameterised `apply_command_base(snap_mt, click_only)`. So `RenderStateCore` is real, but it's a *family* of behaviours, not one.
- **#2 line-count estimate was optimistic.** The three caches were already terse (Linux at 45 lines); the wrapper indirection costs almost as much as the shared plumbing saves. Net is +14 lines on the cache files. The win is structural (one place to add metrics / eviction / instrumentation), not raw lines.
- **#B `start_t: Instant`** was dead code on Windows + Linux (assigned, never read) — not a real shared field, dropped during extraction.

## What this audit did NOT cover

- **Cross-os FFI bindings** — windows-rs vs core-foundation-rs vs x11rb — each platform crate's transitive deps are different; deduping is mostly about depending on the right shared crate, not refactoring code.
- **MCP protocol layer** — already lives in `mcp-server`, no duplication.
- **`installed_apps.rs` (Linux) / `apps/` (macOS) / `win32/apps.rs` (Windows)** — same shape but completely different impls (XDG desktop files / NSWorkspace / Start Menu shortcuts). Sharing the response struct only.
- **#5 `list_apps` / `list_windows` response shape** — recommended skip (high risk for ~50 lines).
- **#6 Tool description strings** — recommended skip (intentionally per-platform).
