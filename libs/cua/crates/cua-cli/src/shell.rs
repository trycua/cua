//! Commands and interactive PTY shells over cua-spacesd's Process
//! service.

use cua_sdk::{CuaError, PtySize, SpacesdClient, SpacesdCommand};
use std::{collections::HashMap, io::Write};

/// An `SpacesdCommand` for argv.
pub fn command_of(argv: &[String]) -> SpacesdCommand {
    SpacesdCommand {
        program: argv[0].clone(),
        args: argv[1..].to_vec(),
        env: HashMap::new(),
        cwd: None,
        user: None,
        timeout_ms: None,
        tag: None,
        stdin: false,
        pty: None,
        request_id: None,
        epoch: None,
        scrollback_bytes: None,
        kill_on_disconnect: false,
    }
}

/// Runs argv to completion, streaming nothing; prints stdout/stderr and
/// returns the exit code.
pub async fn exec(
    env: &SpacesdClient,
    argv: &[String],
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    exec_with(env, argv, StreamOpts::default(), out).await
}

/// `cmd` run as a user, in a directory, with a time limit.
pub fn with_opts(mut cmd: SpacesdCommand, opts: StreamOpts) -> SpacesdCommand {
    cmd.user = opts.user;
    cmd.cwd = opts.cwd;
    cmd.timeout_ms = opts.timeout_ms;
    cmd
}

/// [`exec`] as a user, in a directory, with a time limit.
pub async fn exec_with(
    env: &SpacesdClient,
    argv: &[String],
    opts: StreamOpts,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let o = env.run(with_opts(command_of(argv), opts)).await?;
    out.write_all(&o.stdout).ok();
    std::io::stderr().write_all(&o.stderr).ok();
    Ok(exit_code(&o))
}

/// Options for [`stream`].
#[derive(Debug, Clone, Default)]
pub struct StreamOpts {
    /// Run as this guest user.
    pub user: Option<String>,
    /// Start in this guest directory.
    pub cwd: Option<String>,
    /// Stop the process after this many milliseconds (default: never).
    pub timeout_ms: Option<u32>,
}

/// Flags `exec` takes on both `cua spacesd` and `cua sandbox`.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct ExecFlags {
    /// Stream: run the command as argv (no shell), forward this command's
    /// stdin, write stdout and stderr as they arrive, set no time limit,
    /// pass SIGINT/SIGTERM/SIGHUP on, and kill the command when this process
    /// ends. Exits with the command's status (128 + n for signal n).
    #[arg(short = 'i', long)]
    pub stdin: bool,
    /// Run as this guest user.
    #[arg(long)]
    pub user: Option<String>,
    /// Start in this guest directory.
    #[arg(long)]
    pub cwd: Option<String>,
    /// Stop the command after this many seconds (0: no limit).
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u32>,
}

impl ExecFlags {
    /// The options, with `default_ms` when `--timeout` is not given.
    pub fn opts(&self, default_ms: Option<u32>) -> StreamOpts {
        StreamOpts {
            user: self.user.clone(),
            cwd: self.cwd.clone(),
            timeout_ms: match self.timeout {
                Some(0) => None,
                Some(s) => Some(s.saturating_mul(1000)),
                None => default_ms,
            },
        }
    }

    /// Whether anything beyond a plain command is asked for.
    pub fn any(&self) -> bool {
        self.stdin || self.user.is_some() || self.cwd.is_some() || self.timeout.is_some()
    }
}

/// Names of the signals [`stream`] forwards, as `SpacesdProcess::signal`
/// takes them.
pub type SignalRx = tokio::sync::mpsc::UnboundedReceiver<&'static str>;

/// Forwards this process's SIGINT, SIGTERM and SIGHUP into the returned
/// channel until the receiver is dropped (Unix; elsewhere never fires).
pub fn os_signals() -> SignalRx {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        for (kind, name) in [
            (SignalKind::interrupt(), "int"),
            (SignalKind::terminate(), "term"),
            (SignalKind::hangup(), "hup"),
        ] {
            let Ok(mut s) = signal(kind) else { continue };
            let tx = tx.clone();
            tokio::spawn(async move {
                while s.recv().await.is_some() {
                    if tx.send(name).is_err() {
                        break;
                    }
                }
            });
        }
    }
    #[cfg(not(unix))]
    drop(tx);
    rx
}

