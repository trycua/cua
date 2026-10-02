// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua.env.v1.ComputerService`: screenshots, pointer, keyboard, clipboard,
//! cursor position and displays.

use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cua_proto::env::v1::{
    computer_service_server::ComputerService, keyboard_request, pointer_request,
    screenshot_request, ClipboardContent, CoordinateSpace, Delivery, DeliveryReport, ErrorReason,
    GetClipboardRequest, GetClipboardResponse, GetCursorPositionRequest, GetCursorPositionResponse,
    ImageFormat, InputTarget, KeyboardRequest, KeyboardResponse, ListDisplaysRequest,
    ListDisplaysResponse, MouseButton, PixelSize, Point, PointerRequest, PointerResponse,
    PrincipalKind, Rect, ScreenshotRequest, ScreenshotResponse, ScrollUnit, SetClipboardRequest,
    SetClipboardResponse, TextEntryMode,
};
use cua_spacesd_session::media::leases::LeaseConflict;
use tonic::{Code, Request, Response, Status};

use super::backend::{
    CapturedImage, ClipboardData, DeliveryRequest, DeliveryResult, DeliveryUsed, KeyAction,
    PointerAction, WindowRecord, MAX_CLIPBOARD_IMAGE_BYTES,
};
use super::caller_principal;
use super::status::{self, invalid, join_error, provider};
use super::{
    keys, proto_display, random_id, DesktopState, ScreenshotRecord, ScreenshotSource,
    SCREENSHOT_HISTORY,
};

pub(crate) struct Computer(pub Arc<DesktopState>);

/// A viewer ticket reaches the clipboard only with the clipboard grant.
/// Returns the grant (for confining clipboard file paths).
fn viewer_clipboard_allowed<T>(
    request: &Request<T>,
) -> Result<Option<Arc<cua_spacesd_server::ViewerGrant>>, Status> {
    match cua_spacesd_server::caller(request).viewer {
        Some(grant) if !grant.clipboard => Err(status::status(
            Code::PermissionDenied,
            ErrorReason::PermissionDenied,
            "viewer ticket: clipboard access was not granted",
        )),
        other => Ok(other),
    }
}

static CLIPBOARD_GENERATION: AtomicU64 = AtomicU64::new(1);
static CLIPBOARD_HASH: Mutex<Option<u64>> = Mutex::new(None);

/// Pixels per wheel line for `SCROLL_UNIT_PIXEL`.
const PIXELS_PER_LINE: f64 = 40.0;

/// x, y, width, height in global logical points.
type Bounds = (f64, f64, f64, f64);

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Status> + Send + 'static,
) -> Result<T, Status> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(join_error)?
}

fn delivery_request(value: i32) -> DeliveryRequest {
    match Delivery::try_from(value).unwrap_or(Delivery::Unspecified) {
        Delivery::Background => DeliveryRequest::Background,
        Delivery::Foreground => DeliveryRequest::Foreground,
        Delivery::Auto | Delivery::Unspecified => DeliveryRequest::Auto,
    }
}

fn report(result: DeliveryResult) -> DeliveryReport {
    DeliveryReport {
        delivery: match result.delivery {
            DeliveryUsed::Background => Delivery::Background as i32,
            DeliveryUsed::Foreground => Delivery::Foreground as i32,
        },
        focus_changed: result.focus_changed,
        pointer_moved: result.pointer_moved,
        detail: result.detail,
    }
}

fn button_name(value: i32) -> String {
    match MouseButton::try_from(value).unwrap_or(MouseButton::Unspecified) {
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::Back => "back",
        MouseButton::Forward => "forward",
        MouseButton::Left | MouseButton::Unspecified => "left",
    }
    .into()
}

impl Computer {
    fn target_window(&self, target: &InputTarget) -> Result<Option<WindowRecord>, Status> {
        target
            .window
            .as_ref()
            .map(|window| self.0.window(window))
            .transpose()
    }

    fn display_bounds(&self, display_id: &str) -> Result<(String, Bounds), Status> {
        let displays = self.0.backend.displays().map_err(provider)?;
        let display = displays
            .iter()
            .find(|display| {
                display.id == display_id
                    || ((display_id.is_empty() || display_id == "primary") && display.primary)
            })
            .or_else(|| {
                (display_id.is_empty() || display_id == "primary")
                    .then(|| displays.first())
                    .flatten()
            })
            .ok_or_else(|| status::not_found(format!("unknown display {display_id}")))?;
        Ok((display.id.clone(), display.bounds))
    }

