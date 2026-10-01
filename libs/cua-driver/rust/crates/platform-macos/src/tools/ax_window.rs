//! Exact-window AX operations; own-process work never leaves the main thread.
use crate::{ax::bindings::*, windows::WindowOwner};
use core_foundation::base::{CFType, TCFType};
use cua_driver_core::native_operation::NativeOperation;
use std::{
    ffi::c_void,
    sync::{mpsc, Arc},
    time::Duration,
};

pub(super) struct Window {
    pub pid: i32,
    pub id: u32,
    pub operation: Arc<NativeOperation>,
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
        let (request, callback) = MainQueueCall::new(action);
        unsafe {
            dispatch_async_f(
                &raw const _dispatch_main_q as *const c_void,
                Box::into_raw(Box::new(callback)).cast(),
                run_on_main,
            );
        }
        request.wait(Duration::from_secs(5))
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

struct MainQueueCall<T> {
    queued: Arc<NativeOperation>,
    rx: mpsc::Receiver<Result<T, String>>,
}

impl<T: Send + 'static> MainQueueCall<T> {
    fn new(
        action: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> (Self, Box<dyn FnOnce() + Send>) {
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
        (Self { queued, rx }, callback)
    }

    fn wait(self, timeout: Duration) -> Result<T, String> {
        match self.rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) if self.queued.cancel() => {
                Err("AppKit main queue did not start the window operation".into())
            }
            // The callback won the start race: never report a timeout while it can still write.
            Err(_) => self
                .rx
                .recv()
                .unwrap_or_else(|_| Err("AppKit window operation disconnected".into())),
        }
    }
}

unsafe fn with_window<T>(
    pid: i32,
    window_id: u32,
    work: impl FnOnce(AXUIElementRef) -> Result<T, String>,
) -> Result<T, String> {
    require_owner(
        crate::windows::resolve_window_owner(pid, window_id),
        pid,
        window_id,
    )?;
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(format!(
            "could not create an accessibility element for pid {pid}"
        ));
    }
    AXUIElementSetMessagingTimeout(app, 2.0);
    let app = CFType::wrap_under_create_rule(app.cast());
    let windows = copy_ax_windows(app.as_CFTypeRef() as AXUIElementRef)
        .into_iter()
        .map(|window| CFType::wrap_under_create_rule(window.cast()))
        .collect::<Vec<_>>();
    windows.iter().map(|window| window.as_CFTypeRef() as AXUIElementRef).find(|window| ax_get_window_id(*window) == Some(window_id))
        .ok_or_else(|| format!("window_id {window_id} belongs to pid {pid} in WindowServer but has no matching AXWindow"))
        .and_then(|target| { AXUIElementSetMessagingTimeout(target, 2.0); work(target) })
}

fn require_owner(owner: WindowOwner, pid: i32, window_id: u32) -> Result<(), String> {
    match owner {
        WindowOwner::SamePid => Ok(()),
        WindowOwner::ForeignPid {
            owner_pid,
            owner_app_name,
        } => Err(format!(
            "window_id {window_id} belongs to pid {owner_pid} ({owner_app_name}), not pid {pid}"
        )),
        WindowOwner::Unknown => Err(format!(
            "window_id {window_id} is closed, stale, or unknown to WindowServer"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn unavailable_main_queue_cannot_write_when_it_later_drains() {
        let writes = Arc::new(AtomicUsize::new(0));
        let target = writes.clone();
        let (request, callback) = MainQueueCall::new(move || {
            target.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(
            request.wait(Duration::ZERO).unwrap_err(),
            "AppKit main queue did not start the window operation"
        );
        callback(); // The real queued callback runs after the caller has timed out.
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn queue_timeout_after_native_start_waits_for_truthful_completion() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (request, callback) = MainQueueCall::new(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Err::<(), _>("native write rejected".into())
        });
        let worker = std::thread::spawn(callback);
        started_rx.recv().unwrap();
        let (result_tx, result_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            result_tx.send(request.wait(Duration::ZERO)).unwrap();
        });
        let early = result_rx.recv_timeout(Duration::from_millis(50));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        waiter.join().unwrap();
        assert!(
            matches!(early, Err(mpsc::RecvTimeoutError::Timeout)),
            "a started native request returned before its actual completion"
        );
        assert_eq!(
            result_rx.recv().unwrap().unwrap_err(),
            "native write rejected"
        );
    }

    #[test]
    fn disconnected_started_callback_is_not_reported_as_success() {
        let (request, callback) = MainQueueCall::<()>::new(|| unreachable!());
        assert!(request.queued.begin());
        drop(callback);
        assert_eq!(
            request.wait(Duration::ZERO).unwrap_err(),
            "AppKit window operation disconnected"
        );
    }

    #[test]
    fn closed_or_unknown_window_fails_the_ownership_gate() {
        let owner = crate::windows::resolve_window_owner_in(&[], 17, 23);
        assert_eq!(
            require_owner(owner, 17, 23).unwrap_err(),
            "window_id 23 is closed, stale, or unknown to WindowServer"
        );
    }

    #[test]
    fn wrong_owner_fails_with_the_actual_owner_identity() {
        assert_eq!(
            require_owner(
                WindowOwner::ForeignPid {
                    owner_pid: 41,
                    owner_app_name: "Other application".into(),
                },
                17,
                23
            )
            .unwrap_err(),
            "window_id 23 belongs to pid 41 (Other application), not pid 17"
        );
        assert_eq!(require_owner(WindowOwner::SamePid, 17, 23), Ok(()));
    }
}
