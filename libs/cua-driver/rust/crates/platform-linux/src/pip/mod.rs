//! Linux multi-target Agent View.
//!
//! The backend uses a regular, resizable X11 utility window (and therefore
//! also works through XWayland) with a software-rendered GNOME-like overview.
//! It only presents frames supplied by the shared Agent View model; it never
//! moves, resizes, focuses, or closes the represented applications.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

#[cfg(target_os = "linux")]
mod wayland;

#[cfg(target_os = "linux")]
use image::imageops::FilterType;
use pip_preview::{
    layout_desktop, layout_session_tabs, png_dimensions, LayoutRect, PipBackend, PipBackendFactory,
    PipConfig, PipFrame, PipTargetKind, PipViewModel, PipWorkspaceSummary, SessionTabsLayout,
    TargetSize,
};

const MIN_WIDTH: u16 = 360;
const MIN_HEIGHT: u16 = 260;

pub(super) enum UiMessage {
    Frame(PipFrame),
    RemoveTarget(String, String),
    RemoveWorkspace(String),
    SetInputTransparent(bool, mpsc::SyncSender<anyhow::Result<()>>),
    Shutdown,
}

pub struct LinuxPipBackend {
    tx: Sender<UiMessage>,
}

impl LinuxPipBackend {
    /// Synchronously include or exclude Agent View from X11 pointer hit-testing.
    ///
    /// Cua actions should enable transparency before injecting input and restore
    /// interaction afterward. Waiting for the X11 thread to flush the shape
    /// change prevents an always-on-top Agent View from intercepting the action.
    pub fn set_input_transparent(&self, transparent: bool) -> anyhow::Result<()> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        if self
            .tx
            .send(UiMessage::SetInputTransparent(transparent, reply_tx))
            .is_err()
        {
            return Ok(());
        }
        reply_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| {
                anyhow::anyhow!("timed out updating Agent View input shape: {error}")
            })?
    }
}

impl PipBackend for LinuxPipBackend {
    fn push_frame(&self, frame: PipFrame) {
        tracing::debug!(
            target: "pip",
            view_id = %frame.target.view_id(),
            "queueing Linux Agent View frame"
        );
        let _ = self.tx.send(UiMessage::Frame(frame));
    }

    fn remove_workspace(&self, workspace_id: &str) {
        let _ = self
            .tx
            .send(UiMessage::RemoveWorkspace(workspace_id.to_owned()));
    }

    fn set_input_passthrough(&self, passthrough: bool) -> anyhow::Result<()> {
        self.set_input_transparent(passthrough)
    }

    fn remove_target(&self, workspace_id: &str, identity_key: &str) {
        let _ = self.tx.send(UiMessage::RemoveTarget(
            workspace_id.to_owned(),
            identity_key.to_owned(),
        ));
    }

    fn shutdown(self: Box<Self>) {
        let _ = self.tx.send(UiMessage::Shutdown);
    }
}

pub struct LinuxPipBackendFactory;

impl PipBackendFactory for LinuxPipBackendFactory {
    fn start(&self, cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let cfg = cfg.clone();
        std::thread::Builder::new()
            .name("cua-agent-view-linux".to_owned())
            .spawn(move || {
                #[cfg(target_os = "linux")]
                if should_use_native_wayland() {
                    wayland::run_window(cfg, rx, ready_tx);
                    return;
                }
                run_x11_window(cfg, rx, ready_tx)
            })?;

        ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("Agent View X11 thread exited during startup"))??;
        Ok(Box::new(LinuxPipBackend { tx }))
    }
}

#[cfg(target_os = "linux")]
fn should_use_native_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        && (crate::wayland::is_wayland() || std::env::var_os("DISPLAY").is_none())
}