    /// Convert a point in the request's coordinate space to global logical
    /// points.
    fn to_screen(
        &self,
        target: &InputTarget,
        window: Option<&WindowRecord>,
        point: &Point,
    ) -> Result<(f64, f64), Status> {
        match CoordinateSpace::try_from(target.space).unwrap_or(CoordinateSpace::Unspecified) {
            CoordinateSpace::Unspecified | CoordinateSpace::Screen => Ok((point.x, point.y)),
            CoordinateSpace::Window => {
                let window =
                    window.ok_or_else(|| invalid("COORDINATE_SPACE_WINDOW needs target.window"))?;
                Ok((window.bounds.0 + point.x, window.bounds.1 + point.y))
            }
            CoordinateSpace::Normalized => {
                if !(0.0..=1.0).contains(&point.x) || !(0.0..=1.0).contains(&point.y) {
                    return Err(invalid("normalized coordinates must be within [0, 1]"));
                }
                let bounds = match window {
                    Some(window) => window.bounds,
                    None => self.display_bounds(&target.display_id)?.1,
                };
                Ok((bounds.0 + point.x * bounds.2, bounds.1 + point.y * bounds.3))
            }
            CoordinateSpace::Screenshot => {
                let screenshots = self.0.screenshots.lock().unwrap();
                let record = screenshots
                    .iter()
                    .find(|record| record.id == target.screenshot_id)
                    .ok_or_else(|| {
                        status::status(
                            Code::FailedPrecondition,
                            ErrorReason::StaleGeometry,
                            "unknown or expired screenshot_id",
                        )
                    })?;
                let current = match &record.source {
                    ScreenshotSource::Display(id) => {
                        self.display_bounds(id).map(|(_, bounds)| bounds).ok()
                    }
                    ScreenshotSource::Window(handle) => self
                        .0
                        .backend
                        .windows()
                        .ok()
                        .and_then(|windows| {
                            windows
                                .into_iter()
                                .find(|window| &window.handle.0 == handle)
                        })
                        .map(|window| window.bounds),
                };
                let (x, y, width, height) = record.logical_bounds;
                if current.is_none_or(|current| {
                    (current.2 - width).abs() > 0.5 || (current.3 - height).abs() > 0.5
                }) {
                    return Err(status::status(
                        Code::FailedPrecondition,
                        ErrorReason::StaleGeometry,
                        "the screenshot's source changed size since it was taken",
                    ));
                }
                let scale_x = width / f64::from(record.image_size.0.max(1));
                let scale_y = height / f64::from(record.image_size.1.max(1));
                Ok((x + point.x * scale_x, y + point.y * scale_y))
            }
        }
    }

    fn lease(
        &self,
        request_principal: &Option<cua_proto::env::v1::Principal>,
        window: Option<&WindowRecord>,
    ) -> Result<(), Status> {
        let Some(window) = window else {
            return Ok(());
        };
        let (id, name) = request_principal
            .as_ref()
            .map(|principal| (principal.id.clone(), principal.display_name.clone()))
            .unwrap_or_else(|| ("anonymous".into(), "Anonymous".into()));
        match self
            .0
            .leases
            .acquire(&format!("window:{}", window.handle.0), &id, &name)
        {
            Ok(_) => Ok(()),
            Err(LeaseConflict {
                holder_name,
                retry_after,
                ..
            }) => Err(status::status_with(
                Code::Aborted,
                ErrorReason::RateLimited,
                format!("input to this window is leased to {holder_name}"),
                "",
                &[
                    ("retry_after_ms", retry_after.as_millis().to_string()),
                    ("lease_holder", holder_name),
                ],
            )),
        }
    }

