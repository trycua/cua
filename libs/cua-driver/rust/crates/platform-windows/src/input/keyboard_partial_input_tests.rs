use super::*;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

struct PartialInput {
    release: Mutex<Option<INPUT>>,
    finish: Mutex<bool>,
    wake: Condvar,
}

fn pending() -> &'static Mutex<Option<Arc<PartialInput>>> {
    static PENDING: OnceLock<Mutex<Option<Arc<PartialInput>>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

pub(crate) struct PartialInputGuard(Arc<PartialInput>);

impl PartialInputGuard {
    pub(crate) fn install() -> Self {
        let state = Arc::new(PartialInput {
            release: Mutex::new(None),
            finish: Mutex::new(false),
            wake: Condvar::new(),
        });
        let mut slot = pending().lock().unwrap();
        assert!(slot.is_none(), "partial input fault already installed");
        *slot = Some(state.clone());
        Self(state)
    }

    pub(crate) fn finish(&self) {
        *self.0.finish.lock().unwrap() = true;
        self.0.wake.notify_all();
    }

    pub(crate) fn release(&self) -> bool {
        let mut release = self.0.release.lock().unwrap();
        if let Some(event) = release.as_ref() {
            if unsafe {
                SendInput(
                    std::slice::from_ref(event),
                    std::mem::size_of::<INPUT>() as i32,
                )
            } != 1
            {
                return false;
            }
            *release = None;
        }
        true
    }
}

impl Drop for PartialInputGuard {
    fn drop(&mut self) {
        let mut slot = pending().lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|state| Arc::ptr_eq(state, &self.0))
        {
            *slot = None;
        }
        drop(slot);
        self.finish();
        if !self.release() {
            eprintln!("native partial-input cleanup could not release the accepted key");
        }
    }
}

pub(super) unsafe fn send_input(events: &[INPUT]) -> u32 {
    let fault = pending().lock().unwrap().take();
    match fault {
        None => SendInput(events, std::mem::size_of::<INPUT>() as i32),
        Some(fault) => {
            assert!(events.len() > 1);
            let sent = SendInput(&events[..1], std::mem::size_of::<INPUT>() as i32);
            if sent == 1 {
                let mut release = events[0];
                release.Anonymous.ki.dwFlags |= KEYEVENTF_KEYUP;
                *fault.release.lock().unwrap() = Some(release);
                let (finished, _) = fault
                    .wake
                    .wait_timeout_while(
                        fault.finish.lock().unwrap(),
                        std::time::Duration::from_secs(15),
                        |finished| !*finished,
                    )
                    .unwrap();
                let acknowledged = *finished;
                drop(finished);
                assert!(acknowledged, "native partial input was not independently observed before the syscall boundary returned");
            }
            sent
        }
    }
}
