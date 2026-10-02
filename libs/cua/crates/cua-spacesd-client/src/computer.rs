//! Screen, pointer, keyboard and clipboard wrappers over `ComputerService`,
//! plus media-session setup over `StreamService`.

use crate::{
    client::SpacesdClient,
    error::{Error, Result},
    process::duration_pb,
};
use bytes::Bytes;
use cua_proto::env::v1::{
    self as pb, key_input::Key as KeyKind, keyboard_request::Action as KbAction,
    media_target::Target as MediaTargetKind, pointer_request::Action as PtrAction,
};
use std::time::Duration;

/// Name of the `SystemService.Health` component that reports whether the
/// desktop session can take input.
pub const DESKTOP_COMPONENT: &str = "desktop";

/// Desktop readiness, from [`SpacesdClient::desktop_readiness`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopReadiness {
    /// The desktop can take input.
    pub ready: bool,
    /// What is missing, when not ready.
    pub detail: String,
    /// The spacesd reports the `desktop` component (false: an older spacesd,
    /// judged by its displays only).
    pub reported: bool,
}

/// A captured screenshot.
#[derive(Clone, Debug)]
pub struct Screenshot {
    /// Encoded image.
    pub image: Bytes,
    /// Encoding.
    pub format: pb::ImageFormat,
    /// Pixel width of `image`.
    pub width: u32,
    /// Pixel height of `image`.
    pub height: u32,
    /// Image pixels per logical point.
    pub scale: f64,
    /// Id for `COORDINATE_SPACE_SCREENSHOT` input.
    pub screenshot_id: String,
    /// Full response.
    pub raw: pb::ScreenshotResponse,
}

/// Screenshot options.
#[derive(Clone, Debug, Default)]
pub struct ScreenshotOptions {
    /// Display id ("primary" when unset).
    pub display: Option<String>,
    /// Encoding.
    pub format: pb::ImageFormat,
    /// Lossy quality.
    pub quality: u32,
    /// Long-edge cap.
    pub max_dimension: u32,
    /// Draw the cursor.
    pub include_cursor: bool,
}

/// A key: named (`"enter"`, `"KEY_ENTER"`, `pb::Key::Enter`) or a character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySpec {
    /// Named key.
    Named(pb::Key),
    /// One Unicode character.
    Char(String),
}

impl KeySpec {
    /// Parses a key name. Accepts `KEY_*` proto names, their suffix in any
    /// case (`enter`, `Escape`, `left_control`), common aliases (`ctrl`,
    /// `cmd`, `esc`, `return`) or a single character.
    pub fn parse(name: &str) -> Result<Self> {
        let alias = match name.to_ascii_lowercase().as_str() {
            "ctrl" => Some("CONTROL"),
            "option" | "opt" => Some("ALT"),
            "cmd" | "command" | "super" | "win" => Some("META"),
            "up" => Some("ARROW_UP"),
            "down" => Some("ARROW_DOWN"),
            "left" => Some("ARROW_LEFT"),
            "right" => Some("ARROW_RIGHT"),
            "esc" => Some("ESCAPE"),
            "return" => Some("ENTER"),
            "del" => Some("DELETE"),
            _ => None,
        };
        let upper = alias.map(str::to_string).unwrap_or_else(|| {
            name.trim_start_matches("KEY_")
                .to_ascii_uppercase()
                .replace(['-', ' '], "_")
        });
        if let Some(k) = pb::Key::from_str_name(&format!("KEY_{upper}")) {
            return Ok(KeySpec::Named(k));
        }
        let mut chars = name.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(KeySpec::Char(c.to_string())),
            _ => Err(Error::Protocol(format!("unknown key {name:?}"))),
        }
    }

    fn to_pb(&self) -> pb::KeyInput {
        pb::KeyInput {
            key: Some(match self {
                KeySpec::Named(k) => KeyKind::Named(*k as i32),
                KeySpec::Char(c) => KeyKind::Character(c.clone()),
            }),
        }
    }
}

impl From<pb::Key> for KeySpec {
    fn from(k: pb::Key) -> Self {
        KeySpec::Named(k)
    }
}

fn point(x: f64, y: f64) -> Option<pb::Point> {
    Some(pb::Point { x, y })
}

/// A media session ready for the rcdp wire.
#[derive(Clone, Debug)]
pub struct MediaSession {
    /// Session id.
    pub session_id: String,
    /// Ticket for the media socket.
    pub ticket: String,
    /// Absolute `ws(s)://` URL of the media socket through the same route as
    /// this client (direct, Fleet gateway or relay).
    pub ws_url: String,
    /// Full response (codec, geometry, QUIC endpoint).
    pub raw: pb::OpenMediaResponse,
}