/// Shell exit status for a process ended by a signal: 128 + its number.
fn signal_exit_code(name: &str) -> i32 {
    let n = match name {
        "hup" => 1,
        "int" => 2,
        "quit" => 3,
        "kill" => 9,
        "usr1" => 10,
        "usr2" => 12,
        "term" => 15,
        _ => 0,
    };
    128 + n
}

/// Aborts its task when dropped, so a cancelled [`stream`] leaves no
/// helper holding the guest connection open.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Bytes one read of stdin takes.
const STDIN_READ: usize = 64 * 1024;
/// Most bytes one `SendInput` carries. Each send is a round trip, which
/// through the relay is 100 ms or more, so stdin goes in as few sends as
/// possible: 16 KiB per round trip held a stream at about 60 KB/s.
const STDIN_BATCH: usize = 1024 * 1024;

/// Forwards `stdin` to `send`, one batch at a time and in order, until end
/// of input; `true` then, `false` when a send failed. While a batch is in
/// flight, what is read meanwhile is joined into the next one (up to
/// [`STDIN_BATCH`]). A read that would block (a non-blocking descriptor
/// handed down by a parent, as rsync's `-e` does) or was interrupted is
/// tried again, not taken for end of input.
async fn forward_stdin<R, S, F>(mut stdin: R, mut send: S) -> bool
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    S: FnMut(Vec<u8>) -> F,
    F: std::future::Future<Output = bool>,
{
    use tokio::io::AsyncReadExt;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    let reader = AbortOnDrop(tokio::spawn(async move {
        let mut buf = vec![0u8; STDIN_READ];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Err(_) => break,
            }
        }
    }));
    while let Some(mut batch) = rx.recv().await {
        while batch.len() < STDIN_BATCH {
            match rx.try_recv() {
                Ok(more) => batch.extend_from_slice(&more),
                Err(_) => break,
            }
        }
        if !send(batch).await {
            drop(reader);
            return false;
        }
    }
    true
}

/// Runs argv in the guest as a long-lived stdio stream: `stdin` is forwarded
/// (EOF closes the guest's stdin), stdout and stderr are written as they
/// arrive, and the call returns the process's exit status (128 + n when a
/// signal ended it). There is no time limit unless `opts.timeout_ms` is set.
/// Signals received on `signals` are forwarded to the guest process group, and the
/// guest process is killed when this client's connection ends (so a host
/// that dies, even by SIGKILL, leaves nothing running).
pub async fn stream<R>(
    env: &SpacesdClient,
    argv: &[String],
    opts: StreamOpts,
    stdin: R,
    mut signals: SignalRx,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, CuaError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut cmd = with_opts(command_of(argv), opts);
    cmd.stdin = true;
    cmd.kill_on_disconnect = true;
    let p = env.spawn(cmd).await?;
    let writer = p.clone();
    let _input = AbortOnDrop(tokio::spawn(async move {
        let sent = forward_stdin(stdin, |batch| {
            let w = writer.clone();
            async move { w.write_stdin(batch).await.is_ok() }
        })
        .await;
        if sent {
            let _ = writer.close_stdin().await;
        }
    }));
    let forward = p.clone();
    let _sigs = AbortOnDrop(tokio::spawn(async move {
        while let Some(name) = signals.recv().await {
            let _ = forward.signal(name.into(), true).await;
        }
    }));
    async {
        loop {
            let Some(ev) = p.next_event().await? else {
                return Ok(1);
            };
            if let Some(exit) = ev.exit {
                if let Some(e) = &exit.error {
                    let _ = writeln!(err, "cua: {e}");
                    return Ok(127);
                }
                if exit.timed_out {
                    let _ = writeln!(err, "cua: timed out");
                    return Ok(124);
                }
                return Ok(match (exit.code, &exit.signal) {
                    (Some(c), _) => c,
                    (None, Some(sig)) => signal_exit_code(sig),
                    (None, None) => 1,
                });
            }
            match ev.kind {
                cua_sdk::ProcessEventKind::Stderr => {
                    let _ = err.write_all(&ev.data);
                    let _ = err.flush();
                }
                _ => {
                    let _ = out.write_all(&ev.data);
                    let _ = out.flush();
                }
            }
        }
    }
    .await
}