    fn publish_agent_cursor(
        &self,
        principal: &Option<cua_proto::env::v1::Principal>,
        window: Option<&WindowRecord>,
        point: (f64, f64),
    ) {
        let Some(principal) = principal else {
            return;
        };
        if principal.kind != PrincipalKind::Agent as i32 {
            return;
        }
        let (display_id, bounds) = match self.display_bounds("primary") {
            Ok(display) => display,
            Err(_) => return,
        };
        let (normalized, handle) = match window {
            Some(window) => (
                (
                    (point.0 - window.bounds.0) / window.bounds.2.max(1.0),
                    (point.1 - window.bounds.1) / window.bounds.3.max(1.0),
                ),
                Some((window.handle.0.clone(), window.epoch.0)),
            ),
            None => (
                (
                    (point.0 - bounds.0) / bounds.2.max(1.0),
                    (point.1 - bounds.1) / bounds.3.max(1.0),
                ),
                None,
            ),
        };
        let name = if principal.display_name.is_empty() {
            principal.id.clone()
        } else {
            principal.display_name.clone()
        };
        self.0
            .presence
            .agent_cursor(&principal.id, &name, &display_id, handle, normalized);
    }
}

fn encode_image(
    image: &CapturedImage,
    format: ImageFormat,
    quality: u32,
) -> Result<(Vec<u8>, ImageFormat), Status> {
    let mut rgba = image.bgra.clone();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(image.width, image.height, rgba).ok_or_else(|| {
        status::status(
            Code::Internal,
            ErrorReason::Internal,
            "capture buffer size mismatch",
        )
    })?;
    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(buffer).to_rgb8();
            let quality = if quality == 0 {
                80
            } else {
                quality.clamp(1, 100)
            } as u8;
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .encode_image(&rgb)
                .map_err(|error| {
                    status::status(Code::Internal, ErrorReason::Internal, error.to_string())
                })?;
            Ok((out.into_inner(), ImageFormat::Jpeg))
        }
        // WebP encoding is not bundled; PNG is returned and reported.
        _ => {
            buffer
                .write_to(&mut out, image::ImageFormat::Png)
                .map_err(|error| {
                    status::status(Code::Internal, ErrorReason::Internal, error.to_string())
                })?;
            Ok((out.into_inner(), ImageFormat::Png))
        }
    }
}

fn crop(image: &CapturedImage, region: &Rect) -> Result<CapturedImage, Status> {
    let (bx, by, bw, bh) = image.logical_bounds;
    let scale_x = f64::from(image.width) / bw.max(1.0);
    let scale_y = f64::from(image.height) / bh.max(1.0);
    let x0 = (region.x * scale_x).floor().max(0.0) as u32;
    let y0 = (region.y * scale_y).floor().max(0.0) as u32;
    let x1 = ((region.x + region.width) * scale_x)
        .ceil()
        .min(f64::from(image.width)) as u32;
    let y1 = ((region.y + region.height) * scale_y)
        .ceil()
        .min(f64::from(image.height)) as u32;
    if x1 <= x0 || y1 <= y0 {
        return Err(invalid("region is empty or outside the source"));
    }
    let width = x1 - x0;
    let height = y1 - y0;
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for row in y0..y1 {
        let start = ((row * image.width + x0) * 4) as usize;
        bgra.extend_from_slice(&image.bgra[start..start + (width * 4) as usize]);
    }
    Ok(CapturedImage {
        bgra,
        width,
        height,
        logical_bounds: (
            bx + f64::from(x0) / scale_x,
            by + f64::from(y0) / scale_y,
            f64::from(width) / scale_x,
            f64::from(height) / scale_y,
        ),
        display_id: image.display_id.clone(),
    })
}