/// Options for [`SpacesdClient::open_media`].
#[derive(Clone, Debug, Default)]
pub struct MediaOptions {
    /// Display id; `None` = primary. Ignored when `window` is set.
    pub display: Option<String>,
    /// Window target.
    pub window: Option<pb::WindowRef>,
    /// Acceptable codecs.
    pub codecs: Vec<pb::MediaCodec>,
    /// FPS cap.
    pub max_fps: u32,
    /// Long-edge cap.
    pub max_dimension: u32,
    /// Ticket TTL.
    pub ticket_ttl: Option<Duration>,
    /// Ask for a direct QUIC endpoint.
    pub prefer_quic: bool,
    /// Audio tracks (`None` = no audio).
    pub audio: Option<pb::AudioOptions>,
    /// Audio-only session (no video track).
    pub disable_video: bool,
}

impl SpacesdClient {
    /// Captures the primary display (or per `options`).
    pub async fn screenshot(&self, options: ScreenshotOptions) -> Result<Screenshot> {
        let req = pb::ScreenshotRequest {
            source: options
                .display
                .map(pb::screenshot_request::Source::DisplayId),
            region: None,
            format: options.format as i32,
            quality: options.quality,
            max_dimension: options.max_dimension,
            include_cursor: options.include_cursor,
        };
        let raw = self
            .retry_policy()
            .run(|_| {
                let req = req.clone();
                async move { Ok(self.computer().screenshot(req).await?.into_inner()) }
            })
            .await?;
        let size = raw.image_size.unwrap_or_default();
        Ok(Screenshot {
            image: raw.image.clone().into(),
            format: pb::ImageFormat::try_from(raw.format).unwrap_or_default(),
            width: size.width,
            height: size.height,
            scale: raw.scale,
            screenshot_id: raw.screenshot_id.clone(),
            raw,
        })
    }

    /// Raw pointer request. Fails with [`Error::DeliveryFailed`] when the
    /// server reports that a screen-space action it delivered through the
    /// global input stream did not move the pointer (the input went
    /// nowhere), instead of reporting success.
    pub async fn pointer(
        &self,
        target: Option<pb::InputTarget>,
        action: PtrAction,
    ) -> Result<pb::PointerResponse> {
        let screen_space = target.as_ref().is_none_or(|t| t.window.is_none());
        let positioned = pointer_positioned(&action);
        let resp = self
            .computer()
            .pointer(pb::PointerRequest {
                target,
                action: Some(action),
            })
            .await?
            .into_inner();
        if screen_space && positioned {
            check_pointer_delivered(resp.report.as_ref())?;
        }
        Ok(resp)
    }

    /// Left click at screen point (x, y).
    pub async fn click(&self, x: f64, y: f64) -> Result<pb::PointerResponse> {
        self.click_with(x, y, pb::MouseButton::Left, 1).await
    }

    /// Double click.
    pub async fn double_click(&self, x: f64, y: f64) -> Result<pb::PointerResponse> {
        self.click_with(x, y, pb::MouseButton::Left, 2).await
    }

    /// Right click.
    pub async fn right_click(&self, x: f64, y: f64) -> Result<pb::PointerResponse> {
        self.click_with(x, y, pb::MouseButton::Right, 1).await
    }

    /// Click with a button and count.
    pub async fn click_with(
        &self,
        x: f64,
        y: f64,
        button: pb::MouseButton,
        count: u32,
    ) -> Result<pb::PointerResponse> {
        self.pointer(
            None,
            PtrAction::Click(pb::PointerClick {
                position: point(x, y),
                button: button as i32,
                count,
                modifiers: vec![],
            }),
        )
        .await
    }

    /// Moves the pointer.
    pub async fn move_to(&self, x: f64, y: f64) -> Result<pb::PointerResponse> {
        self.pointer(
            None,
            PtrAction::Move(pb::PointerMove {
                position: point(x, y),
                duration: None,
            }),
        )
        .await
    }

    /// Scrolls by (dx, dy) lines at the current position. Positive dy
    /// scrolls content down.
    pub async fn scroll(&self, dx: f64, dy: f64) -> Result<pb::PointerResponse> {
        self.pointer(
            None,
            PtrAction::Scroll(pb::PointerScroll {
                position: None,
                delta_x: dx,
                delta_y: dy,
                unit: pb::ScrollUnit::Line as i32,
            }),
        )
        .await
    }

