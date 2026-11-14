// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! AppKit main loop helper for `serve` when the cursor overlay is off.

/// Park the main thread in `NSApplication.run()`. Used by `cua-driver serve`
/// whenever the cursor overlay is off and the process has a graphic session,
/// so work sent to the main queue (the drag-and-drop pointer-hold shield) is
/// drained. Mirrors the cursor overlay's `run_appkit` startup (Accessory
/// activation policy, finishLaunching, run) without installing the overlay's
/// window.
///
/// Never returns: the background serve thread calls `std::process::exit`
/// when it finishes, which tears down NSApp at the same time.
pub fn run_appkit_main_loop() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let _mtm = objc2_foundation::MainThreadMarker::new()
        .expect("run_appkit_main_loop must be called from the main thread");
    // AppKit uses Text Input Sources on this thread from here on.
    crate::input::input_source::main_thread_runs_event_loop();
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        // Accessory policy: no Dock icon, no menu bar.
        let _: bool = msg_send![app, setActivationPolicy: 1i64];
        let _: () = msg_send![app, finishLaunching];
        let _: () = msg_send![app, run];
    }
}
