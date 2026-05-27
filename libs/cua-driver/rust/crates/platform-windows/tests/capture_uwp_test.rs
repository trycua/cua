#![cfg(target_os = "windows")]

//! Ignored integration coverage for UWP / WinUI screenshot capture.
//!
//! Run manually on an interactive Windows 10 1903+ / Windows 11 desktop:
//!
//!   cargo test -p platform-windows --test capture_uwp_test -- --ignored --nocapture

use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use platform_windows::{capture, launch_uwp, wgc, win32::windows::WindowInfo};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MINIMIZE, SW_RESTORE};

const CALCULATOR_AUMID: &str = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";

fn launch_calculator_and_wait_for_window() -> Result<WindowInfo> {
    let _pid = launch_uwp::launch_uwp(CALCULATOR_AUMID, "")?;
    let deadline = Instant::now() + Duration::from_secs(8);

    while Instant::now() < deadline {
        if let Some(window) = platform_windows::win32::windows::list_windows(None)
            .into_iter()
            .find(|w| w.title.to_ascii_lowercase().contains("calculator"))
        {
            return Ok(window);
        }
        sleep(Duration::from_millis(100));
    }

    bail!("Calculator window did not appear after launching {CALCULATOR_AUMID}");
}

fn assert_real_calculator_capture(png: &[u8]) -> Result<()> {
    let (w, h) = capture::png_dimensions_pub(png)?;
    assert!(
        w > 120 && h > 30,
        "expected real Calculator UI dimensions, got collapsed title-bar size {w}x{h}"
    );
    assert!(
        h > 120,
        "expected Calculator content area to be captured, got only {w}x{h}"
    );
    Ok(())
}

#[test]
#[ignore]
fn background_launched_calculator_screenshot_uses_wgc() -> Result<()> {
    let window = launch_calculator_and_wait_for_window()?;
    let png = capture::screenshot_window_bytes(window.hwnd)?;
    assert_real_calculator_capture(&png)
}

#[test]
#[ignore]
fn minimized_calculator_wgc_attempts_composited_capture() -> Result<()> {
    let window = launch_calculator_and_wait_for_window()?;
    let hwnd = HWND(window.hwnd as *mut _);

    unsafe {
        let _ = ShowWindow(hwnd, SW_MINIMIZE);
    }
    sleep(Duration::from_millis(500));

    let result = wgc::screenshot_window_via_wgc(window.hwnd);

    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }

    let (pixels, w, h) = result?;
    assert!(
        w > 120 && h > 30,
        "expected real Calculator UI dimensions from minimized WGC capture, got {w}x{h}"
    );
    assert_eq!(pixels.len(), w as usize * h as usize * 4);
    Ok(())
}