/// Exit code of a finished process.
pub fn exit_code(o: &cua_sdk::ProcessOutput) -> i32 {
    o.exit.code.unwrap_or(if o.exit.success { 0 } else { 1 })
}

fn terminal_size() -> (u32, u32) {
    #[cfg(unix)]
    {
        // SAFETY: TIOCGWINSZ fills a plain struct.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
                return (ws.ws_col as u32, ws.ws_row as u32);
            }
        }
    }
    (120, 40)
}

/// Puts the local terminal in raw mode until dropped (Unix, TTY only).
struct RawMode {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl RawMode {
    fn enable() -> Self {
        #[cfg(unix)]
        {
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() {
                return Self { saved: None };
            }
            // SAFETY: termios calls on stdin with a zeroed, then filled struct.
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                    return Self { saved: None };
                }
                let saved = t;
                libc::cfmakeraw(&mut t);
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
                Self { saved: Some(saved) }
            }
        }
        #[cfg(not(unix))]
        Self {}
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(t) = self.saved {
            // SAFETY: restores the attributes read in `enable`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
            }
        }
    }
}

/// An interactive PTY session: raw local terminal, window-size tracking,
/// stdin forwarded byte for byte. Runs `command` (default: the login
/// shell, `/bin/sh -i` fallback).
pub async fn interactive(
    env: &SpacesdClient,
    command: Option<Vec<String>>,
    cols: Option<u32>,
    rows: Option<u32>,
) -> Result<i32, CuaError> {
    let (c, r) = terminal_size();
    let argv = command.filter(|c| !c.is_empty()).unwrap_or_else(|| {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "exec ${SHELL:-/bin/sh} -i".into(),
        ]
    });
    let mut cmd = command_of(&argv);
    cmd.stdin = true;
    cmd.pty = Some(PtySize {
        cols: cols.unwrap_or(c),
        rows: rows.unwrap_or(r),
    });
    let p = env.spawn(cmd).await?;
    let raw = RawMode::enable();
    let writer = p.clone();
    let input = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut stdin = tokio::io::stdin();
        let mut buf = vec![0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    let _ = writer.write_pty(vec![4]).await; // EOT
                    break;
                }
                Ok(n) => {
                    if writer.write_pty(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    #[cfg(unix)]
    let resize = {
        let fixed = cols.is_some() || rows.is_some();
        let p = p.clone();
        tokio::spawn(async move {
            if fixed {
                return;
            }
            let Ok(mut s) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())
            else {
                return;
            };
            while s.recv().await.is_some() {
                let (c, r) = terminal_size();
                if p.resize(c, r).await.is_err() {
                    break;
                }
            }
        })
    };
    let mut code = 0;
    let result = async {
        while let Some(ev) = p.next_event().await? {
            if let Some(exit) = ev.exit {
                code = exit.code.unwrap_or(1);
                break;
            }
            let mut o = std::io::stdout();
            let _ = o.write_all(&ev.data);
            let _ = o.flush();
        }
        Ok::<(), CuaError>(())
    }
    .await;
    input.abort();
    #[cfg(unix)]
    resize.abort();
    drop(raw);
    result?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_daemon::fixtures;
    use cua_sdk::{Cua, CuaConfig};
    use std::{sync::Arc, time::Duration};
    use tokio::sync::mpsc::unbounded_channel;

    async fn client(url: &str) -> Arc<SpacesdClient> {
        let home = tempfile::tempdir().unwrap();
        let cua = Cua::embedded(CuaConfig {
            state_dir: Some(home.path().display().to_string()),
            ..Default::default()
        })
        .unwrap();
        cua.spacesd(url.to_string(), Some("t".into()))
            .await
            .unwrap()
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn no_signals() -> SignalRx {
        unbounded_channel().1
    }

    /// Runs `stream` to the end with `input` as stdin.
    async fn run(
        env: &SpacesdClient,
        words: &[&str],
        opts: StreamOpts,
        input: &'static [u8],
    ) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = stream(
            env,
            &argv(words),
            opts,
            input,
            no_signals(),
            &mut out,
            &mut err,
        )
        .await
        .unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stdin_round_trips_and_the_start_request_is_a_stream() {
        let fx = fixtures::start_env(Some("t"), None).await;
        let env = client(&fx.url).await;
        let opts = StreamOpts {
            user: Some("agent".into()),
            cwd: Some("/work".into()),
            timeout_ms: None,
        };
        let (code, out, _) = run(&env, &["cat"], opts, b"hello\nworld").await;
        assert_eq!((code, out.as_str()), (0, "hello\nworld"));
        let started = fx.mock.state.started_processes();
        let req = started.last().unwrap();
        let cfg = req.config.as_ref().unwrap();
        assert!(req.stdin && req.kill_on_disconnect && req.pty.is_none());
        assert!(cfg.timeout.is_none(), "a stream has no time limit");
        assert_eq!((cfg.user.as_str(), cfg.cwd.as_str()), ("agent", "/work"));
        assert_eq!(cfg.command, "cat", "argv is passed through, not joined");
    }

    #[tokio::test]
    async fn stdin_goes_in_few_large_sends_while_one_is_in_flight() {
        let input: Vec<u8> = (0..4 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
        let (mut w, r) = tokio::io::duplex(STDIN_READ);
        let data = input.clone();
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            w.write_all(&data).await.unwrap();
        });
        let got = Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
        let g = got.clone();
        let ok = forward_stdin(r, move |b| {
            let g = g.clone();
            async move {
                // A slow round trip, as through the relay.
                tokio::time::sleep(Duration::from_millis(50)).await;
                g.lock().unwrap().push(b);
                true
            }
        })
        .await;
        writer.await.unwrap();
        assert!(ok);
        let sends = got.lock().unwrap();
        assert_eq!(sends.concat(), input, "bytes arrive whole and in order");
        assert!(
            sends.len() <= 16,
            "{} sends for 4 MiB: reads are batched while a send is in flight",
            sends.len()
        );
        assert!(sends.iter().all(|b| b.len() <= STDIN_BATCH + STDIN_READ));
    }

    /// Stdin that says "would block" (a non-blocking descriptor) before
    /// its data.
    struct WouldBlockFirst(u8, &'static [u8]);

    impl tokio::io::AsyncRead for WouldBlockFirst {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.0 > 0 {
                self.0 -= 1;
                return std::task::Poll::Ready(Err(std::io::ErrorKind::WouldBlock.into()));
            }
            let data = std::mem::take(&mut self.1);
            buf.put_slice(data);
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_read_that_would_block_is_retried_not_end_of_input() {
        let got = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let g = got.clone();
        let ok = forward_stdin(WouldBlockFirst(3, b"after EAGAIN"), move |b| {
            g.lock().unwrap().extend(b);
            async { true }
        })
        .await;
        assert!(ok);
        assert_eq!(got.lock().unwrap().as_slice(), b"after EAGAIN");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_codes_and_stderr_propagate() {
        let fx = fixtures::start_env(Some("t"), None).await;
        let env = client(&fx.url).await;
        let (code, out, err) = run(&env, &["fail", "7"], StreamOpts::default(), b"").await;
        assert_eq!((code, out.as_str(), err.as_str()), (7, "", "failed\n"));
        let (code, _, err) = run(&env, &["nope"], StreamOpts::default(), b"").await;
        assert_eq!(code, 127, "{err}");
        assert!(err.contains("executable not found"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn signals_are_forwarded_and_become_128_plus_n() {
        let fx = fixtures::start_env(Some("t"), None).await;
        let env = client(&fx.url).await;
        let (tx, rx) = unbounded_channel();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        // Stdin that never ends, so only the signal can finish `cat`.
        let (_keep, held) = tokio::io::duplex(8);
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            tx.send("term").unwrap();
        });
        let code = stream(
            &env,
            &argv(&["cat"]),
            StreamOpts::default(),
            held,
            rx,
            &mut out,
            &mut err,
        )
        .await
        .unwrap();
        sender.await.unwrap();
        assert_eq!(code, 128 + 15);
        let signals = fx.mock.state.signals();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].1, cua_spacesd_client::pb::Signal::Term as i32);
        assert!(
            signals[0].2,
            "a client signal reaches the whole process group"
        );
    }
}
