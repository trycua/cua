//! DPI context for threads owned by an embedded Cua Driver runtime.
//!
//! The standalone driver executable declares Per-Monitor V2 awareness in its
//! manifest. An in-process SDK cannot rely on the importing application to do
//! the same, so Cua-owned worker threads opt into the physical-pixel context
//! used by Windows capture and input code.

use std::cell::RefCell;
use std::future::{poll_fn, Future};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;

use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Sticky failure state shared by every thread in one embedded executor.
/// All SDK handles using that executor share its failure state; unrelated host
/// runtimes and independently-started owned threads do not.
pub struct OwnedThreadDpi {
    failure: Mutex<Option<String>>,
    initialize: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}

impl std::fmt::Debug for OwnedThreadDpi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedThreadDpi")
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl Default for OwnedThreadDpi {
    fn default() -> Self {
        Self::new(Arc::new(use_per_monitor_v2_for_current_thread))
    }
}

thread_local! {
    static OWNED_THREAD_DPI: RefCell<Option<Arc<OwnedThreadDpi>>> = const { RefCell::new(None) };
}

/// The embedded SDK ABI executor publishes its owner here so explicitly
/// created recorder threads can join the same terminal readiness state.
static ABI_EXECUTOR_OWNER: OnceLock<Arc<OwnedThreadDpi>> = OnceLock::new();

/// Publish the owner shared by the embedded SDK executor's worker threads.
/// Re-registering that same owner is harmless; replacing it would split the
/// physical-pixel failure state and is therefore rejected.
pub fn register_abi_executor_owner(owner: Arc<OwnedThreadDpi>) -> Result<(), String> {
    match ABI_EXECUTOR_OWNER.set(owner.clone()) {
        Ok(()) => Ok(()),
        Err(owner) => match ABI_EXECUTOR_OWNER.get() {
            Some(current) if Arc::ptr_eq(current, &owner) => Ok(()),
            Some(_) => {
                Err("a different Windows ABI executor DPI owner is already registered".into())
            }
            None => Err("Windows ABI executor DPI owner registration raced".into()),
        },
    }
}

impl OwnedThreadDpi {
    pub fn new(initialize: Arc<dyn Fn() -> Result<(), String> + Send + Sync>) -> Self {
        Self {
            failure: Mutex::new(None),
            initialize,
        }
    }

    /// Called by Tokio on every owned async/blocking thread, including threads
    /// created after the runtime has been exposed to callers.
    pub fn initialize_current_thread(self: &Arc<Self>) {
        OWNED_THREAD_DPI.with(|slot| *slot.borrow_mut() = Some(self.clone()));
        if let Err(error) = (self.initialize)() {
            let mut failure = self.failure.lock().unwrap_or_else(|e| e.into_inner());
            if failure.is_none() {
                *failure = Some(format!(
                    "initialize Windows physical-pixel context for Cua Driver ABI threads: {error}"
                ));
            }
        }
    }

    pub fn check(&self) -> Result<(), String> {
        match &*self.failure.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

/// Initialize explicitly-created Windows threads outside Tokio's callback.
/// Inherit an owner when the submitting thread has one (e.g. UIA work inside
/// the ABI executor). Otherwise create an independent owner: registry creation
/// can start the overlay synchronously on a host thread before entering the
/// executor. That overlay uses the same initializer, not the executor's sticky
/// failure state. The caller/host thread's DPI context is never changed.
pub fn owned_thread<F, T>(work: F) -> impl FnOnce() -> Result<T, String> + Send + 'static
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let owner = OWNED_THREAD_DPI
        .with(|slot| slot.borrow().clone())
        .unwrap_or_default();
    move || {
        owner.initialize_current_thread();
        owner.check()?;
        let result = work();
        owner.check().map(|()| result)
    }
}

/// Initialize a recorder-owned state-capture thread. Unlike overlay threads
/// that may start synchronously during host-side registry construction, this
/// worker belongs to the embedded ABI runtime and must share its sticky
/// failure state. Existing Cua-owned thread context takes precedence.
pub fn initialize_recording_state_thread() -> Result<(), String> {
    let current = OWNED_THREAD_DPI.with(|slot| slot.borrow().clone());
    let owner = current.or_else(|| ABI_EXECUTOR_OWNER.get().cloned());
    let Some(owner) = owner else {
        // Standalone/platform callers without an embedded ABI runtime retain
        // an independent Cua-owned context.
        let owner = Arc::new(OwnedThreadDpi::default());
        owner.initialize_current_thread();
        return owner.check();
    };

    let already_initialized_here = OWNED_THREAD_DPI.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &owner))
    });
    if !already_initialized_here {
        owner.initialize_current_thread();
    }
    owner.check()
}

/// Run Windows recording work only after the recorder thread has joined its
/// owner and verified the physical-pixel context. Check again after native
/// work so a sticky failure cannot be hidden by an optional capture result.
pub fn with_recording_state_thread<F, T>(work: F) -> Result<T, String>
where
    F: FnOnce() -> T,
{
    initialize_recording_state_thread()?;
    let result = work();
    check_owned_thread().map(|()| result)
}

