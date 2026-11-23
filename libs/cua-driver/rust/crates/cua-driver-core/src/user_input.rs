// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `get_user_input_events`: the mouse and keyboard input the operating system
//! saw while the driver ran, so a client can tell when someone else acted.
//!
//! A platform monitor records each input it observes into a [`UserInputLog`]
//! with its source (hardware, the driver's own injection, or another
//! injector) and the window it went to. The log numbers events, folds runs of
//! mouse moves into one event, and keeps a bounded history; a reader asks for
//! the events after the cursor its previous read returned. The tool only
//! reports: what a client does about an intervention is its own policy.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef};
use crate::tool_args::parse_typed_input;

pub const TOOL_NAME: &str = "get_user_input_events";

/// Events the log keeps. A reader whose cursor fell out of this window gets
/// `history_lost: true` along with what is left.
pub const DEFAULT_CAPACITY: usize = 1024;

/// Who produced an input, as far as the platform can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    /// Not flagged as injected: a physical device.
    Hardware,
    /// Injected by this driver.
    Driver,
    /// Injected by some other software.
    Injected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    MouseMove,
    MouseDown,
    MouseUp,
    Wheel,
    KeyDown,
    KeyUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// One input as a platform monitor observed it. Keyboard input carries no
/// key identity: the log says that a key was pressed, never which.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservedInput {
    pub kind: InputKind,
    pub source: InputSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<MouseButton>,
    /// Screen position of a mouse input; absent for keyboard input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// Process that owns the window the input went to: the top-level window
    /// under the pointer for mouse input, the foreground window for keys.
    /// This is the target, not the process that produced the input.
    pub pid: Option<u32>,
    pub window_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserInputEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub input: ObservedInput,
    /// Unix milliseconds of the first input this event covers.
    pub first_time_ms: u64,
    /// Unix milliseconds of the last input this event covers.
    pub time_ms: u64,
    /// Inputs folded into this event: consecutive mouse moves from the same
    /// source to the same window become one event at the latest position.
    pub count: u32,
}

/// A read position. `epoch` names the log, so a cursor from an earlier driver
/// process is refused instead of being read against new numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    epoch: u64,
    seq: u64,
}

impl Cursor {
    pub fn parse(text: &str) -> Option<Self> {
        let (epoch, seq) = text.split_once('-')?;
        Some(Self {
            epoch: epoch.parse().ok()?,
            seq: seq.parse().ok()?,
        })
    }
}

impl std::fmt::Display for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.epoch, self.seq)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Read {
    pub cursor: Cursor,
    pub events: Vec<UserInputEvent>,
    /// Events after the reader's cursor were dropped to stay within capacity.
    pub history_lost: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The cursor belongs to another log or is ahead of this one.
    UnknownCursor,
}

pub struct UserInputLog {
    epoch: u64,
    capacity: usize,
    state: Mutex<LogState>,
}

#[derive(Default)]
struct LogState {
    events: VecDeque<UserInputEvent>,
    last_seq: u64,
    evicted_through: u64,
}

impl UserInputLog {
    pub fn new(capacity: usize) -> Self {
        Self::with_epoch(capacity, now_ms())
    }