#[cfg(target_os = "linux")]
fn run_x11_window(
    cfg: PipConfig,
    rx: Receiver<UiMessage>,
    ready: mpsc::SyncSender<anyhow::Result<()>>,
) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, PropMode,
        WindowClass,
    };
    use x11rb::protocol::Event;
    use x11rb::wrapper::ConnectionExt as _;

    let setup = (|| -> anyhow::Result<_> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let root_depth = screen.root_depth;
        let root_visual = screen.root_visual;
        let screen_width = screen.width_in_pixels;
        let width = cfg
            .geometry
            .width
            .clamp(u32::from(MIN_WIDTH), u32::from(u16::MAX)) as u16;
        let height = cfg
            .geometry
            .height
            .clamp(u32::from(MIN_HEIGHT), u32::from(u16::MAX)) as u16;
        let inset = 24i16;
        let x = cfg.geometry.x.map(clamp_i16).unwrap_or_else(|| {
            clamp_i16(i32::from(screen_width) - i32::from(width) - i32::from(inset))
        });
        let y = cfg.geometry.y.map(clamp_i16).unwrap_or(inset);
        let window = conn.generate_id()?;
        conn.create_window(
            root_depth,
            window,
            root,
            x,
            y,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            root_visual,
            &CreateWindowAux::new()
                .background_pixel(0x1820_27)
                .border_pixel(0)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::STRUCTURE_NOTIFY
                        | EventMask::PROPERTY_CHANGE
                        | EventMask::BUTTON_PRESS,
                ),
        )?
        .check()?;

        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?.check()?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            cfg.title.as_bytes(),
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"cua-agent-view\0CuaAgentView\0",
        )?;
        let net_wm_pid = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            net_wm_pid,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )?;

        let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
        let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            wm_protocols,
            AtomEnum::ATOM,
            &[wm_delete],
        )?;
        let net_wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
        let net_wm_state_above = conn
            .intern_atom(false, b"_NET_WM_STATE_ABOVE")?
            .reply()?
            .atom;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            net_wm_state,
            AtomEnum::ATOM,
            &[net_wm_state_above],
        )?;
        let net_wm_window_type = conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE")?
            .reply()?
            .atom;
        let net_wm_window_type_utility = conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE_UTILITY")?
            .reply()?
            .atom;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            net_wm_window_type,
            AtomEnum::ATOM,
            &[net_wm_window_type_utility],
        )?;
        conn.map_window(window)?.check()?;
        conn.flush()?;
        Ok((
            conn,
            window,
            gc,
            root_depth,
            wm_protocols,
            wm_delete,
            width,
            height,
        ))
    })();

    let (conn, window, gc, depth, wm_protocols, wm_delete, mut width, mut height) = match setup {
        Ok(values) => {
            let _ = ready.send(Ok(()));
            values
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    let mut model = PipViewModel::new(12);
    let mut dirty = true;
    let mut running = true;
    while running {
        while let Ok(message) = rx.try_recv() {
            handle_x11_message(
                &conn,
                window,
                width,
                height,
                message,
                &mut model,
                &mut dirty,
                &mut running,
            );
        }

        loop {
            match conn.poll_for_event() {
                Ok(Some(Event::Expose(_))) => dirty = true,
                Ok(Some(Event::ConfigureNotify(event))) => {
                    let next_width = event.width.max(1);
                    let next_height = event.height.max(1);
                    if next_width != width || next_height != height {
                        width = next_width;
                        height = next_height;
                        dirty = true;
                    }
                }
                Ok(Some(Event::ButtonPress(event))) => {
                    let workspaces = model.workspaces();
                    let tabs = session_tabs_for_bounds(
                        width,
                        height,
                        &workspaces,
                        model.selected_workspace_id(),
                    );
                    if let Some(workspace_id) =
                        tabs.hit_test(f64::from(event.event_x), f64::from(event.event_y))
                    {
                        dirty |= model.select_workspace(workspace_id);
                    }
                }
                Ok(Some(Event::ClientMessage(event)))
                    if is_delete_message(&event, wm_protocols, wm_delete) =>
                {
                    running = false;
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(target: "pip", "Agent View X11 event error: {error}");
                    running = false;
                    break;
                }
            }
        }

        if dirty && running {
            let frames = model.selected_frames();
            let workspaces = model.workspaces();
            let pixels = render_agent_view(
                width,
                height,
                &frames,
                &workspaces,
                model.selected_workspace_id(),
                model.active_view_id(),
            );
            if let Err(error) = upload_image(&conn, window, gc, depth, width, height, &pixels) {
                tracing::warn!(target: "pip", "Agent View X11 paint failed: {error}");
                running = false;
            }
            dirty = false;
        }

        if running {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(message) => handle_x11_message(
                    &conn,
                    window,
                    width,
                    height,
                    message,
                    &mut model,
                    &mut dirty,
                    &mut running,
                ),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => running = false,
            }
        }
    }

    let _ = conn.free_gc(gc);
    let _ = conn.destroy_window(window);
    let _ = conn.flush();
}

#[cfg(target_os = "linux")]
fn handle_x11_message(
    conn: &impl x11rb::connection::Connection,
    window: u32,
    width: u16,
    height: u16,
    message: UiMessage,
    model: &mut PipViewModel,
    dirty: &mut bool,
    running: &mut bool,
) {
    match message {
        UiMessage::SetInputTransparent(transparent, reply) => {
            let result = set_x11_input_transparent(conn, window, width, height, transparent);
            let _ = reply.send(result);
        }
        message => handle_message(message, model, dirty, running),
    }
}

