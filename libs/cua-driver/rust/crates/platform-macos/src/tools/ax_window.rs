//! Exact-window AX operations; own-process work never leaves the main thread.
use crate::{ax::bindings::*, windows::WindowOwner};
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::native_operation::NativeOperation;
use std::{
    ffi::c_void,
    sync::{mpsc, Arc},
    time::Duration,
};

pub(super) struct Window {
    pub pid: i32,
    pub id: u32,
    operation: Arc<NativeOperation>,
}

#[link(name = "System", kind = "framework")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

unsafe extern "C" fn run_on_main(context: *mut c_void) {
    let work = unsafe { Box::from_raw(context.cast::<Box<dyn FnOnce() + Send>>()) };
    work();
}

impl Window {
    pub fn new(pid: i32, id: u32) -> Self {
        Self {
            pid,
            id,
            operation: NativeOperation::current(),
        }
    }

    pub fn with<T: Send + 'static>(
        &self,
        work: impl FnOnce(AXUIElementRef) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (pid, id) = (self.pid, self.id);
        let operation = self.operation.clone();
        let action = move || {
            if !operation.begin() {
                return Err("native window operation cancelled before start".into());
            }
            unsafe { with_window(pid, id, work) }
        };
        if pid != std::process::id() as i32 || objc2_foundation::MainThreadMarker::new().is_some() {
            return action();
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let queued = Arc::new(NativeOperation::new());
        let request = queued.clone();
        let callback: Box<dyn FnOnce() + Send> = Box::new(move || {
            if request.begin() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action))
                    .unwrap_or_else(|_| Err("native window operation panicked".into()));
                let _ = tx.send(result);
            }
        });
        unsafe {
            dispatch_async_f(
                &raw const _dispatch_main_q as *const c_void,
                Box::into_raw(Box::new(callback)).cast(),
                run_on_main,
            );
        }
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) if queued.cancel() => {
                Err("AppKit main queue did not start the window operation".into())
            }
            // The callback won the start race: never report a timeout while it can still write.
            Err(_) => rx
                .recv()
                .unwrap_or_else(|_| Err("AppKit window operation disconnected".into())),
        }
    }

    pub fn focus(&self) -> Result<bool, String> {
        self.with(|target| unsafe {
            let raised = perform_action(target, "AXRaise") == 0;
            let main = set_bool_attr_true(target, "AXMain") == 0;
            let focused = set_bool_attr_true(target, "AXFocused") == 0;
            Ok(raised || main || focused)
        })
    }
}

unsafe fn with_window<T>(
    pid: i32,
    window_id: u32,
    work: impl FnOnce(AXUIElementRef) -> Result<T, String>,
) -> Result<T, String> {
    match crate::windows::resolve_window_owner(pid, window_id) {
        WindowOwner::SamePid => {}
        WindowOwner::ForeignPid {
            owner_pid,
            owner_app_name,
        } => {
            return Err(format!(
            "window_id {window_id} belongs to pid {owner_pid} ({owner_app_name}), not pid {pid}"
        ))
        }
        WindowOwner::Unknown => {
            return Err(format!(
                "window_id {window_id} is closed, stale, or unknown to WindowServer"
            ))
        }
    }
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(format!(
            "could not create an accessibility element for pid {pid}"
        ));
    }
    AXUIElementSetMessagingTimeout(app, 2.0);
    struct Elements(Vec<AXUIElementRef>);
    impl Drop for Elements {
        fn drop(&mut self) {
            for element in &self.0 {
                unsafe {
                    CFRelease(*element as CFTypeRef);
                }
            }
        }
    }
    let app = Elements(vec![app]);
    let windows = Elements(copy_ax_windows(app.0[0]));
    windows.0.iter().copied().find(|window| ax_get_window_id(*window) == Some(window_id))
        .ok_or_else(|| format!("window_id {window_id} belongs to pid {pid} in WindowServer but has no matching AXWindow"))
        .and_then(|target| { AXUIElementSetMessagingTimeout(target, 2.0); work(target) })
}
