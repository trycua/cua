// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What a Space's preview card shows before (or instead of) its live
//! desktop: the cover.
//!
//! - A running Space opens its desktop stream at once ("Connect to the
//!   desktop automatically", on by default) and shows "Connecting…" over a
//!   blurred preview until the first frame.
//! - With that setting off, it shows the same preview with a Connect
//!   button; pressing it starts the stream (then "Connecting…", then the
//!   live desktop).
//! - A Space that cannot stream (off, starting, being created or deleted,
//!   unreachable) shows the detail's own line ([`super::sidebar::SpaceDetail::preview_text`]).
//! - A stream that failed says so, with Try again.
//!
//! The preview is the Space's latest thumbnail, the same one the notch
//! tiles show ([`thumbnail_policy`] says how fresh the shells keep it).
//! Shells draw it with a strong blur and a slight dim, or plain black when
//! there is none yet.

use serde::{Deserialize, Serialize};

/// The cover's line while the stream opens.
pub const CONNECTING_TEXT: &str = "Connecting\u{2026}";
/// The manual-connect button.
pub const CONNECT_BUTTON: &str = "Connect";
/// Its tooltip.
pub const CONNECT_HELP: &str = "Show the live desktop";
/// The line when the stream could not open.
pub const FAILED_TEXT: &str = "Could not connect to the desktop";
/// The retry button.
pub const RETRY_BUTTON: &str = "Try again";

/// Where the shell's stream session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamPhase {
    /// No session yet.
    NoSession,
    /// A session that has not started.
    Idle,
    /// Opening (no frame yet).
    Connecting,
    /// Frames arrive.
    Streaming,
    /// Paused by the Space (the stream view says why).
    Suspended,
    /// It could not open.
    Failed,
}

/// What the cover is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCoverInput {
    /// The detail's `can_stream`.
    pub can_stream: bool,
    /// The detail's `preview_text` (why it cannot stream).
    pub preview_text: String,
    /// Settings: "Connect to the desktop automatically".
    pub auto_connect: bool,
    /// Connect (or Try again) was pressed for this Space.
    pub connect_requested: bool,
    /// The shell's stream session.
    pub stream: StreamPhase,
}

/// How the cover draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopCoverKind {
    /// No cover: the live desktop (or the stream view's own badge).
    Stream,
    /// The preview with "Connecting…".
    Connecting,
    /// The preview with a Connect button.
    Connect,
    /// The preview with a line (and maybe a button): the Space cannot
    /// stream, or the stream failed.
    Status,
}

/// The cover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCover {
    /// How it draws.
    pub kind: DesktopCoverKind,
    /// The line, centered.
    pub text: Option<String>,
    /// A button under (or instead of) the line.
    pub button: Option<String>,
    /// Its tooltip.
    pub button_help: Option<String>,
    /// The shell opens a stream session now (none is open, and one is
    /// wanted).
    pub open_stream: bool,
    /// The shell starts the open session again (Try again was pressed).
    pub retry: bool,
}

/// The cover for `input`.
pub fn desktop_cover(input: &DesktopCoverInput) -> DesktopCover {
    let cover = |kind, text: Option<&str>, button: Option<&str>, help: Option<&str>| DesktopCover {
        kind,
        text: text.map(str::to_string),
        button: button.map(str::to_string),
        button_help: help.map(str::to_string),
        open_stream: false,
        retry: false,
    };
    if !input.can_stream {
        return cover(
            DesktopCoverKind::Status,
            Some(&input.preview_text),
            None,
            None,
        );
    }
    let wanted = input.auto_connect || input.connect_requested;
    match input.stream {
        StreamPhase::Streaming | StreamPhase::Suspended => {
            cover(DesktopCoverKind::Stream, None, None, None)
        }
        StreamPhase::Failed => DesktopCover {
            retry: input.connect_requested,
            ..cover(
                DesktopCoverKind::Status,
                Some(FAILED_TEXT),
                Some(RETRY_BUTTON),
                None,
            )
        },
        StreamPhase::NoSession | StreamPhase::Idle if !wanted => cover(
            DesktopCoverKind::Connect,
            None,
            Some(CONNECT_BUTTON),
            Some(CONNECT_HELP),
        ),
        phase => DesktopCover {
            open_stream: phase == StreamPhase::NoSession,
            ..cover(
                DesktopCoverKind::Connecting,
                Some(CONNECTING_TEXT),
                None,
                None,
            )
        },
    }
}

/// How fresh the shells keep each Space's thumbnail (the notch tiles and
/// the preview cover read the same one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailPolicy {
    /// While the notch is open: every running Space, this often.
    pub open_interval_ms: u64,
    /// Otherwise (the app visible, not in Low Power Mode): a running Space
    /// whose thumbnail is older than this gets a new one.
    pub background_interval_ms: u64,
    /// The long edge of a captured thumbnail, in pixels.
    pub max_dimension: u32,
    /// A thumbnail kept on disk longer than this is dropped at launch.
    pub keep_ms: u64,
}

/// The thumbnail policy.
pub fn thumbnail_policy() -> ThumbnailPolicy {
    ThumbnailPolicy {
        open_interval_ms: super::THUMBNAIL_INTERVAL_MS,
        background_interval_ms: 90_000,
        max_dimension: 320,
        keep_ms: 7 * 24 * 60 * 60 * 1_000,
    }
}