#[cfg(target_os = "linux")]
fn set_x11_input_transparent(
    conn: &impl x11rb::connection::Connection,
    window: u32,
    width: u16,
    height: u16,
    transparent: bool,
) -> anyhow::Result<()> {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{ClipOrdering, Rectangle};

    let interactive_region = [Rectangle {
        x: 0,
        y: 0,
        width,
        height,
    }];
    let rectangles = if transparent {
        &[][..]
    } else {
        &interactive_region[..]
    };
    conn.shape_rectangles(
        SO::SET,
        SK::INPUT,
        ClipOrdering::UNSORTED,
        window,
        0,
        0,
        rectangles,
    )?
    .check()?;
    conn.flush()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn upload_image(
    conn: &impl x11rb::connection::Connection,
    window: u32,
    gc: u32,
    depth: u8,
    width: u16,
    height: u16,
    pixels: &[u8],
) -> anyhow::Result<()> {
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

    let stride = usize::from(width) * 4;
    // Keep each PutImage below the common 256 KiB X11 request limit.
    let rows_per_request = (200_000usize / stride.max(1)).max(1);
    for start_row in (0..usize::from(height)).step_by(rows_per_request) {
        let rows = rows_per_request.min(usize::from(height) - start_row);
        let start = start_row * stride;
        let end = start + rows * stride;
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            window,
            gc,
            width,
            rows as u16,
            0,
            start_row as i16,
            0,
            depth,
            &pixels[start..end],
        )?
        .check()?;
    }
    conn.flush()?;
    Ok(())
}

fn handle_message(
    message: UiMessage,
    model: &mut PipViewModel,
    dirty: &mut bool,
    running: &mut bool,
) {
    match message {
        UiMessage::Frame(frame) => {
            tracing::debug!(
                target: "pip",
                view_id = %frame.target.view_id(),
                "rendering Linux Agent View frame"
            );
            model.upsert(frame);
            *dirty = true;
        }
        UiMessage::RemoveTarget(workspace_id, identity_key) => {
            *dirty |= model.remove_target(&workspace_id, &identity_key);
        }
        UiMessage::RemoveWorkspace(workspace_id) => {
            *dirty |= !model.remove_workspace(&workspace_id).is_empty();
        }
        UiMessage::SetInputTransparent(_, reply) => {
            let _ = reply.send(Err(anyhow::anyhow!(
                "Agent View input shape update reached a non-X11 handler"
            )));
        }
        UiMessage::Shutdown => *running = false,
    }
}

#[cfg(not(target_os = "linux"))]
fn run_x11_window(
    _cfg: PipConfig,
    _rx: Receiver<UiMessage>,
    ready: mpsc::SyncSender<anyhow::Result<()>>,
) {
    let _ = ready.send(Err(anyhow::anyhow!(
        "Linux Agent View can only start on Linux"
    )));
}

#[cfg(target_os = "linux")]
fn is_delete_message(
    event: &x11rb::protocol::xproto::ClientMessageEvent,
    wm_protocols: u32,
    wm_delete: u32,
) -> bool {
    event.type_ == wm_protocols && event.data.as_data32()[0] == wm_delete
}

fn clamp_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

pub(super) fn render_agent_view(
    width: u16,
    height: u16,
    frames: &[&PipFrame],
    workspaces: &[PipWorkspaceSummary],
    selected_workspace_id: Option<&str>,
    active_view_id: Option<&str>,
) -> Vec<u8> {
    let mut canvas = Canvas::new(width, height);
    canvas.vertical_gradient(Color::rgb(45, 57, 66), Color::rgb(24, 29, 34));
    canvas.radial_glow(
        width as f64 * 0.18,
        height as f64 * 0.02,
        width as f64 * 0.72,
        Color::rgba(93, 129, 145, 70),
    );
    canvas.radial_glow(
        width as f64 * 0.88,
        height as f64 * 0.72,
        width as f64 * 0.55,
        Color::rgba(113, 90, 72, 42),
    );
    canvas.rounded_rect(
        Rect::new(1.0, 1.0, width as f64 - 2.0, height as f64 - 2.0),
        15.0,
        Color::rgba(15, 18, 21, 142),
    );
    canvas.stroke_rounded_rect(
        Rect::new(1.5, 1.5, width as f64 - 3.0, height as f64 - 3.0),
        14.5,
        1.0,
        Color::rgba(210, 221, 226, 92),
    );

    let sizes = frames
        .iter()
        .map(|frame| {
            png_dimensions(&frame.png_bytes).unwrap_or(TargetSize {
                width: 16,
                height: 10,
            })
        })
        .collect::<Vec<_>>();
    let selector_height = if workspaces.len() > 1 { 34.0 } else { 0.0 };
    let mut layout = layout_desktop(
        width.into(),
        (f64::from(height) - selector_height).max(1.0),
        &sizes,
    );
    offset_layout_y(&mut layout, selector_height);
    if frames.is_empty() {
        render_waiting_state(&mut canvas, layout.desktop);
    } else {
        for (frame, target) in frames.iter().zip(&layout.targets) {
            render_target(&mut canvas, frame, target.window);
            if active_view_id == Some(frame.target.view_id().as_str()) {
                canvas.stroke_rounded_rect(
                    Rect::from_layout(target.window).expand(3.0),
                    13.0,
                    2.0,
                    Color::rgba(55, 148, 255, 242),
                );
            }
        }
        // Agent View has no permanent launcher; session tabs remain the only
        // presentation-level navigation surface.
    }
    render_session_selector(
        &mut canvas,
        width,
        height,
        workspaces,
        selected_workspace_id,
    );
    render_resize_affordance(&mut canvas, width, height);
    canvas.into_bgrx()
}

