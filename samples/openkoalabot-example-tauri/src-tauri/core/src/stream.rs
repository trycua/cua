// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Space's screen, for the PiP view.
//!
//! Two ways in, both over rcdp wire v2:
//!
//! - [`open_ticket`]: mint a media session and hand the webview a ticketed
//!   `ws_url`; the page decodes H.264 with WebCodecs (the ticket is safe in a
//!   URL and authorizes that one session only).
//! - [`FrameWatch`] + [`watch_desktop`]: a native session whose frames are
//!   counted, not kept (memory-bounded by construction). The shell uses it
//!   for the stream health line; the scenario uses it to prove the first
//!   frame is a keyframe.

use crate::{Error, Result};
use cua_spaces::Space;
use cua_spaces::stream::{
    FrameSink, StreamEvent, StreamOptions, StreamSession, StreamStats, StreamTarget, StreamTicket,
    VideoFrame, WindowTarget,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Mints a desktop media session for the webview.
// #region docs:rs-stream
pub async fn open_ticket(space: &Space, max_fps: u32, max_dimension: u32) -> Result<StreamTicket> {
    Ok(space
        .open_stream(
            StreamTarget::Display(None),
            StreamOptions {
                max_fps,
                max_dimension,
                ..Default::default()
            },
        )
        .await?)
}
// #endregion docs:rs-stream

/// Mints a media session for one window (a per-window PiP).
pub async fn open_window_ticket(
    space: &Space,
    window_id: &str,
    max_fps: u32,
    max_dimension: u32,
) -> Result<StreamTicket> {
    if window_id.is_empty() {
        return Err(Error::Invalid("a window stream needs a window id".into()));
    }
    Ok(space
        .open_stream(
            StreamTarget::Window(window_id.into()),
            StreamOptions {
                max_fps,
                max_dimension,
                ..Default::default()
            },
        )
        .await?)
}

/// One row of the Computer panel's window list.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct WindowRow {
    pub window_id: String,
    pub app: String,
    pub title: String,
    pub width: f64,
    pub height: f64,
    /// The app's icon as the Space's desktop shows it, as a `data:` URL
    /// (from the SDK's `Space::app_icon`); absent when the Space has none,
    /// and then the row shows no icon at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip)]
    pub app_id: String,
    #[serde(skip)]
    pub pid: u32,
}

impl WindowRow {
    /// One row per window of the SDK's list (`Space::windows` lists only
    /// streamable windows).
    pub fn from_targets(targets: Vec<WindowTarget>) -> Vec<WindowRow> {
        targets
            .into_iter()
            .map(|w| WindowRow {
                window_id: w.window_id,
                app: w.app_name,
                title: w.title,
                width: w.bounds[2],
                height: w.bounds[3],
                icon: None,
                app_id: w.app_id,
                pid: w.pid,
            })
            .collect()
    }
}

