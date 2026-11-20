// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Issue #7613: a daemon without the cursor overlay parks its main thread in
//! `serve_main_queue`, and work sent to the main dispatch queue (the Finder
//! folder handoff) must still run there, on the real main thread.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use std::ffi::c_void;
    use std::sync::mpsc;
    use std::time::Duration;

    extern "C" {
        static _dispatch_main_q: c_void;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
        fn pthread_main_np() -> i32;
    }

    extern "C" fn on_main(context: *mut c_void) {
        let tx = unsafe { Box::from_raw(context as *mut mpsc::SyncSender<bool>) };
        let _ = tx.send(unsafe { pthread_main_np() } == 1);
    }

    std::thread::spawn(|| {
        for _ in 0..2 {
            let (tx, rx) = mpsc::sync_channel(1);
            unsafe {
                dispatch_async_f(
                    &raw const _dispatch_main_q,
                    Box::into_raw(Box::new(tx)) as *mut c_void,
                    on_main,
                );
            }
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(true) => {}
                Ok(false) => {
                    eprintln!("main_queue_without_overlay: block ran off the main thread");
                    std::process::exit(1);
                }
                Err(error) => {
                    eprintln!("main_queue_without_overlay: main queue not served: {error}");
                    std::process::exit(1);
                }
            }
        }
        println!("main_queue_without_overlay: passed");
        std::process::exit(0);
    });
    platform_macos::cursor::overlay::serve_main_queue();
}