fn render_waiting_state(canvas: &mut Canvas, desktop: LayoutRect) {
    let center_x = desktop.x + desktop.width / 2.0;
    let center_y = desktop.y + desktop.height / 2.0;
    canvas.circle(center_x, center_y - 7.0, 22.0, Color::rgba(48, 59, 67, 220));
    canvas.stroke_circle(
        center_x,
        center_y - 7.0,
        22.0,
        1.0,
        Color::rgba(223, 230, 233, 88),
    );
    for offset in [-8.0, 0.0, 8.0] {
        canvas.circle(
            center_x + offset,
            center_y - 7.0,
            2.1,
            Color::rgba(235, 239, 241, 170),
        );
    }
    canvas.rounded_rect(
        Rect::new(center_x - 58.0, center_y + 25.0, 116.0, 3.0),
        1.5,
        Color::rgba(224, 230, 232, 45),
    );
}

fn render_target(canvas: &mut Canvas, frame: &PipFrame, rect: LayoutRect) {
    let rect = Rect::from_layout(rect);
    canvas.shadow(rect, 9.0, 6.0, Color::rgba(0, 0, 0, 118));
    canvas.rounded_rect(rect.expand(1.0), 8.0, Color::rgba(220, 225, 227, 68));
    canvas.rounded_rect(rect, 7.0, Color::rgb(21, 25, 28));
    if !draw_frame_image(canvas, frame, rect) {
        canvas.vertical_gradient_in(rect, Color::rgb(47, 55, 61), Color::rgb(27, 31, 35));
        let accent = target_accent(frame.target.target_kind);
        canvas.circle(
            rect.x + rect.width / 2.0,
            rect.y + rect.height / 2.0,
            12.0,
            accent,
        );
    }
    canvas.stroke_rounded_rect(rect, 7.0, 1.0, Color::rgba(225, 230, 232, 76));
}

#[cfg(target_os = "linux")]
fn draw_frame_image(canvas: &mut Canvas, frame: &PipFrame, rect: Rect) -> bool {
    let Ok(image) = image::load_from_memory(&frame.png_bytes) else {
        return false;
    };
    canvas.draw_image_contain(&image.to_rgba8(), rect, 6.5);
    true
}

#[cfg(not(target_os = "linux"))]
fn draw_frame_image(_canvas: &mut Canvas, _frame: &PipFrame, _rect: Rect) -> bool {
    false
}

fn session_tabs_for_bounds(
    width: u16,
    height: u16,
    workspaces: &[PipWorkspaceSummary],
    selected_workspace_id: Option<&str>,
) -> SessionTabsLayout {
    let visible = workspaces.len().min(6);
    let desired_width = visible as f64 * 190.0 + visible.saturating_sub(1) as f64 * 6.0;
    let panel_width = desired_width.min((f64::from(width) - 16.0).max(1.0));
    layout_session_tabs(
        LayoutRect {
            x: ((f64::from(width) - panel_width) / 2.0).max(0.0),
            y: 4.0_f64.min(f64::from(height).max(0.0)),
            width: panel_width,
            height: 34.0,
        },
        workspaces,
        selected_workspace_id,
    )
}

fn render_session_selector(
    canvas: &mut Canvas,
    width: u16,
    height: u16,
    workspaces: &[PipWorkspaceSummary],
    selected_workspace_id: Option<&str>,
) {
    let tabs = session_tabs_for_bounds(width, height, workspaces, selected_workspace_id);
    let Some(first) = tabs.tabs.first() else {
        return;
    };
    let last = tabs.tabs.last().expect("session tabs are non-empty");
    let dock = Rect::new(
        first.rect.x - 6.0,
        first.rect.y - 3.0,
        last.rect.x + last.rect.width - first.rect.x + 12.0,
        first.rect.height + 6.0,
    );
    canvas.shadow(dock, 12.0, 5.0, Color::rgba(0, 0, 0, 100));
    canvas.rounded_rect(dock, dock.height * 0.27, Color::rgba(67, 70, 72, 195));
    canvas.stroke_rounded_rect(
        dock,
        dock.height * 0.27,
        1.0,
        Color::rgba(231, 235, 236, 66),
    );
    for (workspace, tab) in workspaces.iter().zip(&tabs.tabs) {
        let icon = Rect::from_layout(tab.rect);
        let accent = Color::rgb(tab.accent.0, tab.accent.1, tab.accent.2);
        if tab.selected {
            canvas.rounded_rect(
                icon.expand(2.0),
                icon.width * 0.27,
                Color::rgba(245, 248, 249, 220),
            );
        }
        canvas.rounded_rect(icon, icon.width * 0.23, accent);
        let glyph = icon.inset(icon.width * 0.23);
        canvas.circle(
            glyph.x + glyph.width / 2.0,
            glyph.y + glyph.height * 0.36,
            glyph.width * 0.22,
            Color::rgba(250, 252, 253, 235),
        );
        canvas.rounded_rect(
            Rect::new(
                glyph.x + glyph.width * 0.16,
                glyph.y + glyph.height * 0.62,
                glyph.width * 0.68,
                glyph.height * 0.28,
            ),
            glyph.width * 0.14,
            Color::rgba(250, 252, 253, 225),
        );
        let indicators = workspace.target_count.min(3);
        for index in 0..indicators {
            canvas.circle(
                icon.x + icon.width / 2.0 + (index as f64 - (indicators - 1) as f64 / 2.0) * 5.0,
                dock.y + dock.height - 4.0,
                1.4,
                Color::rgba(242, 245, 246, if tab.selected { 245 } else { 145 }),
            );
        }
    }
}

