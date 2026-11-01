//! F61: `Relay.machine` / `connect` / `start_sharing` / `stop_sharing` did
//! device-session network I/O on the caller's executor, which has no Tokio
//! reactor, so they panicked ("there is no reactor running") from plain
//! sync or asyncio callers. They now run it on the SDK runtime. Here the
//! futures are driven by a bare thread-parking executor (no Tokio at all);
//! the relay is a closed loopback port, so each call must fail with an
//! error, never panic.

use cua_sdk::Relay;
use std::{
    future::Future,
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread::{self, Thread},
};

struct Unpark(Thread);
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        thread::park();
    }
}

#[test]
fn per_machine_calls_work_without_a_tokio_reactor() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary, before any thread reads it.
    unsafe { std::env::set_var("CUA_HOME", home.path()) };
    unsafe { std::env::set_var("CUA_TEST", "1") };
    let relay = Relay::new(Some("http://127.0.0.1:9".into()), "token".into()).unwrap();
    assert!(block_on(relay.machines()).is_err());
    assert!(block_on(relay.machine("m1".into())).is_err());
    assert!(block_on(relay.stop_sharing("m1".into())).is_err());
    assert!(block_on(relay.start_sharing("m1".into())).is_err());
    assert!(block_on(relay.connect("m1".into())).is_err());
}
