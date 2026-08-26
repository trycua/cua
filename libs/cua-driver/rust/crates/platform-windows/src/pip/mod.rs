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
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    use image::imageops::FilterType;
    use pip_preview::{
        layout_desktop, png_dimensions, LayoutRect, PipBackend, PipBackendFactory, PipConfig,
        PipFrame, PipTargetKind, PipViewModel, TargetSize,
    };
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE,
    };
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, EndPaint, InvalidateRect, StretchDIBits, UpdateWindow, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, PAINTSTRUCT, SRCCOPY,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, LoadCursorW, PostMessageW,
        PostQuitMessage, RegisterClassExW, SetWindowLongPtrW, ShowWindow, TranslateMessage,
        CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HTBOTTOM, HTBOTTOMLEFT,
        HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
        IDC_ARROW, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_SHOWNOACTIVATE, WM_APP, WM_CLOSE, WM_DESTROY,
        WM_ERASEBKGND, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_SIZE, WNDCLASSEXW,
        WS_CLIPCHILDREN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        WS_THICKFRAME,
    };

    use super::WindowsPipBackendFactory;

    const REDRAW: u32 = WM_APP + 1;
    const SHUTDOWN: u32 = WM_APP + 2;

    struct State {
        hwnd: AtomicIsize,
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

        fn shutdown(self: Box<Self>) {
            self.state.post(SHUTDOWN);
        }
    }

    impl PipBackendFactory for WindowsPipBackendFactory {
        fn start(&self, cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
            let state = Arc::new(State {
                hwnd: AtomicIsize::new(0),
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
            WM_NCHITTEST => {
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
        let frames = state
            .model
            .lock()
            .unwrap()
            .ordered_frames()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let pixels = render_view(width, height, &frames);
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

    fn render_view(width: u32, height: u32, frames: &[PipFrame]) -> Vec<u8> {
        let mut canvas = Canvas::new(width, height);
        canvas.smoked_shell();
        let desktop = desktop_rect(width, height);
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
        let layout = layout_desktop(desktop.2 as f64, desktop.3 as f64, &sizes);
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
        }
        let dock = offset_pixels(layout.dock, desktop.0, desktop.1);
        canvas.shadow(dock);
        canvas.fill(dock, 11, [40, 30, 23, 218]);
        canvas.border(dock, 11, [204, 176, 145, 118]);
        for (index, icon) in layout.dock_icons.iter().enumerate() {
            canvas.icon(
                offset_pixels(*icon, desktop.0, desktop.1),
                frames[index].target.target_kind,
                index,
            );
        }
        canvas.data
    }

    fn desktop_rect(width: u32, height: u32) -> (i32, i32, i32, i32) {
        const SIDE_INSET: i32 = 8;
        const TOP_INSET: i32 = 14;
        const BOTTOM_INSET: i32 = 9;
        (
            SIDE_INSET,
            TOP_INSET,
            (width as i32 - SIDE_INSET * 2).max(1),
            (height as i32 - TOP_INSET - BOTTOM_INSET).max(1),
        )
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
            self.fill(
                (rect.0 + rect.2 / 5, rect.1 + rect.3 + 4, rect.2 * 3 / 5, 3),
                1,
                [245, 248, 255, 235],
            );
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

        #[test]
        fn renderer_tracks_each_target_and_requested_size() {
            let frame = |id: &str, kind| PipFrame {
                target: PipTarget {
                    workspace_id: "workspace".to_owned(),
                    workspace_label: "Agent".to_owned(),
                    target_id: id.to_owned(),
                    target_kind: kind,
                    target_label: id.to_owned(),
                },
                png_bytes: Vec::new(),
                action_label: "click".to_owned(),
                timestamp_ms: 1,
            };
            let data = render_view(
                480,
                320,
                &[
                    frame("edge", PipTargetKind::BrowserTab),
                    frame("notes", PipTargetKind::NativeWindow),
                ],
            );
            assert_eq!(data.len(), 480 * 320 * 4);
            assert!(data.chunks_exact(4).all(|pixel| pixel[3] == 255));
        }

        #[test]
        fn renderer_preserves_target_pixels_across_mixed_form_factors() {
            let frame = |id: &str, kind, png_bytes| PipFrame {
                target: PipTarget {
                    workspace_id: "workspace".to_owned(),
                    workspace_label: "Agent".to_owned(),
                    target_id: id.to_owned(),
                    target_kind: kind,
                    target_label: id.to_owned(),
                },
                png_bytes,
                action_label: "click".to_owned(),
                timestamp_ms: 1,
            };
            let frames = [
                frame(
                    "wide",
                    PipTargetKind::BrowserTab,
                    solid_png(80, 32, [220, 30, 20, 255]),
                ),
                frame(
                    "tall",
                    PipTargetKind::NativeWindow,
                    solid_png(24, 72, [20, 90, 230, 255]),
                ),
            ];
            let width = 520;
            let height = 360;
            let data = render_view(width, height, &frames);
            let desktop = desktop_rect(width, height);
            let layout = layout_desktop(
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
            let data = render_view(width, 240, &[]);
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
    }
}
