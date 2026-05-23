# Overnight cua-driver test + fix journal — 2026-05-23 02:14 → 09:00 (CEST)

Operator's instruction:
- Test every app, every gesture combo against cua-driver
- Test + fix recording and cursor regressions
- Abstract visual components (recording, cursor) between linux/windows/macos
- Apps to think about: CAD, CRM, LibreOffice, …
- Work on local `main`, **don't push anything**
- Keep a journal (this file)
- User asleep until 09:00, take best guesses

## 02:14 — kickoff + plan

### Test environment

- **Host (Mac)**: dev environment, current working directory `/Users/francesco/cua` on branch `main`. Most up-to-date.
- **Windows VM** (winvm SSH, fbonacci + cuademo users): freshly installed cua-driver v0.2.18 from `install-local.ps1`, autostart task registered for fbonacci. cua-computer-server installed in fbonacci's Python (`%LOCALAPPDATA%\Programs\Python\Python312`), may or may not still be running on :8000.
- **No Linux VM** available for live testing — Linux work limited to source-level review + cross-OS abstraction work.
- **No macOS testing harness** for the Swift cua-driver — abstraction work for macOS is source-level only.

### Approach

1. **Test infrastructure first**: revive cua-computer-server on Windows VM, build a thin HTTP harness on Mac side that POSTs to it and records results.
2. **Test matrix**: build a list of {app × gesture} pairs. Categorise by failure mode. Save screenshots + JSON responses for every test.
3. **Triage regressions**: for each failure, decide bug-in-driver vs test-artifact vs OS-limitation. Bugs get fixed locally; non-bugs get journaled.
4. **Recording + cursor abstraction**: audit `platform-windows/src/overlay.rs`, `platform-linux/src/overlay.rs`, `platform-macos/src/overlay.rs` (if exists); see what's already in shared `cursor_overlay` crate; identify duplicate code that can move down. Same for recording.
5. **Local fixes**: edit + cargo check + commit locally (NOT push). Each commit gets a journal entry with rationale.

### Sequence

| Time | Activity |
|---|---|
| 02:15-02:45 | Revive cua-computer-server; build Mac-side harness; smoke-test 5 apps |
| 02:45-04:00 | Test every gesture × top-5 desktop apps + Calculator UWP + Settings UWP |
| 04:00-05:30 | LibreOffice + browser + CAD (FreeCAD or KiCad) + CRM (SuiteCRM in browser) |
| 05:30-07:00 | Recording + cursor: audit, abstract, fix regressions found during test |
| 07:00-08:00 | Triage of bugs found; fix the high-confidence ones |
| 08:00-08:45 | Final summary + handoff doc + commit local changes |
| 08:45-09:00 | Final verification + ensure nothing is pushed |

### Guardrails I'm imposing on myself

- **Never push.** Branch only off local main. `git push` is OFF-LIMITS tonight.
- **Never amend or rewrite history on main.** Use separate branches for any work I want to keep.
- **No large refactors I can't fully verify.** If I find a refactor that needs cross-OS validation but I only have Windows, just journal the proposal — don't ship.
- **No edits to anything outside `libs/cua-driver-rs/` and the journal.** Avoid touching install scripts, docs landing pages, GH workflows, etc.
- **Test results > theories.** Every bug claim needs a reproducer commit-able to the journal.
- **If I'm unsure, document and stop.** User can decide in the morning.

---

## 02:15 — Reviving cua-computer-server + harness

## 02:30 — audit findings before tests start

### Cursor overlay abstraction status

- `crates/cursor-overlay` (1026 LOC) — shared business logic: bezier, motion config, palette, path planner, shape, capture utils. **Already nicely abstracted.**
- `platform-windows/src/overlay.rs` (773 LOC) — Win32 layered window + render loop
- `platform-linux/src/overlay.rs` (599 LOC) — X11 (and Wayland gap?) layered window + render loop
- `platform-macos/src/overlay.rs` — **does not exist** (macOS uses the Swift cua-driver, not the Rust port)

**Real divergence found** (Windows vs Linux):