pub(crate) fn check_owned_thread() -> Result<(), String> {
    OWNED_THREAD_DPI.with(|slot| match &*slot.borrow() {
        Some(state) => state.check(),
        // Other runtimes (including the manifested standalone executable)
        // retain their existing DPI ownership. Never change a host thread here.
        None => Ok(()),
    })
}

/// Gate each poll, not just initial submission: async tasks can migrate between
/// workers. Check completion too so a tool cannot swallow a blocking-worker
/// failure and accidentally return a successful physical-pixel result.
pub async fn guard_future<F: Future>(future: F) -> Result<F::Output, String> {
    let mut future = std::pin::pin!(future);
    poll_fn(move |cx| {
        if let Err(error) = check_owned_thread() {
            return Poll::Ready(Err(error));
        }
        match future.as_mut().poll(cx) {
            Poll::Ready(value) => Poll::Ready(check_owned_thread().map(|()| value)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// Start immediately, like Tokio's API. Check on the actual blocking worker,
/// before running any native capture/input closure; a submission-side check
/// alone cannot detect failure while the pool creates a new thread.
pub fn spawn_blocking<F, T>(work: F) -> impl Future<Output = Result<T, String>> + Send
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let task = tokio::task::spawn_blocking(move || {
        check_owned_thread()?;
        let result = work();
        check_owned_thread().map(|()| result)
    });
    async move { task.await.map_err(|error| error.to_string())? }
}

/// Detached Windows helper tasks use the same per-poll gate as ABI operations.
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<Result<F::Output, String>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(guard_future(future))
}

/// Configure the current Cua-owned thread to use the physical-pixel coordinate
/// contract expected by the Windows backend.
///
/// Returns an error when Windows rejects the requested context. Callers that
/// provide the physical-pixel desktop contract must not continue in that
/// case, because Windows would virtualize capture and input coordinates.
pub fn use_per_monitor_v2_for_current_thread() -> Result<(), String> {
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if previous.0.is_null() {
        Err(format!(
            "SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) failed: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

/// Report whether the current thread is running with the Cua Windows DPI
/// contract. This is used by the SDK executor regression test.
pub fn current_thread_uses_per_monitor_v2() -> bool {
    unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn current_owner() -> Arc<OwnedThreadDpi> {
        OWNED_THREAD_DPI.with(|slot| slot.borrow().clone().expect("owned thread"))
    }

    #[test]
    fn host_started_threads_have_independent_owners_without_changing_host_dpi() {
        // A fresh OS thread models the synchronous host-side registry builder,
        // not an ABI worker. It must not acquire an executor owner implicitly.
        std::thread::spawn(|| {
            assert!(OWNED_THREAD_DPI.with(|slot| slot.borrow().is_none()));
            let host_dpi = unsafe { GetThreadDpiAwarenessContext() };
            let start = || {
                std::thread::spawn(owned_thread(|| {
                    assert!(current_thread_uses_per_monitor_v2());
                    current_owner()
                }))
                .join()
                .unwrap()
                .unwrap()
            };
            let first = start();
            let second = start();
            assert!(!Arc::ptr_eq(&first, &second));
            assert!(OWNED_THREAD_DPI.with(|slot| slot.borrow().is_none()));
            assert!(unsafe {
                AreDpiAwarenessContextsEqual(host_dpi, GetThreadDpiAwarenessContext()).as_bool()
            });
        })
        .join()
        .unwrap();
    }

    #[test]
    fn owned_thread_inherits_the_submitting_threads_owner() {
        std::thread::spawn(owned_thread(|| {
            let parent = current_owner();
            let child = std::thread::spawn(owned_thread(current_owner))
                .join()
                .unwrap()
                .unwrap();
            assert!(Arc::ptr_eq(&parent, &child));
        }))
        .join()
        .unwrap()
        .unwrap();
    }

    #[test]
    fn recording_worker_uses_executor_owner_and_late_failure_is_sticky() {
        let reject_recording_thread = Arc::new(AtomicBool::new(false));
        let reject = reject_recording_thread.clone();
        let owner = Arc::new(OwnedThreadDpi::new(Arc::new(move || {
            if reject.load(Ordering::SeqCst) {
                Err("late recorder thread PMv2 rejection".to_owned())
            } else {
                use_per_monitor_v2_for_current_thread()
            }
        })));
        register_abi_executor_owner(owner.clone()).unwrap();

        let expected_owner = owner.clone();
        std::thread::spawn(move || {
            with_recording_state_thread(|| {
                assert!(current_thread_uses_per_monitor_v2());
                assert!(Arc::ptr_eq(&current_owner(), &expected_owner));
            })
        })
        .join()
        .unwrap()
        .unwrap();

        // A later recorder-created OS thread must initialize against the same
        // owner. Rejection poisons the ABI executor before native work runs.
        reject_recording_thread.store(true, Ordering::SeqCst);
        let native_work_ran = Arc::new(AtomicBool::new(false));
        let native_work = native_work_ran.clone();
        let error = std::thread::spawn(move || {
            with_recording_state_thread(|| native_work.store(true, Ordering::SeqCst))
        })
        .join()
        .unwrap()
        .unwrap_err();

        assert!(error.contains("late recorder thread PMv2 rejection"));
        assert!(!native_work_ran.load(Ordering::SeqCst));
        assert!(owner
            .check()
            .unwrap_err()
            .contains("late recorder thread PMv2 rejection"));
    }
}