fn offset_layout_y(layout: &mut pip_preview::DesktopLayout, offset: f64) {
    layout.desktop.y += offset;
    layout.dock.y += offset;
    for icon in &mut layout.dock_icons {
        icon.y += offset;
    }
    for target in &mut layout.targets {
        target.window.y += offset;
        target.content.y += offset;
    }
}

fn render_dash(canvas: &mut Canvas, frames: &[&PipFrame], dock: &LayoutRect, icons: &[LayoutRect]) {
    let dock = Rect::from_layout(*dock);
    canvas.shadow(dock, 12.0, 5.0, Color::rgba(0, 0, 0, 100));
    canvas.rounded_rect(dock, dock.height * 0.27, Color::rgba(67, 70, 72, 195));
    canvas.stroke_rounded_rect(
        dock,
        dock.height * 0.27,
        1.0,
        Color::rgba(231, 235, 236, 66),
    );
    for (frame, icon) in frames.iter().zip(icons) {
        let icon = Rect::from_layout(*icon);
        let accent = target_accent(frame.target.target_kind);
        canvas.rounded_rect(icon, icon.width * 0.23, accent);
        let glyph = icon.inset(icon.width * 0.23);
        match frame.target.target_kind {
            PipTargetKind::BrowserTab => {
                canvas.stroke_circle(
                    glyph.x + glyph.width / 2.0,
                    glyph.y + glyph.height / 2.0,
                    glyph.width * 0.44,
                    (glyph.width * 0.08).max(1.0),
                    Color::rgba(250, 252, 253, 235),
                );
                canvas.line(
                    glyph.x + glyph.width / 2.0,
                    glyph.y,
                    glyph.x + glyph.width / 2.0,
                    glyph.y + glyph.height,
                    (glyph.width * 0.07).max(1.0),
                    Color::rgba(250, 252, 253, 215),
                );
            }
            PipTargetKind::NativeWindow => {
                canvas.rounded_rect(glyph, glyph.width * 0.12, Color::rgba(250, 252, 253, 228));
                canvas.rect(
                    Rect::new(
                        glyph.x + glyph.width * 0.12,
                        glyph.y + glyph.height * 0.25,
                        glyph.width * 0.76,
                        (glyph.height * 0.09).max(1.0),
                    ),
                    accent,
                );
            }
        }
        canvas.circle(
            icon.x + icon.width / 2.0,
            dock.y + dock.height - 4.0,
            1.6,
            Color::rgba(242, 245, 246, 225),
        );
    }
}

fn render_resize_affordance(canvas: &mut Canvas, width: u16, height: u16) {
    let x = f64::from(width) - 15.0;
    let y = f64::from(height) - 8.0;
    let color = Color::rgba(226, 231, 233, 116);
    canvas.line(x, y, x + 7.0, y - 7.0, 1.2, color);
    canvas.line(x + 5.0, y, x + 9.0, y - 4.0, 1.2, color);
}

