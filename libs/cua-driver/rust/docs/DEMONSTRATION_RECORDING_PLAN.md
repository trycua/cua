# Demonstration Recording → Skills (cua-driver)

Goal: let any agent harness that drives cua-driver record a **human (or agent) demonstration on a
specific window** — with a visible glowing "you are being recorded" border — and get back a
**readable, token-cheap trajectory** it can turn into a reusable skill. Optionally auto-author the
skill via the Anthropic API when a key is present.

This generalizes the `cua skills record` CLI feature (TS/Python, browser/VNC-based) and the
computer-use demonstration-skill format down into cua-driver itself, so it works for every harness
(Claude Code, OpenClaw, OpenCode, raw MCP).

## What exists today (baseline)

- `RecordingSession` (`crates/cua-driver-core/src/recording.rs`) records **only the agent's own
  tool calls** routed through `ToolRegistry::invoke` (`tool.rs:422`). It writes `turn-NNNNN/`
  folders (`action.json`, `screenshot.png`, `click.png`, `app_state.json`), plus session-level
  `session.json`, `cursor.jsonl`, optional `recording.mp4`.
- **No human keyboard/mouse capture.** Only passive *cursor-position* polling (`cursor_sampler.rs`).
- **No glowing window border** on Windows/Linux. macOS has a reusable `FocusRect` overlay primitive
  in `cursor-overlay` (`render_state.rs:505`), wired only on macOS; Windows/Linux drop the command.
- **No jsonl→readable-markdown processing.** `recording_render.rs` only makes a polished zoom MP4.
- MCP tools live in `recording_tools.rs`, registered via `register_recording_tools` (`tool.rs:311`).

## Decisions (locked)

1. **Cross-platform core, Windows impl first.** Capture/border/processing logic lives behind traits
   in `cua-driver-core`; Windows backend is implemented and tested now; mac/linux stubbed.
2. **Window-scoped, redacted keys.** The input hook only records events while the recorded window is
   foreground. Keystrokes are coalesced into `type_text` events that store **redacted** text by
   default (length + character classes, e.g. `"•••• (8 chars, alnum)"`); navigation/control/hotkeys
   (Enter, Tab, Esc, Ctrl+C, …) are recorded by semantic name. Raw literal capture is **opt-in**
   (`capture_raw_text: true`). The hook is installed only between `start`/`stop` and force-detached
   on stop, process exit, and panic. **Not a system-wide keylogger at rest.**
3. **Local Anthropic-key tool + always-on skill.** Processing always emits the readable markdown
   trajectory + a bundled meta-skill (`demonstration-to-skill`) that teaches a host agent how to
   read a trajectory and author a good SKILL.md. Additionally, an MCP tool
   `process_recording` (and `author_skill`) calls the Anthropic API to auto-write SKILL.md **only
   when `ANTHROPIC_API_KEY` is set**; otherwise it returns the processed dir + instructions.

## Architecture

### New crate: `input-capture` (`crates/input-capture/`)
Window-scoped human input capture behind a platform trait.

```rust
pub struct CaptureConfig { pub pid: i64, pub window_id: u64, pub capture_raw_text: bool }
pub enum HumanEvent {
    MouseDown { x, y, button, t_ms }, MouseUp{..}, Click{ x, y, button, t_ms },
    Scroll { x, y, dx, dy, t_ms },
    Drag { from:(f64,f64), to:(f64,f64), button, t_ms },
    Key { name: String, mods: Mods, t_ms },           // semantic: Enter/Tab/F2/Ctrl+C
    Text { redacted: String, raw: Option<String>, char_count: usize, t_ms }, // coalesced typing
    FocusGained{t_ms}, FocusLost{t_ms},
}
pub trait InputCapture: Send {
    fn start(cfg: CaptureConfig, sink: Sender<HumanEvent>) -> Result<Self> where Self: Sized;
    fn stop(self);
}
```

- **Windows backend** (`SetWindowsHookEx(WH_KEYBOARD_LL, WH_MOUSE_LL)` on a dedicated thread with
  its own message loop). Every callback first checks `GetForegroundWindow()`/owning pid == recorded
  window; events outside the window are **dropped, never stored**. Keystrokes feed a
  `KeystrokeCoalescer` that emits `Text` (redacted) on focus loss / non-text key / timeout. Hook is
  unhooked on `stop`/drop. (`WH_*_LL` need a running message loop, not necessarily elevation.)