/// Whether a thumbnail captured at `captured_ms` (none: never) is due for a
/// new one at `now_ms`, every `interval_ms`.
pub fn thumbnail_due(captured_ms: Option<u64>, now_ms: u64, interval_ms: u64) -> bool {
    captured_ms.is_none_or(|t| now_ms.saturating_sub(t) >= interval_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(stream: StreamPhase) -> DesktopCoverInput {
        DesktopCoverInput {
            can_stream: true,
            preview_text: "Loading the desktop\u{2026}".into(),
            auto_connect: true,
            connect_requested: false,
            stream,
        }
    }

    #[test]
    fn auto_connect_opens_the_stream_and_says_connecting() {
        let c = desktop_cover(&input(StreamPhase::NoSession));
        assert_eq!(c.kind, DesktopCoverKind::Connecting);
        assert_eq!(c.text.as_deref(), Some(CONNECTING_TEXT));
        assert!(c.open_stream && c.button.is_none());
        // The session exists and opens: still connecting, nothing to open.
        for phase in [StreamPhase::Idle, StreamPhase::Connecting] {
            let c = desktop_cover(&input(phase));
            assert_eq!(c.kind, DesktopCoverKind::Connecting);
            assert!(!c.open_stream);
        }
        // The first frame: no cover.
        assert_eq!(
            desktop_cover(&input(StreamPhase::Streaming)).kind,
            DesktopCoverKind::Stream
        );
        assert_eq!(
            desktop_cover(&input(StreamPhase::Suspended)).kind,
            DesktopCoverKind::Stream
        );
    }

    #[test]
    fn manual_connect_waits_for_the_button() {
        let manual = DesktopCoverInput {
            auto_connect: false,
            ..input(StreamPhase::NoSession)
        };
        let c = desktop_cover(&manual);
        assert_eq!(c.kind, DesktopCoverKind::Connect);
        assert_eq!(c.button.as_deref(), Some(CONNECT_BUTTON));
        assert_eq!(c.button_help.as_deref(), Some(CONNECT_HELP));
        assert!(c.text.is_none() && !c.open_stream);
        // Pressed: open it, connecting.
        let pressed = DesktopCoverInput {
            connect_requested: true,
            ..manual.clone()
        };
        let c = desktop_cover(&pressed);
        assert_eq!(c.kind, DesktopCoverKind::Connecting);
        assert!(c.open_stream);
        let c = desktop_cover(&DesktopCoverInput {
            stream: StreamPhase::Connecting,
            ..pressed.clone()
        });
        assert_eq!(
            (c.kind, c.open_stream),
            (DesktopCoverKind::Connecting, false)
        );
        let c = desktop_cover(&DesktopCoverInput {
            stream: StreamPhase::Streaming,
            ..pressed
        });
        assert_eq!(c.kind, DesktopCoverKind::Stream);
    }

    #[test]
    fn a_space_that_cannot_stream_shows_its_line() {
        for auto_connect in [true, false] {
            let c = desktop_cover(&DesktopCoverInput {
                can_stream: false,
                preview_text: "Stopped".into(),
                auto_connect,
                ..input(StreamPhase::NoSession)
            });
            assert_eq!(c.kind, DesktopCoverKind::Status);
            assert_eq!(c.text.as_deref(), Some("Stopped"));
            assert!(c.button.is_none() && !c.open_stream && !c.retry);
        }
    }

    #[test]
    fn a_failed_stream_offers_try_again() {
        let failed = desktop_cover(&input(StreamPhase::Failed));
        assert_eq!(failed.kind, DesktopCoverKind::Status);
        assert_eq!(failed.text.as_deref(), Some(FAILED_TEXT));
        assert_eq!(failed.button.as_deref(), Some(RETRY_BUTTON));
        assert!(!failed.retry && !failed.open_stream);
        // Try again pressed: the shell starts the session again.
        let c = desktop_cover(&DesktopCoverInput {
            connect_requested: true,
            ..input(StreamPhase::Failed)
        });
        assert!(c.retry);
    }

    #[test]
    fn copy_has_no_em_dashes() {
        for text in [
            CONNECTING_TEXT,
            CONNECT_BUTTON,
            CONNECT_HELP,
            FAILED_TEXT,
            RETRY_BUTTON,
        ] {
            assert!(!text.contains('\u{2014}'), "{text}");
        }
    }

    #[test]
    fn thumbnails_refresh_on_the_policy() {
        let p = thumbnail_policy();
        assert_eq!(p.open_interval_ms, super::super::THUMBNAIL_INTERVAL_MS);
        assert!((60_000..=120_000).contains(&p.background_interval_ms));
        assert!(p.max_dimension <= 480);
        assert!(thumbnail_due(None, 0, p.background_interval_ms));
        assert!(!thumbnail_due(
            Some(1_000),
            60_000,
            p.background_interval_ms
        ));
        assert!(thumbnail_due(Some(1_000), 91_000, p.background_interval_ms));
        // A clock that went backwards is not due.
        assert!(!thumbnail_due(Some(5_000), 1_000, p.background_interval_ms));
    }
}