fn target_accent(kind: PipTargetKind) -> Color {
    match kind {
        PipTargetKind::BrowserTab => Color::rgb(53, 154, 220),
        PipTargetKind::NativeWindow => Color::rgb(239, 112, 54),
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width: width.max(0.0),
            height: height.max(0.0),
        }
    }

    fn from_layout(rect: LayoutRect) -> Self {
        Self::new(rect.x, rect.y, rect.width, rect.height)
    }

    fn inset(self, amount: f64) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            self.width - amount * 2.0,
            self.height - amount * 2.0,
        )
    }

    fn expand(self, amount: f64) -> Self {
        Self::new(
            self.x - amount,
            self.y - amount,
            self.width + amount * 2.0,
            self.height + amount * 2.0,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

impl Color {
    const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

struct Canvas {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new(width: u16, height: u16) -> Self {
        Self {
            width: usize::from(width),
            height: usize::from(height),
            rgba: vec![0; usize::from(width) * usize::from(height) * 4],
        }
    }

    fn into_bgrx(mut self) -> Vec<u8> {
        for pixel in self.rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 0;
        }
        self.rgba
    }

    fn blend(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let index = (y as usize * self.width + x as usize) * 4;
        let alpha = u32::from(color.a);
        let inverse = 255 - alpha;
        self.rgba[index] =
            ((u32::from(color.r) * alpha + u32::from(self.rgba[index]) * inverse) / 255) as u8;
        self.rgba[index + 1] =
            ((u32::from(color.g) * alpha + u32::from(self.rgba[index + 1]) * inverse) / 255) as u8;
        self.rgba[index + 2] =
            ((u32::from(color.b) * alpha + u32::from(self.rgba[index + 2]) * inverse) / 255) as u8;
        self.rgba[index + 3] = 255;
    }

    fn vertical_gradient(&mut self, top: Color, bottom: Color) {
        self.vertical_gradient_in(
            Rect::new(0.0, 0.0, self.width as f64, self.height as f64),
            top,
            bottom,
        );
    }

    fn vertical_gradient_in(&mut self, rect: Rect, top: Color, bottom: Color) {
        let (x0, y0, x1, y1) = self.bounds(rect);
        let height = (y1 - y0).max(1) as f64;
        for y in y0..y1 {
            let t = f64::from(y - y0) / height;
            let color = lerp_color(top, bottom, t);
            for x in x0..x1 {
                self.blend(x, y, color);
            }
        }
    }

    fn radial_glow(&mut self, cx: f64, cy: f64, radius: f64, color: Color) {
        let rect = Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0);
        let (x0, y0, x1, y1) = self.bounds(rect);
        for y in y0..y1 {
            for x in x0..x1 {
                let distance = ((f64::from(x) - cx).powi(2) + (f64::from(y) - cy).powi(2)).sqrt();
                if distance < radius {
                    let mut glow = color;
                    glow.a = (f64::from(color.a) * (1.0 - distance / radius).powi(2)) as u8;
                    self.blend(x, y, glow);
                }
            }
        }
    }

    fn rect(&mut self, rect: Rect, color: Color) {
        let (x0, y0, x1, y1) = self.bounds(rect);
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, color);
            }
        }
    }

    fn rounded_rect(&mut self, rect: Rect, radius: f64, color: Color) {
        let (x0, y0, x1, y1) = self.bounds(rect);
        for y in y0..y1 {
            for x in x0..x1 {
                if rounded_contains(rect, radius, f64::from(x) + 0.5, f64::from(y) + 0.5) {
                    self.blend(x, y, color);
                }
            }
        }
    }

    fn stroke_rounded_rect(&mut self, rect: Rect, radius: f64, width: f64, color: Color) {
        let inner = rect.inset(width);
        let (x0, y0, x1, y1) = self.bounds(rect);
        for y in y0..y1 {
            for x in x0..x1 {
                let px = f64::from(x) + 0.5;
                let py = f64::from(y) + 0.5;
                if rounded_contains(rect, radius, px, py)
                    && !rounded_contains(inner, (radius - width).max(0.0), px, py)
                {
                    self.blend(x, y, color);
                }
            }
        }
    }

    fn shadow(&mut self, rect: Rect, radius: f64, y_offset: f64, color: Color) {
        let steps = 6;
        for step in (1..=steps).rev() {
            let spread = radius * f64::from(step) / f64::from(steps);
            let mut layer = color;
            layer.a = color.a / (steps as u8 + 1);
            self.rounded_rect(
                Rect::new(
                    rect.x - spread,
                    rect.y - spread + y_offset,
                    rect.width + spread * 2.0,
                    rect.height + spread * 2.0,
                ),
                7.0 + spread,
                layer,
            );
        }
    }

    fn circle(&mut self, cx: f64, cy: f64, radius: f64, color: Color) {
        let (x0, y0, x1, y1) = self.bounds(Rect::new(
            cx - radius,
            cy - radius,
            radius * 2.0,
            radius * 2.0,
        ));
        let radius_squared = radius * radius;
        for y in y0..y1 {
            for x in x0..x1 {
                if (f64::from(x) + 0.5 - cx).powi(2) + (f64::from(y) + 0.5 - cy).powi(2)
                    <= radius_squared
                {
                    self.blend(x, y, color);
                }
            }
        }
    }

    fn stroke_circle(&mut self, cx: f64, cy: f64, radius: f64, width: f64, color: Color) {
        let inner = (radius - width).max(0.0);
        let (x0, y0, x1, y1) = self.bounds(Rect::new(
            cx - radius,
            cy - radius,
            radius * 2.0,
            radius * 2.0,
        ));
        for y in y0..y1 {
            for x in x0..x1 {
                let distance =
                    ((f64::from(x) + 0.5 - cx).powi(2) + (f64::from(y) + 0.5 - cy).powi(2)).sqrt();
                if distance >= inner && distance <= radius {
                    self.blend(x, y, color);
                }
            }
        }
    }

    fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, width: f64, color: Color) {
        let steps = ((x1 - x0).abs().max((y1 - y0).abs()).ceil() as usize).max(1);
        for step in 0..=steps {
            let t = step as f64 / steps as f64;
            self.circle(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, width / 2.0, color);
        }
    }

    #[cfg(target_os = "linux")]
    fn draw_image_contain(&mut self, image: &image::RgbaImage, rect: Rect, radius: f64) {
        let target_width = rect.width.ceil().max(1.0) as u32;
        let target_height = rect.height.ceil().max(1.0) as u32;
        let scale = (target_width as f64 / image.width() as f64)
            .min(target_height as f64 / image.height() as f64);
        let scaled_width = (image.width() as f64 * scale).floor().max(1.0) as u32;
        let scaled_height = (image.height() as f64 * scale).floor().max(1.0) as u32;
        let scaled =
            image::imageops::resize(image, scaled_width, scaled_height, FilterType::Triangle);
        let image_rect = Rect::new(
            rect.x + (rect.width - f64::from(scaled_width)) / 2.0,
            rect.y + (rect.height - f64::from(scaled_height)) / 2.0,
            f64::from(scaled_width),
            f64::from(scaled_height),
        );
        let (x0, y0, x1, y1) = self.bounds(image_rect);
        for y in y0..y1 {
            for x in x0..x1 {
                let px = f64::from(x) + 0.5;
                let py = f64::from(y) + 0.5;
                if !rounded_contains(rect, radius, px, py) {
                    continue;
                }
                let source_x = (f64::from(x) + 0.5 - image_rect.x).floor().max(0.0) as u32;
                let source_y = (f64::from(y) + 0.5 - image_rect.y).floor().max(0.0) as u32;
                if source_x < scaled.width() && source_y < scaled.height() {
                    let pixel = scaled.get_pixel(source_x, source_y);
                    self.blend(x, y, Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3]));
                }
            }
        }
    }

    fn bounds(&self, rect: Rect) -> (i32, i32, i32, i32) {
        (
            rect.x.floor().max(0.0) as i32,
            rect.y.floor().max(0.0) as i32,
            (rect.x + rect.width).ceil().min(self.width as f64) as i32,
            (rect.y + rect.height).ceil().min(self.height as f64) as i32,
        )
    }
}

