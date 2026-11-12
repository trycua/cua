//! Keeping this machine awake while a durable run needs it, with a reason
//! anyone can see (`pmset -g assertions` on a Mac lists it by name).
//!
//! An idle-sleep assertion stops the machine sleeping because nobody touched
//! it. It does not stop a laptop sleeping when its lid closes, unless it is
//! plugged in with an external display (clamshell mode). Callers that tell
//! the user what will happen must say so ([`LID_NOTE`]).

use std::sync::Mutex;

/// What to tell the user when the only machine is a laptop.
pub const LID_NOTE: &str =
    "Closing the lid still puts a laptop to sleep unless it is plugged in with a display attached.";

static REASONS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The reasons this process currently keeps the machine awake.
pub fn active_reasons() -> Vec<String> {
    REASONS
        .lock()
        .expect("reasons")
        .iter()
        .map(|(_, r)| r.clone())
        .collect()
}

/// Holds the machine awake until dropped.
#[derive(Debug)]
pub struct SleepGuard {
    id: u64,
    #[allow(dead_code)]
    os: Os,
}

impl SleepGuard {
    /// Keeps the machine from sleeping on idle for `reason` (shown to the
    /// user: "Cua: the durable agent ada is running").
    pub fn hold(reason: &str) -> SleepGuard {
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        REASONS
            .lock()
            .expect("reasons")
            .push((id, reason.to_string()));
        SleepGuard {
            id,
            os: Os::acquire(reason),
        }
    }
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        REASONS
            .lock()
            .expect("reasons")
            .retain(|(i, _)| *i != self.id);
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CString, c_char, c_void};

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: *const c_void,
            level: u32,
            name: *const c_void,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    const UTF8: u32 = 0x0800_0100;
    const LEVEL_ON: u32 = 255;

    #[derive(Debug)]
    pub struct Os(Option<u32>);

    fn cf(s: &str) -> *const c_void {
        let c = CString::new(s.replace('\0', " ")).unwrap_or_default();
        // SAFETY: a valid C string; the result is released by the caller.
        unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
    }

    impl Os {
        pub fn acquire(reason: &str) -> Os {
            let (kind, name) = (cf("PreventUserIdleSystemSleep"), cf(reason));
            let mut id = 0u32;
            // SAFETY: valid CFStrings and a valid out pointer.
            let r = unsafe { IOPMAssertionCreateWithName(kind, LEVEL_ON, name, &mut id) };
            // SAFETY: both were created above and are not used again.
            unsafe {
                CFRelease(kind);
                CFRelease(name);
            }
            Os((r == 0).then_some(id))
        }
    }

    impl Drop for Os {
        fn drop(&mut self) {
            if let Some(id) = self.0 {
                // SAFETY: an assertion id this process created.
                unsafe { IOPMAssertionRelease(id) };
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    #[derive(Debug)]
    pub struct Os;

    impl Os {
        pub fn acquire(_reason: &str) -> Os {
            Os
        }
    }
}

use imp::Os;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guard_shows_its_reason_until_dropped() {
        let before = active_reasons().len();
        let g = SleepGuard::hold("Cua: the durable agent ada is running");
        assert!(active_reasons().contains(&"Cua: the durable agent ada is running".to_string()));
        assert_eq!(active_reasons().len(), before + 1);
        drop(g);
        assert_eq!(active_reasons().len(), before);
    }
}
