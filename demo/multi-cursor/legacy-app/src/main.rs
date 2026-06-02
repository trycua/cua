//! Legacy-looking form app for the cua-driver multi-cursor demo.
//!
//! Two modes (argv[1]):
//!   gdi    <title>  — a custom GDI-drawn form with NO accessibility tree.
//!                     Proves cua-driver drives apps WITHOUT a11y (pixel path).
//!   master <title>  — the same form built from real Win32 controls (EDIT +
//!                     BUTTON), instrumented to EMIT the user's actions on
//!                     stdout so the orchestrator can replay them onto the
//!                     background corner windows. This is the foreground window
//!                     the human actually interacts with.
//!
//! Emitted protocol (one per line, tab-separated) — master only:
//!   TYPE\t<text>          the committed field text
//!   CLICK\t<rx>\t<ry>     a click at relative [0,1] client coords
//!
//! Fixed client size so a relative point maps to the same control in every
//! framework's copy of this form.

#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::io::Write;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DrawTextW, EndPaint, FillRect, FrameRect, GetStockObject, InvalidateRect,
    Rectangle, SelectObject, SetBkMode, SetTextColor, CreateSolidBrush, DeleteObject,
    DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HBRUSH, PAINTSTRUCT, TRANSPARENT,
    DEFAULT_GUI_FONT, BLACK_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLIENT_W: i32 = 480;
const CLIENT_H: i32 = 300;

// SUBMIT button rectangle (client coords), shared by both modes.
const BTN: RECT = RECT { left: 160, top: 150, right: 320, bottom: 196 };
// Text field rectangle (client coords).
const FIELD: RECT = RECT { left: 90, top: 70, right: 390, bottom: 98 };

const ID_EDIT: isize = 1001;
const ID_BUTTON: isize = 1002;
const ID_STATUS: isize = 1003;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Gdi,
    Master,
}

struct State {
    mode: Mode,
    title: String,
    clicks: u32,
    text: String,
    hedit: HWND,
    hstatus: HWND,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn emit(line: &str) {
    let _ = writeln!(std::io::stdout(), "{line}");
    let _ = std::io::stdout().flush();
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = match args.get(1).map(|s| s.as_str()) {
        Some("master") => Mode::Master,
        _ => Mode::Gdi,
    };
    let title = args.get(2).cloned().unwrap_or_else(|| match mode {
        Mode::Master => "Win32 Controls (master)".into(),
        Mode::Gdi => "Win32 GDI (no a11y)".into(),
    });

    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let class = w!("CuaDemoLegacyForm");
        let bg = CreateSolidBrush(COLORREF(0x00C0C0C0)); // classic gray
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hbrBackground: bg,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);

        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                mode,
                title: title.clone(),
                clicks: 0,
                text: String::new(),
                hedit: HWND::default(),
                hstatus: HWND::default(),
            })
        });

        // Client size -> window size (account for frame).
        let mut r = RECT { left: 0, top: 0, right: CLIENT_W, bottom: CLIENT_H };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let _ = AdjustWindowRect(&mut r, style, false);
        let title_w = wide(&title);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(title_w.as_ptr()),
            style,
            CW_USEDEFAULT, CW_USEDEFAULT,
            r.right - r.left, r.bottom - r.top,
            None, None, HINSTANCE(hinst.0), None,
        )
        .expect("CreateWindowExW");

        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn create_master_controls(parent: HWND) {
    let hmod = GetModuleHandleW(None).unwrap();
    let hinst = HINSTANCE(hmod.0);
    // EDIT field
    let hedit = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("EDIT"),
        w!(""),
        WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        FIELD.left, FIELD.top, FIELD.right - FIELD.left, FIELD.bottom - FIELD.top,
        parent, HMENU(ID_EDIT as *mut core::ffi::c_void), hinst, None,
    ).unwrap_or_default();
    // SUBMIT button
    let _hbtn = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("BUTTON"),
        w!("SUBMIT"),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        BTN.left, BTN.top, BTN.right - BTN.left, BTN.bottom - BTN.top,
        parent, HMENU(ID_BUTTON as *mut core::ffi::c_void), hinst, None,
    ).unwrap_or_default();
    // Status static
    let hstatus = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("STATIC"),
        w!("Clicks: 0    Last: (none)"),
        WS_CHILD | WS_VISIBLE,
        20, 220, 440, 24,
        parent, HMENU(ID_STATUS as *mut core::ffi::c_void), hinst, None,
    ).unwrap_or_default();

    // Nicer (still legacy) GUI font on the children.
    let font = GetStockObject(DEFAULT_GUI_FONT);
    for h in [hedit, hstatus] {
        SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    }

    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.hedit = hedit;
            st.hstatus = hstatus;
        }
    });
}