fn rounded_contains(rect: Rect, radius: f64, x: f64, y: f64) -> bool {
    if x < rect.x || y < rect.y || x >= rect.x + rect.width || y >= rect.y + rect.height {
        return false;
    }
    let radius = radius.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
    // Equivalent half-width bounds can differ by a sub-ULP after arithmetic;
    // max/min preserves the rounded geometry without f64::clamp panicking.
    let nearest_x = x.max(rect.x + radius).min(rect.x + rect.width - radius);
    let nearest_y = y.max(rect.y + radius).min(rect.y + rect.height - radius);
    (x - nearest_x).powi(2) + (y - nearest_y).powi(2) <= radius * radius
}

fn lerp_color(from: Color, to: Color, t: f64) -> Color {
    let channel = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t) as u8;
    Color::rgba(
        channel(from.r, to.r),
        channel(from.g, to.g),
        channel(from.b, to.b),
        channel(from.a, to.a),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: &str) -> PipWorkspaceSummary {
        PipWorkspaceSummary {
            workspace_id: id.to_owned(),
            workspace_label: id.to_owned(),
            target_count: 1,
            updated_ms: 0,
        }
    }

    #[test]
    fn renderer_emits_x11_bgrx_at_the_requested_size() {
        let rendered = render_agent_view(360, 260, &[], &[], None, None);
        assert_eq!(rendered.len(), 360 * 260 * 4);
        assert!(rendered.chunks_exact(4).all(|pixel| pixel[3] == 0));
    }

    #[test]
    fn session_selector_is_hidden_for_zero_or_one_workspace() {
        assert!(session_tabs_for_bounds(720, 520, &[], None).tabs.is_empty());
        assert!(
            session_tabs_for_bounds(720, 520, &[workspace("one")], Some("one"))
                .tabs
                .is_empty()
        );
        assert_eq!(
            session_tabs_for_bounds(720, 520, &[workspace("one"), workspace("two")], Some("one"),)
                .tabs
                .len(),
            2
        );
    }

    #[test]
    fn session_selector_hit_testing_only_accepts_session_tabs() {
        let workspaces = [workspace("one"), workspace("two"), workspace("three")];
        let tabs = session_tabs_for_bounds(720, 520, &workspaces, Some("one"));
        let first = tabs.tabs[0].rect;
        assert_eq!(
            tabs.hit_test(first.x + first.width / 2.0, first.y + first.height / 2.0),
            Some("one")
        );
        assert_eq!(
            tabs.hit_test(first.x - 2.0, first.y + first.height / 2.0),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a live pure-Wayland compositor with layer-shell"]
    fn native_wayland_agent_view_starts_and_stops() {
        assert!(std::env::var_os("WAYLAND_DISPLAY").is_some());
        assert!(std::env::var_os("DISPLAY").is_none());
        let backend = LinuxPipBackendFactory
            .start(&PipConfig::default())
            .expect("native Wayland Agent View should start");
        backend.shutdown();
    }

    #[test]
    fn target_kind_uses_distinct_gnome_dash_accents() {
        assert_ne!(
            target_accent(PipTargetKind::NativeWindow),
            target_accent(PipTargetKind::BrowserTab)
        );
    }

    #[test]
    fn rounded_rect_clips_its_corner() {
        let rect = Rect::new(10.0, 10.0, 40.0, 30.0);
        assert!(!rounded_contains(rect, 8.0, 10.0, 10.0));
        assert!(rounded_contains(rect, 8.0, 18.0, 10.5));
        assert!(rounded_contains(rect, 8.0, 30.0, 25.0));
    }

    #[test]
    fn default_position_clamps_to_x11_coordinates() {
        assert_eq!(clamp_i16(i32::MAX), i16::MAX);
        assert_eq!(clamp_i16(i32::MIN), i16::MIN);
        assert_eq!(clamp_i16(24), 24);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a live X11 server (run under xvfb-run)"]
    fn x11_input_transparency_toggle_changes_input_shape() -> anyhow::Result<()> {
        use x11rb::connection::Connection;
        use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK};
        use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass};

        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let window = conn.generate_id()?;
        let width = 320;
        let height = 240;
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new(),
        )?
        .check()?;
        conn.map_window(window)?.check()?;
        conn.flush()?;

        set_x11_input_transparent(&conn, window, width, height, true)?;
        let transparent = conn.shape_get_rectangles(window, SK::INPUT)?.reply()?;
        assert!(transparent.rectangles.is_empty());

        set_x11_input_transparent(&conn, window, width, height, false)?;
        let interactive = conn.shape_get_rectangles(window, SK::INPUT)?.reply()?;
        assert_eq!(interactive.rectangles.len(), 1);
        assert_eq!(interactive.rectangles[0].width, width);
        assert_eq!(interactive.rectangles[0].height, height);

        conn.destroy_window(window)?.check()?;
        conn.flush()?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires an interactive X11 display"]
    fn x11_visual_smoke_presents_multiple_targets() {
        use std::io::Cursor;

        use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
        use pip_preview::{PipGeometry, PipTarget};

        fn png(width: u32, height: u32, base: [u8; 3]) -> Vec<u8> {
            let mut image = RgbaImage::new(width, height);
            for (x, y, pixel) in image.enumerate_pixels_mut() {
                let shade = ((x * 31 / width.max(1) + y * 23 / height.max(1)) % 44) as u8;
                *pixel = Rgba([
                    base[0].saturating_add(shade),
                    base[1].saturating_add(shade),
                    base[2].saturating_add(shade),
                    255,
                ]);
            }
            let mut bytes = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(image)
                .write_to(&mut bytes, ImageFormat::Png)
                .unwrap();
            bytes.into_inner()
        }

        let config = PipConfig {
            enabled: true,
            geometry: PipGeometry {
                width: 720,
                height: 520,
                x: Some(1080),
                y: Some(80),
            },
            title: "Cua Agent View Linux visual smoke".to_owned(),
        };
        let backend = LinuxPipBackendFactory.start(&config).unwrap();
        for (index, (workspace_id, workspace_label, width, height, base, kind)) in [
            (
                "linux-browser",
                "Browser session",
                1280,
                760,
                [20, 72, 92],
                PipTargetKind::BrowserTab,
            ),
            (
                "linux-browser",
                "Browser session",
                760,
                760,
                [25, 91, 67],
                PipTargetKind::BrowserTab,
            ),
            (
                "linux-native",
                "Native session",
                840,
                1060,
                [47, 58, 65],
                PipTargetKind::NativeWindow,
            ),
            (
                "linux-native",
                "Native session",
                1440,
                800,
                [91, 59, 34],
                PipTargetKind::NativeWindow,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            backend.push_frame(PipFrame {
                target: PipTarget {
                    workspace_id: workspace_id.to_owned(),
                    workspace_label: workspace_label.to_owned(),
                    target_id: format!("target-{index}"),
                    identity_key: format!("target-{index}"),
                    target_kind: kind,
                    target_label: format!("Target {index}"),
                    native_container: None,
                },
                png_bytes: png(width, height, base),
                action_label: "observe".to_owned(),
                timestamp_ms: index as u64,
                cursor_position: None,
            });
        }
        let seconds = std::env::var("CUA_AGENT_VIEW_SMOKE_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3);
        std::thread::sleep(Duration::from_secs(seconds));
        backend.shutdown();
    }
}
