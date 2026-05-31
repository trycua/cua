//! Platform-independent recording / replay tools.
//!
//! Registered on all platforms via `ToolRegistry::register_recording_tools()`.
//!
//! - `start_recording`     — enable trajectory recording to disk (+ optional video)
//! - `stop_recording`      — disable trajectory recording, finalize video
//! - `get_recording_state` — query current recording state
//! - `replay_trajectory`   — replay a previously recorded trajectory
//!
//! Renamed from the older `set_recording(enabled: bool)` toggle in
//! `02f1f033..` — see `JOURNAL_VIDEO.md` for rationale. The split makes
//! the verbs match the CLI subcommand names (`cua-driver recording start|
//! stop|status`) and removes the "is this a setting write?" ambiguity of
//! the old `set_*` name.

use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{
    protocol::ToolResult,
    recording::{RecordingSession, RecordingState},
    tool::{Tool, ToolDef, ToolRegistry},
};

// ── Process-global weak reference to the registry (for replay) ───────────────
//
// Set once by `ToolRegistry::init_replay(weak)` after `Arc::new(registry)`.

static REPLAY_REGISTRY: OnceLock<Weak<ToolRegistry>> = OnceLock::new();

/// Called from `main.rs` after wrapping the registry in `Arc`.
pub fn init_replay_registry(weak: Weak<ToolRegistry>) {
    let _ = REPLAY_REGISTRY.set(weak);
}

fn get_replay_registry() -> Option<Arc<ToolRegistry>> {
    REPLAY_REGISTRY.get()?.upgrade()
}

// ── start_recording ──────────────────────────────────────────────────────────

pub struct StartRecordingTool {
    session: Arc<RecordingSession>,
}

impl StartRecordingTool {
    pub fn new(session: Arc<RecordingSession>) -> Self { Self { session } }
}