    pub fn with_epoch(capacity: usize, epoch: u64) -> Self {
        Self {
            epoch,
            capacity: capacity.max(1),
            state: Mutex::new(LogState::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, LogState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record an input observed at `time_ms`. A mouse move that continues the
    /// newest event's run updates that event and gives it the next sequence
    /// number, so a reader past its old number still sees the new position.
    pub fn record(&self, input: ObservedInput, time_ms: u64) {
        let mut state = self.state();
        state.last_seq += 1;
        let seq = state.last_seq;
        if input.kind == InputKind::MouseMove {
            if let Some(tail) = state.events.back_mut() {
                let same_run = tail.input.kind == InputKind::MouseMove
                    && tail.input.source == input.source
                    && tail.input.pid == input.pid
                    && tail.input.window_id == input.window_id;
                if same_run {
                    tail.seq = seq;
                    tail.input.x = input.x;
                    tail.input.y = input.y;
                    tail.time_ms = time_ms;
                    tail.count = tail.count.saturating_add(1);
                    return;
                }
            }
        }
        state.events.push_back(UserInputEvent {
            seq,
            input,
            first_time_ms: time_ms,
            time_ms,
            count: 1,
        });
        while state.events.len() > self.capacity {
            if let Some(evicted) = state.events.pop_front() {
                state.evicted_through = evicted.seq;
            }
        }
    }

    /// The events after `since`, or none and the newest position when
    /// `since` is absent.
    pub fn read(&self, since: Option<Cursor>) -> Result<Read, ReadError> {
        let state = self.state();
        let cursor = Cursor {
            epoch: self.epoch,
            seq: state.last_seq,
        };
        let Some(since) = since else {
            return Ok(Read {
                cursor,
                events: Vec::new(),
                history_lost: false,
            });
        };
        if since.epoch != self.epoch || since.seq > state.last_seq {
            return Err(ReadError::UnknownCursor);
        }
        Ok(Read {
            cursor,
            events: state
                .events
                .iter()
                .filter(|event| event.seq > since.seq)
                .cloned()
                .collect(),
            history_lost: since.seq < state.evicted_through,
        })
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// The platform side: watches system input and records it into the log.
pub trait UserInputMonitor: Send + Sync {
    /// Start recording into `log`. Called on every read; a monitor that is
    /// already recording returns `Ok`. `Err` says why it cannot record.
    fn ensure_started(&self, log: &Arc<UserInputLog>) -> Result<(), String>;
}

/// The one log of this driver process.
pub fn process_log() -> Arc<UserInputLog> {
    static LOG: OnceLock<Arc<UserInputLog>> = OnceLock::new();
    LOG.get_or_init(|| Arc::new(UserInputLog::new(DEFAULT_CAPACITY)))
        .clone()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    #[serde(default)]
    since: Option<String>,
    #[serde(default, rename = "session")]
    _session: Option<String>,
}

pub struct UserInputEventsTool {
    monitor: Arc<dyn UserInputMonitor>,
    log: Arc<UserInputLog>,
}

impl UserInputEventsTool {
    pub fn new(monitor: Arc<dyn UserInputMonitor>, log: Arc<UserInputLog>) -> Self {
        Self { monitor, log }
    }
}

fn refusal(code: &str, message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "refusal": { "code": code, "message": message },
    }))
}

static DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for UserInputEventsTool {
    fn def(&self) -> &ToolDef {
        DEF.get_or_init(|| ToolDef {
            name: TOOL_NAME.into(),
            description: "Report the mouse and keyboard input the system saw, so you can tell \
                when someone else acted while you did. Call it without `since` to start \
                watching: it returns a `cursor` and no events. Pass that cursor as `since` on \
                the next call to get every input since, and a new cursor. Each event has \
                `kind` (mouse_move, mouse_down, mouse_up, wheel, key_down, key_up), `source` \
                (`hardware` for a physical device, `driver` for this driver's own input, \
                `injected` for other injected input, including the driver's pen and touch \
                background input, which Windows cannot tag), screen `x`/`y` for mouse input, \
                and the `pid` and `window_id` of the window the input went to: the window \
                under the pointer for mouse input, the foreground window for keys. They name \
                the target, not the sender. Keys are reported without their identity. \
                Consecutive mouse moves from one source to one window fold into one event \
                with a `count`. `history_lost: true` means some events after your cursor were \
                dropped. Input the driver posts straight to a window's message queue does not \
                pass through the system input queue and does not appear."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "since": {
                        "type": "string",
                        "description": "The `cursor` from your previous call. Omit it to \
                            start watching and get the current cursor."
                    },
                    "session": crate::tool_schema::session_schema(),
                },
                "additionalProperties": false,
            }),
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input: Input = match parse_typed_input(TOOL_NAME, args) {
            Ok(input) => input,
            Err(error) => return error,
        };
        let since = match input.since.as_deref() {
            None => None,
            Some(text) => match Cursor::parse(text) {
                Some(cursor) => Some(cursor),
                None => return unknown_cursor(text),
            },
        };
        if let Err(error) = self.monitor.ensure_started(&self.log) {
            return refusal(
                "monitor_unavailable",
                format!("{TOOL_NAME} refused: input monitoring could not start: {error}"),
            );
        }
        let read = match self.log.read(since) {
            Ok(read) => read,
            Err(ReadError::UnknownCursor) => {
                return unknown_cursor(input.since.as_deref().unwrap_or_default())
            }
        };
        let count = |source| {
            read.events
                .iter()
                .filter(|event| event.input.source == source)
                .count()
        };
        let mut text = format!(
            "{} input event(s): {} hardware, {} injected, {} driver. cursor: {}",
            read.events.len(),
            count(InputSource::Hardware),
            count(InputSource::Injected),
            count(InputSource::Driver),
            read.cursor,
        );
        if read.history_lost {
            text.push_str("\nSome events after `since` were dropped (history_lost).");
        }
        ToolResult::text(text).with_structured(json!({
            "cursor": read.cursor.to_string(),
            "events": read.events,
            "history_lost": read.history_lost,
        }))
    }
}

fn unknown_cursor(text: &str) -> ToolResult {
    refusal(
        "unknown_cursor",
        format!(
            "{TOOL_NAME} refused: cursor {text:?} is not from this driver process. Call it \
             without `since` to get a current cursor."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse(kind: InputKind, source: InputSource, window_id: u64, x: i32) -> ObservedInput {
        ObservedInput {
            kind,
            source,
            button: (kind != InputKind::MouseMove).then_some(MouseButton::Left),
            x: Some(x),
            y: Some(0),
            pid: Some(7),
            window_id: Some(window_id),
        }
    }

    fn key(source: InputSource) -> ObservedInput {
        ObservedInput {
            kind: InputKind::KeyDown,
            source,
            button: None,
            x: None,
            y: None,
            pid: Some(9),
            window_id: Some(3),
        }
    }

    fn start(log: &UserInputLog) -> Cursor {
        log.read(None).unwrap().cursor
    }

    #[test]
    fn a_read_returns_only_the_events_after_its_cursor() {
        let log = UserInputLog::with_epoch(16, 1);
        log.record(key(InputSource::Hardware), 10);
        let cursor = start(&log);
        log.record(mouse(InputKind::MouseDown, InputSource::Hardware, 1, 5), 11);
        log.record(mouse(InputKind::MouseUp, InputSource::Hardware, 1, 5), 12);

        let read = log.read(Some(cursor)).unwrap();
        let kinds: Vec<_> = read.events.iter().map(|e| e.input.kind).collect();
        assert_eq!(kinds, [InputKind::MouseDown, InputKind::MouseUp]);
        assert!(!read.history_lost);
        assert!(log.read(Some(read.cursor)).unwrap().events.is_empty());
    }

    #[test]
    fn a_move_run_folds_into_one_event_that_a_reader_past_it_still_sees() {
        let log = UserInputLog::with_epoch(16, 1);
        let cursor = start(&log);
        log.record(mouse(InputKind::MouseMove, InputSource::Hardware, 1, 1), 10);
        let middle = log.read(Some(cursor)).unwrap().cursor;
        log.record(mouse(InputKind::MouseMove, InputSource::Hardware, 1, 2), 11);
        log.record(mouse(InputKind::MouseMove, InputSource::Hardware, 1, 3), 12);

        let all = log.read(Some(cursor)).unwrap().events;
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].input.x, all[0].count), (Some(3), 3));
        assert_eq!((all[0].first_time_ms, all[0].time_ms), (10, 12));
        let later = log.read(Some(middle)).unwrap().events;
        assert_eq!(later, all);
    }

