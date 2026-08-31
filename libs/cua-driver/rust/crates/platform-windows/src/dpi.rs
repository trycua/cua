//! DPI context for threads owned by an embedded Cua Driver runtime.
//!
//! The standalone driver executable declares Per-Monitor V2 awareness in its
//! manifest. An in-process SDK cannot rely on the importing application to do
//! the same, so Cua-owned worker threads opt into the physical-pixel context
//! used by Windows capture and input code.

use std::cell::RefCell;
use std::future::{poll_fn, Future};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Sticky failure state shared by every thread in one embedded executor.
/// A late thread failure poisons that executor, not another SDK/host runtime.
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

/// Carry the owner into explicitly-created Windows threads (UIA, package
/// enumeration, overlay). Those threads are not covered by Tokio's callback.
/// The caller/host thread's context is never changed.
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
