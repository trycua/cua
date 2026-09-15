use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

struct PendingFocus {
    platform: &'static str,
    reached: mpsc::Sender<usize>,
    released: Mutex<Option<bool>>,
    wake: Condvar,
}

fn pending() -> &'static Mutex<Option<Arc<PendingFocus>>> {
    static PENDING: OnceLock<Mutex<Option<Arc<PendingFocus>>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

pub struct NativeFocusBarrier {
    state: Arc<PendingFocus>,
    reached: mpsc::Receiver<usize>,
}

impl NativeFocusBarrier {
    pub fn install(platform: &'static str) -> Self {
        let (sender, reached) = mpsc::channel();
        let state = Arc::new(PendingFocus {
            platform,
            reached: sender,
            released: Mutex::new(None),
            wake: Condvar::new(),
        });
        let mut slot = pending().lock().unwrap();
        assert!(slot.is_none(), "native focus barrier already installed");
        *slot = Some(state.clone());
        Self { state, reached }
    }

    pub fn wait(&self) -> usize {
        self.reached
            .recv_timeout(Duration::from_secs(15))
            .expect("native focus boundary was not reached")
    }

    pub fn release(&self) {
        *self.state.released.lock().unwrap() = Some(true);
        self.state.wake.notify_all();
    }
}

impl Drop for NativeFocusBarrier {
    fn drop(&mut self) {
        {
            let mut slot = pending().lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|state| Arc::ptr_eq(state, &self.state))
            {
                *slot = None;
            }
        }
        self.state.released.lock().unwrap().get_or_insert(false);
        self.state.wake.notify_all();
    }
}

pub fn pause_native_focus(platform: &'static str, pointer: usize) {
    let state = {
        let mut slot = pending().lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|state| state.platform == platform)
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(state) = state {
        state.reached.send(pointer).unwrap();
        let (released, _) = state
            .wake
            .wait_timeout_while(
                state.released.lock().unwrap(),
                Duration::from_secs(15),
                |released| released.is_none(),
            )
            .unwrap();
        assert_eq!(
            *released,
            Some(true),
            "native focus barrier abandoned or timed out before actuation"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn paused_native_boundary_requires_release_and_abandonment_prevents_actuation() {
        pause_native_focus("test", 0);
        for release in [true, false] {
            let barrier = NativeFocusBarrier::install("test");
            let acted = Arc::new(AtomicBool::new(false));
            let observed = acted.clone();
            let worker = std::thread::spawn(move || {
                pause_native_focus("test", 42);
                observed.store(true, Ordering::SeqCst);
            });
            assert_eq!(barrier.wait(), 42);
            assert!(!acted.load(Ordering::SeqCst));
            if release {
                barrier.release();
            }
            drop(barrier);
            assert_eq!(worker.join().is_ok(), release);
            assert_eq!(acted.load(Ordering::SeqCst), release);
        }
    }
}
