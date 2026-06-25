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
                    },
                    "demonstration": {
                        "type": "boolean",
                        "description": "Record a HUMAN demonstration on a specific window. \
                            Shows a glowing recording border around the target window and \
                            captures the human's clicks/scrolls/drags/typing on that \
                            window (in addition to any agent tool calls), interleaved on \
                            one timeline. Capture is window-scoped and gated on the visible \
                            border being painted (you cannot capture input without the \
                            border showing). Typed text is REDACTED by default. Requires \
                            `pid` and `window_id`. Windows only for now. Default: false."
                    },
                    "pid": {
                        "type": "integer",
                        "description": "Target window's process id (demonstration mode)."
                    },
                    "window_id": {
                        "type": "integer",
                        "description": "Target window id / HWND (demonstration mode)."
                    },
                    "capture_raw_text": {
                        "type": "boolean",
                        "description": "Demonstration mode: also store literal typed text \
                            (needed for faithful replay) instead of only a redacted \
                            summary. Stores secrets — opt in deliberately. Default: false."
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
        let demonstration = args.bool_or("demonstration", false);
        let capture_raw_text = args.bool_or("capture_raw_text", false);
        let demo_pid = args.opt_i64("pid");
        let demo_window = args.opt_u64("window_id");
        // Daemon-injected ownership key (absent for one-shot CLI / anonymous
        // sessions). Stamps the recording so a session-scoped teardown
        // (session_end) only stops the recording its own session started.
        let owner = args.opt_str("_session_id");

        if demonstration && (demo_pid.is_none() || demo_window.is_none()) {
            return ToolResult::error(
                "demonstration mode requires both `pid` and `window_id`.",
            );
        }

        match self.session.start(output_dir.as_deref().unwrap(), record_video, owner.as_deref()) {
            Ok(()) => {
                // Begin human-input capture + glowing border if requested.
                let mut demo_note = String::new();
                if demonstration {
                    match self.session.begin_demonstration(
                        demo_pid.unwrap(),
                        demo_window.unwrap(),
                        capture_raw_text,
                    ) {
                        Ok(()) => {
                            demo_note = format!(
                                "\n\n🔴 Demonstration mode: glowing border on window {} — \
                                 human input on that window is being captured{}.",
                                demo_window.unwrap(),
                                if capture_raw_text { " (raw text)" } else { " (text redacted)" }
                            );
                        }
                        Err(e) => {
                            demo_note = format!(
                                "\n\n⚠️ Demonstration capture failed (agent-action recording \
                                 still running): {e}"
                            );
                        }
                    }
                }
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
                    let hint = if crate::video_ffmpeg::find_ffmpeg().is_none() {
                        "\n\nffmpeg was not found. Call install_ffmpeg (then again with \
                         confirm=true) to install it, then restart recording."
                    } else {
                        ""
                    };
                    format!("\n\n⚠️ Video capture failed (per-turn JSON+screenshot still running):\n{err}{hint}")
                } else { String::new() };
                let msg = format!("✅ Recording started -> {}{}{}",
                    state.output_dir.as_deref().unwrap_or("?"),
                    video_note, demo_note);
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
                A manual `stop_recording` is **unconditional** — it stops whatever \
                recording is active regardless of which session started it. \
                Ownership-scoped teardown (so one MCP client disconnecting can't stop a \
                recording a later client started) is handled by the daemon's \
                `session_end` lifecycle signal, not by this tool.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        // Manual stop is unconditional — `None` requester tears down whatever
        // recording is active. Session-scoped teardown is driven by the
        // daemon's `session_end` arm (serve.rs), which calls `stop_owner(sid)`.
        match self.session.stop_owner(None) {
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
        // Session that owns the live recording (the daemon-injected
        // `_session_id`), or null when started anonymously. Informational; the
        // proxy-exit teardown drives ownership via the daemon `session_end`
        // signal rather than reading this back.
        "owner": state.owner,
        // True when a human-input demonstration is active (glowing border on +
        // window-scoped capture). The "recording-LED": capture cannot run
        // without the border being painted.
        "demonstration": state.demonstration,
        "human_turns": state.human_turns,
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

// ── install_ffmpeg ────────────────────────────────────────────────────────────
//
// Confirmation-gated installer for the ffmpeg binary that the Linux/Windows
// video backend shells out to. Called without `confirm` it only REPORTS the
// command it would run (read-only preview); `confirm: true` runs it. Marked
// destructive + open_world so conforming MCP clients also gate it behind a
// human approval. ffmpeg is invoked as a separate process, never linked.

pub struct InstallFfmpegTool;
static INSTALL_FFMPEG_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for InstallFfmpegTool {
    fn def(&self) -> &ToolDef {
        INSTALL_FFMPEG_DEF.get_or_init(|| ToolDef {
            name: "install_ffmpeg".into(),
            description: "Install the ffmpeg binary used by start_recording's video \
                capture (Linux/Windows; macOS records natively and needs no ffmpeg). \
                Two-step and confirmed: called without `confirm` it only REPORTS the \
                exact install command for this platform's package manager; pass \
                `confirm: true` to actually run it. No-op if ffmpeg is already on PATH. \
                ffmpeg is run as a separate process, never linked into the driver."
                .into(),
            input_schema: json!({"type":"object","properties":{
                "confirm":{"type":"boolean","description":"Run the install command. Without it, only the planned command is reported."}
            },"additionalProperties":false}),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::tool_args::ArgsExt;

        if let Some(path) = crate::video_ffmpeg::find_ffmpeg() {
            return ToolResult::text(format!(
                "✅ ffmpeg already available ({}). Nothing to install.",
                path.display()
            ))
            .with_structured(json!({
                "installed": true, "ran": false, "path": path.display().to_string()
            }));
        }

        let Some(plan) = crate::ffmpeg_install::install_plan() else {
            return ToolResult::error(
                "ffmpeg is not installed and no supported package manager was found to \
                 install it automatically. Install ffmpeg manually and put it on PATH \
                 (Linux: apt/dnf/pacman/zypper/apk/snap; macOS: `brew install ffmpeg`; \
                 Windows: `winget install Gyan.FFmpeg`).",
            );
        };

        if !args.bool_or("confirm", false) {
            return ToolResult::text(format!(
                "ffmpeg is not installed. To install it via {}, re-call install_ffmpeg \
                 with confirm=true.\n\nCommand that will run:\n  {}",
                plan.manager,
                plan.display()
            ))
            .with_structured(json!({
                "installed": false, "ran": false,
                "manager": plan.manager, "command": plan.display()
            }));
        }

        let display = plan.display();
        let result = tokio::task::spawn_blocking(move || crate::ffmpeg_install::run_install(&plan)).await;
        match result {
            Ok(Ok((cmd_ok, output))) => match crate::video_ffmpeg::find_ffmpeg() {
                Some(path) => ToolResult::text(format!("✅ ffmpeg installed via `{display}`."))
                    .with_structured(json!({
                        "installed": true, "ran": true,
                        "command": display, "path": path.display().to_string()
                    })),
                None => ToolResult::error(format!(
                    "Ran the install command but ffmpeg is still not found.\n\
                     Command: {display}\ncommand_succeeded={cmd_ok}\nOutput tail:\n{output}"
                )),
            },
            Ok(Err(e)) => ToolResult::error(format!("ffmpeg install failed: {e}\nCommand: {display}")),
            Err(e) => ToolResult::error(format!("install task error: {e}")),
        }
    }
}

// ── process_recording ─────────────────────────────────────────────────────────
//
// Turn a recorded trajectory directory into a token-cheap, readable
// `TRAJECTORY.md` (+ `SUMMARY.json`) so an agent can read the demonstration
// without ingesting base64 screenshots, then author a skill from it. When
// `author_skill: true` AND `ANTHROPIC_API_KEY` is set, also calls the Anthropic
// API to write a `SKILL.md` directly; otherwise it returns the processed paths
// plus instructions and points at the bundled cua-driver `DEMONSTRATION.md` guide.

pub struct ProcessRecordingTool;
static PROCESS_REC_DEF: OnceLock<ToolDef> = OnceLock::new();

/// System prompt for the optional Anthropic-backed skill author. Kept in sync
/// with the bundled cua-driver `DEMONSTRATION.md` guide that a host agent follows
/// when no API key is present.
const SKILL_AUTHOR_SYSTEM: &str = "You are authoring a reusable agent SKILL from a recorded \
    computer-use demonstration, following the Open Agent Skills Standard (SKILL.md). You are given \
    a TRAJECTORY.md describing the human/agent \
    actions taken on a window, with screenshots referenced by relative path. Write a single \
    SKILL.md:\n\
    - YAML frontmatter with exactly two keys: `name` (kebab-case) and `description`. The \
      description is a ROUTING signal, not a summary — front-load 'Use when …' and, when useful, \
      'Do NOT use when …'.\n\
    - `## Inputs` — the variables the task needs, written as `{placeholders}` (e.g. `{month}`).\n\
    - `## Steps` — a numbered list of SEMANTIC actions ('Click the **Submit** button', not 'click \
      at 412,880'). Generalize pixel coordinates and literal text into intent and `{placeholders}`. \
      Never hardcode coordinates or secrets. Refer to UI elements by name/role.\n\
    - `## Verification` — how to confirm the task succeeded, and brief failure recovery.\n\
    Output ONLY the SKILL.md content, no preamble.";

#[async_trait]
impl Tool for ProcessRecordingTool {
    fn def(&self) -> &ToolDef {
        PROCESS_REC_DEF.get_or_init(|| ToolDef {
            name: "process_recording".into(),
            description: "Process a recorded trajectory directory into a token-cheap, \
                human-readable `TRAJECTORY.md` (+ `SUMMARY.json`): readable action prose, \
                screenshots referenced by RELATIVE PATH (never inlined as base64), and only \
                a few key frames embedded so reading it doesn't blow up your context. Read \
                the resulting TRAJECTORY.md, then author a skill from it (see the bundled \
                cua-driver `DEMONSTRATION.md` guide). If `author_skill: true` AND the \
                ANTHROPIC_API_KEY env var is set, this also calls the Anthropic API to write \
                a `SKILL.md` into the directory for you."
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["dir"],
                "properties": {
                    "dir": { "type": "string",
                        "description": "Recording directory (the `output_dir` passed to start_recording)." },
                    "max_inline_frames": { "type": "integer",
                        "description": "Max screenshots embedded inline in TRAJECTORY.md (rest are linked). Default 4." },
                    "author_skill": { "type": "boolean",
                        "description": "Also write SKILL.md via the Anthropic API (requires ANTHROPIC_API_KEY). Default false." },
                    "model": { "type": "string",
                        "description": "Model for author_skill. Default claude-haiku-4-5." }
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
        let Some(dir) = args.opt_str("dir").filter(|s| !s.is_empty()) else {
            return ToolResult::error("`dir` is required.");
        };
        let dir = crate::recording::expand_tilde(&dir);
        let opts = crate::recording_markdown::ProcessOptions {
            max_inline_frames: args.opt_u64("max_inline_frames").map(|n| n as usize).unwrap_or(4),
        };

        let dir2 = dir.clone();
        let processed = tokio::task::spawn_blocking(move || {
            crate::recording_markdown::process(&dir2, &opts)
        })
        .await;

        let result = match processed {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return ToolResult::error(format!("Failed to process recording: {e}")),
            Err(e) => return ToolResult::error(format!("processing task error: {e}")),
        };

        let traj_path = result.trajectory_md.clone();
        let mut structured = json!({
            "trajectory_md": result.trajectory_md.to_string_lossy(),
            "summary_json": result.summary_json.to_string_lossy(),
            "summary": {
                "turn_count": result.summary.turn_count,
                "human_turns": result.summary.human_turns,
                "agent_turns": result.summary.agent_turns,
                "duration_ms": result.summary.duration_ms,
            },
            "skill_authored": false,
        });

        let author = args.bool_or("author_skill", false);
        let mut msg = format!(
            "✅ Processed {} steps -> {}\n\nRead TRAJECTORY.md, then author a skill from it \
             (see the bundled cua-driver `DEMONSTRATION.md` guide).",
            result.summary.turn_count,
            traj_path.display()
        );

        if author {
            let model = args.opt_str("model").unwrap_or_else(|| "claude-haiku-4-5".into());
            match author_skill(&traj_path, &model).await {
                Ok(skill_path) => {
                    structured["skill_authored"] = json!(true);
                    structured["skill_md"] = json!(skill_path.to_string_lossy());
                    msg.push_str(&format!("\n\n📝 Authored SKILL.md -> {}", skill_path.display()));
                }
                Err(e) => {
                    msg.push_str(&format!(
                        "\n\n⚠️ author_skill skipped: {e}\nThe TRAJECTORY.md is ready — author \
                         the skill yourself using the `DEMONSTRATION.md` guide."
                    ));
                }
            }
        }

        ToolResult::text(msg).with_structured(structured)
    }
}

/// Call the Anthropic Messages API to author a SKILL.md from a TRAJECTORY.md.
/// Errors (with a friendly message) when ANTHROPIC_API_KEY is unset so the
/// caller can fall back to authoring it themselves.
async fn author_skill(
    traj_path: &std::path::Path,
    model: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY is not set"))?;
    let traj = std::fs::read_to_string(traj_path)?;
    let out_path = traj_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("SKILL.md");
    let model = model.to_string();

    let skill = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let body = json!({
            "model": model,
            "max_tokens": 2048,
            "system": SKILL_AUTHOR_SYSTEM,
            "messages": [{
                "role": "user",
                "content": format!("Here is the demonstration trajectory:\n\n{traj}")
            }]
        });
        let resp: Value = ureq::post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .send_json(&body)
            .map_err(|e| anyhow::anyhow!("Anthropic request failed: {e}"))?
            .body_mut()
            .read_json::<Value>()
            .map_err(|e| anyhow::anyhow!("Anthropic response parse failed: {e}"))?;
        let text = resp
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|b| b.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow::anyhow!("unexpected Anthropic response shape: {resp}"))?;
        Ok(text.to_string())
    })
    .await
    .map_err(|e| anyhow::anyhow!("author task error: {e}"))??;

    std::fs::write(&out_path, skill)?;
    Ok(out_path)
}

// ── start_demonstration / stop_demonstration ────────────────────────────────
//
// Intuitive one-call surface for the demonstration -> skill workflow, so an
// agent doesn't have to know the `start_recording(demonstration:true, ...)`
// flag combo:
//
//   start_demonstration(window_id, pid)  -> border on, capture begins
//   ...human performs the task...
//   stop_demonstration()                 -> stops, processes, returns TRAJECTORY.md
//
// Both wrap the same RecordingSession the other recording tools use.

pub struct StartDemonstrationTool {
    session: Arc<RecordingSession>,
}
impl StartDemonstrationTool {
    pub fn new(session: Arc<RecordingSession>) -> Self { Self { session } }
}
static START_DEMO_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for StartDemonstrationTool {
    fn def(&self) -> &ToolDef {
        START_DEMO_DEF.get_or_init(|| ToolDef {
            name: "start_demonstration".into(),
            description: "Record a HUMAN demonstration on one window, to turn into a skill. \
                Shows a glowing red recording border around the target window and captures the \
                human's clicks / scrolls / drags / typing on THAT window (window-scoped — input \
                elsewhere is ignored). Typed text is REDACTED by default. Call this, let the human \
                perform the task, then call `stop_demonstration` to finish and get a readable \
                TRAJECTORY.md you can author a skill from (see the bundled `DEMONSTRATION.md` \
                guide). Get `window_id` + `pid` from `list_windows`. Windows only for now."
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["window_id", "pid"],
                "properties": {
                    "window_id": { "type": "integer", "description": "Target window id / HWND (from list_windows)." },
                    "pid": { "type": "integer", "description": "Target window's process id (from list_windows)." },
                    "output_dir": { "type": "string", "description": "Where to write the recording. Default: a fresh temp dir (returned in the result)." },
                    "capture_raw_text": { "type": "boolean", "description": "Also store literal typed text (needed for faithful replay) instead of only a redacted summary. Stores secrets — opt in deliberately. Default: false." }
                },
                "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::tool_args::ArgsExt;
        let Some(window_id) = args.opt_u64("window_id") else {
            return ToolResult::error("`window_id` is required.");
        };
        let Some(pid) = args.opt_i64("pid") else {
            return ToolResult::error("`pid` is required.");
        };
        let capture_raw_text = args.bool_or("capture_raw_text", false);
        let owner = args.opt_str("_session_id");
        let dir = args.opt_str("output_dir").filter(|s| !s.is_empty()).unwrap_or_else(|| {
            crate::recording::expand_tilde(&format!(
                "{}/cua-demonstrations/demo-{}",
                std::env::temp_dir().to_string_lossy(),
                crate::recording::now_ms()
            ))
            .to_string_lossy()
            .into_owned()
        });

        if let Err(e) = self.session.start(&dir, false, owner.as_deref()) {
            return ToolResult::error(format!("Failed to start demonstration: {e}"));
        }
        if let Err(e) = self.session.begin_demonstration(pid, window_id, capture_raw_text) {
            let _ = self.session.stop_owner(None);
            return ToolResult::error(format!("Failed to start demonstration capture: {e}"));
        }
        let state = self.session.current_state();
        let msg = format!(
            "🔴 Demonstration started — glowing border on window {window_id}. Human input on that \
             window is being captured ({}). Perform the task, then call `stop_demonstration`.\n\n\
             Recording -> {dir}",
            if capture_raw_text { "raw text" } else { "text redacted" }
        );
        ToolResult::text(msg).with_structured(recording_state_json(&state))
    }
}

pub struct StopDemonstrationTool {
    session: Arc<RecordingSession>,
}
impl StopDemonstrationTool {
    pub fn new(session: Arc<RecordingSession>) -> Self { Self { session } }
}
static STOP_DEMO_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for StopDemonstrationTool {
    fn def(&self) -> &ToolDef {
        STOP_DEMO_DEF.get_or_init(|| ToolDef {
            name: "stop_demonstration".into(),
            description: "Stop the active demonstration (removes the border + capture) and process \
                it into a readable, token-cheap TRAJECTORY.md (+ SUMMARY.json): action prose with \
                screenshots referenced by relative path, only a few key frames embedded. Read the \
                returned TRAJECTORY.md, then author a SKILL.md from it (see the bundled \
                `DEMONSTRATION.md` guide). If `author_skill: true` AND ANTHROPIC_API_KEY is set, \
                also drafts the SKILL.md for you."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "author_skill": { "type": "boolean", "description": "Also draft SKILL.md via the Anthropic API (requires ANTHROPIC_API_KEY). Default false." },
                    "model": { "type": "string", "description": "Model for author_skill. Default claude-haiku-4-5." }
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
        // Capture the output dir BEFORE stopping (stop clears it).
        let dir = self.session.current_state().output_dir;
        if let Err(e) = self.session.stop_owner(None) {
            return ToolResult::error(format!("Failed to stop demonstration: {e}"));
        }
        let Some(dir) = dir else {
            return ToolResult::text("Stopped. (No active recording to process.)");
        };
        let dir = crate::recording::expand_tilde(&dir);

        let dir2 = dir.clone();
        let processed = tokio::task::spawn_blocking(move || {
            crate::recording_markdown::process(&dir2, &crate::recording_markdown::ProcessOptions::default())
        })
        .await;
        let result = match processed {
            Ok(Ok(r)) => r,
            // No turns captured is a normal outcome (the human didn't act on the
            // window), not an error — report it softly.
            Ok(Err(_)) => {
                return ToolResult::text(format!(
                    "✅ Demonstration stopped. No input was captured on the target window \
                     (nothing to process). Recording dir: {}",
                    dir.display()
                ))
            }
            Err(e) => return ToolResult::error(format!("Stopped, but processing task error: {e}")),
        };

        let traj_path = result.trajectory_md.clone();
        let mut structured = json!({
            "trajectory_md": result.trajectory_md.to_string_lossy(),
            "summary_json": result.summary_json.to_string_lossy(),
            "summary": {
                "turn_count": result.summary.turn_count,
                "human_turns": result.summary.human_turns,
                "agent_turns": result.summary.agent_turns,
                "duration_ms": result.summary.duration_ms,
            },
            "skill_authored": false,
        });
        let mut msg = format!(
            "✅ Demonstration stopped — {} steps captured.\n📄 {}\n\nRead TRAJECTORY.md, then \
             author a SKILL.md from it (see the bundled `DEMONSTRATION.md` guide).",
            result.summary.turn_count,
            traj_path.display()
        );
        if args.bool_or("author_skill", false) {
            let model = args.opt_str("model").unwrap_or_else(|| "claude-haiku-4-5".into());
            match author_skill(&traj_path, &model).await {
                Ok(p) => {
                    structured["skill_authored"] = json!(true);
                    structured["skill_md"] = json!(p.to_string_lossy());
                    msg.push_str(&format!("\n\n📝 Authored SKILL.md -> {}", p.display()));
                }
                Err(e) => msg.push_str(&format!("\n\n⚠️ author_skill skipped: {e}")),
            }
        }
        ToolResult::text(msg).with_structured(structured)
    }
}
