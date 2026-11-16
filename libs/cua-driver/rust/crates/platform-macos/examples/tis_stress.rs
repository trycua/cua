// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Stress the keyboard-layout lookup that `press_key` and `hotkey` run, the
//! way the daemon runs it: AppKit's event loop on the main thread, key
//! lookups on worker threads, and input-source switches in between.
//!
//! ```text
//! cargo run --release -p platform-macos --example tis_stress -- \
//!     [--threads 8] [--calls 500] [--switch com.apple.keylayout.French] \
//!     [--switch-every 25] [--no-appkit] [--raw]
//! tis_stress --switch-only --switch <layout id> [--switch-count 40] [--interval-ms 250]
//!     [--until-file <path>]   (stop early once the file exists)
//! ```
//!
//! `--raw` calls Text Input Sources directly on the workers, as the driver
//! did before CUA-3763: in a GUI session that aborts or traps (exit 134/133).
//! Without it, every lookup goes through `layout_key_code`. Run it from a
//! GUI session (Terminal.app), not over ssh: TSM only enforces its thread
//! rules for a process in the user's Aqua session. Exits 0 and prints the
//! call count when every lookup returned.

#[cfg(target_os = "macos")]
fn main() {
    stress::main();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("tis_stress runs on macOS only");
}

#[cfg(target_os = "macos")]
mod stress {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    use platform_macos::input::input_source;
    use platform_macos::input::keyboard::layout_key_code;

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        static kTISPropertyInputSourceID: *const c_void;
        static kTISPropertyUnicodeKeyLayoutData: *const c_void;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
        fn TISCopyCurrentKeyboardInputSource() -> *const c_void;
        fn TISGetInputSourceProperty(source: *const c_void, key: *const c_void) -> *const c_void;
        fn TISCreateInputSourceList(properties: *const c_void, include_all: bool) -> *const c_void;
        fn TISEnableInputSource(source: *const c_void) -> i32;
        fn TISSelectInputSource(source: *const c_void) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: *const c_void);
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
    }

    fn arg(args: &[String], name: &str) -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    }

    /// The old, unmarshalled lookup: TIS straight from the calling thread.
    fn raw_lookup() {
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if !source.is_null() {
                let _ = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
                CFRelease(source);
            }
        }
    }

    fn source_id(source: *const c_void) -> String {
        use core_foundation::base::TCFType;
        use core_foundation::string::{CFString, CFStringRef};
        unsafe {
            let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID);
            if id.is_null() {
                return String::new();
            }
            CFString::wrap_under_get_rule(id as CFStringRef).to_string()
        }
    }

    /// Current input source id. TIS: call through `input_source::run`.
    fn current_source_id() -> String {
        unsafe {
            let source = TISCopyCurrentKeyboardInputSource();
            let id = source_id(source);
            CFRelease(source);
            id
        }
    }

    /// Enable and select the input source `id`. TIS: call through
    /// `input_source::run`.
    fn select_source(id: &str) -> bool {
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;
        unsafe {
            let value = CFString::new(id);
            let keys = [kTISPropertyInputSourceID];
            let values = [value.as_concrete_TypeRef() as *const c_void];
            let filter = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks as *const c_void,
                &raw const kCFTypeDictionaryValueCallBacks as *const c_void,
            );
            let list = TISCreateInputSourceList(filter, true);
            CFRelease(filter);
            if list.is_null() {
                return false;
            }
            let ok = CFArrayGetCount(list) > 0 && {
                let source = CFArrayGetValueAtIndex(list, 0);
                TISEnableInputSource(source);
                TISSelectInputSource(source) == 0
            };
            CFRelease(list);
            ok
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        let threads: usize = arg(&args, "--threads")
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        let calls: usize = arg(&args, "--calls")
            .and_then(|v| v.parse().ok())
            .unwrap_or(500);
        let switch_to = arg(&args, "--switch");
        let switch_every: usize = arg(&args, "--switch-every")
            .and_then(|v| v.parse().ok())
            .unwrap_or(25);
        let raw = args.iter().any(|a| a == "--raw");
        let appkit = !args.iter().any(|a| a == "--no-appkit");

        if args.iter().any(|a| a == "--switch-only") {
            // Switch layouts for a daemon under test; this process stays non-UI.
            let target = switch_to.expect("--switch-only needs --switch <id>");
            let count: usize = arg(&args, "--switch-count")
                .and_then(|v| v.parse().ok())
                .unwrap_or(40);
            let pause = std::time::Duration::from_millis(
                arg(&args, "--interval-ms")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(250),
            );
            let original = input_source::run(current_source_id).unwrap_or_default();
            let mut switches = 0;
            let until = arg(&args, "--until-file").map(std::path::PathBuf::from);
            for i in 0..count {
                if until.as_ref().is_some_and(|path| path.exists()) {
                    break;
                }
                let id = if i.is_multiple_of(2) {
                    target.clone()
                } else {
                    original.clone()
                };
                if input_source::run(move || select_source(&id)) == Some(true) {
                    switches += 1;
                }
                std::thread::sleep(pause);
            }
            input_source::run(move || select_source(&original));
            println!("OK switches={switches}");
            return;
        }
        let done = Arc::new(AtomicUsize::new(0));
        let switches = Arc::new(AtomicUsize::new(0));
        let started = Instant::now();
        eprintln!(
        "tis_stress: threads={threads} calls={calls} raw={raw} appkit={appkit} switch={switch_to:?}"
    );

        let worker = {
            let done = Arc::clone(&done);
            let switches = Arc::clone(&switches);
            move || {
                // `--raw` must not warm TSM up on the main queue first.
                let original = if raw {
                    current_source_id()
                } else {
                    input_source::run(current_source_id).unwrap_or_default()
                };
                let handles: Vec<_> = (0..threads)
                    .map(|t| {
                        let done = Arc::clone(&done);
                        let switches = Arc::clone(&switches);
                        let switch_to = switch_to.clone();
                        let original = original.clone();
                        std::thread::spawn(move || {
                            for i in 0..calls.div_ceil(threads) {
                                if t == 0 && i > 0 && i.is_multiple_of(switch_every) {
                                    if let Some(target) = &switch_to {
                                        let id =
                                            if switches.load(Ordering::Relaxed).is_multiple_of(2) {
                                                target.clone()
                                            } else {
                                                original.clone()
                                            };
                                        if raw {
                                            select_source(&id);
                                        } else {
                                            input_source::run(move || select_source(&id));
                                        }
                                        switches.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                if raw {
                                    raw_lookup();
                                } else {
                                    layout_key_code("a", &["cmd"]).expect("key a");
                                }
                                done.fetch_add(1, Ordering::Relaxed);
                            }
                        })
                    })
                    .collect();
                for handle in handles {
                    handle.join().expect("worker");
                }
                if switch_to.is_some() && !original.is_empty() {
                    input_source::run(move || select_source(&original));
                }
                println!(
                    "OK calls={} switches={} elapsed_ms={}",
                    done.load(Ordering::Relaxed),
                    switches.load(Ordering::Relaxed),
                    started.elapsed().as_millis()
                );
                std::process::exit(0);
            }
        };

        if !appkit {
            worker();
            return;
        }
        // Same order as the daemon's cursor overlay: claim TIS for the main
        // thread, create NSApp, then run its event loop forever.
        if !raw {
            input_source::main_thread_runs_event_loop();
        }
        std::thread::spawn(worker);
        unsafe {
            use objc2::runtime::AnyObject;
            use objc2::{class, msg_send};
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let _: bool = msg_send![app, setActivationPolicy: 1i64];
            let _: () = msg_send![app, finishLaunching];
            let _: () = msg_send![app, run];
        }
    }
}
