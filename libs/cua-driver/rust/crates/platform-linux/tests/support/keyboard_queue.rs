use cua_driver_core::cursor_events::{self, CursorEvent, CursorEventPhase};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::Notify;

pub(super) struct OverlayBarrier {
    released: Arc<(Mutex<bool>, Condvar)>,
    pub(super) finished: Arc<Notify>,
    pub(super) queued: Arc<Notify>,
}

impl OverlayBarrier {
    pub(super) fn install() -> Self {
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let finished = Arc::new(Notify::new());
        let queued = Arc::new(Notify::new());
        let wait = released.clone();
        let first = finished.clone();
        let second = queued.clone();
        cursor_events::install_cursor_event_sink(Arc::new(move |event| {
            if let CursorEvent::Action { session, phase, .. } = event {
                if session.ends_with("keyboard-held") && phase == CursorEventPhase::End {
                    first.notify_one();
                    let (lock, wake) = &*wait;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = wake.wait(released).unwrap();
                    }
                }
                if session.ends_with("keyboard-queued") && phase == CursorEventPhase::Begin {
                    second.notify_one();
                }
            }
        }));
        Self {
            released,
            finished,
            queued,
        }
    }

    pub(super) fn release(&self) {
        let (lock, wake) = &*self.released;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }
}

impl Drop for OverlayBarrier {
    fn drop(&mut self) {
        self.release();
        cursor_events::clear_cursor_event_sink();
    }
}