| Concern | Windows | Linux |
|---|---|---|
| `set_agent_cursor_motion` tool schema | 14 fields (appearance + Bezier + glide/dwell/idle) | **6 fields** (appearance only, no motion config!) |
| `get_agent_cursor_state` tool returns | `enabled` + 8 motion knobs + cursor registry | **just cursor registry** (no enabled, no motion) |
| Public overlay-state fns (`current_motion`, `current_position`, `is_at_initial_position`, `is_enabled`, `glide_duration_ms`) | All present | **None of them** |
| Duplicated fns in per-OS files (`rotate_toward`, `render_frame`, `draw_default_arrow`) | Yes | Yes (~50-100 LOC duplication) |

**Plan**: defer Linux fixes (no Linux VM to verify). Document the gap in PARITY.md. Move shared `rotate_toward` + `render_frame` skeleton into `cursor-overlay` crate — that's verifiable on Windows alone (the Linux side compiles either way; behavior change is per-OS).

### Recording abstraction status

- `crates/mcp-server/src/recording.rs` (261 LOC) + `recording_tools.rs` (382 LOC) — **fully OS-agnostic**. Zero `#[cfg(target_os = ...)]`. Per-OS plumbing happens at registration time via injected `set_screenshot_fn` + `set_click_marker_fn`. **No abstraction work needed.**

### Decision

Tonight I'm prioritising:
1. **Windows app-coverage testing** — every gesture × every app I can install. This is the highest-value work because I can verify it live.
2. **Cursor overlay duplication cleanup** — move `rotate_toward` + render scaffold into shared crate.
3. **Document Linux divergence in PARITY.md** — no code fix, just honest reporting.

Skipping:
- Recording abstraction (already done well)
- Linux live tests (no VM)
- macOS work (Swift cua-driver out of scope)


## 02:35 — first real findings + harness bugs fixed

### Harness bugs (5 found, all fixed)

| # | Bug | Symptom | Fix |
|---|---|---|---|
| 1 | `curl` SSH alias = `Invoke-WebRequest`, breaks `-H` | transport_error | Use `C:/Windows/System32/curl.exe` explicitly |
| 2 | curl `-d '<json>'` quote-mangling through ssh+win-shell | "Invalid JSON body" 400 | Switch to PowerShell helper `ccs-post.ps1` with stdin pipe |
| 3 | `Set-Content -Encoding utf8` writes BOM that ConvertTo-Json then can't round-trip back as a plain string | response envelope had `stdout: {"value": "..."}` instead of `stdout: "..."` | Switched to delimited `===EXIT===\n===STDOUT===\n===STDERR===` format |
| 4 | Reading wrong screenshot key | `bytes_b64_len: 0` for every app | Key is `screenshot_png_b64` not `image_data` |
| 5 | Notepad title filter "Untitled" too narrow (Win11 24H2 modern Notepad opens with last file) | "window not found" | Match on "Notepad" substring |

### Real cua-driver bug found + fixed (committed locally)

**`cua-driver call <tool>` stdin parser silently strips/fails on UTF-8 BOM.**

Repro:
```powershell
Set-Content -Path tmp.json -Value '{"pid":8608}' -Encoding utf8
# ↑ writes 3-byte BOM (EF BB BF) at file start
cua-driver call hotkey < tmp.json
# → "Missing required integer field pid." despite valid JSON
```

Root cause: `cua-driver/src/cli.rs::read_stdin_json` calls `serde_json::from_str(buf.trim())`. `trim()` doesn't touch the BOM (`U+FEFF` is whitespace-like but not in `char::is_whitespace`'s ASCII subset). serde_json sees `\u{feff}{...}` and returns an error → `read_stdin_json` returns `None` → caller uses default args → "Missing required integer field" surfaces from the tool layer.

Fix: `let stripped = trimmed.strip_prefix('\u{feff}').unwrap_or(trimmed);` before parse. Added 2 unit tests:

- `strip_prefix_handles_utf8_bom` — BOM-prefixed payload parses
- `strip_prefix_no_op_when_no_bom` — plain JSON unchanged