- **mac/linux**: trait stubs returning `Unsupported` (mac later via `CGEventTap`+AX grant).

### Recording integration (`recording.rs`)
- `start()` gains `demonstration: bool` + `capture_raw_text` + `pid/window_id`. When set, spins up
  `InputCapture` and shows the border overlay. Human events are written as `turn-NNNNN/` folders in
  the **same schema** as agent actions (`source: "human"` field), interleaved with any agent
  actions by `t_ms`. Screenshot per human turn captured via existing `screenshot_for`.
- On `stop()`, capture + border are torn down; `session.json` records `mode: "demonstration"`.

### Glowing border overlay
- Add `OverlayCommand::ShowRecordingBorder(Option<[f64;4]>)` (persistent, animated pulse, distinct
  red/amber from the cyan FocusRect). Reuse `cursor-overlay` rasterization.
- **Windows**: implement a layered click-through `WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_NOACTIVATE`
  topmost overlay window that tracks the recorded window's rect (reuse focus-monitor patterns). This
  also unblocks the currently-dropped `ShowFocusRect` on Windows.
- macOS reuses FocusRect path with a non-fading variant; linux stub.

### Processing pipeline (`recording_markdown.rs` — new)
`process(dir) -> ProcessedDir`:
- Walks `turn-*/`, builds `TRAJECTORY.md`: one section per turn with human-readable action text,
  **relative** links to `turn-NNNNN/screenshot.png` / `click.png` (never base64 inline).
- Dedupes/selects "key frames" (e.g. first, last, frames where window state changed materially) so
  a reader sees 1–2 images, not 100. Full per-turn images remain on disk, linked but not inlined.
- Emits `SUMMARY.json` (counts, duration, apps/windows touched, action histogram).
- Pure Rust, no network. This is the always-available output.

### Skills
- Bundle `Skills/cua-driver/demonstration-to-skill/SKILL.md`: how to read `TRAJECTORY.md` and author
  a good SKILL.md (Open Agent Skills Standard: frontmatter, Steps with Context/Intent/Expected Result, Agent
  Prompt). Linked from the main cua-driver SKILL.md and `RECORDING.md`.

### MCP tools (`recording_tools.rs`)
- Extend `start_recording`: `mode:"demonstration"`, `pid`, `window_id`, `capture_raw_text`.
- New `process_recording({dir})` → runs the markdown pipeline; if `ANTHROPIC_API_KEY` set and
  `author_skill:true`, calls Anthropic to write `SKILL.md` into the dir. Returns paths + the
  meta-skill instructions when no key.
- `get_recording_state` reports `mode` and human-event count.

## Status (this PR)

Implemented + tested on Windows:
- `input-capture` crate: events/redaction/coalescer/gate/Windows hook + glowing
  border indicator + `Demonstration` RAII coupling. 20 unit tests.
- `recording_markdown`: readable `TRAJECTORY.md` + `SUMMARY.json`. 3 unit tests,
  validated against a real cua-driver recording.
- `recording.rs` demonstration mode (human turns interleaved with agent turns),
  `start_recording` demonstration params, `process_recording` (+ optional
  Anthropic `author_skill`). 2 integration tests incl. a real Windows
  border+capture lifecycle/no-deadlock test.
- Glowing border visually confirmed around a live Notepad window.
- `DEMONSTRATION.md` skill guide (Open Agent Skills Standard convention).

Follow-ups (NOT in this PR):
- macOS (`CGEventTap` + `FocusRect` reuse) and Linux (`XInput2`/libei) capture
  backends — currently `Unsupported` stubs.
- The cua-bench parity harness below — designed here, not yet wired. Human input
  can't be injected without the `*_INJECTED` flag (which capture deliberately
  drops), so faithful parity needs a hardware-level injector or a test seam that
  feeds synthetic `HumanEvent`s past the OS hook.

