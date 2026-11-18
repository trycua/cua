//! asciinema v2 recordings and scrubbing playback.
//!
//! Ports `CastRecording.swift` and `CastPlayer.swift`. The format is a
//! documented standard rather than something invented here, so a golden stays
//! inspectable with `head` and `jq` and can be replayed by `asciinema play` by
//! anyone who would rather check the fixture with their own eyes than trust
//! our parser.

use crate::emulator::TerminalEmulator;
use crate::screen::ScreenBuffer;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    #[error("malformed cast: {0}")]
    MalformedCast(String),
}

#[derive(Clone, Debug, Default)]
pub struct CastHeader {
    pub version: i64,
    pub width: usize,
    pub height: usize,
    pub timestamp: Option<i64>,
    pub cli: Option<String>,
    /// The CLI's own `--version` output, recorded verbatim at capture time. A
    /// parser result that does not travel with the version of the thing it
    /// parsed is not evidence about anything.
    pub cli_version: Option<String>,
    pub script_name: Option<String>,
    pub description: Option<String>,
    pub scrubbed: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventKind {
    Output,
    Input,
    Marker,
    Resize,
}

#[derive(Clone, Debug)]
pub struct CastEvent {
    pub time: f64,
    pub kind: EventKind,
    pub data: String,
}

#[derive(Clone, Debug)]
pub struct CastRecording {
    pub header: CastHeader,
    pub events: Vec<CastEvent>,
}

impl CastRecording {
    pub fn duration(&self) -> f64 {
        self.events.last().map(|event| event.time).unwrap_or(0.0)
    }

    pub fn parse(text: &str) -> Result<CastRecording, TranscriptError> {
        let mut lines = text.split('\n').filter(|line| !line.is_empty());
        let header_line = lines
            .next()
            .ok_or_else(|| TranscriptError::MalformedCast("empty file".into()))?;
        let raw: Value = serde_json::from_str(header_line).map_err(|_| {
            TranscriptError::MalformedCast("header is not an asciinema v2 header".into())
        })?;
        let object = raw.as_object().ok_or_else(|| {
            TranscriptError::MalformedCast("header is not an asciinema v2 header".into())
        })?;
        let version = object.get("version").and_then(Value::as_i64);
        let width = object.get("width").and_then(Value::as_i64);
        let height = object.get("height").and_then(Value::as_i64);
        let (Some(version), Some(width), Some(height)) = (version, width, height) else {
            return Err(TranscriptError::MalformedCast(
                "header is not an asciinema v2 header".into(),
            ));
        };
        if version != 2 {
            return Err(TranscriptError::MalformedCast(format!(
                "unsupported cast version {version}"
            )));
        }
        let string = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(|value| value.to_string())
        };
        let strings = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_array)
                .map(|array| {
                    array
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|value| value.to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        let header = CastHeader {
            version,
            width: width.max(0) as usize,
            height: height.max(0) as usize,
            timestamp: object.get("timestamp").and_then(Value::as_i64),
            cli: string("cua_cli"),
            cli_version: string("cua_cli_version"),
            script_name: string("cua_script"),
            description: string("cua_description"),
            scrubbed: strings("cua_scrubbed"),
            notes: strings("cua_notes"),
        };

        let mut events = Vec::new();
        for line in lines {
            let Ok(Value::Array(array)) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if array.len() < 3 {
                continue;
            }
            let (Some(time), Some(kind), Some(payload)) =
                (array[0].as_f64(), array[1].as_str(), array[2].as_str())
            else {
                continue;
            };
            let kind = match kind {
                "i" => EventKind::Input,
                "m" => EventKind::Marker,
                "r" => EventKind::Resize,
                _ => EventKind::Output,
            };
            events.push(CastEvent {
                time,
                kind,
                data: payload.to_string(),
            });
        }
        Ok(CastRecording { header, events })
    }
}

/// A screen, plus everything known about how it was produced. The only input
/// to the parser: anything the parser claims has to be justifiable from this.
#[derive(Clone, Debug)]
pub struct RenderedFrame {
    pub screen: ScreenBuffer,
    /// The last OSC window title set at or before this instant. Claude Code
    /// writes the live spinner glyph and the turn's subject there, which is
    /// the one piece of turn state that survives a mid-repaint frame.
    pub window_title: String,
    pub requested_time: f64,
    /// The timestamp of the last event actually applied. Never greater than
    /// `requested_time`.
    pub effective_time: f64,
    pub events_applied: u32,
    pub unsupported_sequences: u32,
    pub unsupported_resize: bool,
    pub cli: Option<String>,
    pub cli_version: Option<String>,
}

impl RenderedFrame {
    pub fn lines(&self) -> Vec<String> {
        self.screen.lines()
    }
    pub fn text(&self) -> String {
        self.screen.text()
    }