#[tonic::async_trait]
impl ComputerService for Computer {
    async fn screenshot(
        &self,
        request: Request<ScreenshotRequest>,
    ) -> Result<Response<ScreenshotResponse>, Status> {
        let request = request.into_inner();
        let state = self.0.clone();
        let (source, window) = match &request.source {
            Some(screenshot_request::Source::Window(reference)) => {
                let window = self.0.window(reference)?;
                (
                    ScreenshotSource::Window(window.handle.0.clone()),
                    Some(window),
                )
            }
            Some(screenshot_request::Source::DisplayId(id)) => {
                (ScreenshotSource::Display(self.display_bounds(id)?.0), None)
            }
            None => (
                ScreenshotSource::Display(self.display_bounds("primary")?.0),
                None,
            ),
        };
        let format = ImageFormat::try_from(request.format).unwrap_or(ImageFormat::Png);
        let source_for_capture = source.clone();
        let (captured, native, encoded, encoded_format) = blocking(move || {
            let mut captured = match (&source_for_capture, &window) {
                (_, Some(window)) => state.backend.capture_window(window),
                (ScreenshotSource::Display(id), None) => state.backend.capture_display(id),
                _ => unreachable!("window source always resolves a window"),
            }
            .map_err(provider)?;
            if let Some(region) = &request.region {
                captured = crop(&captured, region)?;
            }
            let native = (captured.width, captured.height);
            if request.max_dimension > 0
                && captured.width.max(captured.height) > request.max_dimension
            {
                let long = captured.width.max(captured.height);
                let width = (u64::from(captured.width) * u64::from(request.max_dimension)
                    / u64::from(long))
                .max(1) as u32;
                let height = (u64::from(captured.height) * u64::from(request.max_dimension)
                    / u64::from(long))
                .max(1) as u32;
                captured.bgra = cua_spacesd_session::media::encoder::downscale_bgra(
                    &captured.bgra,
                    captured.width,
                    captured.height,
                    captured.width * 4,
                    width,
                    height,
                );
                captured.width = width;
                captured.height = height;
            }
            let (encoded, encoded_format) = encode_image(&captured, format, request.quality)?;
            Ok((captured, native, encoded, encoded_format))
        })
        .await?;
        let id = random_id("shot");
        {
            let mut screenshots = self.0.screenshots.lock().unwrap();
            screenshots.push_back(ScreenshotRecord {
                id: id.clone(),
                logical_bounds: captured.logical_bounds,
                image_size: (captured.width, captured.height),
                source,
            });
            while screenshots.len() > SCREENSHOT_HISTORY {
                screenshots.pop_front();
            }
        }
        let (x, y, width, height) = captured.logical_bounds;
        Ok(Response::new(ScreenshotResponse {
            image: encoded,
            format: encoded_format as i32,
            image_size: Some(PixelSize {
                width: captured.width,
                height: captured.height,
            }),
            native_size: Some(PixelSize {
                width: native.0,
                height: native.1,
            }),
            scale: f64::from(captured.width) / width.max(1.0),
            logical_bounds: Some(Rect {
                x,
                y,
                width,
                height,
            }),
            screenshot_id: id,
            display_id: captured.display_id,
            captured_at: Some(super::timestamp(std::time::SystemTime::now())),
        }))
    }

