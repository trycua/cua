//! Capture the daemon's overlay window content directly via PrintWindow.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
fn main() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    // Send the daemon a move command first.
    let mut pipe = std::fs::OpenOptions::new()
        .read(true).write(true)
        .open(r"\\.\pipe\cua-driver")
        .expect("open pipe");
    fn req(p: &mut std::fs::File, json: &str) {
        p.write_all(format!("{json}\n").as_bytes()).unwrap();
        p.flush().ok();
        let mut buf = [0u8; 8192];
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut out = Vec::new();
        loop {
            if Instant::now() > deadline { break; }
            let n = p.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            out.extend_from_slice(&buf[..n]);
            if out.contains(&b'\n') { break; }
        }
    }
    req(&mut pipe, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
    req(&mut pipe, r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"set_agent_cursor_enabled","arguments":{"enabled":true}}}"#);
    req(&mut pipe, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"move_cursor","arguments":{"x":700.0,"y":500.0}}}"#);

    std::thread::sleep(Duration::from_millis(1500));

    // Find overlay HWND.
    let class_name: Vec<u16> = OsStr::new("TropeCUA.AgentCursorOverlay\0")
        .encode_wide().collect();
    let hwnd = unsafe { FindWindowW(
        windows::core::PCWSTR(class_name.as_ptr()),
        windows::core::PCWSTR::null(),
    ) };
    let hwnd = match hwnd { Ok(h) => h, Err(_) => { eprintln!("no overlay window"); std::process::exit(1); } };
    println!("overlay HWND: 0x{:X}", hwnd.0 as isize);

    let mut rect = windows::Win32::Foundation::RECT::default();
    unsafe { let _ = GetWindowRect(hwnd, &mut rect); }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    println!("rect: {},{} {}x{}", rect.left, rect.top, w, h);

    // PrintWindow capture.
    unsafe {
        let screen_dc = GetDC(HWND::default());
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
        let old = SelectObject(mem_dc, bitmap);
        // PW_RENDERFULLCONTENT = 0x2 — needed for layered windows
        let ok = PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(0x2));
        println!("PrintWindow returned: {}", ok.as_bool());

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (w * h * 4) as u32,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let got = GetDIBits(mem_dc, bitmap, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        println!("GetDIBits scanlines: {}", got);

        // Find non-zero pixels.
        let mut nonzero = 0u64;
        let mut sum_x = 0i64; let mut sum_y = 0i64;
        let mut max_a = 0u8;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 4;
                let b = pixels[i]; let g = pixels[i+1]; let r = pixels[i+2]; let a = pixels[i+3];
                max_a = max_a.max(a);
                if r as u32 + g as u32 + b as u32 > 30 {
                    nonzero += 1;
                    sum_x += x as i64; sum_y += y as i64;
                }
            }
        }
        println!("non-zero RGB pixels in overlay surface: {} max_alpha={}", nonzero, max_a);
        if nonzero > 0 {
            println!("centroid: ({}, {})", sum_x / nonzero as i64, sum_y / nonzero as i64);
        }

        SelectObject(mem_dc, old);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND::default(), screen_dc);
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {}
