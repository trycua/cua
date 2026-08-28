//! Windows multi-target Agent View.
//!
//! The preview owns a Win32 message-loop thread and never activates, so it can
//! remain visible without stealing focus from the application being automated.

#[cfg(not(target_os = "windows"))]
use pip_preview::{PipBackend, PipBackendFactory, PipConfig};

#[cfg(not(target_os = "windows"))]
impl PipBackendFactory for WindowsPipBackendFactory {
    fn start(&self, _cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
        Err(anyhow::anyhow!("Windows Agent View requires Windows"))
    }
}

pub struct WindowsPipBackendFactory;

#[cfg(target_os = "windows")]
mod native {
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    use cursor_overlay::{rasterize_inter_text, TextRaster};
    use image::imageops::FilterType;
    use pip_preview::{
        layout_desktop_with_shell, layout_session_tabs, png_dimensions, LayoutRect, PipBackend,
        PipBackendFactory, PipConfig, PipFrame, PipTargetKind, PipViewModel, PipWorkspaceSummary,
        ShellStyle, TargetSize,
    };
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE,
    };
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, EndPaint, InvalidateRect, ScreenToClient, StretchDIBits, UpdateWindow,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, PAINTSTRUCT, SRCCOPY,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, LoadCursorW, PostMessageW,
        PostQuitMessage, RegisterClassExW, SendMessageW, SetWindowLongPtrW, ShowWindow,
        TranslateMessage, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, GWL_EXSTYLE,
        HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP,
        HTTOPLEFT, HTTOPRIGHT, HTTRANSPARENT, IDC_ARROW, MA_NOACTIVATE, MSG, SM_CXSCREEN,
        SM_CYSCREEN, SW_SHOWNOACTIVATE, WM_APP, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONUP,
        WM_MOUSEACTIVATE, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_SIZE, WNDCLASSEXW,
        WS_CLIPCHILDREN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
        WS_POPUP, WS_THICKFRAME,
    };

    use super::WindowsPipBackendFactory;

    const REDRAW: u32 = WM_APP + 1;
    const SHUTDOWN: u32 = WM_APP + 2;
    const SET_INPUT_PASSTHROUGH: u32 = WM_APP + 3;

    struct State {
        hwnd: AtomicIsize,
        input_passthrough: AtomicBool,
        model: Mutex<PipViewModel>,
    }

    impl State {
        fn post(&self, message: u32) {
            let hwnd = self.hwnd.load(Ordering::Acquire);
            if hwnd != 0 {
                let _ =
                    unsafe { PostMessageW(HWND(hwnd as *mut _), message, WPARAM(0), LPARAM(0)) };
            }
        }

        fn set_input_passthrough(&self, passthrough: bool) -> anyhow::Result<()> {
            let hwnd = self.hwnd.load(Ordering::Acquire);
            if hwnd == 0 {
                return Ok(());
            }
            let applied = unsafe {
                SendMessageW(
                    HWND(hwnd as *mut _),
                    SET_INPUT_PASSTHROUGH,
                    WPARAM(usize::from(passthrough)),
                    LPARAM(0),
                )
            };
            anyhow::ensure!(
                applied.0 == 1,
                "Agent View rejected the synchronous input-passthrough update"
            );
            Ok(())
        }
    }

    struct WindowsPipBackend {
        state: Arc<State>,
    }

    impl PipBackend for WindowsPipBackend {
        fn push_frame(&self, frame: PipFrame) {
            self.state.model.lock().unwrap().upsert(frame);
            self.state.post(REDRAW);
        }

        fn remove_workspace(&self, workspace_id: &str) {
            self.state
                .model
                .lock()
                .unwrap()
                .remove_workspace(workspace_id);
            self.state.post(REDRAW);
        }

        fn remove_target(&self, workspace_id: &str, identity_key: &str) {
            self.state
                .model
                .lock()
                .unwrap()
                .remove_target(workspace_id, identity_key);
            self.state.post(REDRAW);
        }

        fn set_input_passthrough(&self, passthrough: bool) -> anyhow::Result<()> {
            self.state.set_input_passthrough(passthrough)
        }

        fn shutdown(self: Box<Self>) {
            self.state.post(SHUTDOWN);
        }
    }

    impl PipBackendFactory for WindowsPipBackendFactory {
        fn start(&self, cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
            let state = Arc::new(State {
                hwnd: AtomicIsize::new(0),
                input_passthrough: AtomicBool::new(false),
                model: Mutex::new(PipViewModel::new(12)),
            });
            let thread_state = Arc::clone(&state);
            let cfg = cfg.clone();
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            std::thread::Builder::new()
                .name("cua-agent-view-windows".to_owned())
                .spawn(move || run_window(cfg, thread_state, ready_tx))?;
            ready_rx
                .recv()
                .map_err(|_| anyhow::anyhow!("Agent View UI thread exited during startup"))??;
            Ok(Box::new(WindowsPipBackend { state }))
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn run_window(cfg: PipConfig, state: Arc<State>, ready: mpsc::SyncSender<anyhow::Result<()>>) {
        let hwnd = match unsafe { create_window(&cfg, &state) } {
            Ok(hwnd) => hwnd,
            Err(error) => {
                let _ = ready.send(Err(error));
                return;
            }
        };
        state.hwnd.store(hwnd.0 as isize, Ordering::Release);
        let _ = ready.send(Ok(()));
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = UpdateWindow(hwnd);
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        state.hwnd.store(0, Ordering::Release);
    }

    unsafe fn create_window(cfg: &PipConfig, state: &Arc<State>) -> anyhow::Result<HWND> {
        let instance = GetModuleHandleW(None)?;
        let class_name = wide("CuaAgentViewWindow");
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            let error = windows::core::Error::from_win32();
            if error.code().0 != 0x8007_0582u32 as i32 {
                return Err(error.into());
            }
        }

        let width = cfg.geometry.width.max(320) as i32;
        let height = cfg.geometry.height.max(240) as i32;
        let x = cfg
            .geometry
            .x
            .unwrap_or_else(|| (GetSystemMetrics(SM_CXSCREEN) - width - 24).max(0));
        let y = cfg
            .geometry
            .y
            .unwrap_or(24)
            .min((GetSystemMetrics(SM_CYSCREEN) - height).max(0));
        let title = wide(&cfg.title);
        let state_ptr = Box::into_raw(Box::new(Arc::clone(state)));
        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
            x,
            y,
            width,
            height,
            None,
            None,
            instance,
            Some(state_ptr.cast()),
        ) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                drop(Box::from_raw(state_ptr));
                return Err(error.into());
            }
        };
        let corner = 2i32; // DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner as *const i32).cast(),
            std::mem::size_of_val(&corner) as u32,
        );
        let backdrop = 3i32; // DWMSBT_TRANSIENTWINDOW
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&backdrop as *const i32).cast(),
            std::mem::size_of_val(&backdrop) as u32,
        );
        Ok(hwnd)
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            return LRESULT(1);
        }
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Arc<State>;
        match message {
            REDRAW | WM_SIZE => {
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_PAINT => {
                if !state_ptr.is_null() {
                    paint(hwnd, &*state_ptr);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            SET_INPUT_PASSTHROUGH => {
                let passthrough = wparam.0 != 0;
                if !state_ptr.is_null() {
                    (&*state_ptr)
                        .input_passthrough
                        .store(passthrough, Ordering::Release);
                }
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    extended_style_with_input_passthrough(style, passthrough),
                );
                LRESULT(1)
            }
            WM_LBUTTONUP => {
                if !state_ptr.is_null() {
                    let x = lparam.0 as i16 as i32;
                    let y = (lparam.0 >> 16) as i16 as i32;
                    let mut client = RECT::default();
                    if GetClientRect(hwnd, &mut client).is_ok() {
                        let state = &*state_ptr;
                        let mut model = state.model.lock().unwrap();
                        let workspaces = model.workspaces();
                        let tabs = session_tabs_for_width(
                            client.right - client.left,
                            client.bottom - client.top,
                            &workspaces,
                            model.selected_workspace_id(),
                        );
                        let changed = tabs
                            .hit_test(f64::from(x), f64::from(y))
                            .map(|workspace_id| model.select_workspace(workspace_id))
                            .unwrap_or(false);
                        if changed {
                            let _ = InvalidateRect(hwnd, None, false);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_NCHITTEST => {
                if !state_ptr.is_null() && (&*state_ptr).input_passthrough.load(Ordering::Acquire) {
                    return LRESULT(HTTRANSPARENT as isize);
                }
                let hit = DefWindowProcW(hwnd, message, wparam, lparam);
                if hit.0 == HTCLIENT as isize {
                    let screen_x = lparam.0 as i16 as i32;
                    let screen_y = (lparam.0 >> 16) as i16 as i32;
                    let mut window = RECT::default();
                    if GetWindowRect(hwnd, &mut window).is_ok() {
                        let border = 8;
                        let left = screen_x - window.left < border;
                        let right = window.right - screen_x <= border;
                        let top = screen_y - window.top < border;
                        let bottom = window.bottom - screen_y <= border;
                        let resize_hit = match (left, right, top, bottom) {
                            (true, _, true, _) => Some(HTTOPLEFT),
                            (_, true, true, _) => Some(HTTOPRIGHT),
                            (true, _, _, true) => Some(HTBOTTOMLEFT),
                            (_, true, _, true) => Some(HTBOTTOMRIGHT),
                            (true, _, _, _) => Some(HTLEFT),
                            (_, true, _, _) => Some(HTRIGHT),
                            (_, _, true, _) => Some(HTTOP),
                            (_, _, _, true) => Some(HTBOTTOM),
                            _ => None,
                        };
                        if let Some(resize_hit) = resize_hit {
                            return LRESULT(resize_hit as isize);
                        }
                        let mut client_point = POINT {
                            x: screen_x,
                            y: screen_y,
                        };
                        let mut client = RECT::default();
                        let tabs_hit = if !state_ptr.is_null()
                            && ScreenToClient(hwnd, &mut client_point).as_bool()
                            && GetClientRect(hwnd, &mut client).is_ok()
                        {
                            let model = (&*state_ptr).model.lock().unwrap();
                            let workspaces = model.workspaces();
                            session_tabs_for_width(
                                client.right - client.left,
                                client.bottom - client.top,
                                &workspaces,
                                model.selected_workspace_id(),
                            )
                            .hit_test(f64::from(client_point.x), f64::from(client_point.y))
                            .is_some()
                        } else {
                            false
                        };
                        if tabs_hit {
                            return LRESULT(HTCLIENT as isize);
                        }
                        if screen_y - window.top < 28 {
                            return LRESULT(HTCAPTION as isize);
                        }
                    }
                }
                hit
            }
            SHUTDOWN | WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if !state_ptr.is_null() {
                    drop(Box::from_raw(state_ptr));
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    unsafe fn paint(hwnd: HWND, state: &Arc<State>) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rect = RECT::default();
        if GetClientRect(hwnd, &mut rect).is_err() {
            let _ = EndPaint(hwnd, &ps);
            return;
        }
        let width = (rect.right - rect.left).max(1) as u32;
        let height = (rect.bottom - rect.top).max(1) as u32;
        let (frames, workspaces, selected_workspace_id, active_view_id) = {
            let model = state.model.lock().unwrap();
            (
                model
                    .selected_frames()
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>(),
                model.workspaces(),
                model.selected_workspace_id().map(str::to_owned),
                model.active_view_id().map(str::to_owned),
            )
        };
        let pixels = render_view(
            width,
            height,
            &frames,
            &workspaces,
            selected_workspace_id.as_deref(),
            active_view_id.as_deref(),
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: pixels.len() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        StretchDIBits(
            hdc,
            0,
            0,
            width as i32,
            height as i32,
            0,
            0,
            width as i32,
            height as i32,
            Some(pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        let _ = EndPaint(hwnd, &ps);
    }

    fn render_view(
        width: u32,
        height: u32,
        frames: &[PipFrame],
        workspaces: &[PipWorkspaceSummary],
        selected_workspace_id: Option<&str>,
        active_view_id: Option<&str>,
    ) -> Vec<u8> {
        let mut canvas = Canvas::new(width, height);
        canvas.smoked_shell();
        let show_switcher = workspaces.len() > 1;
        let desktop = desktop_rect(width, height, show_switcher);
        canvas.wallpaper(desktop);
        let sizes = frames
            .iter()
            .map(|frame| {
                png_dimensions(&frame.png_bytes).unwrap_or(TargetSize {
                    width: 16,
                    height: 10,
                })
            })
            .collect::<Vec<_>>();
        let layout =
            layout_desktop_with_shell(desktop.2 as f64, desktop.3 as f64, &sizes, ShellStyle::None);
        canvas.border((0, 0, width as i32, height as i32), 14, [19, 12, 7, 225]);
        canvas.border(
            (1, 1, width as i32 - 2, height as i32 - 2),
            13,
            [211, 184, 151, 118],
        );
        canvas.border(
            (2, 2, width as i32 - 4, height as i32 - 4),
            12,
            [108, 82, 57, 105],
        );
        canvas.border(desktop, 10, [181, 151, 122, 138]);
        if show_switcher {
            canvas.switcher(
                switcher_rect(width as i32),
                workspaces,
                selected_workspace_id,
            );
        }
        for (frame, target) in frames.iter().zip(&layout.targets) {
            let rect = offset_pixels(target.content, desktop.0, desktop.1);
            canvas.shadow(rect);
            canvas.fill(rect, 7, [244, 247, 251, 255]);
            if let Ok(image) = image::load_from_memory(&frame.png_bytes) {
                let image = image
                    .resize_exact(rect.2 as u32, rect.3 as u32, FilterType::Lanczos3)
                    .to_rgba8();
                canvas.blit(&image, rect, 7);
            }
            canvas.border(rect, 7, [40, 52, 72, 76]);
            if active_view_id == Some(frame.target.view_id().as_str()) {
                canvas.border(
                    (rect.0 - 3, rect.1 - 3, rect.2 + 6, rect.3 + 6),
                    9,
                    [230, 158, 73, 232],
                );
            }
        }
        canvas.data
    }

    /// Index of the card whose capture is newest, which the taskbar marks as
    /// the foreground app.
    ///
    /// Ties resolve to the earliest card so one repaint never reorders the
    /// highlight for an unchanged set of frames.
    fn active_target_index(frames: &[PipFrame]) -> Option<usize> {
        frames
            .iter()
            .enumerate()
            .max_by_key(|(index, frame)| (frame.timestamp_ms, std::cmp::Reverse(*index)))
            .map(|(index, _)| index)
    }

    /// `HH:MM` for one epoch-millisecond capture stamp, in UTC.
    ///
    /// The clock reads the newest card's capture time rather than the wall
    /// clock so a rendered frame stays reproducible from its inputs alone.
    fn clock_label(timestamp_ms: u64) -> String {
        let minutes = timestamp_ms / 60_000;
        format!("{:02}:{:02}", minutes / 60 % 24, minutes % 60)
    }

    /// Separable box blur over a tightly packed 3-channel buffer.
    fn box_blur(source: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
        let mut pass = source.to_vec();
        let mut out = vec![0u8; source.len()];
        for vertical in [false, true] {
            let (major, minor) = if vertical {
                (width, height)
            } else {
                (height, width)
            };
            for outer in 0..major {
                for inner in 0..minor {
                    let low = inner.saturating_sub(radius);
                    let high = (inner + radius).min(minor - 1);
                    let taps = (high - low + 1) as u32;
                    for channel in 0..3 {
                        let mut sum = 0u32;
                        for tap in low..=high {
                            let at = if vertical {
                                tap * width + outer
                            } else {
                                outer * width + tap
                            };
                            sum += pass[at * 3 + channel] as u32;
                        }
                        let at = if vertical {
                            inner * width + outer
                        } else {
                            outer * width + inner
                        };
                        out[at * 3 + channel] = (sum / taps) as u8;
                    }
                }
            }
            std::mem::swap(&mut pass, &mut out);
        }
        pass
    }

    fn desktop_rect(width: u32, height: u32, show_switcher: bool) -> (i32, i32, i32, i32) {
        const SIDE_INSET: i32 = 8;
        const BOTTOM_INSET: i32 = 9;
        let top_inset = if show_switcher { 36 } else { 14 };
        (
            SIDE_INSET,
            top_inset,
            (width as i32 - SIDE_INSET * 2).max(1),
            (height as i32 - top_inset - BOTTOM_INSET).max(1),
        )
    }

    fn switcher_rect(width: i32) -> (i32, i32, i32, i32) {
        (12, 8, (width - 24).max(1), 28)
    }

    fn session_tabs_for_width(
        width: i32,
        height: i32,
        workspaces: &[PipWorkspaceSummary],
        selected_workspace_id: Option<&str>,
    ) -> pip_preview::SessionTabsLayout {
        layout_session_tabs(
            LayoutRect {
                x: 12.0,
                y: 8.0,
                width: f64::from((width - 24).max(1)),
                height: f64::from(height.max(1)),
            },
            workspaces,
            selected_workspace_id,
        )
    }

    fn contains(rect: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
        x >= rect.0 && y >= rect.1 && x < rect.0 + rect.2 && y < rect.1 + rect.3
    }

    fn extended_style_with_input_passthrough(style: isize, passthrough: bool) -> isize {
        let transparent = WS_EX_TRANSPARENT.0 as isize;
        if passthrough {
            style | transparent
        } else {
            style & !transparent
        }
    }

    fn pixels(rect: LayoutRect) -> (i32, i32, i32, i32) {
        (
            rect.x.round() as i32,
            rect.y.round() as i32,
            rect.width.round().max(1.0) as i32,
            rect.height.round().max(1.0) as i32,
        )
    }

    fn offset_pixels(rect: LayoutRect, x: i32, y: i32) -> (i32, i32, i32, i32) {
        let rect = pixels(rect);
        (rect.0 + x, rect.1 + y, rect.2, rect.3)
    }

    struct Canvas {
        width: u32,
        height: u32,
        data: Vec<u8>,
    }

    impl Canvas {
        fn new(width: u32, height: u32) -> Self {
            Self {
                width,
                height,
                data: vec![0; width as usize * height as usize * 4],
            }
        }

        fn wallpaper(&mut self, rect: (i32, i32, i32, i32)) {
            let w = rect.2.max(1) as f32;
            let h = rect.3.max(1) as f32;
            for y in 0..rect.3 {
                for x in 0..rect.2 {
                    if !Self::inside(x, y, rect.2, rect.3, 10) {
                        continue;
                    }
                    let nx = x as f32 / w;
                    let ny = y as f32 / h;
                    let upper_glow =
                        (1.0 - (((nx - 0.72).powi(2) + (ny - 0.08).powi(2)).sqrt() * 1.5)).max(0.0);
                    let lower_glow =
                        (1.0 - (((nx - 0.18).powi(2) + (ny - 0.88).powi(2)).sqrt() * 1.8)).max(0.0);
                    let ribbon =
                        (1.0 - ((ny - 0.64 + 0.08 * (nx * 5.4).sin()).abs() * 6.5)).max(0.0);
                    self.set(
                        rect.0 + x,
                        rect.1 + y,
                        [
                            (35.0 + 45.0 * upper_glow + 25.0 * lower_glow + 19.0 * ribbon) as u8,
                            (25.0 + 36.0 * upper_glow + 17.0 * lower_glow + 11.0 * ribbon) as u8,
                            (18.0 + 22.0 * upper_glow + 10.0 * lower_glow + 5.0 * ribbon) as u8,
                            255,
                        ],
                    );
                }
            }
        }

        fn smoked_shell(&mut self) {
            let width = self.width as i32;
            let height = self.height as i32;
            self.fill((0, 0, width, height), 0, [15, 10, 7, 255]);
            self.fill((0, 0, width, height), 14, [24, 16, 10, 255]);

            let header_height = (height / 12).clamp(24, 42);
            self.fill(
                (2, 2, width.saturating_sub(4), header_height),
                12,
                [66, 50, 36, 92],
            );
            self.fill((16, 3, width.saturating_sub(32), 1), 0, [235, 211, 181, 92]);

            // Fine, low-contrast grain keeps the painted fallback from looking
            // flat when the system backdrop is unavailable (for example RDP).
            for y in (8..height.saturating_sub(8)).step_by(4) {
                for x in ((y & 7)..width.saturating_sub(8)).step_by(8) {
                    self.blend(x, y, [205, 181, 155, 7]);
                }
            }
        }

        fn switcher(
            &mut self,
            rect: (i32, i32, i32, i32),
            workspaces: &[PipWorkspaceSummary],
            selected_workspace_id: Option<&str>,
        ) {
            let tabs = layout_session_tabs(
                LayoutRect {
                    x: f64::from(rect.0),
                    y: f64::from(rect.1),
                    width: f64::from(rect.2),
                    height: f64::from(rect.3),
                },
                workspaces,
                selected_workspace_id,
            );
            for tab in tabs.tabs {
                let tab_rect = offset_pixels(tab.rect, 0, 0);
                self.shadow(tab_rect);
                self.fill(
                    tab_rect,
                    9,
                    [42, 31, 24, if tab.selected { 242 } else { 204 }],
                );
                self.border(
                    tab_rect,
                    9,
                    [
                        tab.accent.0,
                        tab.accent.1,
                        tab.accent.2,
                        if tab.selected { 235 } else { 128 },
                    ],
                );
            }
        }

        fn set(&mut self, x: i32, y: i32, color: [u8; 4]) {
            if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                return;
            }
            let at = (y as usize * self.width as usize + x as usize) * 4;
            self.data[at..at + 4].copy_from_slice(&color);
        }

        fn blend(&mut self, x: i32, y: i32, color: [u8; 4]) {
            if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                return;
            }
            let at = (y as usize * self.width as usize + x as usize) * 4;
            let alpha = color[3] as u16;
            for channel in 0..3 {
                self.data[at + channel] = ((color[channel] as u16 * alpha
                    + self.data[at + channel] as u16 * (255 - alpha))
                    / 255) as u8;
            }
            self.data[at + 3] = 255;
        }

        fn inside(x: i32, y: i32, width: i32, height: i32, radius: i32) -> bool {
            let dx = if x < radius {
                radius - x
            } else if x >= width - radius {
                x - width + radius + 1
            } else {
                0
            };
            let dy = if y < radius {
                radius - y
            } else if y >= height - radius {
                y - height + radius + 1
            } else {
                0
            };
            dx == 0 || dy == 0 || dx * dx + dy * dy <= radius * radius
        }

        fn fill(&mut self, rect: (i32, i32, i32, i32), radius: i32, color: [u8; 4]) {
            for y in 0..rect.3 {
                for x in 0..rect.2 {
                    if Self::inside(x, y, rect.2, rect.3, radius) {
                        self.blend(rect.0 + x, rect.1 + y, color);
                    }
                }
            }
        }

        fn border(&mut self, rect: (i32, i32, i32, i32), radius: i32, color: [u8; 4]) {
            for y in 0..rect.3 {
                for x in 0..rect.2 {
                    let outer = Self::inside(x, y, rect.2, rect.3, radius);
                    let inner = x > 0
                        && y > 0
                        && x < rect.2 - 1
                        && y < rect.3 - 1
                        && Self::inside(x - 1, y - 1, rect.2 - 2, rect.3 - 2, radius - 1);
                    if outer && !inner {
                        self.blend(rect.0 + x, rect.1 + y, color);
                    }
                }
            }
        }

        fn shadow(&mut self, rect: (i32, i32, i32, i32)) {
            for spread in (1..=8).rev() {
                self.border(
                    (
                        rect.0 - spread,
                        rect.1 - spread + 2,
                        rect.2 + spread * 2,
                        rect.3 + spread * 2,
                    ),
                    7 + spread,
                    [8, 24, 49, (26 / spread) as u8],
                );
            }
        }

        fn blit(&mut self, image: &image::RgbaImage, rect: (i32, i32, i32, i32), radius: i32) {
            for y in 0..rect.3 {
                for x in 0..rect.2 {
                    if Self::inside(x, y, rect.2, rect.3, radius) {
                        let p = image.get_pixel(x as u32, y as u32).0;
                        self.blend(rect.0 + x, rect.1 + y, [p[2], p[1], p[0], p[3]]);
                    }
                }
            }
        }

        fn icon(&mut self, rect: (i32, i32, i32, i32), kind: PipTargetKind, index: usize) {
            let colors = match kind {
                PipTargetKind::BrowserTab => ([31, 180, 210, 255], [205, 91, 28, 255]),
                PipTargetKind::NativeWindow => [
                    ([205, 91, 138, 255], [131, 53, 92, 255]),
                    ([78, 166, 66, 255], [35, 104, 26, 255]),
                    ([43, 145, 235, 255], [21, 84, 180, 255]),
                ][index % 3],
            };
            self.fill(rect, (rect.2 / 4).max(3), colors.0);
            let inset = (rect.2 / 4).max(3);
            self.fill(
                (
                    rect.0 + inset,
                    rect.1 + inset,
                    rect.2 - inset * 2,
                    rect.3 - inset * 2,
                ),
                3,
                colors.1,
            );
        }

        /// Acrylic-like backdrop for the edge-anchored taskbar band.
        ///
        /// The band is a material, not a card: it blurs whatever Agent View
        /// already composited beneath it, tints the result, and adds fine
        /// noise, which is why it runs after the wallpaper and the target
        /// cards. Painting is clipped to the miniature desktop so the bar
        /// keeps the desktop's rounded bottom corners.
        fn taskbar_material(&mut self, rect: (i32, i32, i32, i32), clip: (i32, i32, i32, i32)) {
            const BLUR_RADIUS: usize = 5;
            /// Tint strength out of 255. Leaving some backdrop through is what
            /// separates acrylic from a flat fill.
            const TINT_ALPHA: u32 = 196;
            const TINT: [u32; 3] = [44, 34, 27];

            let width = rect.2.max(0) as usize;
            let height = rect.3.max(0) as usize;
            if width == 0 || height == 0 {
                return;
            }
            // Sample with clamped coordinates so the blur never smears the
            // surrounding shell frame into the band.
            let mut backdrop = vec![0u8; width * height * 3];
            for y in 0..height {
                for x in 0..width {
                    let sx = (rect.0 + x as i32).clamp(0, self.width as i32 - 1);
                    let sy = (rect.1 + y as i32).clamp(0, self.height as i32 - 1);
                    let from = (sy as usize * self.width as usize + sx as usize) * 4;
                    let to = (y * width + x) * 3;
                    backdrop[to..to + 3].copy_from_slice(&self.data[from..from + 3]);
                }
            }
            let blurred = box_blur(&backdrop, width, height, BLUR_RADIUS);

            for y in 0..height {
                for x in 0..width {
                    let px = rect.0 + x as i32;
                    let py = rect.1 + y as i32;
                    if !Self::inside(px - clip.0, py - clip.1, clip.2, clip.3, 10) {
                        continue;
                    }
                    let sample = (y * width + x) * 3;
                    let mut color = [0u8; 4];
                    for channel in 0..3 {
                        color[channel] = ((blurred[sample + channel] as u32 * (255 - TINT_ALPHA)
                            + TINT[channel] * TINT_ALPHA)
                            / 255) as u8;
                    }
                    color[3] = 255;
                    self.set(px, py, color);
                    // Deterministic grain, the visual signature of acrylic.
                    if (x ^ y) & 3 == 0 {
                        self.blend(px, py, [188, 170, 150, 6]);
                    }
                }
            }

            // A single bright hairline along the top edge reads as the lit
            // lip of the bar and keeps it separated from the wallpaper.
            for x in 0..rect.2 {
                let px = rect.0 + x;
                if Self::inside(px - clip.0, rect.1 - clip.1, clip.2, clip.3, 10) {
                    self.blend(px, rect.1, [214, 190, 162, 96]);
                }
            }
        }

        /// Four-pane launcher glyph anchored at the head of the icon cluster.
        fn start_glyph(&mut self, rect: (i32, i32, i32, i32)) {
            let gap = (rect.2 / 9).max(1);
            let pane_width = ((rect.2 - gap) / 2).max(1);
            let pane_height = ((rect.3 - gap) / 2).max(1);
            // Center the glyph inside its slot, which is sized like an app icon.
            let x = rect.0 + (rect.2 - (pane_width * 2 + gap)) / 2;
            let y = rect.1 + (rect.3 - (pane_height * 2 + gap)) / 2;
            for row in 0..2 {
                for column in 0..2 {
                    self.fill(
                        (
                            x + column * (pane_width + gap),
                            y + row * (pane_height + gap),
                            pane_width,
                            pane_height,
                        ),
                        1,
                        [230, 158, 73, 235],
                    );
                }
            }
        }

        /// Running-app mark beneath one taskbar icon.
        ///
        /// The foreground card gets the full accent pill; the rest get a
        /// shorter, dimmer mark, matching how a Windows taskbar distinguishes
        /// the active window from other running ones.
        fn running_indicator(&mut self, rect: (i32, i32, i32, i32), active: bool) {
            let (width, color) = if active {
                (rect.2, [230, 158, 73, 255])
            } else {
                ((rect.2 / 2).max(2), [176, 158, 138, 190])
            };
            self.fill(
                (rect.0 + (rect.2 - width) / 2, rect.1, width, rect.3),
                rect.3 / 2,
                color,
            );
        }

        /// Trailing status band: a chevron, a battery mark, and the clock.
        ///
        /// Every element is dropped rather than crowded when the band is too
        /// narrow, so a small Agent View degrades instead of overlapping the
        /// centered icon cluster.
        fn tray(&mut self, rect: (i32, i32, i32, i32), timestamp_ms: Option<u64>) {
            const INK: [u8; 4] = [224, 210, 190, 235];
            let padding = 8;
            let mut right = rect.0 + rect.2 - padding;

            let font_size = (rect.3 as f32 * 0.2).clamp(8.0, 13.0);
            let clock = timestamp_ms
                .map(clock_label)
                .and_then(|label| rasterize_inter_text(&label, font_size));
            if let Some(raster) = clock {
                if raster.width as i32 + padding * 2 <= rect.2 {
                    right -= raster.width as i32;
                    let y = rect.1 + (rect.3 - raster.height as i32) / 2;
                    self.text(&raster, right, y, INK);
                }
            }

            let center_y = rect.1 + rect.3 / 2;
            // Battery: an outline pill with a nub, drawn only when it fits
            // clear of the clock.
            let battery_width = 14;
            if right - (battery_width + 10) >= rect.0 + padding {
                right -= battery_width + 10;
                self.border((right, center_y - 4, battery_width, 8), 2, INK);
                self.fill((right + battery_width, center_y - 2, 2, 4), 1, INK);
            }
            // Chevron: the "show hidden icons" affordance.
            if right - 12 >= rect.0 + padding {
                right -= 12;
                for step in 0..4 {
                    self.blend(right + step, center_y + 1 - step, INK);
                    self.blend(right + 6 - step, center_y + 1 - step, INK);
                }
            }
        }

        /// Composite one rasterized text mask in the buffer's channel order.
        fn text(&mut self, raster: &TextRaster, x: i32, y: i32, color: [u8; 4]) {
            for row in 0..raster.height {
                for column in 0..raster.width {
                    let coverage = raster.coverage[(row * raster.width + column) as usize];
                    if coverage == 0 {
                        continue;
                    }
                    let alpha = (color[3] as u16 * coverage as u16 / 255) as u8;
                    self.blend(
                        x + column as i32,
                        y + row as i32,
                        [color[0], color[1], color[2], alpha],
                    );
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::io::Cursor;

        use super::*;
        use image::{ImageFormat, Rgba, RgbaImage};
        use pip_preview::PipTarget;

        fn solid_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
            let image = RgbaImage::from_pixel(width, height, Rgba(color));
            let mut bytes = Cursor::new(Vec::new());
            image.write_to(&mut bytes, ImageFormat::Png).unwrap();
            bytes.into_inner()
        }

        fn pixel(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
            let offset = ((y * width + x) * 4) as usize;
            data[offset..offset + 4].try_into().unwrap()
        }

        fn frame(
            workspace: &str,
            id: &str,
            kind: PipTargetKind,
            png_bytes: Vec<u8>,
            timestamp_ms: u64,
        ) -> PipFrame {
            PipFrame {
                target: PipTarget {
                    workspace_id: workspace.to_owned(),
                    workspace_label: workspace.to_owned(),
                    target_id: id.to_owned(),
                    identity_key: id.to_owned(),
                    target_kind: kind,
                    target_label: id.to_owned(),
                    native_container: None,
                },
                png_bytes,
                action_label: "click".to_owned(),
                timestamp_ms,
                cursor_position: None,
            }
        }

        fn workspace(id: &str, target_count: usize, updated_ms: u64) -> PipWorkspaceSummary {
            PipWorkspaceSummary {
                workspace_id: id.to_owned(),
                workspace_label: id.to_owned(),
                target_count,
                updated_ms,
            }
        }

        #[test]
        fn renderer_tracks_each_target_and_requested_size() {
            let workspaces = [workspace("workspace", 2, 1)];
            let data = render_view(
                480,
                320,
                &[
                    frame(
                        "workspace",
                        "edge",
                        PipTargetKind::BrowserTab,
                        Vec::new(),
                        1,
                    ),
                    frame(
                        "workspace",
                        "notes",
                        PipTargetKind::NativeWindow,
                        Vec::new(),
                        1,
                    ),
                ],
                &workspaces,
                Some("workspace"),
                None,
            );
            assert_eq!(data.len(), 480 * 320 * 4);
            assert!(data.chunks_exact(4).all(|pixel| pixel[3] == 255));
        }

        #[test]
        fn renderer_preserves_target_pixels_across_mixed_form_factors() {
            let frames = [
                frame(
                    "workspace",
                    "wide",
                    PipTargetKind::BrowserTab,
                    solid_png(80, 32, [220, 30, 20, 255]),
                    1,
                ),
                frame(
                    "workspace",
                    "tall",
                    PipTargetKind::NativeWindow,
                    solid_png(24, 72, [20, 90, 230, 255]),
                    1,
                ),
            ];
            let width = 520;
            let height = 360;
            let workspaces = [workspace("workspace", 2, 1)];
            let data = render_view(width, height, &frames, &workspaces, Some("workspace"), None);
            let desktop = desktop_rect(width, height, false);
            let layout = layout_desktop_with_shell(
                desktop.2 as f64,
                desktop.3 as f64,
                &[
                    TargetSize {
                        width: 80,
                        height: 32,
                    },
                    TargetSize {
                        width: 24,
                        height: 72,
                    },
                ],
                ShellStyle::EdgeTaskbar,
            );

            let wide = offset_pixels(layout.targets[0].content, desktop.0, desktop.1);
            let tall = offset_pixels(layout.targets[1].content, desktop.0, desktop.1);
            assert_eq!(
                pixel(
                    &data,
                    width,
                    (wide.0 + wide.2 / 2) as u32,
                    (wide.1 + wide.3 / 2) as u32,
                ),
                [20, 30, 220, 255]
            );
            assert_eq!(
                pixel(
                    &data,
                    width,
                    (tall.0 + tall.2 / 2) as u32,
                    (tall.1 + tall.3 / 2) as u32,
                ),
                [230, 90, 20, 255]
            );
            assert!(wide.2 > wide.3);
            assert!(tall.3 > tall.2);
            assert!(wide.0 >= desktop.0 && wide.1 >= desktop.1);
            assert!(tall.0 + tall.2 <= desktop.0 + desktop.2);
            assert!(tall.1 + tall.3 <= desktop.1 + desktop.3);
        }

        #[test]
        fn renderer_separates_dark_shell_from_inset_desktop() {
            let width = 360;
            let data = render_view(width, 240, &[], &[], None, None);
            let shell = pixel(&data, width, 4, 100);
            let desktop = pixel(&data, width, 180, 100);
            let highlight = pixel(&data, width, 180, 1);
            let brightness = |color: [u8; 4]| color[0] as u16 + color[1] as u16 + color[2] as u16;

            assert_ne!(shell, desktop);
            assert!(brightness(shell) < brightness(desktop));
            assert!(shell[0] > shell[1] && shell[1] > shell[2]);
            assert!(brightness(highlight) > brightness(shell));
            assert!(highlight[0] > highlight[1] && highlight[1] > highlight[2]);
        }

        #[test]
        fn switcher_is_hidden_for_one_workspace_and_cycles_without_target_activation() {
            let width = 420;
            let one = [workspace("agent-a", 1, 1)];
            let two = [workspace("agent-b", 1, 2), workspace("agent-a", 1, 1)];
            let single = render_view(width, 280, &[], &one, Some("agent-a"), None);
            let multiple = render_view(width, 280, &[], &two, Some("agent-a"), None);
            let switcher = switcher_rect(width as i32);
            let sample_x = (switcher.0 + switcher.2 / 2) as u32;
            let sample_y = (switcher.1 + switcher.3 / 2) as u32;

            assert_eq!(desktop_rect(width, 280, false).1, 14);
            assert_eq!(desktop_rect(width, 280, true).1, 36);
            assert_ne!(
                pixel(&single, width, sample_x, sample_y),
                pixel(&multiple, width, sample_x, sample_y)
            );

            let mut model = PipViewModel::new(4);
            model.upsert(frame(
                "agent-a",
                "window-a",
                PipTargetKind::NativeWindow,
                Vec::new(),
                1,
            ));
            model.upsert(frame(
                "agent-b",
                "window-b",
                PipTargetKind::NativeWindow,
                Vec::new(),
                2,
            ));
            assert!(model.select_workspace("agent-a"));
            assert_eq!(model.selected_frames()[0].target.workspace_id, "agent-a");
            assert!(model.select_next_workspace());
            assert_eq!(model.selected_frames()[0].target.workspace_id, "agent-b");
        }

        #[test]
        fn exact_target_removal_and_workspace_cleanup_redraw_the_selected_session() {
            let mut model = PipViewModel::new(6);
            model.upsert(frame(
                "agent-a",
                "wide",
                PipTargetKind::BrowserTab,
                Vec::new(),
                1,
            ));
            model.upsert(frame(
                "agent-a",
                "tall",
                PipTargetKind::NativeWindow,
                Vec::new(),
                2,
            ));
            model.upsert(frame(
                "agent-b",
                "other",
                PipTargetKind::NativeWindow,
                Vec::new(),
                3,
            ));
            assert!(model.select_workspace("agent-a"));
            assert!(model.remove_target("agent-a", "wide"));
            assert_eq!(model.selected_frames().len(), 1);
            assert_eq!(model.selected_frames()[0].target.identity_key, "tall");
            assert_eq!(model.remove_workspace("agent-a").len(), 1);
            assert_eq!(model.selected_workspace_id(), Some("agent-b"));
        }

        fn taskbar_layout(width: u32, height: u32, targets: usize) -> pip_preview::DesktopLayout {
            let desktop = desktop_rect(width, height, false);
            let sizes = vec![
                TargetSize {
                    width: 16,
                    height: 10,
                };
                targets
            ];
            layout_desktop_with_shell(
                desktop.2 as f64,
                desktop.3 as f64,
                &sizes,
                ShellStyle::EdgeTaskbar,
            )
        }

        /// Mean of one channel over a small window, which averages out the
        /// material's deterministic grain.
        fn mean_channel(data: &[u8], width: u32, x: u32, y: u32, channel: usize) -> f32 {
            let mut sum = 0u32;
            for dy in 0..4 {
                for dx in 0..4 {
                    sum += pixel(data, width, x + dx, y + dy)[channel] as u32;
                }
            }
            sum as f32 / 16.0
        }

        #[test]
        fn taskbar_is_edge_anchored_and_spans_the_miniature_desktop() {
            let (width, height) = (520u32, 360u32);
            let desktop = desktop_rect(width, height, false);
            let layout = taskbar_layout(width, height, 2);

            assert_eq!(layout.shell, ShellStyle::EdgeTaskbar);
            let bar = offset_pixels(layout.dock, desktop.0, desktop.1);
            assert_eq!(bar.0, desktop.0);
            assert_eq!(bar.2, desktop.2);
            assert!(((bar.1 + bar.3) - (desktop.1 + desktop.3)).abs() <= 1);

            // The band is a distinct material, so the wallpaper just above the
            // bar's top edge never matches the pixels just inside it.
            let data = render_view(
                width,
                height,
                &[
                    frame("w", "a", PipTargetKind::BrowserTab, Vec::new(), 1),
                    frame("w", "b", PipTargetKind::NativeWindow, Vec::new(), 2),
                ],
                &[workspace("w", 2, 2)],
                Some("w"),
                None,
            );
            let center_x = (desktop.0 + desktop.2 / 2) as u32;
            let above = pixel(&data, width, center_x, (bar.1 - 6) as u32);
            let inside = pixel(&data, width, center_x, (bar.1 + 6) as u32);
            assert_ne!(above, inside);
        }

        #[test]
        fn taskbar_material_carries_the_blurred_backdrop_instead_of_a_flat_fill() {
            let (width, height) = (520u32, 360u32);
            let desktop = desktop_rect(width, height, false);
            let layout = taskbar_layout(width, height, 1);
            let bar = offset_pixels(layout.dock, desktop.0, desktop.1);
            let data = render_view(
                width,
                height,
                &[frame("w", "a", PipTargetKind::BrowserTab, Vec::new(), 1)],
                &[workspace("w", 1, 1)],
                Some("w"),
                None,
            );

            // The wallpaper's lower glow sits left of center, so a material
            // that samples what it covers stays brighter on the left. A flat
            // fill would make these identical.
            let sample_y = (bar.1 + bar.3 / 3) as u32;
            let left = mean_channel(&data, width, (desktop.0 + 80) as u32, sample_y, 0);
            let right = mean_channel(
                &data,
                width,
                (desktop.0 + desktop.2 - 90) as u32,
                sample_y,
                0,
            );
            assert!(
                left - right >= 3.0,
                "expected a blurred backdrop gradient, got {left} vs {right}"
            );
            // It is still a heavy tint, not a window into the wallpaper.
            assert!(left < 90.0);
        }

        #[test]
        fn taskbar_marks_the_newest_card_more_strongly_than_the_others() {
            let (width, height) = (560u32, 380u32);
            let desktop = desktop_rect(width, height, false);
            let layout = taskbar_layout(width, height, 2);
            let data = render_view(
                width,
                height,
                &[
                    frame("w", "a", PipTargetKind::BrowserTab, Vec::new(), 1),
                    frame("w", "b", PipTargetKind::NativeWindow, Vec::new(), 9),
                ],
                &[workspace("w", 2, 9)],
                Some("w"),
                None,
            );
            assert_eq!(layout.indicators.len(), 2);
            let brightness = |index: usize| {
                let mark = offset_pixels(layout.indicators[index], desktop.0, desktop.1);
                let color = pixel(
                    &data,
                    width,
                    (mark.0 + mark.2 / 2) as u32,
                    (mark.1 + mark.3 / 2) as u32,
                );
                color[0] as i32 + color[1] as i32 + color[2] as i32
            };
            // Index 1 is newest, so it wears the accent pill.
            assert!(brightness(1) != brightness(0));
            let active = offset_pixels(layout.indicators[1], desktop.0, desktop.1);
            let accent = pixel(
                &data,
                width,
                (active.0 + active.2 / 2) as u32,
                (active.1 + active.3 / 2) as u32,
            );
            assert!(accent[0] > accent[1] && accent[1] > accent[2]);
        }

        #[test]
        fn start_slot_and_tray_paint_inside_the_bar() {
            let (width, height) = (560u32, 380u32);
            let desktop = desktop_rect(width, height, false);
            let layout = taskbar_layout(width, height, 2);
            let bar = offset_pixels(layout.dock, desktop.0, desktop.1);
            let start = offset_pixels(layout.start_button.unwrap(), desktop.0, desktop.1);
            let tray = offset_pixels(layout.tray.unwrap(), desktop.0, desktop.1);
            assert!(start.1 >= bar.1 && start.1 + start.3 <= bar.1 + bar.3);
            assert_eq!(tray.0 + tray.2, bar.0 + bar.2);

            let data = render_view(
                width,
                height,
                &[
                    frame("w", "a", PipTargetKind::BrowserTab, Vec::new(), 42 * 60_000),
                    frame("w", "b", PipTargetKind::NativeWindow, Vec::new(), 1),
                ],
                &[workspace("w", 2, 42 * 60_000)],
                Some("w"),
                None,
            );
            let empty = render_view(width, height, &[], &[workspace("w", 0, 1)], Some("w"), None);
            let glyph_region = |data: &[u8], rect: (i32, i32, i32, i32)| {
                let mut ink = 0u32;
                for y in rect.1..rect.1 + rect.3 {
                    for x in rect.0..rect.0 + rect.2 {
                        ink += pixel(data, width, x as u32, y as u32)[0] as u32;
                    }
                }
                ink
            };
            // Both the launcher glyph and the tray band paint measurably
            // brighter than the bare material behind them.
            assert!(glyph_region(&data, start) > glyph_region(&empty, start));
            assert!(glyph_region(&data, tray) > glyph_region(&empty, tray));
        }

        #[test]
        fn clock_and_foreground_selection_are_derived_only_from_frame_inputs() {
            assert_eq!(clock_label(0), "00:00");
            assert_eq!(clock_label(9 * 3_600_000 + 5 * 60_000), "09:05");
            assert_eq!(clock_label(23 * 3_600_000 + 59 * 60_000), "23:59");
            // Wrapping past a day keeps a valid time rather than overflowing.
            assert_eq!(clock_label(25 * 3_600_000), "01:00");

            let make = |stamps: [u64; 3]| {
                stamps
                    .iter()
                    .enumerate()
                    .map(|(index, stamp)| {
                        frame(
                            "w",
                            &index.to_string(),
                            PipTargetKind::NativeWindow,
                            Vec::new(),
                            *stamp,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(active_target_index(&[]), None);
            assert_eq!(active_target_index(&make([1, 7, 3])), Some(1));
            // Ties resolve to the earliest card so repaints stay stable.
            assert_eq!(active_target_index(&make([7, 7, 7])), Some(0));
        }

        #[test]
        fn box_blur_flattens_an_impulse_without_changing_a_uniform_field() {
            let uniform = vec![120u8; 9 * 9 * 3];
            assert_eq!(box_blur(&uniform, 9, 9, 2), uniform);

            let mut impulse = vec![0u8; 9 * 9 * 3];
            let center = (4 * 9 + 4) * 3;
            impulse[center..center + 3].copy_from_slice(&[255, 255, 255]);
            let blurred = box_blur(&impulse, 9, 9, 2);
            assert!(blurred[center] < 255);
            // Energy spreads to the neighbours the impulse never touched.
            assert!(blurred[((4 * 9 + 6) * 3)] > 0);
            assert!(blurred[((2 * 9 + 4) * 3)] > 0);
            // ...but not past the kernel radius.
            assert_eq!(blurred[((4 * 9 + 8) * 3)], 0);
        }

        #[test]
        fn input_passthrough_only_toggles_the_transparent_extended_style() {
            let base = (WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).0 as isize;
            let passthrough = extended_style_with_input_passthrough(base, true);
            assert_ne!(passthrough & WS_EX_TRANSPARENT.0 as isize, 0);
            assert_eq!(passthrough & base, base);

            let interactive = extended_style_with_input_passthrough(passthrough, false);
            assert_eq!(interactive, base);
        }
    }
}