    /// Drags from one point to another.
    pub async fn drag(&self, from: (f64, f64), to: (f64, f64)) -> Result<pb::PointerResponse> {
        self.pointer(
            None,
            PtrAction::Drag(pb::PointerDrag {
                from: point(from.0, from.1),
                to: point(to.0, to.1),
                path: vec![],
                button: pb::MouseButton::Left as i32,
                duration: None,
                modifiers: vec![],
            }),
        )
        .await
    }

    /// Raw keyboard request.
    pub async fn keyboard(
        &self,
        target: Option<pb::InputTarget>,
        action: KbAction,
    ) -> Result<pb::KeyboardResponse> {
        Ok(self
            .computer()
            .keyboard(pb::KeyboardRequest {
                target,
                action: Some(action),
            })
            .await?
            .into_inner())
    }

    /// Types text.
    pub async fn type_text(&self, text: &str) -> Result<pb::KeyboardResponse> {
        self.keyboard(
            None,
            KbAction::Type(pb::KeyboardType {
                text: text.into(),
                mode: pb::TextEntryMode::Keystrokes as i32,
                delay: None,
            }),
        )
        .await
    }

    /// Presses one key (`"enter"`, `"a"`, `pb::Key::Tab`...).
    pub async fn press(
        &self,
        key: impl TryInto<KeySpec, Error = Error>,
    ) -> Result<pb::KeyboardResponse> {
        let key = key.try_into()?;
        self.keyboard(
            None,
            KbAction::Press(pb::KeyboardPress {
                key: Some(key.to_pb()),
                modifiers: vec![],
                repeat: 1,
            }),
        )
        .await
    }

    /// Presses a chord, for example `["ctrl", "shift", "t"]`.
    pub async fn hotkey(&self, keys: &[&str]) -> Result<pb::KeyboardResponse> {
        let keys = keys
            .iter()
            .map(|k| KeySpec::parse(k).map(|k| k.to_pb()))
            .collect::<Result<Vec<_>>>()?;
        self.keyboard(None, KbAction::Hotkey(pb::KeyboardHotkey { keys }))
            .await
    }

    /// Clipboard text (`None` when the clipboard has no text flavor).
    pub async fn get_clipboard(&self) -> Result<Option<String>> {
        Ok(self
            .computer()
            .get_clipboard(pb::GetClipboardRequest {})
            .await?
            .into_inner()
            .content
            .and_then(|c| c.text))
    }

    /// Sets clipboard text.
    pub async fn set_clipboard(&self, text: &str) -> Result<u64> {
        Ok(self
            .computer()
            .set_clipboard(pb::SetClipboardRequest {
                content: Some(pb::ClipboardContent {
                    text: Some(text.into()),
                    file_paths: vec![],
                    image_png: None,
                }),
            })
            .await?
            .into_inner()
            .generation)
    }

    /// Pointer position in screen points.
    pub async fn cursor_position(&self) -> Result<(f64, f64)> {
        let p = self
            .computer()
            .get_cursor_position(pb::GetCursorPositionRequest {})
            .await?
            .into_inner()
            .position
            .unwrap_or_default();
        Ok((p.x, p.y))
    }

    /// Attached displays.
    pub async fn displays(&self) -> Result<Vec<pb::Display>> {
        Ok(self
            .computer()
            .list_displays(pb::ListDisplaysRequest {})
            .await?
            .into_inner()
            .displays)
    }

    /// Whether the desktop session can take input, from the `desktop`
    /// component of `SystemService.Health` (display reachable, a window
    /// manager running where the platform has one, cua-driver input
    /// loaded). A spacesd that predates the component is judged by whether
    /// it reports a display.
    pub async fn desktop_readiness(&self) -> Result<DesktopReadiness> {
        let health = self.health().await?;
        if let Some(c) = health
            .components
            .iter()
            .find(|c| c.name == DESKTOP_COMPONENT)
        {
            return Ok(DesktopReadiness {
                ready: c.status == pb::HealthStatus::Serving as i32,
                detail: c.detail.clone(),
                reported: true,
            });
        }
        let displays = self.displays().await?;
        Ok(DesktopReadiness {
            ready: !displays.is_empty(),
            detail: if displays.is_empty() {
                "no display reported yet".into()
            } else {
                String::new()
            },
            reported: false,
        })
    }

