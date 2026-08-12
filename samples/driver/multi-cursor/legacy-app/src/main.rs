//! "Meridian CRM — Account Record" — a dense, realistic Windows office form for
//! the cua-driver multi-cursor demo. Menu bar, toolbar, a packed left form
//! (Account Name, Account Type / Region dropdowns, Priority / Credit sliders),
//! a SAVE button, a signature doodle pad, a spreadsheet grid, and a status bar.
//! No padding between elements beyond their 1px borders.
//!
//! Modes (argv[1]):
//!   gdi    <node>  — fully GDI-drawn (NO accessibility tree → cua-driver drives
//!                    it via the pixel path). The Account-Name field and SAVE
//!                    button are drawn; the doodle pad is live.
//!   master <node>  — same chrome, but Account Name + SAVE are real Win32
//!                    controls, instrumented to EMIT the user's action on stdout
//!                    (TYPE<text>, CLICK<rx><ry>) for the orchestrator to replay.

#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::io::Write;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    FrameRect, InvalidateRect, LineTo, MoveToEx, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, GetStockObject, HBRUSH, HFONT, PAINTSTRUCT, PS_SOLID, TRANSPARENT, BLACK_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

// ── dense fractional layout (shared by gdi + master) ──────────────────────────
const MENU_B: f64 = 0.05;
const TOOL_B: f64 = 0.10;
// left form rows
const NAME: [f64; 4] = [0.16, 0.110, 0.41, 0.170];  // Account Name field box
const TYPE: [f64; 4] = [0.16, 0.175, 0.41, 0.235];
const REGION: [f64; 4] = [0.16, 0.240, 0.41, 0.300];
const PRIO: [f64; 4] = [0.16, 0.305, 0.41, 0.365];
const CREDIT: [f64; 4] = [0.16, 0.370, 0.41, 0.430];
const SAVE: [f64; 4] = [0.04, 0.460, 0.40, 0.530];  // SAVE button
const DOODLE: [f64; 4] = [0.42, 0.140, 0.99, 0.420];
const GRID: [f64; 4] = [0.01, 0.560, 0.99, 0.930];
const STATUS_T: f64 = 0.94;
// SAVE button relative center (master emits this for the CLICK event).
const SAVE_CX: f64 = (SAVE[0] + SAVE[2]) / 2.0;
const SAVE_CY: f64 = (SAVE[1] + SAVE[3]) / 2.0;

const ID_EDIT: isize = 1001;
const ID_BUTTON: isize = 1002;

#[derive(Clone, Copy, PartialEq)]
enum Mode { Gdi, Master }

struct State {
    mode: Mode,
    node: String,
    records: Vec<[String; 5]>,
    name: String,
    // line tool: one straight segment per press-drag-release (down→up)
    segs: Vec<(POINT, POINT)>,
    down: Option<POINT>,
    cur: POINT,
    hedit: HWND,
    hbtn: HWND,
    f_title: HFONT,
    f_label: HFONT,
    f_mono: HFONT,
    cw: i32,
    ch: i32,
}

thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

fn emit(line: &str) { let _ = writeln!(std::io::stdout(), "{line}"); let _ = std::io::stdout().flush(); }
fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
fn fr(cw: i32, ch: i32, r: [f64; 4]) -> RECT {
    RECT { left: (cw as f64 * r[0]) as i32, top: (ch as f64 * r[1]) as i32,
           right: (cw as f64 * r[2]) as i32, bottom: (ch as f64 * r[3]) as i32 }
}
fn band(cw: i32, ch: i32, y0: f64, y1: f64) -> RECT {
    RECT { left: 0, top: (ch as f64 * y0) as i32, right: cw, bottom: (ch as f64 * y1) as i32 }
}
fn pt_in(r: &RECT, x: i32, y: i32) -> bool { x >= r.left && x < r.right && y >= r.top && y < r.bottom }
unsafe fn mk_font(h: i32, w: i32, face: PCWSTR) -> HFONT { CreateFontW(h, 0, 0, 0, w, 0, 0, 0, 0, 0, 0, 0, 0, face) }

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = if args.get(1).map(|s| s.as_str()) == Some("master") { Mode::Master } else { Mode::Gdi };
    let node = args.get(2).cloned().unwrap_or_else(|| match mode { Mode::Master => "Master".into(), Mode::Gdi => "Win32 GDI".into() });

    unsafe {
        let hmod = GetModuleHandleW(None).unwrap();
        let class = w!("MeridianCrmForm");
        let bg = CreateSolidBrush(COLORREF(0x00F0F0F0));
        let wc = WNDCLASSW { lpfnWndProc: Some(wnd_proc), hInstance: hmod.into(), lpszClassName: class,
            hbrBackground: bg, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);

        STATE.with(|s| *s.borrow_mut() = Some(State { mode, node: node.clone(), records: Vec::new(),
            name: String::new(), segs: Vec::new(), down: None, cur: POINT { x: 0, y: 0 }, hedit: HWND::default(), hbtn: HWND::default(),
            f_title: HFONT::default(), f_label: HFONT::default(), f_mono: HFONT::default(), cw: 0, ch: 0 }));

        let title = format!("Meridian CRM — Account Record [{node}]");
        let tw = wide(&title);
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR(tw.as_ptr()), WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT, CW_USEDEFAULT, 960, 600, None, None, HINSTANCE(hmod.0), None).expect("CreateWindowExW");
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
}