    /// Build a frame from a raw terminal stream with no recording behind it —
    /// the `RunSnapshot.outputTail` case. `time` is meaningless here and is
    /// reported as zero.
    pub fn render_stream(
        stream: &str,
        columns: usize,
        rows: usize,
        cli: Option<String>,
        cli_version: Option<String>,
    ) -> RenderedFrame {
        let mut emulator = TerminalEmulator::new(columns, rows);
        emulator.feed_str(stream);
        RenderedFrame {
            screen: emulator.screen.clone(),
            window_title: emulator.window_title.clone(),
            requested_time: 0.0,
            effective_time: 0.0,
            events_applied: 1,
            unsupported_sequences: emulator.unsupported_sequences,
            unsupported_resize: false,
            cli,
            cli_version,
        }
    }
}

/// Scrubbing playback: "what did the screen look like at time T?"
///
/// Fidelity limits, stated rather than discovered: resolution is per event and
/// not per byte, time is relative to the recording rather than the CLI,
/// scrollback is not retained, and terminal size is fixed at the header's
/// dimensions — resize events are reported, never applied.
pub struct CastPlayer {
    pub recording: CastRecording,
    pub columns: usize,
    pub rows: usize,
}

impl CastPlayer {
    pub fn new(recording: CastRecording) -> Self {
        let columns = recording.header.width;
        let rows = recording.header.height;
        CastPlayer {
            recording,
            columns,
            rows,
        }
    }

    pub fn parse(text: &str) -> Result<CastPlayer, TranscriptError> {
        Ok(CastPlayer::new(CastRecording::parse(text)?))
    }

    pub fn frame(&self, time: f64) -> RenderedFrame {
        let mut emulator = TerminalEmulator::new(self.columns, self.rows);
        let mut applied = 0u32;
        let mut last_time = 0.0f64;
        let mut saw_resize = false;
        for event in &self.recording.events {
            if event.time > time {
                break;
            }
            match event.kind {
                EventKind::Output => {
                    emulator.feed_str(&event.data);
                    applied += 1;
                    last_time = event.time;
                }
                EventKind::Resize => saw_resize = true,
                EventKind::Input | EventKind::Marker => last_time = event.time,
            }
        }
        RenderedFrame {
            screen: emulator.screen.clone(),
            window_title: emulator.window_title.clone(),
            requested_time: time,
            effective_time: last_time,
            events_applied: applied,
            unsupported_sequences: emulator.unsupported_sequences,
            unsupported_resize: saw_resize,
            cli: self.recording.header.cli.clone(),
            cli_version: self.recording.header.cli_version.clone(),
        }
    }

    pub fn final_frame(&self) -> RenderedFrame {
        self.frame(self.recording.duration() + 1.0)
    }

    pub fn output_times(&self) -> Vec<f64> {
        self.recording
            .events
            .iter()
            .filter(|event| event.kind == EventKind::Output)
            .map(|event| event.time)
            .collect()
    }
}
