// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use super::running_app_pids;
use crate::win32::WindowInfo;

fn window(pid: u32, hwnd: u64) -> WindowInfo {
    WindowInfo {
        hwnd,
        pid,
        title: String::new(),
        x: 0,
        y: 0,
        width: 100,
        height: 100,
        is_on_screen: true,
        minimized: false,
    }
}

#[test]
fn hosted_apps_follow_their_frame_host_once_each() {
    // Host pid 100's frames 7 and 8 host app pids 200 and 300; frame 9 hosts
    // nothing the platform can attribute. Pid 42 is a plain window.
    let windows = [
        window(42, 1),
        window(100, 7),
        window(100, 8),
        window(100, 9),
        window(42, 2),
        window(100, 7),
    ];
    let hosted = |pid: u32, hwnd: u64| match (pid, hwnd) {
        (100, 7) => Some(200),
        (100, 8) => Some(300),
        _ => None,
    };
    assert_eq!(running_app_pids(&windows, hosted), vec![42, 100, 200, 300]);
}
