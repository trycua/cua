//! The session-lock check: logind's `LockedHint` for niri's session.

use super::*;

/// How long each logind method call may take.
const LOGIND_TIMEOUT: Duration = Duration::from_secs(1);
/// The method calls of one lock query: `GetSession`, then `LockedHint`.
const LOGIND_CALLS: u32 = 2;
/// The share of a lock query left for opening the system-bus connection,
/// which zbus does not bound itself.
const LOGIND_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// Refuse input and capture while the niri session is locked; input routes
/// add the `foreground_unavailable` code (see [`input_unavailable`]). niri records
/// its lock in logind's `LockedHint` of the session it started in
/// (`XDG_SESSION_ID`), and only when it runs as the session instance
/// (`niri --session`; `update_locked_hint` in niri v26.04 `src/niri.rs`).
/// A niri started any other way, such as a nested one, or with `--session`
/// but no `XDG_SESSION_ID`, sets no hint, so this check cannot see its lock
/// and passes. Keyboard input does not rely on it:
/// a locked niri reports no focused window (see [`keyboard_focus`]).
pub fn refuse_if_locked() -> Result<()> {
    let pid = recognised()?;
    lock_refusal(logind_session(pid).and_then(|session| match session {
        Some(session) => locked_hint_with(move || logind_locked_hint(&session)),
        None => Ok(false),
    }))
}

/// The refusal for a lock read: a lock, or a hint niri sets but that cannot
/// be read, refuses.
fn lock_refusal(checked: Result<bool>) -> Result<()> {
    match checked {
        Ok(false) => Ok(()),
        Ok(true) => bail!("the niri session is locked (logind LockedHint)"),
        Err(error) => bail!("cannot read whether the niri session is locked: {error:#}"),
    }
}

/// The logind session niri sets its locked hint on, or `None` when niri runs
/// without `--session` or without `XDG_SESSION_ID` and sets none. Fixed for
/// the recognised process.
fn logind_session(pid: libc::pid_t) -> Result<Option<String>> {
    static SESSION: OnceLock<Option<String>> = OnceLock::new();
    if let Some(session) = SESSION.get() {
        return Ok(session.clone());
    }
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
        .with_context(|| format!("read niri's command line (pid {pid})"))?;
    let session = if session_instance(&cmdline) {
        let environ = std::fs::read(format!("/proc/{pid}/environ"))
            .with_context(|| format!("read niri's environment (pid {pid})"))?;
        session_id(&environ)?
    } else {
        None
    };
    Ok(SESSION.get_or_init(|| session).clone())
}

/// Whether a `/proc/<pid>/cmdline` image has niri's `--session` argument.
/// Arguments after `--` are niri's startup command, not niri's own.
fn session_instance(cmdline: &[u8]) -> bool {
    cmdline
        .split(|&byte| byte == 0)
        .take_while(|&arg| arg != b"--")
        .any(|arg| arg == b"--session")
}

/// `XDG_SESSION_ID` from a `/proc/<pid>/environ` image. niri started with
/// `--session` but without it sets no hint, like a niri started without
/// `--session`, so its lock cannot be seen and this is `None`.
fn session_id(environ: &[u8]) -> Result<Option<String>> {
    let Some(id) = environ
        .split(|&byte| byte == 0)
        .find_map(|entry| entry.strip_prefix(b"XDG_SESSION_ID="))
    else {
        return Ok(None);
    };
    let id = std::str::from_utf8(id).context("XDG_SESSION_ID is not UTF-8")?;
    // `self` and `auto` would name the caller's own session, not niri's.
    if id.is_empty()
        || id == "self"
        || id == "auto"
        || !id.chars().all(|c| c.is_ascii_alphanumeric())
    {
        bail!("XDG_SESSION_ID {id:?} is not a logind session id");
    }
    Ok(Some(id.to_owned()))
}

/// A call that runs on a thread of its own, at most one at a time: one that
/// outlives its wait is left to finish, and until it does, later calls
/// refuse instead of starting another thread that a hung bus would hold.
struct Flight<T> {
    pending: Mutex<Option<std::sync::mpsc::Receiver<T>>>,
}

impl<T: Send + 'static> Flight<T> {
    const fn new() -> Self {
        Self {
            pending: Mutex::new(None),
        }
    }

    /// Run `call` and wait at most `timeout` for it. The answer of an
    /// earlier call that has since finished is stale and dropped.
    fn run(
        &self,
        timeout: Duration,
        what: &str,
        call: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("could not {what}: its lock is poisoned"))?;
        if let Some(earlier) = pending.as_ref() {
            if let Err(std::sync::mpsc::TryRecvError::Empty) = earlier.try_recv() {
                bail!("could not {what}: an earlier attempt has not finished");
            }
            *pending = None;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("cua-niri-logind".into())
            .spawn(move || {
                let _ = sender.send(call());
            })
            .with_context(|| format!("start a thread to {what}"))?;
        match receiver.recv_timeout(timeout) {
            Ok(answer) => Ok(answer),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                *pending = Some(receiver);
                bail!("could not {what} within {timeout:?}")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("could not {what}: the attempt ended without an answer")
            }
        }
    }
}

/// How long one whole logind query may take: the connection's share and
/// both method calls, so a slow but working bus still answers in time.
const LOCK_QUERY_TIMEOUT: Duration =
    LOGIND_CONNECT_TIMEOUT.saturating_add(LOGIND_TIMEOUT.saturating_mul(LOGIND_CALLS));