unsafe fn create_master_controls(parent: HWND) {
    let hinst = HINSTANCE(GetModuleHandleW(None).unwrap().0);
    let hedit = CreateWindowExW(WS_EX_CLIENTEDGE, w!("EDIT"), w!(""),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        0, 0, 10, 10, parent, HMENU(ID_EDIT as *mut core::ffi::c_void), hinst, None).unwrap_or_default();
    let hbtn = CreateWindowExW(WINDOW_EX_STYLE(0), w!("BUTTON"), w!("Add Record"),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        0, 0, 10, 10, parent, HMENU(ID_BUTTON as *mut core::ffi::c_void), hinst, None).unwrap_or_default();
    STATE.with(|s| if let Some(st) = s.borrow_mut().as_mut() { st.hedit = hedit; st.hbtn = hbtn; });
}

unsafe fn relayout(hwnd: HWND, cw: i32, ch: i32) {
    STATE.with(|s| {
        let mut b = s.borrow_mut(); let Some(st) = b.as_mut() else { return };
        st.cw = cw; st.ch = ch;
        for f in [st.f_title, st.f_label, st.f_mono] { if !f.is_invalid() { let _ = DeleteObject(f); } }
        let seg = wide("Segoe UI"); let mono = wide("Consolas");
        st.f_title = mk_font(-(ch / 34).clamp(12, 22), 700, PCWSTR(seg.as_ptr()));
        st.f_label = mk_font(-(ch / 40).clamp(11, 18), 400, PCWSTR(seg.as_ptr()));
        st.f_mono  = mk_font(-(ch / 44).clamp(10, 16), 400, PCWSTR(mono.as_ptr()));
        if st.mode == Mode::Master && !st.hedit.0.is_null() {
            let n = fr(cw, ch, NAME);
            let _ = MoveWindow(st.hedit, n.left + 2, n.top + 2, n.right - n.left - 4, n.bottom - n.top - 4, true);
            let sv = fr(cw, ch, SAVE);
            let _ = MoveWindow(st.hbtn, sv.left, sv.top, sv.right - sv.left, sv.bottom - sv.top, true);
            SendMessageW(st.hedit, WM_SETFONT, WPARAM(st.f_label.0 as usize), LPARAM(1));
            SendMessageW(st.hbtn, WM_SETFONT, WPARAM(st.f_label.0 as usize), LPARAM(1));
        }
    });
    let _ = InvalidateRect(hwnd, None, true);
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => { if STATE.with(|s| s.borrow().as_ref().map(|st| st.mode)) == Some(Mode::Master) { create_master_controls(hwnd); } LRESULT(0) }
            WM_SIZE => { let (cw, ch) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32); if cw > 0 && ch > 0 { relayout(hwnd, cw, ch); } LRESULT(0) }
            WM_COMMAND => { if (wp.0 & 0xFFFF) as isize == ID_BUTTON && ((wp.0 >> 16) & 0xFFFF) as u32 == BN_CLICKED { on_submit(hwnd); } LRESULT(0) }
            WM_LBUTTONDOWN => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                let (mode, cw, ch) = STATE.with(|s| { let b = s.borrow(); let st = b.as_ref().unwrap(); (st.mode, st.cw, st.ch) });
                if mode == Mode::Gdi && pt_in(&fr(cw, ch, SAVE), x, y) { commit(hwnd); }
                else if pt_in(&fr(cw, ch, DOODLE), x, y) {
                    // line tool: remember the press point; commit on button-up.
                    STATE.with(|s| if let Some(st) = s.borrow_mut().as_mut() { st.down = Some(POINT { x, y }); st.cur = POINT { x, y }; });
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if (wp.0 & 0x0001) != 0 {
                    let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                    let go = STATE.with(|s| { let mut b = s.borrow_mut(); let st = b.as_mut().unwrap();
                        if st.down.is_some() { st.cur = POINT { x, y }; true } else { false } });
                    if go { let (cw, ch) = STATE.with(|s| { let b = s.borrow(); (b.as_ref().unwrap().cw, b.as_ref().unwrap().ch) });
                        let d = fr(cw, ch, DOODLE); let _ = InvalidateRect(hwnd, Some(&d), false); }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                STATE.with(|s| if let Some(st) = s.borrow_mut().as_mut() {
                    if let Some(p0) = st.down.take() { st.segs.push((p0, POINT { x, y })); } });
                let (cw, ch) = STATE.with(|s| { let b = s.borrow(); (b.as_ref().unwrap().cw, b.as_ref().unwrap().ch) });
                let d = fr(cw, ch, DOODLE); let _ = InvalidateRect(hwnd, Some(&d), false);
                LRESULT(0)
            }
            WM_CHAR => {
                if STATE.with(|s| s.borrow().as_ref().map(|st| st.mode)) == Some(Mode::Gdi) {
                    let ch = char::from_u32(wp.0 as u32).unwrap_or('\0');
                    STATE.with(|s| if let Some(st) = s.borrow_mut().as_mut() {
                        match ch { '\u{8}' => { st.name.pop(); }, '\r' | '\n' => {}, c if !c.is_control() => st.name.push(c), _ => {} } });
                    let (cw, ch2) = STATE.with(|s| { let b = s.borrow(); (b.as_ref().unwrap().cw, b.as_ref().unwrap().ch) });
                    let n = fr(cw, ch2, NAME); let _ = InvalidateRect(hwnd, Some(&n), false);
                }
                LRESULT(0)
            }
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe fn on_submit(hwnd: HWND) {
    let text = STATE.with(|s| { let b = s.borrow(); let st = b.as_ref()?; if st.hedit.0.is_null() { return None; }
        let len = GetWindowTextLengthW(st.hedit); let mut buf = vec![0u16; (len + 1) as usize];
        let n = GetWindowTextW(st.hedit, &mut buf); Some(String::from_utf16_lossy(&buf[..n as usize])) });
    if let Some(t) = text {
        emit(&format!("TYPE\t{t}"));
        emit(&format!("CLICK\t{SAVE_CX:.4}\t{SAVE_CY:.4}"));
        STATE.with(|s| if let Some(st) = s.borrow_mut().as_mut() { st.name = t; });
        commit(hwnd);
        STATE.with(|s| if let Some(st) = s.borrow().as_ref() { if !st.hedit.0.is_null() { let _ = SetWindowTextW(st.hedit, w!("")); } });
    }
}

unsafe fn commit(hwnd: HWND) {
    STATE.with(|s| if let Some(st) = s.borrow_mut().as_mut() {
        let nm = if st.name.trim().is_empty() { "(unnamed)".into() } else { st.name.trim().chars().take(22).collect::<String>() };
        st.records.push([nm, "Enterprise".into(), "North".into(), "3".into(), "$40,000".into()]);
        st.name.clear();
    });
    let _ = InvalidateRect(hwnd, None, true);
}

unsafe fn line(hdc: windows::Win32::Graphics::Gdi::HDC, x: i32, y: i32, text: &str) {
    let mut r = RECT { left: x, top: y, right: x + 6000, bottom: y + 60 }; let mut t = wide(text);
    DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    SetBkMode(hdc, TRANSPARENT);
    STATE.with(|s| {
        let b = s.borrow(); let Some(st) = b.as_ref() else { return };
        let (cw, ch) = (st.cw.max(1), st.ch.max(1));
        let face = CreateSolidBrush(COLORREF(0x00F0F0F0));
        let bar = CreateSolidBrush(COLORREF(0x00E8E8E8));
        let hdr = CreateSolidBrush(COLORREF(0x00F5F1EE));
        let white = CreateSolidBrush(COLORREF(0x00FFFFFF));
        let line_c = CreateSolidBrush(COLORREF(0x00D2D2D2));
        let labelbg = CreateSolidBrush(COLORREF(0x00F0F0F0));
        let black = GetStockObject(BLACK_BRUSH);
        let sel = CreateSolidBrush(COLORREF(0x00F5C28A)); // accent for slider thumb / save

        let frame = |r: &RECT| { FrameRect(hdc, r, HBRUSH(black.0)); };
        let hline = |y: i32| { let r = RECT { left: 0, top: y, right: cw, bottom: y + 1 }; FillRect(hdc, &r, line_c); };
        let vline = |x: i32, y0: i32, y1: i32| { let r = RECT { left: x, top: y0, right: x + 1, bottom: y1 }; FillRect(hdc, &r, line_c); };

        // menu bar
        let mut m = band(cw, ch, 0.0, MENU_B); FillRect(hdc, &m, bar);
        SelectObject(hdc, st.f_label); SetTextColor(hdc, COLORREF(0x00000000));
        let mut mt = wide("  File      Edit      View      Record      Tools      Help");
        DrawTextW(hdc, &mut mt, &mut m, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        hline((ch as f64 * MENU_B) as i32);
        // toolbar
        let mut tb = band(cw, ch, MENU_B, TOOL_B); FillRect(hdc, &tb, face);
        let mut tt = wide("  New    Open    Save    Delete    │    ◀ Prev    Next ▶    │    Find");
        DrawTextW(hdc, &mut tt, &mut tb, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        hline((ch as f64 * TOOL_B) as i32);

        // ── left form ──
        let label = |r: [f64; 4], text: &str| {
            let lr = RECT { left: (cw as f64 * 0.01) as i32, top: (ch as f64 * r[1]) as i32,
                            right: (cw as f64 * (r[0] - 0.005)) as i32, bottom: (ch as f64 * r[3]) as i32 };
            FillRect(hdc, &lr, labelbg);
            let mut t = wide(text);
            let mut tr = RECT { left: lr.left + 6, ..lr };
            DrawTextW(hdc, &mut t, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        };
        let dropdown = |r: [f64; 4], val: &str| {
            let bx = fr(cw, ch, r); FillRect(hdc, &bx, white); frame(&bx);
            let mut tr = bx; tr.left += 6; tr.right -= 24; let mut t = wide(val);
            DrawTextW(hdc, &mut t, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            let ar = RECT { left: bx.right - 20, top: bx.top, right: bx.right, bottom: bx.bottom };
            FillRect(hdc, &ar, bar); frame(&ar); let mut a = wide("▼");
            DrawTextW(hdc, &mut a, &mut ar.clone(), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        };
        let slider = |r: [f64; 4], frac: f64| {
            let bx = fr(cw, ch, r); let midy = (bx.top + bx.bottom) / 2;
            let track = RECT { left: bx.left + 4, top: midy - 1, right: bx.right - 4, bottom: midy + 1 }; FillRect(hdc, &track, line_c);
            let tx = bx.left + 4 + ((bx.right - bx.left - 8) as f64 * frac) as i32;
            let thumb = RECT { left: tx - 4, top: bx.top + 6, right: tx + 4, bottom: bx.bottom - 6 }; FillRect(hdc, &thumb, sel); frame(&thumb);
        };
        SetTextColor(hdc, COLORREF(0x00000000));
        label(NAME, "Account Name"); label(TYPE, "Account Type"); label(REGION, "Region");
        label(PRIO, "Priority"); label(CREDIT, "Credit Limit");

        if st.mode == Mode::Gdi {
            let n = fr(cw, ch, NAME); FillRect(hdc, &n, white); frame(&n);
            let mut tr = n; tr.left += 6; let mut nm = wide(&st.name);
            DrawTextW(hdc, &mut nm, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            let sv = fr(cw, ch, SAVE); FillRect(hdc, &sv, sel); frame(&sv);
            let mut bt = wide("Add Record"); SelectObject(hdc, st.f_title);
            DrawTextW(hdc, &mut bt, &mut sv.clone(), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(hdc, st.f_label);
        }
        // dropdowns + sliders are visual in both modes
        dropdown(TYPE, "Enterprise"); dropdown(REGION, "North");
        slider(PRIO, 0.5); slider(CREDIT, 0.4);

        // ── signature doodle ──
        let dh = RECT { left: (cw as f64 * DOODLE[0]) as i32, top: (ch as f64 * 0.105) as i32,
                        right: (cw as f64 * DOODLE[2]) as i32, bottom: (ch as f64 * DOODLE[1]) as i32 };
        FillRect(hdc, &dh, hdr); let mut sh = wide(" Signature / Notes");
        SelectObject(hdc, st.f_title); DrawTextW(hdc, &mut sh, &mut dh.clone(), DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        SelectObject(hdc, st.f_label);
        let dz = fr(cw, ch, DOODLE); FillRect(hdc, &dz, white); frame(&dz);
        // line-tool segments (blue, 2px) + thin rubber-band preview while drawing
        let pen = CreatePen(PS_SOLID, 2, COLORREF(0x00701919));
        let old_pen = SelectObject(hdc, pen);
        for (a, b) in &st.segs { let _ = MoveToEx(hdc, a.x, a.y, None); let _ = LineTo(hdc, b.x, b.y); }
        if let Some(a) = st.down { let _ = MoveToEx(hdc, a.x, a.y, None); let _ = LineTo(hdc, st.cur.x, st.cur.y); }
        SelectObject(hdc, old_pen); let _ = DeleteObject(pen);

        // ── spreadsheet ──
        let g = fr(cw, ch, GRID); FillRect(hdc, &g, white); frame(&g);
        let cols = [("Account", 0.45), ("Type", 0.62), ("Region", 0.76), ("Pri", 0.85), ("Credit", 0.99)];
        let cx = |fx: f64| (cw as f64 * (GRID[0] + (GRID[2] - GRID[0]) * ((fx - 0.0) / 1.0))) as i32; // placeholder
        let _ = cx;
        // column x positions in absolute (fractions are of full width but cols listed as cumulative within grid)
        let gx = |fx: f64| (cw as f64 * fx) as i32;
        let row_h = ((st.ch as f64 / 44.0).clamp(10.0, 16.0) * 1.7) as i32;
        // header
        let hrow = RECT { left: g.left, top: g.top, right: g.right, bottom: g.top + row_h }; FillRect(hdc, &hrow, hdr);
        SelectObject(hdc, st.f_label);
        let mut prev = g.left + 6;
        for (name, fx) in cols { let mut t = wide(name); let mut tr = RECT { left: prev, top: g.top, right: gx(fx), bottom: g.top + row_h };
            DrawTextW(hdc, &mut t, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE); vline(gx(fx), g.top, g.bottom); prev = gx(fx) + 6; }
        hline(g.top + row_h);
        // rows
        SelectObject(hdc, st.f_mono);
        let mut y = g.top + row_h + 2;
        for rec in &st.records {
            if y + row_h > g.bottom { break; }
            let xs = [g.left + 6, gx(0.45) + 6, gx(0.62) + 6, gx(0.76) + 6, gx(0.85) + 6];
            for (i, val) in rec.iter().enumerate() { line(hdc, xs[i], y + 2, val); }
            y += row_h; hline(y);
        }

        // ── status bar ──
        let mut sb = band(cw, ch, STATUS_T, 1.0); FillRect(hdc, &sb, bar); hline((ch as f64 * STATUS_T) as i32);
        SelectObject(hdc, st.f_label); SetTextColor(hdc, COLORREF(0x00000000));
        let mut sbt = wide(&format!("  Ready    │    Records: {}    │    USER: SYSTEM  ▌ CONNECTED    │    [{}]", st.records.len(), st.node));
        DrawTextW(hdc, &mut sbt, &mut sb, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        for o in [face, bar, hdr, white, line_c, labelbg, sel] { let _ = DeleteObject(o); }
    });
    let _ = EndPaint(hwnd, &ps);
}