    async fn pointer(
        &self,
        request: Request<PointerRequest>,
    ) -> Result<Response<PointerResponse>, Status> {
        let principal = caller_principal(&request);
        let request = request.into_inner();
        let target = request.target.unwrap_or_default();
        let window = self.target_window(&target)?;
        let delivery = delivery_request(target.delivery);
        let action = request
            .action
            .ok_or_else(|| invalid("pointer action is required"))?;
        let current = {
            let state = self.0.clone();
            blocking(move || state.backend.cursor_position().map_err(provider)).await?
        };
        let point_or_current = |point: &Option<Point>| -> Result<(f64, f64), Status> {
            match point {
                Some(point) => self.to_screen(&target, window.as_ref(), point),
                None => Ok(current),
            }
        };
        let (point, action) = match action {
            pointer_request::Action::Click(click) => (
                point_or_current(&click.position)?,
                PointerAction::Click {
                    button: button_name(click.button),
                    count: click.count.clamp(1, 3),
                    modifiers: keys::modifier_names(&click.modifiers),
                },
            ),
            pointer_request::Action::Move(movement) => {
                (point_or_current(&movement.position)?, PointerAction::Move)
            }
            pointer_request::Action::Down(down) => (
                point_or_current(&down.position)?,
                PointerAction::Down {
                    button: button_name(down.button),
                },
            ),
            pointer_request::Action::Up(up) => (
                point_or_current(&up.position)?,
                PointerAction::Up {
                    button: button_name(up.button),
                },
            ),
            pointer_request::Action::Drag(drag) => {
                let from = point_or_current(&drag.from)?;
                let to = drag.to.as_ref().ok_or_else(|| invalid("drag needs `to`"))?;
                let mut path = Vec::new();
                for point in &drag.path {
                    path.push(self.to_screen(&target, window.as_ref(), point)?);
                }
                let end = self.to_screen(&target, window.as_ref(), to)?;
                if path.is_empty() {
                    // Intermediate moves so drag-aware surfaces see a gesture.
                    for step in 1..10 {
                        let t = f64::from(step) / 10.0;
                        path.push((from.0 + (end.0 - from.0) * t, from.1 + (end.1 - from.1) * t));
                    }
                }
                path.push(end);
                (
                    from,
                    PointerAction::Drag {
                        path,
                        button: button_name(drag.button),
                        modifiers: keys::modifier_names(&drag.modifiers),
                    },
                )
            }
            pointer_request::Action::Scroll(scroll) => {
                if !scroll.delta_x.is_finite() || !scroll.delta_y.is_finite() {
                    return Err(invalid("scroll deltas must be finite"));
                }
                let divisor =
                    match ScrollUnit::try_from(scroll.unit).unwrap_or(ScrollUnit::Unspecified) {
                        ScrollUnit::Pixel => PIXELS_PER_LINE,
                        _ => 1.0,
                    };
                (
                    point_or_current(&scroll.position)?,
                    PointerAction::Scroll {
                        dx: scroll.delta_x / divisor,
                        dy: scroll.delta_y / divisor,
                    },
                )
            }
        };
        self.lease(&principal, window.as_ref())?;
        let state = self.0.clone();
        let window_for_call = window.clone();
        let rests_at = match &action {
            PointerAction::Drag { path, .. } => path.last().copied().unwrap_or(point),
            _ => point,
        };
        // Presence: this principal now drives the pointer.
        let announced = self
            .0
            .activity
            .announce(principal.as_ref().map(|p| p.id.as_str()).unwrap_or(""))
            .at(Some(rests_at));
        let result = blocking(move || {
            let _guard = announced.acquire_blocking();
            state
                .backend
                .pointer(window_for_call.as_ref(), delivery, point, &action)
                .map_err(provider)
        })
        .await?;
        let end_point = point;
        self.publish_agent_cursor(&principal, window.as_ref(), end_point);
        let cursor = {
            let state = self.0.clone();
            blocking(move || state.backend.cursor_position().map_err(provider)).await?
        };
        Ok(Response::new(PointerResponse {
            report: Some(report(result)),
            cursor_position: Some(Point {
                x: cursor.0,
                y: cursor.1,
            }),
        }))
    }

    async fn keyboard(
        &self,
        request: Request<KeyboardRequest>,
    ) -> Result<Response<KeyboardResponse>, Status> {
        let principal = caller_principal(&request);
        let request = request.into_inner();
        let target = request.target.unwrap_or_default();
        let window = self.target_window(&target)?;
        let delivery = delivery_request(target.delivery);
        let unknown_key = || invalid("unknown or unsupported key");
        let action = match request
            .action
            .ok_or_else(|| invalid("keyboard action is required"))?
        {
            keyboard_request::Action::Type(typed) => {
                if typed.text.len() > 64 * 1024 {
                    return Err(status::status(
                        Code::InvalidArgument,
                        ErrorReason::LimitExceeded,
                        "text exceeds 64 KiB",
                    ));
                }
                KeyAction::Type {
                    text: typed.text,
                    insert: TextEntryMode::try_from(typed.mode).ok() == Some(TextEntryMode::Insert),
                }
            }
            keyboard_request::Action::Press(press) => KeyAction::Press {
                key: press
                    .key
                    .as_ref()
                    .and_then(keys::key_input_name)
                    .ok_or_else(unknown_key)?,
                modifiers: keys::modifier_names(&press.modifiers),
                repeat: press.repeat.clamp(1, 100),
            },
            keyboard_request::Action::Hotkey(hotkey) => {
                let keys = hotkey
                    .keys
                    .iter()
                    .map(keys::key_input_name)
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(unknown_key)?;
                if keys.is_empty() {
                    return Err(invalid("hotkey needs at least one key"));
                }
                KeyAction::Hotkey { keys }
            }
            keyboard_request::Action::Down(down) => KeyAction::Down {
                key: down
                    .key
                    .as_ref()
                    .and_then(keys::key_input_name)
                    .ok_or_else(unknown_key)?,
            },
            keyboard_request::Action::Up(up) => KeyAction::Up {
                key: up
                    .key
                    .as_ref()
                    .and_then(keys::key_input_name)
                    .ok_or_else(unknown_key)?,
            },
        };
        self.lease(&principal, window.as_ref())?;
        let state = self.0.clone();
        let announced = self
            .0
            .activity
            .announce(principal.as_ref().map(|p| p.id.as_str()).unwrap_or(""));
        let result = blocking(move || {
            let _guard = announced.acquire_blocking();
            state
                .backend
                .keyboard(window.as_ref(), delivery, &action)
                .map_err(provider)
        })
        .await?;
        Ok(Response::new(KeyboardResponse {
            report: Some(report(result)),
        }))
    }

