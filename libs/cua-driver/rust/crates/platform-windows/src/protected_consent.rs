//! Capture-excluded, automation-hidden protected consent surface for Windows.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use overlay_ui::{
    place_near_pointer, render_consent, render_indicator, ConsentInteraction, ConsentVisualState,
    HelperDecision, HelperEvent, HelperRequest, InteractionOutcome, Point, Rect, ACCEPT_RECT,
    CONSENT_SIZE, CONTROL_ARM_DELAY, DECLINE_RECT, INDICATOR_SIZE, STOP_RECT,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::*;

static CONTEXT: Mutex<Option<SurfaceContext>> = Mutex::new(None);

struct SurfaceContext {
    mode: SurfaceMode,
    hwnd: HWND,
    scale: f32,
    pointer: Point,
    outcome: Option<HelperEvent>,
}

// The HWND is accessed only by the message-loop thread.
unsafe impl Send for SurfaceContext {}

enum SurfaceMode {
    Consent {
        card: overlay_ui::ConsentCard,
        interaction: ConsentInteraction,
    },
    Indicator {
        card: overlay_ui::IndicatorCard,
        stop_pressed: bool,
    },
}

pub fn interactive_surface_available() -> bool {
    let mut session = 0u32;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session).as_bool() && session != 0 }
}

pub fn run(request: HelperRequest) -> anyhow::Result<()> {
    if !interactive_surface_available() {
        anyhow::bail!("protected Windows UI requires an interactive non-session-0 desktop");
    }
    unsafe { run_win32(request) }
}