    #[test]
    fn a_move_from_another_source_or_window_starts_a_new_event() {
        let log = UserInputLog::with_epoch(16, 1);
        let cursor = start(&log);
        log.record(mouse(InputKind::MouseMove, InputSource::Driver, 1, 1), 10);
        log.record(mouse(InputKind::MouseMove, InputSource::Hardware, 1, 2), 11);
        log.record(mouse(InputKind::MouseMove, InputSource::Hardware, 2, 3), 12);
        log.record(mouse(InputKind::MouseDown, InputSource::Hardware, 2, 3), 13);
        log.record(mouse(InputKind::MouseMove, InputSource::Hardware, 2, 4), 14);

        let sources: Vec<_> = log
            .read(Some(cursor))
            .unwrap()
            .events
            .iter()
            .map(|e| (e.input.kind, e.input.source, e.count))
            .collect();
        assert_eq!(
            sources,
            [
                (InputKind::MouseMove, InputSource::Driver, 1),
                (InputKind::MouseMove, InputSource::Hardware, 1),
                (InputKind::MouseMove, InputSource::Hardware, 1),
                (InputKind::MouseDown, InputSource::Hardware, 1),
                (InputKind::MouseMove, InputSource::Hardware, 1),
            ]
        );
    }

    #[test]
    fn a_cursor_older_than_the_kept_history_reports_the_loss() {
        let log = UserInputLog::with_epoch(2, 1);
        let cursor = start(&log);
        for _ in 0..3 {
            log.record(key(InputSource::Hardware), 10);
        }
        let read = log.read(Some(cursor)).unwrap();
        assert!(read.history_lost);
        assert_eq!(read.events.len(), 2);
        assert!(!log.read(Some(read.cursor)).unwrap().history_lost);
    }

