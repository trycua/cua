#![cfg(target_os = "windows")]

//! An elevated Windows Driver must not hand its administrator token to the
//! apps it launches (#3607). Both `launch_app` routes are covered: the shell
//! route (`ShellExecuteExW` in the standard-user helper) and the direct
//! `start_minimized` executable route. Each launched process must run at
//! Medium integrity when the Driver is elevated, and at the Driver's own
//! integrity level when it is not.
//!
//! GitHub-hosted Windows runners run tests elevated, so CI exercises the
//! standard-user path.

use std::ffi::c_void;
use std::path::PathBuf;

use cua_driver_testkit::{Driver, McpDriver};

type Handle = *mut c_void;

const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_INTEGRITY_LEVEL: u32 = 25;
const MEDIUM_INTEGRITY_RID: u32 = 0x2000;
const HIGH_INTEGRITY_RID: u32 = 0x3000;

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
    fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: Handle, desired_access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        information_class: u32,
        information: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> i32;
    fn GetSidSubAuthorityCount(sid: *mut c_void) -> *mut u8;
    fn GetSidSubAuthority(sid: *mut c_void, index: u32) -> *mut u32;
}

/// Mandatory integrity RID of `process`.
fn integrity_rid(process: Handle) -> u32 {
    unsafe {
        let mut token: Handle = std::ptr::null_mut();
        assert!(
            OpenProcessToken(process, TOKEN_QUERY, &mut token) != 0,
            "OpenProcessToken failed: {}",
            std::io::Error::last_os_error()
        );
        let mut needed = 0u32;
        GetTokenInformation(
            token,
            TOKEN_INTEGRITY_LEVEL,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        let mut label = vec![0u64; (needed as usize).div_ceil(8)];
        let ok = GetTokenInformation(
            token,
            TOKEN_INTEGRITY_LEVEL,
            label.as_mut_ptr().cast(),
            (label.len() * 8) as u32,
            &mut needed,
        );
        CloseHandle(token);
        assert!(
            ok != 0,
            "GetTokenInformation(TokenIntegrityLevel) failed: {}",
            std::io::Error::last_os_error()
        );
        // TOKEN_MANDATORY_LABEL starts with the label SID pointer.
        let sid = *label.as_ptr().cast::<*mut c_void>();
        let count = *GetSidSubAuthorityCount(sid);
        *GetSidSubAuthority(sid, u32::from(count) - 1)
    }
}

fn ping() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
        .join("System32")
        .join("PING.EXE")
}

#[test]
#[ignore = "launches a console process on the interactive desktop"]
fn elevated_driver_launches_apps_with_a_standard_user_token() {
    let driver_rid = integrity_rid(unsafe { GetCurrentProcess() });
    let mut driver = McpDriver::spawn_with_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
        ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
    ])
    .expect("test Driver failed to start");
    let ping = ping();

    for (route, start_minimized) in [("shell", false), ("direct", true)] {
        let launch = driver.call(
            "launch_app",
            serde_json::json!({
                "path": ping,
                "additional_arguments": ["-n", "60", "127.0.0.1"],
                "start_minimized": start_minimized,
            }),
        );
        if start_minimized && launch.structured()["code"] == "background_unavailable" {
            eprintln!(
                "[launch_app_standard_user] direct route not exercised: {}",
                launch.text()
            );
            continue;
        }
        assert!(
            !launch.is_error(),
            "{route} launch failed: {}",
            launch.text()
        );
        let pid = launch.structured()["pid"].as_u64().unwrap_or(0) as u32;
        assert_ne!(pid, 0, "{route} launch reported no pid: {}", launch.text());

        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                0,
                pid,
            )
        };
        assert!(
            !process.is_null(),
            "could not open launched pid {pid}: {}",
            std::io::Error::last_os_error()
        );
        let launched_rid = integrity_rid(process);
        unsafe {
            TerminateProcess(process, 0);
            CloseHandle(process);
        }
        eprintln!(
            "[launch_app_standard_user] {route}: Driver 0x{driver_rid:04x}, launched pid {pid} \
             0x{launched_rid:04x}"
        );
        if driver_rid >= HIGH_INTEGRITY_RID {
            assert!(
                launched_rid <= MEDIUM_INTEGRITY_RID,
                "{route} launch from an elevated Driver ran at integrity 0x{launched_rid:04x}"
            );
        } else {
            assert_eq!(
                launched_rid, driver_rid,
                "{route} launch from a non-elevated Driver changed integrity"
            );
        }
    }
}
