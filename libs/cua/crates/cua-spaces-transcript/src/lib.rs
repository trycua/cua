//! The Spaces transcript core: terminal bytes in, a document out.
//!
//! Pure computation with no I/O and no platform surface, which is what makes
//! it the right first thing to move into Rust. Its fixtures live in
//! `libs/cua/spaces-contract/fixtures/transcript`.

pub mod cast;
pub mod conformance;
pub mod emulator;
pub mod framedoc;
pub mod jsonui;
pub mod parser;
pub mod screen;

pub use cast::{CastPlayer, CastRecording, RenderedFrame, TranscriptError};
pub use emulator::TerminalEmulator;
pub use framedoc::{FRAME_SCHEMA, frame_document, render_frame_document};
pub use jsonui::{JSONUI_SCHEMA, jsonui_document, render_jsonui_document};
pub use parser::{ClaudeCodeParser, Element, LAYOUT_PROFILE, ParsedFrame, Provenance};
pub use screen::{CellAttributes, ScreenBuffer, ScreenCell, TerminalColor};
