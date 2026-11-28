// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Background `invoke_menu` on a classic Win32 menu bar (#6823).
//!
//! The test process owns a window whose menu item is grayed until the window
//! handles `WM_INITMENU`, as apps that update their menus when the menu
//! activates do. A background `invoke_menu` must deliver that item's
//! `WM_COMMAND` without ever activating the window, or the `WM_MENUCOMMAND`
//! the menu asks for when the same handler gives the bar the
//! `MNS_NOTIFYBYPOS` style.
//!
//! `#[ignore]`: it needs a real desktop session. Run explicitly:
//!   cargo test -p cua-driver-e2e --test win32_menu_windows_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cua_driver_testkit::{Driver, McpDriver};
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    EnableMenuItem, GetMenuItemID, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetMenu, SetMenuInfo, ShowWindow, TranslateMessage, CW_USEDEFAULT, HMENU, MENUINFO,
    MF_BYCOMMAND, MF_ENABLED, MF_GRAYED, MF_POPUP, MF_STRING, MIM_STYLE, MNS_NOTIFYBYPOS, MSG,
    SW_SHOWNOACTIVATE, WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_INITMENU, WM_MENUCOMMAND, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const ID_GATED: u32 = 0x4823;

static GATE_MENU: AtomicIsize = AtomicIsize::new(0);
static INIT_MENUS: AtomicUsize = AtomicUsize::new(0);
static ACTIVATIONS: AtomicUsize = AtomicUsize::new(0);
static COMMAND: AtomicU32 = AtomicU32::new(0);
static MENU_COMMAND: AtomicU32 = AtomicU32::new(0);
static NOTIFY_BY_POSITION_ON_INIT: AtomicBool = AtomicBool::new(false);
static CLASS: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

unsafe extern "system" fn menu_window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_INITMENU => {
            INIT_MENUS.fetch_add(1, Ordering::SeqCst);
            let gate = HMENU(GATE_MENU.load(Ordering::SeqCst) as *mut _);
            let _ = EnableMenuItem(gate, ID_GATED, MF_BYCOMMAND | MF_ENABLED);
            if NOTIFY_BY_POSITION_ON_INIT.load(Ordering::SeqCst) {
                let style = MENUINFO {
                    cbSize: std::mem::size_of::<MENUINFO>() as u32,
                    fMask: MIM_STYLE,
                    dwStyle: MNS_NOTIFYBYPOS,
                    ..Default::default()
                };
                let _ = SetMenuInfo(HMENU(wparam.0 as *mut _), &style);
            }
            LRESULT(0)
        }
        WM_MENUCOMMAND => {
            let id = GetMenuItemID(HMENU(lparam.0 as *mut _), wparam.0 as i32);
            MENU_COMMAND.store(id, Ordering::SeqCst);
            LRESULT(0)
        }
        WM_COMMAND => {
            COMMAND.store((wparam.0 & 0xFFFF) as u32, Ordering::SeqCst);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xFFFF) as u32 != WA_INACTIVE {
                ACTIVATIONS.fetch_add(1, Ordering::SeqCst);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// Create the menu window, shown without activation, and pump its messages
/// until it closes.
fn run_menu_window(ready: std::sync::mpsc::Sender<usize>) {
    unsafe {
        let module = GetModuleHandleW(windows::core::PCWSTR::null()).expect("module handle");
        let instance = HINSTANCE(module.0);
        let class = w!("CuaWin32MenuFixture");
        let registered = *CLASS.get_or_init(|| {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(menu_window),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            })
        });
        assert_ne!(registered, 0, "RegisterClassW failed");
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Cua Win32 menu fixture"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            480,
            320,
            None,
            None,
            Some(instance),
            None,
        )
        .expect("CreateWindowExW");
        let bar = CreateMenu().expect("CreateMenu");
        let gate = CreatePopupMenu().expect("CreatePopupMenu");
        GATE_MENU.store(gate.0 as isize, Ordering::SeqCst);
        AppendMenuW(
            gate,
            MF_STRING | MF_GRAYED,
            ID_GATED as usize,
            w!("&Gated\tCtrl+G"),
        )
        .expect("append Gated");
        AppendMenuW(bar, MF_POPUP, gate.0 as usize, w!("&Gate")).expect("append Gate");
        SetMenu(hwnd, Some(bar)).expect("SetMenu");
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        ready.send(hwnd.0 as usize).expect("send hwnd");

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// What the menu window received for one background `invoke_menu` of
/// Gate > Gated.
struct Delivery {
    command: u32,
    menu_command: u32,
}

fn invoke_gated_in_background(notify_by_position_on_init: bool) -> Delivery {
    NOTIFY_BY_POSITION_ON_INIT.store(notify_by_position_on_init, Ordering::SeqCst);
    INIT_MENUS.store(0, Ordering::SeqCst);
    ACTIVATIONS.store(0, Ordering::SeqCst);
    COMMAND.store(0, Ordering::SeqCst);
    MENU_COMMAND.store(0, Ordering::SeqCst);

    let (ready, window) = std::sync::mpsc::channel();
    let pump = std::thread::spawn(move || run_menu_window(ready));
    let hwnd = window
        .recv_timeout(Duration::from_secs(10))
        .expect("menu window was not created");

    let mut driver = McpDriver::spawn_named("win32-menu-background")
        .expect("build cua-driver or set CUA_TEST_DRIVER_BIN");
    let result = driver.call(
        "invoke_menu",
        serde_json::json!({
            "pid": std::process::id(),
            "window_id": hwnd as u64,
            "path": ["Gate", "Gated"]
        }),
    );
    assert!(!result.is_error(), "invoke_menu failed: {}", result.raw);
    assert_eq!(
        result.action_delivery_mode(),
        Some("background"),
        "{}",
        result.raw
    );
    assert_eq!(
        result.action_route(),
        Some("synthetic_events"),
        "{}",
        result.raw
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while COMMAND.load(Ordering::SeqCst) == 0
        && MENU_COMMAND.load(Ordering::SeqCst) == 0
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    // Leave time for a second, wrong message to arrive as well.
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        INIT_MENUS.load(Ordering::SeqCst) >= 1,
        "the menu bar was read without WM_INITMENU"
    );
    assert_eq!(
        ACTIVATIONS.load(Ordering::SeqCst),
        0,
        "background invoke_menu activated the window"
    );

    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    pump.join().expect("menu window thread panicked");
    Delivery {
        command: COMMAND.load(Ordering::SeqCst),
        menu_command: MENU_COMMAND.load(Ordering::SeqCst),
    }
}

#[test]
#[ignore]
fn background_invoke_menu_posts_a_command_gated_on_wm_initmenu() {
    let delivery = invoke_gated_in_background(false);
    assert_eq!(
        delivery.command, ID_GATED,
        "the item enabled in WM_INITMENU never received its WM_COMMAND"
    );
    assert_eq!(
        delivery.menu_command, 0,
        "a WM_COMMAND menu got WM_MENUCOMMAND"
    );
}

#[test]
#[ignore]
fn background_invoke_menu_honors_notify_by_position_set_in_wm_initmenu() {
    let delivery = invoke_gated_in_background(true);
    assert_eq!(
        delivery.menu_command, ID_GATED,
        "the MNS_NOTIFYBYPOS menu never received WM_MENUCOMMAND for the item"
    );
    assert_eq!(
        delivery.command, 0,
        "an MNS_NOTIFYBYPOS menu got WM_COMMAND"
    );
}