    async fn get_clipboard(
        &self,
        request: Request<GetClipboardRequest>,
    ) -> Result<Response<GetClipboardResponse>, Status> {
        viewer_clipboard_allowed(&request)?;
        let state = self.0.clone();
        let data = blocking(move || state.backend.clipboard_get().map_err(provider)).await?;
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            data.hash(&mut hasher);
            hasher.finish()
        };
        let generation = {
            let mut last = CLIPBOARD_HASH.lock().unwrap();
            if last.is_some_and(|previous| previous != hash) {
                CLIPBOARD_GENERATION.fetch_add(1, Ordering::Relaxed);
            }
            *last = Some(hash);
            CLIPBOARD_GENERATION.load(Ordering::Relaxed)
        };
        Ok(Response::new(GetClipboardResponse {
            content: Some(ClipboardContent {
                text: data.text,
                file_paths: data.files,
                image_png: data.image_png,
            }),
            generation,
        }))
    }

    async fn set_clipboard(
        &self,
        request: Request<SetClipboardRequest>,
    ) -> Result<Response<SetClipboardResponse>, Status> {
        let viewer = viewer_clipboard_allowed(&request)?;
        let content = request.into_inner().content.unwrap_or_default();
        if content
            .image_png
            .as_ref()
            .is_some_and(|png| png.len() > MAX_CLIPBOARD_IMAGE_BYTES)
        {
            return Err(status::status(
                Code::InvalidArgument,
                ErrorReason::LimitExceeded,
                "clipboard image exceeds 16 MiB",
            ));
        }
        if content
            .text
            .as_ref()
            .is_some_and(|text| text.len() > 1024 * 1024)
        {
            return Err(status::status(
                Code::InvalidArgument,
                ErrorReason::LimitExceeded,
                "clipboard text exceeds 1 MiB",
            ));
        }
        for path in &content.file_paths {
            if !std::path::Path::new(path).is_absolute() {
                return Err(invalid("clipboard file paths must be absolute"));
            }
            if let Some(grant) = viewer.as_deref() {
                cua_spacesd_server::filesystem::confine(grant, path.into())?;
            }
        }
        let state = self.0.clone();
        blocking(move || {
            state
                .backend
                .clipboard_set(ClipboardData {
                    text: content.text,
                    files: content.file_paths,
                    image_png: content.image_png,
                })
                .map_err(provider)
        })
        .await?;
        let generation = CLIPBOARD_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        *CLIPBOARD_HASH.lock().unwrap() = None;
        Ok(Response::new(SetClipboardResponse { generation }))
    }

    async fn get_cursor_position(
        &self,
        _request: Request<GetCursorPositionRequest>,
    ) -> Result<Response<GetCursorPositionResponse>, Status> {
        let state = self.0.clone();
        let (position, displays) = blocking(move || {
            Ok((
                state.backend.cursor_position().map_err(provider)?,
                state.backend.displays().map_err(provider)?,
            ))
        })
        .await?;
        let display_id = displays
            .iter()
            .find(|display| {
                let (x, y, width, height) = display.bounds;
                position.0 >= x
                    && position.1 >= y
                    && position.0 < x + width
                    && position.1 < y + height
            })
            .map(|display| display.id.clone())
            .unwrap_or_default();
        Ok(Response::new(GetCursorPositionResponse {
            position: Some(Point {
                x: position.0,
                y: position.1,
            }),
            display_id,
        }))
    }

    async fn list_displays(
        &self,
        _request: Request<ListDisplaysRequest>,
    ) -> Result<Response<ListDisplaysResponse>, Status> {
        let state = self.0.clone();
        let displays = blocking(move || state.backend.displays().map_err(provider)).await?;
        Ok(Response::new(ListDisplaysResponse {
            displays: displays.into_iter().map(proto_display).collect(),
        }))
    }
}