/// logind's `LockedHint`, read by `query`. zbus's blocking calls run on its own
/// Tokio runtime and panic on a Tokio worker thread, where capture-bound
/// clicks and recording reach this check, so the whole query runs on a
/// thread of its own.
fn locked_hint_with(query: impl FnOnce() -> Result<bool> + Send + 'static) -> Result<bool> {
    static QUERY: Flight<Result<bool>> = Flight::new();
    QUERY.run(LOCK_QUERY_TIMEOUT, "read logind's LockedHint", query)?
}

fn logind_locked_hint(session: &str) -> Result<bool> {
    static BUS: Mutex<Option<zbus::blocking::Connection>> = Mutex::new(None);
    let mut bus = BUS
        .lock()
        .map_err(|_| anyhow!("logind connection lock poisoned"))?;
    let connection = match bus.as_ref() {
        Some(connection) => connection.clone(),
        None => {
            let connection = zbus::blocking::connection::Builder::system()?
                .method_timeout(LOGIND_TIMEOUT)
                .build()
                .context("connect to the system bus")?;
            bus.insert(connection).clone()
        }
    };
    drop(bus);
    let reply = connection
        .call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "GetSession",
            &(session,),
        )
        .with_context(|| format!("logind GetSession({session})"))?;
    let path: zbus::zvariant::OwnedObjectPath = reply.body().deserialize()?;
    let reply = connection
        .call_method(
            Some("org.freedesktop.login1"),
            path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.login1.Session", "LockedHint"),
        )
        .with_context(|| format!("logind LockedHint of session {session}"))?;
    let hint: zbus::zvariant::OwnedValue = reply.body().deserialize()?;
    bool::try_from(hint).context("logind LockedHint is not a boolean")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn only_a_session_niri_with_a_logind_session_id_has_a_lock_hint() {
        assert!(session_instance(b"niri\0--session\0"));
        assert!(!session_instance(b"niri\0-c\0/tmp/niri--session.kdl\0"));
        // After `--`, `--session` belongs to the startup command.
        assert!(!session_instance(b"niri\0--\0sh\0--session\0"));
        let environ = b"HOME=/home/u\0XDG_SESSION_ID=c2\0";
        assert_eq!(session_id(environ).unwrap().as_deref(), Some("c2"));
        // niri sets no hint without the id, so there is none to read.
        assert_eq!(session_id(b"HOME=/home/u\0").unwrap(), None);
        for environ in [
            &b"XDG_SESSION_ID=\0"[..],
            b"XDG_SESSION_ID=self\0",
            b"XDG_SESSION_ID=auto\0",
            b"XDG_SESSION_ID=-p\0",
        ] {
            assert!(
                session_id(environ).is_err(),
                "{:?}",
                String::from_utf8_lossy(environ)
            );
        }
    }

    /// Capture-bound clicks and recording call the lock check from a Tokio
    /// worker; a zbus blocking call made there panics.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_lock_query_runs_off_the_tokio_worker() {
        let locked = locked_hint_with(|| Ok(zbus::block_on(async { true }))).unwrap();
        assert!(locked);
    }

    #[test]
    fn a_logind_query_that_never_answers_times_out_and_runs_once() {
        let flight = Flight::new();
        let spawned = Arc::new(Mutex::new(0));
        let (release, released) = std::sync::mpsc::channel::<()>();
        let hung = {
            let spawned = Arc::clone(&spawned);
            move || {
                *spawned.lock().unwrap() += 1;
                let _ = released.recv();
                7
            }
        };
        let started = Instant::now();
        let error = flight
            .run(Duration::from_millis(50), "read logind's LockedHint", hung)
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            error.to_string(),
            "could not read logind's LockedHint within 50ms"
        );

        // While it hangs, a later check refuses at once, without a thread.
        let count = Arc::clone(&spawned);
        let error = flight
            .run(
                Duration::from_secs(1),
                "read logind's LockedHint",
                move || {
                    *count.lock().unwrap() += 1;
                    8
                },
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "could not read logind's LockedHint: an earlier attempt has not finished"
        );
        assert_eq!(*spawned.lock().unwrap(), 1);

        // Once it ends, its stale answer is dropped and a fresh query runs.
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let answer = loop {
            let count = Arc::clone(&spawned);
            let answer = flight.run(
                Duration::from_secs(1),
                "read logind's LockedHint",
                move || {
                    *count.lock().unwrap() += 1;
                    8
                },
            );
            match answer {
                Err(error)
                    if Instant::now() < deadline && error.to_string().ends_with("not finished") =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                answer => break answer,
            }
        };
        assert_eq!(answer.unwrap(), 8);
        assert_eq!(*spawned.lock().unwrap(), 2);
    }

    #[test]
    fn a_locked_or_unreadable_session_refuses_input_with_the_code() {
        assert!(lock_refusal(Ok(false)).is_ok());
        for read in [Ok(true), Err(anyhow!("system bus unavailable"))] {
            let error = input_unavailable(lock_refusal(read).unwrap_err());
            assert_eq!(
                crate::input::foreground::error_code(&error),
                Some(crate::input::foreground::CODE_UNAVAILABLE),
                "{error}"
            );
        }
        assert!(lock_refusal(Ok(true))
            .unwrap_err()
            .to_string()
            .contains("the niri session is locked"));
    }
}