    #[test]
    fn a_cursor_from_another_log_or_ahead_of_this_one_is_unknown() {
        let log = UserInputLog::with_epoch(4, 1);
        let other = UserInputLog::with_epoch(4, 2);
        assert_eq!(log.read(Some(start(&other))), Err(ReadError::UnknownCursor));
        let ahead = Cursor { epoch: 1, seq: 5 };
        assert_eq!(log.read(Some(ahead)), Err(ReadError::UnknownCursor));
    }

    #[test]
    fn a_cursor_round_trips_through_its_text() {
        let cursor = Cursor { epoch: 17, seq: 4 };
        assert_eq!(Cursor::parse(&cursor.to_string()), Some(cursor));
        assert_eq!(Cursor::parse("17"), None);
        assert_eq!(Cursor::parse("a-1"), None);
    }

    struct FakeMonitor(Result<(), String>);

    impl UserInputMonitor for FakeMonitor {
        fn ensure_started(&self, _log: &Arc<UserInputLog>) -> Result<(), String> {
            self.0.clone()
        }
    }

    fn tool(monitor: Result<(), String>) -> (UserInputEventsTool, Arc<UserInputLog>) {
        let log = Arc::new(UserInputLog::with_epoch(16, 1));
        (
            UserInputEventsTool::new(Arc::new(FakeMonitor(monitor)), log.clone()),
            log,
        )
    }

    fn refusal_code(result: &ToolResult) -> &str {
        assert_eq!(result.is_error, Some(true));
        result.structured_content.as_ref().unwrap()["refusal"]["code"]
            .as_str()
            .unwrap()
    }

    #[tokio::test]
    async fn the_tool_reports_events_after_the_cursor_it_returned() {
        let (tool, log) = tool(Ok(()));
        let first = tool.invoke(json!({})).await.structured_content.unwrap();
        assert_eq!(first["events"], json!([]));
        log.record(mouse(InputKind::MouseDown, InputSource::Hardware, 4, 5), 10);
        log.record(key(InputSource::Driver), 11);

        let next = tool
            .invoke(json!({"since": first["cursor"]}))
            .await
            .structured_content
            .unwrap();
        assert_eq!(
            next["events"],
            json!([
                {"seq": 1, "kind": "mouse_down", "source": "hardware", "button": "left",
                 "x": 5, "y": 0, "pid": 7, "window_id": 4,
                 "first_time_ms": 10, "time_ms": 10, "count": 1},
                {"seq": 2, "kind": "key_down", "source": "driver", "pid": 9, "window_id": 3,
                 "first_time_ms": 11, "time_ms": 11, "count": 1},
            ])
        );
        assert_eq!(next["history_lost"], false);
        assert_eq!(next["cursor"], "1-2");
    }

    #[tokio::test]
    async fn the_tool_refuses_a_cursor_it_did_not_issue() {
        let (tool, _) = tool(Ok(()));
        assert_eq!(
            refusal_code(&tool.invoke(json!({"since": "2-0"})).await),
            "unknown_cursor"
        );
        assert_eq!(
            refusal_code(&tool.invoke(json!({"since": "nonsense"})).await),
            "unknown_cursor"
        );
    }

    #[tokio::test]
    async fn the_tool_refuses_when_the_monitor_cannot_start() {
        let (tool, _) = tool(Err("hook refused".into()));
        let result = tool.invoke(json!({})).await;
        assert_eq!(refusal_code(&result), "monitor_unavailable");
    }
}