## Benchmark / parity loop (`cua-bench` + Windows examples) — designed, follow-up
Use the existing `replay_trajectory_parity.rs` harness pattern + `cua-bench-basic` simple tasks:
1. In a container/window, send OS-level inputs directly to a `cua-bench-basic` task (e.g.
   `click-button`, `typing-input`) and score via `window.__score`.
2. Record the same actions as a demonstration; process; `replay_trajectory` the resulting turns.
3. Compare scores + action fidelity (directly-actuated vs cua-driver replay). Track:
   - replay success rate, per-action latency, coordinate drift, redaction-safe text round-trip.
4. Iterate on coalescing/timing/coordinate-mapping until replay matches direct actuation.

## Iteration / test loop (Windows, this machine)
1. `cargo build -p input-capture && cargo test -p input-capture` (coalescer/redaction unit tests,
   no real hooks).
2. `cargo build -p cua-driver` then `cua-driver serve`; via MCP: `start_recording` in demonstration
   mode on a Notepad/Calc window, **I drive that window with cua-driver tools AND with raw OS input**
   to confirm both are captured, border shows, focus-scoping drops outside events.
3. `stop_recording`; inspect the output dir + `process_recording` → read `TRAJECTORY.md` (verify
   token-cheap, relative links, redaction).
4. `replay_trajectory` the dir; confirm the window ends in the same state.
5. Parity example against `cua-bench-basic`; record numbers in this doc.
6. Loop. Add parity example `demonstration_parity.rs` mirroring existing parity tests.

## Security model: indicator-gated capture ("recording-LED" coupling)

Hard requirement: **it must be impossible for the shipped, unmodified cua-driver to capture a single
human input event unless the glowing recording border is actually being painted on the target
window** — the software analog of a camera/mic LED wired to the sensor's power rail. We get as close
to that as software allows via three layers:

1. **Structural coupling (RAII).** There is no public API to start an input hook on its own. A
   `Demonstration` guard constructs the visible `Indicator` first and derives a `CaptureGate` from
   it; the hook can only be installed by handing it that gate. Dropping the guard (stop / process
   exit / panic via a drop-guard) tears down the indicator and the hook together.

2. **Proof-of-presence heartbeat (the "LED wire").** The indicator owns a shared
   `IndicatorHeartbeat`. Its render loop bumps `frame_seq` + `last_paint_ms` **only from inside an
   actually-presented frame** (WM_PAINT / present), so the heartbeat is literal evidence of a drawn
   border — it cannot be faked without drawing. The low-level hook callback calls
   `gate.allow(now)` **before buffering any event**, which requires: heartbeat fresh
   (`now - last_paint <= MAX_AGE`, ~250 ms), `frame_seq > 0`, and the indicator's self-reported
   `visible_ok`. No fresh frame ⇒ every event is dropped, never stored.

3. **Tamper watchdog.** The indicator runs a watchdog that each tick verifies its window is
   `WS_VISIBLE`, not cloaked (`DWMWA_CLOAKED`), topmost, layered-alpha above threshold, and still
   geometrically covering the target window's rect. If any check fails (someone hides/minimizes/
   moves/covers/alpha-zeroes the border, or the target moves away), it sets `visible_ok=false` and
   capture goes dark within one heartbeat — and the failure is surfaced in recording state.

Honest scope: a privileged local attacker who patches the binary or installs their own kernel hook
defeats any user-space scheme — that is out of scope and true of every software indicator. The
guarantee is about cua-driver itself: the unmodified binary cannot be driven (by a prompt, a rogue
agent, or a remote MCP caller) into capturing input without a visible, covering, live border.

## Security checklist
- Capture physically gated on a live indicator heartbeat (above); no indicator frame ⇒ no event.
- Hook installed only during an active demonstration; force-detached on stop/exit/panic (drop guard).
- Foreground-window + target-window scoping enforced inside the hook callback before any buffering.
- Redacted-by-default text; raw capture requires explicit `capture_raw_text:true` and is noted in
  `session.json` and surfaced in state.
- No persistence of dropped/out-of-window/indicator-dark events.
- Tamper watchdog stops capture if the border is hidden/moved/covered/dimmed.
- Document all of the above in `RECORDING.md`.

## Rollout
Branch `feat/demonstration-recording`; draft PR. Windows-complete, mac/linux stubbed with tracking
notes. Version bump deferred to a follow-up release commit.