    /// Polls [`SpacesdClient::desktop_readiness`] until the desktop is ready,
    /// for at most `timeout`. Fails with [`Error::DesktopNotReady`] (naming
    /// what is missing) when it is not, and with the underlying error when
    /// the guest has no desktop at all (`FeatureUnsupported`).
    pub async fn wait_desktop_ready(&self, timeout: Duration) -> Result<DesktopReadiness> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut last: String;
        loop {
            match self.desktop_readiness().await {
                Ok(r) if r.ready => return Ok(r),
                Ok(r) => last = r.detail,
                Err(e @ Error::FeatureUnsupported { .. }) => return Err(e),
                Err(e) if e.is_retryable() => last = e.to_string(),
                Err(e) => return Err(e),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::DesktopNotReady(format!(
                    "not ready after {timeout:?}: {}",
                    if last.is_empty() {
                        "starting"
                    } else {
                        last.as_str()
                    }
                )));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Opens a media session and returns the ticketed WebSocket URL.
    pub async fn open_media(&self, options: MediaOptions) -> Result<MediaSession> {
        let target = match (options.window, options.display) {
            (Some(w), _) => MediaTargetKind::Window(w),
            (None, d) => MediaTargetKind::DisplayId(d.unwrap_or_else(|| "primary".into())),
        };
        let raw = self
            .stream()
            .open_media(pb::OpenMediaRequest {
                target: Some(pb::MediaTarget {
                    target: Some(target),
                }),
                codecs: options.codecs.iter().map(|c| *c as i32).collect(),
                max_fps: options.max_fps,
                max_dimension: options.max_dimension,
                bitrate_kbps: 0,
                policy: 0,
                geometry_control: 0,
                ticket_ttl: options.ticket_ttl.map(duration_pb),
                prefer_quic: options.prefer_quic,
                audio: options.audio,
                disable_video: options.disable_video,
                ..Default::default()
            })
            .await?
            .into_inner();
        let ws_path = if raw.ws_path.is_empty() {
            format!(
                "{}?ticket={}",
                cua_proto::metadata::MEDIA_WS_PATH,
                raw.ticket
            )
        } else {
            raw.ws_path.clone()
        };
        Ok(MediaSession {
            session_id: raw.media_session_id.clone(),
            ticket: raw.ticket.clone(),
            ws_url: self.endpoint().ws_url(&ws_path),
            raw,
        })
    }
}

impl TryFrom<&str> for KeySpec {
    type Error = Error;
    fn try_from(s: &str) -> Result<Self> {
        KeySpec::parse(s)
    }
}

/// Whether the action places the pointer at a point (so a foreground
/// delivery must move it).
fn pointer_positioned(action: &PtrAction) -> bool {
    match action {
        PtrAction::Click(c) => c.position.is_some(),
        PtrAction::Move(m) => m.position.is_some(),
        PtrAction::Down(d) => d.position.is_some(),
        PtrAction::Up(u) => u.position.is_some(),
        PtrAction::Drag(_) => true,
        PtrAction::Scroll(s) => s.position.is_some(),
    }
}

/// A screen-space pointer action delivered in the foreground always moves
/// the pointer (cua-driver's `pointer_moved`); when the report says it did
/// not, the input reached nothing. A missing report (older spacesd) and
/// background delivery are not judged.
pub(crate) fn check_pointer_delivered(report: Option<&pb::DeliveryReport>) -> Result<()> {
    let Some(r) = report else {
        return Ok(());
    };
    if r.delivery == pb::Delivery::Foreground as i32 && !r.pointer_moved {
        let detail = if r.detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", r.detail)
        };
        return Err(Error::DeliveryFailed(crate::error::ErrorDetails {
            code: tonic::Code::FailedPrecondition as i32,
            message: format!(
                "the pointer input was not delivered: the guest reports the pointer did not move{detail}; \
                 check that the desktop session is up (`cua sb create --wait desktop`)"
            ),
            metadata: Default::default(),
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys() {
        assert_eq!(
            KeySpec::parse("enter").unwrap(),
            KeySpec::Named(pb::Key::Enter)
        );
        assert_eq!(
            KeySpec::parse("KEY_TAB").unwrap(),
            KeySpec::Named(pb::Key::Tab)
        );
        assert_eq!(
            KeySpec::parse("ctrl").unwrap(),
            KeySpec::Named(pb::Key::Control)
        );
        assert_eq!(KeySpec::parse("é").unwrap(), KeySpec::Char("é".into()));
        assert!(KeySpec::parse("notakey").is_err());
    }
}