/// `bytes` as a `data:` URL.
pub fn data_url(content_type: &str, bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4 + 32);
    out.push_str("data:");
    out.push_str(content_type);
    out.push_str(";base64,");
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= c.len() {
                out.push(A[(n >> shift & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The Space's streamable windows, each with its app icon when the Space
/// has one (every row's icon in one SDK call, through its one icon cache);
/// empty when it cannot stream single windows (no `window_stream`).
pub async fn windows(space: &Space) -> Result<Vec<WindowRow>> {
    if !space.supports("window_stream") {
        return Ok(Vec::new());
    }
    let mut rows = WindowRow::from_targets(space.windows(None).await?);
    let requests: Vec<cua_spaces::IconRequest> = rows
        .iter()
        .map(|r| cua_spaces::IconRequest {
            app_name: r.app.clone(),
            app_id: r.app_id.clone(),
            pid: r.pid,
        })
        .collect();
    if let Ok(icons) = space.app_icons(&requests).await {
        for (row, icon) in rows.iter_mut().zip(icons) {
            row.icon = icon.map(|i| data_url(i.content_type, &i.bytes));
        }
    }
    Ok(rows)
}

/// Counts frames and remembers what the first one was. Keeps no pixels.
#[derive(Default)]
pub struct FrameWatch {
    frames: AtomicU64,
    keyframes: AtomicU64,
    closed: AtomicU64,
    first: Mutex<Option<bool>>,
    size: Mutex<(u32, u32)>,
    codec: Mutex<String>,
}

impl FrameSink for FrameWatch {
    fn on_frame(&self, f: VideoFrame) {
        self.first.lock().unwrap().get_or_insert(f.keyframe);
        *self.size.lock().unwrap() = (f.width, f.height);
        {
            let mut c = self.codec.lock().unwrap();
            if c.is_empty() {
                *c = f.codec.clone();
            }
        }
        if f.keyframe {
            self.keyframes.fetch_add(1, Ordering::SeqCst);
        }
        self.frames.fetch_add(1, Ordering::SeqCst);
    }

    fn on_event(&self, e: StreamEvent) {
        if matches!(e, StreamEvent::Closed { .. }) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// A point-in-time view of a [`FrameWatch`].
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct WatchSnapshot {
    pub frames: u64,
    pub keyframes: u64,
    pub first_frame_keyframe: Option<bool>,
    pub width: u32,
    pub height: u32,
    pub codec: String,
    pub closed: bool,
}

impl FrameWatch {
    /// Current counters.
    pub fn snapshot(&self) -> WatchSnapshot {
        let (width, height) = *self.size.lock().unwrap();
        WatchSnapshot {
            frames: self.frames.load(Ordering::SeqCst),
            keyframes: self.keyframes.load(Ordering::SeqCst),
            first_frame_keyframe: *self.first.lock().unwrap(),
            width,
            height,
            codec: self.codec.lock().unwrap().clone(),
            closed: self.closed.load(Ordering::SeqCst) > 0,
        }
    }

    /// Waits until at least `n` frames arrived: at most `max_polls` polls of
    /// `poll` each, then fails.
    pub async fn wait_frames(&self, n: u64, poll: Duration, max_polls: u32) -> Result<()> {
        for _ in 0..max_polls {
            if self.frames.load(Ordering::SeqCst) >= n {
                return Ok(());
            }
            tokio::time::sleep(poll).await;
        }
        Err(Error::Timeout(format!(
            "{} of {n} frames after {max_polls} polls",
            self.frames.load(Ordering::SeqCst)
        )))
    }
}

/// A native desktop stream with its watch.
pub struct DesktopWatch {
    pub session: StreamSession,
    pub watch: Arc<FrameWatch>,
}

/// Opens a native desktop stream feeding a fresh [`FrameWatch`].
pub async fn watch_desktop(
    space: &Space,
    max_fps: u32,
    max_dimension: u32,
) -> Result<DesktopWatch> {
    let watch = Arc::new(FrameWatch::default());
    let session = space
        .stream_session(
            StreamTarget::Display(None),
            StreamOptions {
                max_fps,
                max_dimension,
                ..Default::default()
            },
            watch.clone(),
            None,
        )
        .await?;
    Ok(DesktopWatch { session, watch })
}

impl DesktopWatch {
    /// Asks for a keyframe (the decoder-resync path).
    pub fn request_keyframe(&self) -> Result<()> {
        Ok(self.session.request_keyframe()?)
    }

    /// Closes the session.
    pub async fn close(self) -> Result<StreamStats> {
        Ok(self.session.close().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(key: bool) -> VideoFrame {
        VideoFrame {
            sequence: 1,
            codec: "h264".into(),
            keyframe: key,
            width: 800,
            height: 600,
            capture_timestamp_us: 0,
            codec_epoch: 1,
            geometry_epoch: 1,
            data: vec![0; 4],
        }
    }

    #[test]
    fn the_watch_remembers_the_first_frame_and_keeps_no_pixels() {
        let w = FrameWatch::default();
        w.on_frame(frame(true));
        w.on_frame(frame(false));
        w.on_event(StreamEvent::Closed {
            code: 1000,
            reason: String::new(),
        });
        let s = w.snapshot();
        assert_eq!(s.frames, 2);
        assert_eq!(s.keyframes, 1);
        assert_eq!(s.first_frame_keyframe, Some(true));
        assert_eq!((s.width, s.height), (800, 600));
        assert_eq!(s.codec, "h264");
        assert!(s.closed);
    }

    fn target(id: &str) -> WindowTarget {
        WindowTarget {
            window_id: id.into(),
            epoch: 1,
            title: format!("{id} title"),
            app_name: "Thunar".into(),
            app_id: String::new(),
            pid: 7,
            bounds: [10.0, 20.0, 640.0, 480.0],
            on_screen: true,
            focused: false,
            available: true,
            limitation: String::new(),
        }
    }

    #[test]
    fn a_window_row_carries_the_title_and_size() {
        let rows = WindowRow::from_targets(vec![target("a")]);
        assert_eq!(
            rows,
            vec![WindowRow {
                window_id: "a".into(),
                app: "Thunar".into(),
                title: "a title".into(),
                width: 640.0,
                height: 480.0,
                icon: None,
                app_id: String::new(),
                pid: 7,
            }]
        );
        let json = serde_json::to_value(&rows[0]).unwrap();
        assert!(
            json.get("icon").is_none() && json.get("pid").is_none(),
            "{json}"
        );
    }

    #[test]
    fn icons_cross_as_data_urls() {
        assert_eq!(data_url("image/png", b""), "data:image/png;base64,");
        assert_eq!(data_url("image/png", b"f"), "data:image/png;base64,Zg==");
        assert_eq!(data_url("image/png", b"fo"), "data:image/png;base64,Zm8=");
        assert_eq!(
            data_url("image/svg+xml", b"foobar"),
            "data:image/svg+xml;base64,Zm9vYmFy"
        );
    }

    #[tokio::test]
    async fn waiting_for_frames_is_bounded() {
        let w = FrameWatch::default();
        let e = w
            .wait_frames(1, Duration::from_millis(1), 3)
            .await
            .unwrap_err();
        assert!(matches!(e, Error::Timeout(_)), "{e}");
    }
}