unsafe fn run_win32(request: HelperRequest) -> anyhow::Result<()> {
    let dpi = GetDpiForSystem().max(96);
    let scale = dpi as f32 / 96.0;
    let logical_size = match &request {
        HelperRequest::Consent(_) => CONSENT_SIZE,
        HelperRequest::Indicator(_) => INDICATOR_SIZE,
    };
    let mut cursor = POINT::default();
    GetCursorPos(&mut cursor)?;
    let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    GetMonitorInfoW(monitor, &mut info)?;
    let work = info.rcWork;
    let work_rect = Rect {
        x: work.left as f64,
        y: work.top as f64,
        width: (work.right - work.left) as f64,
        height: (work.bottom - work.top) as f64,
    };
    let physical_size = overlay_ui::Size {
        width: logical_size.width * scale as f64,
        height: logical_size.height * scale as f64,
    };
    let origin = match &request {
        HelperRequest::Consent(_) => place_near_pointer(
            Point {
                x: cursor.x as f64,
                y: cursor.y as f64,
            },
            work_rect,
            physical_size,
        ),
        HelperRequest::Indicator(_) => Point {
            x: work_rect.x + work_rect.width - physical_size.width - 18.0,
            y: work_rect.y + 18.0,
        },
    };

    let class_name = wide("Cua.ProtectedConsentSurface");
    let title = wide("Cua protected consent");
    let instance = GetModuleHandleW(PCWSTR::null())?;
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        hCursor: LoadCursorW(None, IDC_ARROW)?,
        ..Default::default()
    });
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_POPUP,
        origin.x.round() as i32,
        origin.y.round() as i32,
        physical_size.width.round() as i32,
        physical_size.height.round() as i32,
        None,
        None,
        instance,
        None,
    )?;
    let mode = match request {
        HelperRequest::Consent(card) => SurfaceMode::Consent {
            card,
            interaction: ConsentInteraction::new(Instant::now(), ACCEPT_RECT, DECLINE_RECT),
        },
        HelperRequest::Indicator(card) => SurfaceMode::Indicator {
            card,
            stop_pressed: false,
        },
    };
    *CONTEXT.lock().unwrap() = Some(SurfaceContext {
        mode,
        hwnd,
        scale,
        pointer: Point { x: -1.0, y: -1.0 },
        outcome: None,
    });
    let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    redraw();
    ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    SetTimer(hwnd, 1, CONTROL_ARM_DELAY.as_millis() as u32, None);
    if let Some(delay) = consent_expiry_delay() {
        SetTimer(
            hwnd,
            2,
            delay.as_millis().min(u32::MAX as u128) as u32,
            None,
        );
    }
    crate::protected_consent_event(&HelperEvent::Ready)?;

    let mut message = MSG::default();
    while GetMessageW(&mut message, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }
    let outcome = CONTEXT
        .lock()
        .unwrap()
        .take()
        .and_then(|context| context.outcome)
        .unwrap_or_else(fallback_outcome);
    crate::protected_consent_event(&outcome)?;
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_GETOBJECT => return LRESULT(0),
        WM_MOUSEMOVE => {
            update_pointer(lparam);
            redraw();
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let point = update_pointer(lparam);
            let _ = SetCapture(hwnd);
            with_context(|context| match &mut context.mode {
                SurfaceMode::Consent { interaction, .. } => {
                    interaction.pointer_down(
                        point,
                        overlay_ui::PointerButton::Primary,
                        Instant::now(),
                    );
                }
                SurfaceMode::Indicator { stop_pressed, .. } => {
                    *stop_pressed = STOP_RECT.contains(point);
                }
            });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            let point = update_pointer(lparam);
            let event = with_context(|context| match &mut context.mode {
                SurfaceMode::Consent { card, interaction } => {
                    let action = match interaction.pointer_up(point) {
                        InteractionOutcome::Accept => Some(HelperDecision::Accept),
                        InteractionOutcome::Decline => Some(HelperDecision::Decline),
                        _ => None,
                    }?;
                    Some(HelperEvent::Decision {
                        action,
                        request_digest: card.request_digest.clone(),
                    })
                }
                SurfaceMode::Indicator { card, stop_pressed } => {
                    let pressed = std::mem::take(stop_pressed);
                    (pressed && STOP_RECT.contains(point)).then(|| HelperEvent::Stop {
                        indicator_id: card.indicator_id.clone(),
                    })
                }
            });
            if let Some(event) = event {
                finish(event);
            }
            return LRESULT(0);
        }
        WM_KEYDOWN if wparam.0 as u32 == VK_ESCAPE.0 => {
            finish(fallback_outcome());
            return LRESULT(0);
        }
        WM_TIMER => {
            let _ = KillTimer(hwnd, wparam.0);
            if wparam.0 == 1 {
                redraw();
            } else if wparam.0 == 2 {
                finish(fallback_outcome());
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            finish(fallback_outcome());
            return LRESULT(0);
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn with_context<T>(action: impl FnOnce(&mut SurfaceContext) -> T) -> T {
    let mut context = CONTEXT.lock().unwrap();
    action(context.as_mut().expect("protected context initialized"))
}

fn update_pointer(lparam: LPARAM) -> Point {
    let x = (lparam.0 as u32 & 0xffff) as i16 as f64;
    let y = ((lparam.0 as u32 >> 16) & 0xffff) as i16 as f64;
    with_context(|context| {
        let point = Point {
            x: x / context.scale as f64,
            y: y / context.scale as f64,
        };
        context.pointer = point;
        if let SurfaceMode::Consent { interaction, .. } = &mut context.mode {
            interaction.pointer_moved(point, Instant::now());
        }
        point
    })
}

fn finish(event: HelperEvent) {
    let hwnd = with_context(|context| {
        context.outcome = Some(event);
        context.hwnd
    });
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

fn fallback_outcome() -> HelperEvent {
    let context = CONTEXT.lock().unwrap();
    match context.as_ref().map(|context| &context.mode) {
        Some(SurfaceMode::Consent { card, .. }) => HelperEvent::Decision {
            action: HelperDecision::Cancel,
            request_digest: card.request_digest.clone(),
        },
        Some(SurfaceMode::Indicator { card, .. }) => HelperEvent::Stop {
            indicator_id: card.indicator_id.clone(),
        },
        None => HelperEvent::Failed {
            reason: "protected surface closed without context".to_owned(),
        },
    }
}

fn consent_expiry_delay() -> Option<Duration> {
    CONTEXT.lock().unwrap().as_ref().and_then(|context| {
        let SurfaceMode::Consent { card, .. } = &context.mode else {
            return None;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis();
        Some(Duration::from_millis(
            u128::from(card.expires_unix_ms)
                .saturating_sub(now)
                .clamp(100, u128::from(u64::MAX)) as u64,
        ))
    })
}

unsafe fn redraw() {
    with_context(|context| {
        let pixmap = match &context.mode {
            SurfaceMode::Consent { card, interaction } => render_consent(
                card,
                context.scale,
                ConsentVisualState {
                    accept_armed: interaction.accept_armed(Instant::now()),
                    accept_hovered: ACCEPT_RECT.contains(context.pointer),
                    decline_hovered: DECLINE_RECT.contains(context.pointer),
                },
            ),
            SurfaceMode::Indicator { card, .. } => {
                render_indicator(card, context.scale, STOP_RECT.contains(context.pointer))
            }
        };
        if let Ok(pixmap) = pixmap {
            update_layered_window(context.hwnd, &pixmap);
        }
    });
}

unsafe fn update_layered_window(hwnd: HWND, pixmap: &tiny_skia::Pixmap) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let screen = GetDC(None);
    let memory = CreateCompatibleDC(screen);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let Ok(bitmap) = CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        return;
    };
    let _ = SelectObject(memory, bitmap);
    let output = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (width * height * 4) as usize);
    for (source, target) in pixmap
        .data()
        .chunks_exact(4)
        .zip(output.chunks_exact_mut(4))
    {
        target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
    }
    let source = POINT { x: 0, y: 0 };
    let size = SIZE {
        cx: width,
        cy: height,
    };
    let blend = BLENDFUNCTION {
        BlendOp: 0,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: 1,
    };
    let _ = UpdateLayeredWindow(
        hwnd,
        screen,
        None,
        Some(&size),
        memory,
        Some(&source),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(memory);
    ReleaseDC(None, screen);
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