unsafe fn update_status(hwnd: HWND) {
    STATE.with(|s| {
        if let Some(st) = s.borrow().as_ref() {
            let last = if st.text.is_empty() { "(none)" } else { st.text.as_str() };
            let line = format!("Clicks: {}    Last: {}", st.clicks, last);
            if st.mode == Mode::Master && !st.hstatus.0.is_null() {
                let w = wide(&line);
                let _ = SetWindowTextW(st.hstatus, PCWSTR(w.as_ptr()));
            } else {
                let _ = InvalidateRect(hwnd, None, true);
            }
        }
    });
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                let mode = STATE.with(|s| s.borrow().as_ref().map(|st| st.mode));
                if mode == Some(Mode::Master) {
                    create_master_controls(hwnd);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as isize;
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                if id == ID_BUTTON && code == BN_CLICKED {
                    on_submit(hwnd);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                // Raw click in the parent client area (empty regions). Emit a
                // relative-coordinate click so corners get clicked at the same
                // spot. (Clicks on the button arrive as WM_COMMAND instead.)
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let mode = STATE.with(|s| s.borrow().as_ref().map(|st| st.mode));
                if mode == Some(Mode::Master) {
                    emit(&format!("CLICK\t{:.4}\t{:.4}", x as f64 / CLIENT_W as f64, y as f64 / CLIENT_H as f64));
                } else {
                    // GDI mode: behave like an app — count clicks in the button.
                    if pt_in(&BTN, x, y) {
                        bump_click(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_CHAR => {
                // GDI mode has no EDIT control; maintain our own text buffer so
                // cua-driver type_text (WM_CHAR) is visibly reflected.
                let mode = STATE.with(|s| s.borrow().as_ref().map(|st| st.mode));
                if mode == Some(Mode::Gdi) {
                    let ch = wparam.0 as u8 as char;
                    STATE.with(|s| {
                        if let Some(st) = s.borrow_mut().as_mut() {
                            match ch {
                                '\u{8}' => { st.text.pop(); }            // backspace
                                '\r' | '\n' => {}
                                c if !c.is_control() => st.text.push(c),
                                _ => {}
                            }
                        }
                    });
                    let _ = InvalidateRect(hwnd, None, true);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn on_submit(hwnd: HWND) {
    // Read the EDIT text, emit TYPE + CLICK(button center), bump local state.
    let text = STATE.with(|s| {
        let st = s.borrow();
        let st = st.as_ref()?;
        if st.hedit.0.is_null() { return None; }
        let len = GetWindowTextLengthW(st.hedit);
        let mut buf = vec![0u16; (len + 1) as usize];
        let n = GetWindowTextW(st.hedit, &mut buf);
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    });
    if let Some(t) = text {
        emit(&format!("TYPE\t{t}"));
        let cx = (BTN.left + BTN.right) as f64 / 2.0 / CLIENT_W as f64;
        let cy = (BTN.top + BTN.bottom) as f64 / 2.0 / CLIENT_H as f64;
        emit(&format!("CLICK\t{cx:.4}\t{cy:.4}"));
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.clicks += 1;
                st.text = t;
            }
        });
        update_status(hwnd);
    }
}

unsafe fn bump_click(hwnd: HWND) {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.clicks += 1;
        }
    });
    update_status(hwnd);
}

fn pt_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let font = GetStockObject(DEFAULT_GUI_FONT);
    SelectObject(hdc, font);
    SetBkMode(hdc, TRANSPARENT);

    STATE.with(|s| {
        let st = s.borrow();
        let Some(st) = st.as_ref() else { return };

        // Title banner.
        let mut title_rc = RECT { left: 0, top: 8, right: CLIENT_W, bottom: 36 };
        SetTextColor(hdc, COLORREF(0x00553300));
        let mut tw = wide(&st.title);
        DrawTextW(hdc, &mut tw, &mut title_rc, DT_CENTER | DT_SINGLELINE);

        // "Name:" label.
        SetTextColor(hdc, COLORREF(0x00000000));
        let mut lbl_rc = RECT { left: 20, top: FIELD.top + 2, right: 88, bottom: FIELD.bottom };
        let mut lw = wide("Name:");
        DrawTextW(hdc, &mut lw, &mut lbl_rc, DT_LEFT | DT_SINGLELINE);

        if st.mode == Mode::Gdi {
            // Draw the field box + its text (custom; no real control => no a11y).
            let white = CreateSolidBrush(COLORREF(0x00FFFFFF));
            let mut field = FIELD;
            FillRect(hdc, &field, white);
            let _ = DeleteObject(white);
            let edge = GetStockObject(BLACK_BRUSH);
            FrameRect(hdc, &field, HBRUSH(edge.0));
            let mut tr = RECT { left: FIELD.left + 6, top: FIELD.top, right: FIELD.right - 4, bottom: FIELD.bottom };
            let mut txt = wide(&st.text);
            DrawTextW(hdc, &mut txt, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

            // Draw the SUBMIT button (raised look).
            let face = CreateSolidBrush(COLORREF(0x00C8C8C8));
            let mut b = BTN;
            FillRect(hdc, &b, face);
            let _ = DeleteObject(face);
            let _ = Rectangle(hdc, BTN.left, BTN.top, BTN.right, BTN.bottom);
            let mut br = BTN;
            let mut bw = wide("SUBMIT");
            DrawTextW(hdc, &mut bw, &mut br, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

            // Status line.
            let last = if st.text.is_empty() { "(none)" } else { st.text.as_str() };
            let mut sr = RECT { left: 20, top: 220, right: CLIENT_W - 20, bottom: 244 };
            let mut sw = wide(&format!("Clicks: {}    Last: {}", st.clicks, last));
            DrawTextW(hdc, &mut sw, &mut sr, DT_LEFT | DT_SINGLELINE);
        }
        let _ = font;
    });

    let _ = EndPaint(hwnd, &ps);
}