static START_REC_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for StartRecordingTool {
    fn def(&self) -> &ToolDef {
        START_REC_DEF.get_or_init(|| ToolDef {
            name: "start_recording".into(),
            description: "Start trajectory recording. Every subsequent action-tool \
                invocation (click, right_click, scroll, type_text, press_key, hotkey, \
                set_value) writes a turn folder under `output_dir`:\n\n\
                - `app_state.json` — post-action AX/UIA snapshot for the target pid.\n\
                - `screenshot.png` — post-action per-window screenshot of the target's \
                  frontmost on-screen window.\n\
                - `action.json` — tool name, full input arguments, result summary, pid, \
                  click point (when applicable), ISO-8601 timestamp.\n\
                - `click.png` — for click-family actions only, `screenshot.png` with a \
                  red dot drawn at the click point.\n\n\
                Turn folders are named `turn-00001/`, `turn-00002/`, etc.  Turn \
                numbering restarts at 1 each time recording is (re-)started.\n\n\
                **Video is off by default.** Pass `record_video: true` to also \
                capture the main display to `<output_dir>/recording.mp4` (H.264 / \
                30 fps) for the lifetime of the session. The recording is torn \
                down automatically when the MCP client disconnects.\n\n\
                **macOS uses native ScreenCaptureKit** (in-process SCStream + \
                SCRecordingOutput) so video inherits Cua Driver's own Screen \
                Recording grant — no extra TCC prompt, no ffmpeg subprocess. \
                Requires macOS 15.0+.\n\n\
                **Windows + Linux use an ffmpeg subprocess** (`gdigrab` / \
                `x11grab` + libx264). Requires ffmpeg on PATH (winget install \
                Gyan.FFmpeg / apt install ffmpeg); when ffmpeg is missing or \
                fails on startup the per-turn capture (screenshots + \
                action.json) still runs and the session's `last_error` field \
                carries the diagnostic.\n\n\
                State persists for the life of the daemon / MCP session; a restart \
                resets to disabled with no on-disk state. Call `stop_recording` to \
                disable + finalize the mp4.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["output_dir"],
                "properties": {
                    "output_dir": {
                        "type": "string",
                        "description": "Absolute or ~-rooted directory where turn folders \
                            and (when enabled) the video file are written."
                    },
                    "record_video": {
                        "type": "boolean",
                        "description": "Capture the main display to <output_dir>/recording.mp4. \
                            Default: false. Set to true to also capture the main \
                            display to recording.mp4 (otherwise only the per-turn \
                            screenshots + JSON are recorded). On macOS this uses native \
                            ScreenCaptureKit (no extra TCC prompt, macOS 15.0+); on \
                            Windows + Linux it requires ffmpeg on PATH."
                    }
                },
                "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::tool_args::ArgsExt;
        let output_dir = args.opt_str("output_dir");
        if output_dir.as_deref().map(str::is_empty).unwrap_or(true) {
            return ToolResult::error("`output_dir` is required.");
        }
        let record_video = args.bool_or("record_video", false);

        match self.session.start(output_dir.as_deref().unwrap(), record_video) {
            Ok(()) => {
                let state = self.session.current_state();
                // When the caller asked for video and it failed (e.g. macOS
                // ffmpeg TCC prompt deadlock), surface the actual error
                // prominently — the per-turn capture still runs, but the
                // caller deserves to know the mp4 won't materialize.
                let video_failed = record_video && !state.video_active;
                let video_note = if record_video && state.video_active {
                    " (video → recording.mp4)".to_string()
                } else if video_failed {
                    let err = state.last_error.clone().unwrap_or_else(|| "unknown".into());
                    format!("\n\n⚠️ Video capture failed (per-turn JSON+screenshot still running):\n{err}")
                } else { String::new() };
                let msg = format!("✅ Recording started -> {}{}",
                    state.output_dir.as_deref().unwrap_or("?"),
                    video_note);
                ToolResult::text(msg).with_structured(recording_state_json(&state))
            }
            Err(e) => ToolResult::error(format!("Failed to start recording: {e}")),
        }
    }
}

// ── stop_recording ───────────────────────────────────────────────────────────

pub struct StopRecordingTool {
    session: Arc<RecordingSession>,
}

impl StopRecordingTool {
    pub fn new(session: Arc<RecordingSession>) -> Self { Self { session } }
}

static STOP_REC_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for StopRecordingTool {
    fn def(&self) -> &ToolDef {
        STOP_REC_DEF.get_or_init(|| ToolDef {
            name: "stop_recording".into(),
            description: "Stop trajectory recording. Disables further per-turn capture \
                and, when video was enabled, gracefully terminates the ffmpeg subprocess \
                so the mp4's moov atom is finalized (the file is playable). Calling \
                stop on an already-stopped session is a no-op. The response carries \
                `last_video_path` pointing at the finalized mp4 (when video was on).\n\n\
                **Optional `generation` (ownership guard, #1764).** When present, the \
                stop only acts if it matches the current recording's generation token \
                (surfaced in `start_recording`'s `structuredContent.generation`); a \
                stale token is a silent no-op. This is used by the MCP proxy's \
                disconnect auto-stop so a client exiting doesn't stop a recording a \
                later client started. Omit it for a normal manual stop — that \
                unconditionally stops the active recording.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "generation": {
                        "type": "integer",
                        "description": "Ownership token from a prior start_recording's \
                            structuredContent.generation. When set, stop only acts if \
                            it matches the live recording's generation (stale = no-op). \
                            Omit for an unconditional manual stop."
                    }
                },
                "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::tool_args::ArgsExt;
        // Optional ownership token (#1764). `None` (omitted) → unconditional
        // manual stop; `Some(gen)` → guarded stop that no-ops on a stale token.
        match self.session.stop(args.opt_u64("generation")) {
            Ok(()) => {
                let state = self.session.current_state();
                let video_note = state.last_video_path.as_deref()
                    .map(|p| format!(" (video → {p})"))
                    .unwrap_or_default();
                ToolResult::text(format!("✅ Recording stopped.{video_note}"))
                    .with_structured(recording_state_json(&state))
            }
            Err(e) => ToolResult::error(format!("Failed to stop recording: {e}")),
        }
    }
}

// ── get_recording_state ───────────────────────────────────────────────────────

pub struct GetRecordingStateTool {
    session: Arc<RecordingSession>,
}

impl GetRecordingStateTool {
    pub fn new(session: Arc<RecordingSession>) -> Self { Self { session } }
}

static GET_REC_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for GetRecordingStateTool {
    fn def(&self) -> &ToolDef {
        GET_REC_DEF.get_or_init(|| ToolDef {
            name: "get_recording_state".into(),
            // Description ported from Swift `GetRecordingStateTool.swift`.
            description: "Report the current trajectory recorder state: whether recording \
                is enabled, the output directory (when enabled), and the 1-based counter \
                for the next turn folder that will be written. Counter increments on every \
                recorded action tool call and resets to 1 each time recording is \
                (re-)enabled.\n\n\
                Pure read-only.".into(),
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let state = self.session.current_state();
        // Match Swift text format 1:1:
        //   "✅ recording: enabled output_dir=<path> next_turn=<N>"
        //   "✅ recording: disabled"
        let summary = if state.enabled {
            format!(
                "recording: enabled output_dir={} next_turn={}",
                state.output_dir.as_deref().unwrap_or("?"),
                state.next_turn
            )
        } else {
            "recording: disabled".to_owned()
        };
        ToolResult::text(format!("✅ {summary}")).with_structured(recording_state_json(&state))
    }
}

// ── replay_trajectory ─────────────────────────────────────────────────────────

pub struct ReplayTrajectoryTool;

static REPLAY_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for ReplayTrajectoryTool {
    fn def(&self) -> &ToolDef {
        REPLAY_DEF.get_or_init(|| ToolDef {
            name: "replay_trajectory".into(),
            // Description ported from Swift `ReplayTrajectoryTool.swift`
            // with its caveats about element-indexed actions and recording-
            // during-replay semantics.
            description: "Replay a recorded trajectory by re-invoking every turn's tool call \
                in lexical order. `dir` must point at a directory previously written by \
                `start_recording`. Each `turn-NNNNN/` is parsed for `action.json`, and the \
                recorded tool is called with its recorded `arguments` via the same dispatch \
                path an MCP / CLI call uses.\n\n\
                Caveats:\n\
                - Element-indexed actions (`click({pid, element_index})` etc.) will fail \
                  because element indices are per-snapshot and don't survive across \
                  sessions. Pixel clicks (`click({pid, x, y})`) and all keyboard tools \
                  replay cleanly. Failures are reported but don't stop replay unless \
                  `stop_on_error` is true.\n\
                - `get_window_state` and other read-only tools are NOT currently recorded, \
                  so replays do not re-populate the per-(pid, window_id) element cache.\n\
                - If recording is ENABLED while replay runs, the replay itself is recorded \
                  into the currently configured output directory.  That's deliberate: \
                  recording a replay against a new build and diffing the two trajectories \
                  is the regression-test workflow.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["dir"],
                "properties": {
                    "dir":           { "type": "string",  "description": "Trajectory directory previously written by `set_recording`. Absolute or ~-rooted." },
                    "delay_ms":      { "type": "integer", "minimum": 0, "maximum": 10000, "description": "Milliseconds to sleep between turns, for human-observable pacing. Default 500." },
                    "stop_on_error": { "type": "boolean", "description": "Stop replay on the first tool-call error. Default true — set false to best-effort through the full trajectory." }
                },
                "additionalProperties": false
            }),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let dir_str = match args.get("dir").and_then(|v| v.as_str()) {
            Some(v) if !v.is_empty() => v.to_owned(),
            _ => return ToolResult::error("Missing required string field `dir`."),
        };
        use crate::tool_args::ArgsExt;
        let delay_ms = args.u64_or("delay_ms", 500).min(10_000);
        let stop_on_error = args.bool_or("stop_on_error", true);

        // Expand ~/
        let dir = {
            let p = std::path::PathBuf::from(&dir_str);
            if dir_str.starts_with("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    std::path::PathBuf::from(home).join(&dir_str[2..])
                } else { p }
            } else { p }
        };

        if !dir.exists() {
            return ToolResult::error(format!("Trajectory directory does not exist: {}", dir.display()));
        }

        // Collect and sort turn-NNNNN directories.
        let mut turn_dirs: Vec<_> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_dir() && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("turn-"))
                        .unwrap_or(false))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        turn_dirs.sort();

        if turn_dirs.is_empty() {
            return ToolResult::error(format!("No turn-NNNNN folders found under {}", dir.display()));
        }

        let registry = match get_replay_registry() {
            Some(r) => r,
            None => return ToolResult::error("Replay not available: registry not initialised yet."),
        };

        let mut attempted = 0u32;
        let mut succeeded = 0u32;
        let mut failed = 0u32;
        let mut turns_json = Vec::new();
        let mut first_failure: Option<(String, String, String)> = None;

        for turn_dir in &turn_dirs {
            let turn_name = turn_dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_owned();

            let action_path = turn_dir.join("action.json");
            let (tool_name, tool_args) = match parse_action_json(&action_path) {
                Ok(v) => v,
                Err(e) => {
                    failed += 1;
                    if first_failure.is_none() {
                        first_failure = Some((turn_name.clone(), "action.json".into(), e.to_string()));
                    }
                    turns_json.push(json!({
                        "turn": turn_name,
                        "ok": false,
                        "parse_error": e.to_string()
                    }));
                    if stop_on_error { break; }
                    continue;
                }
            };

            attempted += 1;
            let result = registry.invoke(&tool_name, tool_args).await;
            let is_err = result.is_error.unwrap_or(false);
            let summary = result.content.iter()
                .find_map(|c| {
                    if let crate::protocol::Content::Text { text, .. } = c {
                        Some(text.as_str())
                    } else { None }
                })
                .unwrap_or("")
                .to_owned();

            turns_json.push(json!({
                "turn": turn_name,
                "tool": &tool_name,
                "ok": !is_err,
                "result_summary": &summary,
            }));

            if is_err {
                failed += 1;
                if first_failure.is_none() {
                    first_failure = Some((turn_name.clone(), tool_name.clone(), summary));
                }
                if stop_on_error { break; }
            } else {
                succeeded += 1;
            }

            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
        }

        let dir_name = dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        let mut summary_text = format!(
            "replay {dir_name}: attempted={attempted} succeeded={succeeded} failed={failed}"
        );
        if let Some((ref turn, ref tool, _)) = first_failure {
            summary_text.push_str(&format!(" first_failure={turn}:{tool}"));
        }

        let mut structured = json!({
            "directory": dir.to_string_lossy(),
            "attempted": attempted,
            "succeeded": succeeded,
            "failed": failed,
            "stop_on_error": stop_on_error,
            "turns": turns_json,
        });
        if let Some((turn, tool, error)) = first_failure {
            structured["first_failure"] = json!({ "turn": turn, "tool": tool, "error": error });
        }

        ToolResult::text(summary_text)
            .with_structured(structured)
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn recording_state_json(state: &RecordingState) -> Value {
    json!({
        // "recording" mirrors the Swift field name for parity.
        "recording": state.enabled,
        "enabled": state.enabled,
        "output_dir": state.output_dir,
        "next_turn": state.next_turn,
        "last_error": state.last_error,
        "video_active": state.video_active,
        "last_video_path": state.last_video_path,
        // Ownership token for the proxy-exit teardown guard (#1764). The
        // disconnect auto-stop captures this off a successful start_recording
        // and passes it back so it can't stop a recording a later client owns.
        "generation": state.generation,
    })
}

fn parse_action_json(path: &std::path::Path) -> anyhow::Result<(String, Value)> {
    if !path.exists() {
        anyhow::bail!("Missing action.json");
    }
    let text = std::fs::read_to_string(path)?;
    let obj: Value = serde_json::from_str(&text)?;
    let tool = obj.get("tool")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("action.json missing 'tool' string field"))?
        .to_owned();
    let tool_args = obj.get("arguments").cloned().unwrap_or(Value::Object(Default::default()));
    Ok((tool, tool_args))
}
