#![cfg(target_os = "windows")]

//! RED repro - the Windows named-pipe accept loop in serve::run_serve
//! hard-bails on a failed server.connect() (ERROR_BROKEN_PIPE 109 or
//! ERROR_PIPE_CLOSING 232 when a client opens, holds, and closes while the
//! server's ConnectNamedPipe is in flight). After the bail the pipe is
//! unserviceable ("daemon is not running") even though the daemon PID may
//! stay alive.
//!
//! Mechanism (root-caused with a raw-C kernel probe, C:\mc\probe2/probe3):
//! a client that opens the pipe with FILE_FLAG_OVERLAPPED, then either vanishes
//! immediately or holds the handle briefly and closes, leaves the server's
//! in-flight ConnectNamedPipe dangling; the connect completes with 109/232 and
//! stock serve.rs bails via the question-mark. The probe reproduced this on
//! stock accept-loop semantics (fresh instance per connect, unlimited
//! instances) WITHOUT any extra RPC load, so this test uses only the
//! open-then-close / open-hold-close hammers and no load group.
//!
//! On main the post-hammer get_config fails (RED). With the accept-loop
//! recovery fix the daemon keeps serving through everything (GREEN).
//!
//! Anti-vacuous: the pre-hammer get_config proves the pipe was servable, and
//! the race group must have opened the pipe at least once, so a run that never
//! engages the accept loop cannot silently pass.
//!
//! Part of the red-green stack replacing trycua/cua#3359; the fix lands in the
//! follow-up PR.

use std::os::windows::ffi::OsStrExt;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::{CliDriver, Driver};

/// Total wall time of the race window.
const RACE_SECS: u64 = 30;
/// How long each "hold" race client keeps its handle after the overlapped
/// open before closing. Keeping the server's ConnectNamedPipe in flight during
/// the hold is what turns the close into a broken-pipe connect failure (the
/// probe3 trigger).
const HOLD_MS: u64 = 5;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_OVERLAPPED: u32 = 0x0200_0000;
const INVALID_HANDLE_VALUE: isize = -1;

#[allow(non_snake_case)]
extern "system" {
    fn CreateFileW(
        lpfile_name: *const u16,
        dwdesired_access: u32,
        dwshare_mode: u32,
        lpsecurity_attributes: *mut u8,
        dwcreation_disposition: u32,
        dwflags_and_attributes: u32,
        htemplate_file: *mut u8,
    ) -> isize;
    fn CloseHandle(hobject: *mut u8) -> i32;
    fn CancelIoEx(hfile: *mut u8, lpoverlapped: *mut u8) -> i32;
}

fn wpath(socket: &str) -> Vec<u16> {
    let mut v: Vec<u16> = socket.encode_utf16().collect();
    v.push(0);
    v
}

/// True while a cua-driver.exe process is running. Field signature: the PID
/// may live with the pipe dead, so this is reported (not asserted).
fn daemon_process_alive() -> bool {
    Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq cua-driver.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("cua-driver.exe"))
        .unwrap_or(false)
}

/// Overlapped open; Some(handle) on success.
fn overlapped_open(socket: &str) -> Option<isize> {
    let w = wpath(socket);
    unsafe {
        let h = CreateFileW(
            w.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(h)
        }
    }
}

/// Overlapped open + immediate CloseHandle: the client vanishes while the
/// server may be inside (or about to call) ConnectNamedPipe (probe1/2 style).
fn race_fast(socket: &str) -> bool {
    match overlapped_open(socket) {
        Some(h) => {
            unsafe { CloseHandle(h as *mut u8) };
            true
        }
        None => false,
    }
}

/// Overlapped open + CancelIoEx + CloseHandle: the tokio-IOCP drop path.
fn race_cancel(socket: &str) -> bool {
    match overlapped_open(socket) {
        Some(h) => unsafe {
            let _ = CancelIoEx(h as *mut u8, std::ptr::null_mut());
            CloseHandle(h as *mut u8);
            true
        },
        None => false,
    }
}

