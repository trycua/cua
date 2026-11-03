//! A video backend that blocks in `start` (the portal ScreenCast consent
//! dialog) must not block the rest of the recorder.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cua_driver_core::recording::RecordingSession;
use cua_driver_core::video::{
    set_video_backend_factory, VideoBackend, VideoBackendFactory, VideoMetadata,
};

const PROMPT: Duration = Duration::from_secs(2);

/// Each `start` announces itself and waits for the test to answer.
struct ConsentGate {
    entered: Mutex<Sender<()>>,
    answer: Mutex<Receiver<bool>>,
}

static GATE: OnceLock<Mutex<(Receiver<()>, Sender<bool>)>> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());
static STOPPED: AtomicUsize = AtomicUsize::new(0);

struct Backend;

impl VideoBackend for Backend {
    fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
        STOPPED.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("stopped before any frame")
    }
}

impl VideoBackendFactory for ConsentGate {
    fn start(&self, _: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
        self.entered.lock().unwrap().send(()).unwrap();
        if self.answer.lock().unwrap().recv().unwrap() {
            Ok(Box::new(Backend))
        } else {
            anyhow::bail!("consent denied")
        }
    }
}

fn gate() -> std::sync::MutexGuard<'static, (Receiver<()>, Sender<bool>)> {
    GATE.get_or_init(|| {
        let (entered_tx, entered_rx) = channel();
        let (answer_tx, answer_rx) = channel();
        set_video_backend_factory(Box::new(ConsentGate {
            entered: Mutex::new(entered_tx),
            answer: Mutex::new(answer_rx),
        }));
        Mutex::new((entered_rx, answer_tx))
    })
    .lock()
    .unwrap()
}

/// Run `f` on another thread and fail if it does not return promptly.
fn prompt<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = channel();
    std::thread::spawn(move || tx.send(f()).unwrap());
    rx.recv_timeout(PROMPT)
        .unwrap_or_else(|_| panic!("{what} blocked while the video start waited for consent"))
}

/// Begin a video recording and return once its backend is waiting for consent.
fn start_pending(
    session: &Arc<RecordingSession>,
    dir: &Path,
    owner: Option<&str>,
) -> std::thread::JoinHandle<anyhow::Result<()>> {
    let (session, dir, owner) = (session.clone(), dir.to_owned(), owner.map(str::to_owned));
    let start =
        std::thread::spawn(move || session.start(dir.to_str().unwrap(), true, owner.as_deref()));
    gate().0.recv_timeout(PROMPT).expect("video start reached");
    start
}

#[test]
fn state_second_start_and_stop_stay_responsive_during_consent() {
    let _serial = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let session = Arc::new(RecordingSession::new());
    let dir = tempfile::tempdir().unwrap();
    let start = start_pending(&session, dir.path(), None);

    let s = session.clone();
    let state = prompt("get_recording_state", move || s.current_state());
    assert!(state.starting && !state.enabled, "{state:?}");

    let (s, other) = (session.clone(), dir.path().join("other"));
    let second = prompt("a second start", move || {
        s.start(other.to_str().unwrap(), false, None)
    });
    let error = second
        .expect_err("a second start during consent is rejected")
        .to_string();
    assert!(error.contains("already starting"), "{error}");

    let s = session.clone();
    prompt("stop_recording", move || s.stop_owner(None)).expect("stop cancels the start");
    assert!(!session.current_state().starting);

    // Until the cancelled backend returns, a new start would race it for the
    // same output files, so it is refused.
    let (s, again) = (session.clone(), dir.path().to_owned());
    let error = prompt("a start after the cancel", move || {
        s.start(again.to_str().unwrap(), false, None)
    })
    .expect_err("a start while the cancelled backend is pending is rejected")
    .to_string();
    assert!(error.contains("still waiting"), "{error}");

    let stopped = STOPPED.load(Ordering::SeqCst);
    gate().1.send(true).unwrap();
    let error = start.join().unwrap().expect_err("a cancelled start fails");
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert_eq!(
        STOPPED.load(Ordering::SeqCst),
        stopped + 1,
        "late backend is stopped"
    );
    let state = session.current_state();
    assert!(
        !state.enabled && !state.starting && !state.video_active,
        "{state:?}"
    );
    session
        .start(dir.path().to_str().unwrap(), false, None)
        .expect("the recorder is free once the cancelled start returned");
    assert!(session.current_state().enabled);
    session.stop_owner(None).unwrap();
}

#[test]
fn only_the_owning_session_end_cancels_a_pending_start() {
    let _serial = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let session = Arc::new(RecordingSession::new());
    let dir = tempfile::tempdir().unwrap();
    let start = start_pending(&session, dir.path(), Some("owner"));

    let s = session.clone();
    prompt("another session's end", move || s.stop_owner(Some("other"))).unwrap();
    assert!(session.current_state().starting);
    let s = session.clone();
    prompt("the owner's session end", move || {
        s.stop_owner(Some("owner"))
    })
    .unwrap();
    assert!(!session.current_state().starting);

    gate().1.send(true).unwrap();
    start
        .join()
        .unwrap()
        .expect_err("the owner's end cancels its start");
    assert!(!session.current_state().enabled);
}

#[test]
fn granted_consent_commits_the_recording() {
    let _serial = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let session = Arc::new(RecordingSession::new());
    let dir = tempfile::tempdir().unwrap();
    let start = start_pending(&session, dir.path(), None);
    gate().1.send(true).unwrap();
    start.join().unwrap().expect("start succeeds");
    let state = session.current_state();
    assert!(
        state.enabled && !state.starting && state.video_active,
        "{state:?}"
    );
    let _ = session.stop_owner(None);
    assert!(!session.current_state().enabled);
}

#[test]
fn denied_consent_still_starts_turn_recording() {
    let _serial = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let session = Arc::new(RecordingSession::new());
    let dir = tempfile::tempdir().unwrap();
    let start = start_pending(&session, dir.path(), None);
    gate().1.send(false).unwrap();
    start
        .join()
        .unwrap()
        .expect("turn recording starts without video");
    let state = session.current_state();
    assert!(state.enabled && !state.video_active, "{state:?}");
    assert!(state.last_error.unwrap().contains("consent denied"));
    session.stop_owner(None).unwrap();
}