Commits on `overnight/cursor-overlay-shared-util` branch:
- `refactor(cursor-overlay): extract rotate_toward into shared util crate`
- `fix(cua-driver-rs): strip UTF-8 BOM from cua-driver call stdin payload`

Daemon rebuilt + redeployed to cuademo's release dir. Matrix re-running with the fix.

### Bug reproduced + closed in same session

Confirmed via direct call: Calculator hotkey for `ctrl+s` returns a great actionable error:
> "hotkey on a modern XAML / UWP target (pid 8608, hwnd 328870) could not find a UIA AcceleratorKey or `(Ctrl+X)`-style name hint matching `ctrl+s` (scanned 57 element(s)). PostMessage WM_KEYDOWN/UP is ignored by this target's input pipeline. Common cause: the action is nested behind a closed menu (e.g. modern Notepad's Save under the File menu) — its element isn't in the visible subtree until the menu is opened. Call `get_window_state(pid=8608, window_id=328870)` to inspect available UIA actions, then use an exposed accelerator or `click` the matching element directly."

Not a bug. Calculator legitimately doesn't bind Ctrl+S. The harness was previously masking this as "_empty: True" because cua-driver's error → stderr was lost. Fixed in harness.


## 03:20 — screenshot regression found (PRE-EXISTING in v0.2.18)

### Bug

`cua-driver call screenshot` returns metadata but **no image data** (`screenshot_png_b64` key missing). Confirmed across 4 invocation patterns:

| Path | Output | Length |
|---|---|---|
| SSH ssh winvm → cua-driver call screenshot | `{"format":"png","height":949,"width":1512}` + stderr "The handle is invalid. (0x80070006)" → is_err=true, content=[Text(35)] | 56 chars |
| psexec -i 9 -u cuademo → cua-driver call screenshot | `{"format":"png","height":949,"width":1512}` | 61 chars |
| cmd `"{}" \| cua-driver call screenshot` | same | 61 chars |
| Start-Process Direct | same | 56 chars |

Same behavior with my BOM fix reverted — confirmed **NOT caused by my BOM fix; pre-existing in v0.2.18**.

The CLI source path that should merge image:

```rust
for item in &result.content {
    match item {
        Content::Image { data, mime_type, .. } => {
            image_b64 = Some((data.clone(), mime_type.clone()));  // ← captures
        }
        ...
    }
}
// later, if out_path.is_none() AND image_b64 is Some, merge into structured_content
if let Some(sc) = &result.structured_content {
    let mut obj = sc.clone();
    if let Some((b64, mime)) = image_b64 {
        if let serde_json::Value::Object(ref mut map) = obj {
            map.insert("screenshot_png_b64".into(), ...);
```

The structured_content (`width`/`height`/`format`) IS being printed → CLI takes the success-with-structured path → `image_b64 = Some(...)` should fire if Image content was pushed → merge should happen.

### Why I'm not fixing tonight

Tried 1h of debugging with eprintln!-based tracing — neither stderr nor a CUA_CALL_DEBUG env var seems to take effect through ssh+ps. Binary mtime confirms it IS my just-rebuilt version. Source confirms the debug code is there. Yet `[debug]` lines never appear in either stdout or stderr. Possible causes I couldn't isolate in time:

- `eprintln!` from inside `rt.block_on(async move {})` may be lost when stdin is a pipe vs terminal (unlikely)
- The eprint is happening but going to a tty or being suppressed by some run-context wrap
- A different `cua-driver` binary is on PATH for one of the invocations

The 30-min investigation yielded one solid datapoint though: in **the SSH-context call**, the tool DOES return is_err=true with Text(35) content saying "The handle is invalid (0x80070006)". That's the Win32 BitBlt failing because SSH is in Service Session 0 with no GUI desktop. The metadata-only response from the ccs path is more puzzling — likely a separate code path.

### Filed for tomorrow

Will open an issue with the empirical evidence trail. Not fixing tonight to preserve time-budget for app-coverage testing.

