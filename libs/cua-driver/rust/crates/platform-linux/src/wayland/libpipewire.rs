// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! libpipewire, loaded when a portal recording starts instead of linked.
//!
//! Linking it would stop cua-driver and the SDK library from loading at all on
//! hosts without PipeWire, although only portal ScreenCast video uses it. The
//! `spa_*` helpers the capture also calls are header-only and compiled in.

use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::{Mutex, PoisonError};

use libspa::sys::{spa_dict, spa_direction, spa_hook, spa_pod};
use pipewire_sys::{
    pw_buffer, pw_context, pw_core, pw_loop, pw_main_loop, pw_properties, pw_proxy, pw_stream,
    pw_stream_events, pw_stream_flags,
};

const SONAME: &str = "libpipewire-0.3.so.0";

macro_rules! entry_points {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// The libpipewire functions the portal capture calls.
        pub(crate) struct PipeWire {
            $(pub(crate) $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            _library: libloading::Library,
        }

        impl PipeWire {
            fn open(soname: &str) -> Result<Self, libloading::Error> {
                // SAFETY: libpipewire has no load-time initialisers with preconditions.
                let library = unsafe { libloading::Library::new(soname) }?;
                $(
                    // Fails to compile if the type drifts from pipewire-sys's prototype.
                    const _: unsafe extern "C" fn($($arg),*) $(-> $ret)? = pipewire_sys::$name;
                    // SAFETY: the symbol has the type checked above.
                    let $name = *unsafe {
                        library.get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(
                            concat!(stringify!($name), "\0").as_bytes(),
                        )
                    }?;
                )*
                Ok(Self { $($name,)* _library: library })
            }
        }
    };
}

entry_points! {
    pw_init: fn(*mut c_int, *mut *mut *mut c_char);
    pw_main_loop_new: fn(*const spa_dict) -> *mut pw_main_loop;
    pw_main_loop_get_loop: fn(*mut pw_main_loop) -> *mut pw_loop;
    pw_main_loop_run: fn(*mut pw_main_loop) -> c_int;
    pw_main_loop_quit: fn(*mut pw_main_loop) -> c_int;
    pw_main_loop_destroy: fn(*mut pw_main_loop);
    pw_context_new: fn(*mut pw_loop, *mut pw_properties, usize) -> *mut pw_context;
    pw_context_connect_fd: fn(*mut pw_context, c_int, *mut pw_properties, usize) -> *mut pw_core;
    pw_context_destroy: fn(*mut pw_context);
    pw_core_disconnect: fn(*mut pw_core) -> c_int;
    pw_properties_new_dict: fn(*const spa_dict) -> *mut pw_properties;
    pw_proxy_destroy: fn(*mut pw_proxy);
    pw_stream_new: fn(*mut pw_core, *const c_char, *mut pw_properties) -> *mut pw_stream;
    pw_stream_add_listener: fn(*mut pw_stream, *mut spa_hook, *const pw_stream_events, *mut c_void);
    pw_stream_connect: fn(*mut pw_stream, spa_direction, u32, pw_stream_flags, *mut *const spa_pod, u32) -> c_int;
    pw_stream_dequeue_buffer: fn(*mut pw_stream) -> *mut pw_buffer;
    pw_stream_queue_buffer: fn(*mut pw_stream, *mut pw_buffer) -> c_int;
    pw_stream_destroy: fn(*mut pw_stream);
}

fn load(soname: &str) -> anyhow::Result<PipeWire> {
    PipeWire::open(soname).map_err(|error| {
        anyhow::anyhow!(
            "portal ScreenCast video needs the PipeWire client library, and {soname} could not \
             be loaded ({error}). Install it (libpipewire-0.3-0 on Debian and Ubuntu, \
             pipewire-libs on Fedora, libpipewire on Arch) and start the recording again."
        )
    })
}

/// Load and initialise libpipewire on first use. A failed load is not cached,
/// so installing PipeWire takes effect without restarting the driver.
pub(crate) fn libpipewire() -> anyhow::Result<&'static PipeWire> {
    static LOADED: Mutex<Option<&'static PipeWire>> = Mutex::new(None);
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(pw) = *loaded {
        return Ok(pw);
    }
    let pw: &'static PipeWire = Box::leak(Box::new(load(SONAME)?));
    // SAFETY: called once, before any other libpipewire function.
    unsafe { (pw.pw_init)(ptr::null_mut(), ptr::null_mut()) };
    *loaded = Some(pw);
    Ok(pw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_names_the_package_to_install() {
        let error = load("libpipewire-cua-test-absent.so.0")
            .err()
            .expect("no such library")
            .to_string();
        assert!(
            error.contains("libpipewire-cua-test-absent.so.0"),
            "{error}"
        );
        assert!(error.contains("libpipewire-0.3-0"), "{error}");
    }

    struct NoSink;

    impl crate::wayland::portal_screencast::FrameSink for NoSink {
        fn frame(
            &mut self,
            _: crate::wayland::portal_screencast::Frame<'_>,
        ) -> anyhow::Result<crate::wayland::portal_screencast::FrameFlow> {
            unreachable!()
        }
    }

    #[test]
    fn the_crate_does_not_link_libpipewire() {
        // Keep the capture code live in this binary; the linker would not
        // record a library that only dead code references.
        std::hint::black_box(
            crate::wayland::portal_screencast::run_frame_stream::<NoSink> as *const (),
        );
        let exe = std::env::current_exe().unwrap();
        let output = std::process::Command::new("readelf")
            .arg("-d")
            .arg(&exe)
            .output()
            .expect("readelf (binutils) is needed to inspect the test binary");
        assert!(
            output.status.success(),
            "readelf -d {} failed",
            exe.display()
        );
        let dynamic = String::from_utf8_lossy(&output.stdout);
        assert!(dynamic.contains("(NEEDED)"), "{dynamic}");
        assert!(!dynamic.contains("libpipewire"), "{dynamic}");
    }
}