/// Overlapped open, hold the handle for HOLD_MS, then CloseHandle: the client
/// vanishes while the server's ConnectNamedPipe is in flight (probe3 style,
/// the most reproducible trigger).
fn race_hold(socket: &str) -> bool {
    match overlapped_open(socket) {
        Some(h) => {
            thread::sleep(Duration::from_millis(HOLD_MS));
            unsafe { CloseHandle(h as *mut u8) };
            true
        }
        None => false,
    }
}

#[test]
fn accept_loop_survives_connect_disconnect_races() {
    let mut driver = CliDriver::new();
    assert!(driver.available(), "test daemon failed to start");
    let socket = driver.daemon_socket().expect("test daemon socket").to_string();

    // 0. Serviceable before the hammer (anti-vacuous: the pipe was engaged).
    let before = driver.call("get_config", serde_json::json!({}));
    assert!(!before.is_error(), "pre-hammer get_config failed: {}", before.text());

    // 1. The race group: four hammer styles racing the accept loop for the
    // whole window. No RPC load group: the C probe reproduced the connect
    // failure with no load, and stable load clients consume the pending pipe
    // instance and mask the race (v11 did not trigger with a load group).
    let fast_opened = Arc::new(AtomicU64::new(0));
    let cancel_opened = Arc::new(AtomicU64::new(0));
    let hold_opened = Arc::new(AtomicU64::new(0));
    let hold1 = Arc::clone(&hold_opened);
    let hold2 = Arc::clone(&hold_opened);
    let deadline = Instant::now() + Duration::from_secs(RACE_SECS);
    thread::scope(|s| {
        let f = Arc::clone(&fast_opened);
        let c = Arc::clone(&cancel_opened);
        let s1 = socket.clone();
        let s2 = socket.clone();
        let s3 = socket.clone();
        let s4 = socket.clone();
        s.spawn(move || {
            let mut local = 0u64;
            while Instant::now() < deadline {
                if race_fast(&s1) { local += 1; } else { thread::sleep(Duration::from_millis(5)); }
            }
            f.fetch_add(local, Ordering::Relaxed);
        });
        s.spawn(move || {
            let mut local = 0u64;
            while Instant::now() < deadline {
                if race_cancel(&s2) { local += 1; } else { thread::sleep(Duration::from_millis(5)); }
            }
            c.fetch_add(local, Ordering::Relaxed);
        });
        // Two hold threads: the probe3-proven trigger.
        s.spawn(move || {
            let h = hold1;
            let mut local = 0u64;
            while Instant::now() < deadline {
                if race_hold(&s3) { local += 1; } else { thread::sleep(Duration::from_millis(5)); }
            }
            h.fetch_add(local, Ordering::Relaxed);
        });
        s.spawn(move || {
            let h = hold2;
            let mut local = 0u64;
            while Instant::now() < deadline {
                if race_hold(&s4) { local += 1; } else { thread::sleep(Duration::from_millis(5)); }
            }
            // Add to the same hold counter.
            h.fetch_add(local, Ordering::Relaxed);
        });
    });
    let fast_opened = fast_opened.load(Ordering::Relaxed);
    let cancel_opened = cancel_opened.load(Ordering::Relaxed);
    let hold_opened = hold_opened.load(Ordering::Relaxed);
    eprintln!(
        "[piperecovery] {}s race (no load): fast {} / cancel {} / hold {} opens; daemon alive: {}",
        RACE_SECS, fast_opened, cancel_opened, hold_opened, daemon_process_alive()
    );

    // Anti-vacuous: the race group must have reached the pipe.
    assert!(
        fast_opened + cancel_opened + hold_opened >= 1,
        "race group never reached the pipe (fast {}, cancel {}, hold {})",
        fast_opened, cancel_opened, hold_opened
    );

    // 2. Still serviceable? On main this is the RED assertion: a dead accept
    // loop leaves the pipe unserviceable while the daemon PID may live.
    let r = driver.call("get_config", serde_json::json!({}));
    assert!(
        !r.is_error(),
        "accept loop died under connect/disconnect race (fast {}, cancel {}, hold {}, daemon alive: {}); get_config: {}",
        fast_opened, cancel_opened, hold_opened, daemon_process_alive(), r.text()
    );

    eprintln!("[piperecovery] survived race; daemon alive: {}", daemon_process_alive());
}



