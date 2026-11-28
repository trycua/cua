// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Client end of the daemon's local socket or named pipe.
//!
//! [`LocalIpcClient`] is the one synchronous client: the daemon request path
//! and the SDK's trusted service session both send newline-delimited JSON
//! through it, with the same connect retry, poll slices and deadline on every
//! OS. [`LocalIpcStream`] is the async connection the MCP proxy and the MCP
//! envelope relay hold open. Framing beyond one line per message, and the
//! request protocol itself, belong to the callers.

#[cfg(windows)]
mod windows;

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

#[cfg(windows)]
pub(crate) use self::windows::wait_named_pipe;

/// How a connect retries while the endpoint is not accepting yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectPolicy {
    /// Pause between attempts.
    pub retry: Duration,
    /// How long after the first attempt a failed attempt is retried.
    pub budget: Duration,
    /// Retry only while the endpoint is there but busy: a Unix listener whose
    /// accept queue is full. Any other failure ends the connect at once.
    pub only_while_busy: bool,
}

impl ConnectPolicy {
    /// One attempt.
    pub const ONCE: Self = Self {
        retry: Duration::ZERO,
        budget: Duration::ZERO,
        only_while_busy: false,
    };

    /// Retry for up to 3 s, for a daemon that may still be starting.
    pub const STARTUP: Self = Self {
        retry: Duration::from_millis(50),
        budget: Duration::from_secs(3),
        only_while_busy: false,
    };

    /// One request to a daemon that is already serving. A named pipe refuses
    /// a client while every instance is busy, until the daemon creates the
    /// next one, so Windows retries as [`Self::STARTUP`] does. A Unix listener
    /// queues the connection instead, so a refused connect means no daemon.
    /// Unix retries only a full accept queue, for as long as
    /// [`Self::STARTUP`] would.
    #[cfg(windows)]
    pub const REQUEST: Self = Self::STARTUP;
    #[cfg(not(windows))]
    pub const REQUEST: Self = Self {
        only_while_busy: true,
        ..Self::STARTUP
    };

    /// The same policy with its budget cut to `limit`.
    pub fn within(self, limit: Duration) -> Self {
        Self {
            budget: self.budget.min(limit),
            ..self
        }
    }

    /// The pause before the next attempt, cut to what is left of the budget,
    /// or `None` once the budget is spent.
    fn pause(self, give_up: Instant, error: &std::io::Error) -> Option<Duration> {
        if self.only_while_busy && error.kind() != std::io::ErrorKind::WouldBlock {
            return None;
        }
        let left = give_up.saturating_duration_since(Instant::now());
        (!left.is_zero()).then(|| self.retry.min(left))
    }

    fn retry<T>(self, mut attempt: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
        let give_up = Instant::now() + self.budget;
        loop {
            match attempt() {
                Ok(connected) => return Ok(connected),
                Err(error) => match self.pause(give_up, &error) {
                    Some(pause) => std::thread::sleep(pause),
                    None => return Err(error),
                },
            }
        }
    }
}

/// Longest one read waits before the exchange checks its deadline again.
const READ_POLL: Duration = Duration::from_secs(10);
/// Longest one write waits before the exchange checks its deadline again.
const WRITE_POLL: Duration = Duration::from_secs(5);

/// Why [`LocalIpcClient::exchange_line`] returned no reply line.
#[derive(Debug)]
pub enum ExchangeError {
    /// Sending the request failed, or the peer did not drain it in time.
    Write(std::io::Error),
    /// Reading the reply failed, or the reply is not UTF-8.
    Read(std::io::Error),
    /// The deadline passed before a full reply line arrived.
    TimedOut { received: usize },
    /// The peer closed the connection before sending any reply byte.
    Closed,
}

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;

/// One connection attempt to the Unix socket at `socket_path` that never
/// waits. A listener whose accept queue is full answers `WouldBlock` rather
/// than holding the caller, as a blocking connect does on Linux. The stream it
/// returns is blocking.
#[cfg(unix)]
pub fn try_connect_unix(socket_path: &str) -> std::io::Result<std::os::unix::net::UnixStream> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};

    let path = socket_path.as_bytes();
    // SAFETY: an all-zero sockaddr_un is a valid empty address.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path.len() >= address.sun_path.len() {
        return Err(std::io::Error::from_raw_os_error(libc::ENAMETOOLONG));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in address.sun_path.iter_mut().zip(path) {
        *slot = libc::c_char::from_ne_bytes([*byte]);
    }
    // SAFETY: plain socket creation; the descriptor is owned right away.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `raw` is a fresh descriptor nothing else owns.
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: fcntl on a descriptor this function owns.
    let ready = unsafe {
        libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) == 0
            && libc::fcntl(raw, libc::F_SETFL, libc::O_NONBLOCK) == 0
    };
    if !ready {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `address` is a valid, NUL-terminated sockaddr_un of the length
    // passed.
    let connected = unsafe {
        libc::connect(
            socket.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
        )
    };
    if connected != 0 {
        let error = std::io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::EAGAIN | libc::EINPROGRESS) => std::io::ErrorKind::WouldBlock.into(),
            _ => error,
        });
    }
    let stream = std::os::unix::net::UnixStream::from(socket);
    stream.set_nonblocking(false)?;
    Ok(stream)
}
#[cfg(windows)]
type Stream = self::windows::PipeStream;

/// A synchronous connection to a Unix socket or named pipe.
#[derive(Debug)]
pub struct LocalIpcClient {
    stream: Stream,
    /// Bytes read past the last reply line.
    pending: Vec<u8>,
}

impl LocalIpcClient {
    pub fn connect(path: &str, policy: ConnectPolicy) -> std::io::Result<Self> {
        #[cfg(unix)]
        let stream = policy.retry(|| try_connect_unix(path))?;
        #[cfg(windows)]
        let stream = policy.retry(|| Stream::connect(path))?;
        // Every read and write waits in `poll` for at most what is left of
        // its slice, so none can outlast the exchange deadline.
        #[cfg(unix)]
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            pending: Vec::new(),
        })
    }

    /// Send `request` and its newline, then read one reply line, without its
    /// newline. A reply the peer ends by closing the connection is returned
    /// as it is. No read or write runs past `deadline`.
    pub fn exchange_line(
        &mut self,
        request: &[u8],
        deadline: Instant,
    ) -> Result<String, ExchangeError> {
        self.exchange_polling(request, deadline, READ_POLL)
    }

    fn exchange_polling(
        &mut self,
        request: &[u8],
        deadline: Instant,
        read_poll: Duration,
    ) -> Result<String, ExchangeError> {
        let mut line = Vec::with_capacity(request.len() + 1);
        line.extend_from_slice(request);
        line.push(b'\n');
        write_all(&mut self.stream, &line, deadline).map_err(ExchangeError::Write)?;
        read_line(&mut self.stream, &mut self.pending, deadline, read_poll)
    }
}

/// One read or write that waits at most `timeout` for the peer. A wait that
/// runs out fails with `TimedOut` or `WouldBlock`.
trait Transport {
    fn read_within(&mut self, buffer: &mut [u8], timeout: Duration) -> std::io::Result<usize>;
    fn write_within(&mut self, buffer: &[u8], timeout: Duration) -> std::io::Result<usize>;
}

/// On a nonblocking socket.
#[cfg(unix)]
impl Transport for std::os::unix::net::UnixStream {
    fn read_within(&mut self, buffer: &mut [u8], timeout: Duration) -> std::io::Result<usize> {
        use std::io::Read as _;
        let stream: &Self = self;
        when_ready(stream, libc::POLLIN, timeout, || {
            let mut stream = stream;
            stream.read(buffer)
        })
    }

    fn write_within(&mut self, buffer: &[u8], timeout: Duration) -> std::io::Result<usize> {
        use std::io::Write as _;
        let stream: &Self = self;
        when_ready(stream, libc::POLLOUT, timeout, || {
            let mut stream = stream;
            stream.write(buffer)
        })
    }
}

/// Run `io`; if the socket is not ready, wait up to `timeout` for `events`
/// and run it once more.
#[cfg(unix)]
fn when_ready<T>(
    stream: &std::os::unix::net::UnixStream,
    events: libc::c_short,
    timeout: Duration,
    mut io: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    use std::os::fd::AsRawFd as _;
    match io() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            let mut descriptor = libc::pollfd {
                fd: stream.as_raw_fd(),
                events,
                revents: 0,
            };
            let milliseconds = timeout
                .as_nanos()
                .div_ceil(1_000_000)
                .min(libc::c_int::MAX as u128) as libc::c_int;
            // SAFETY: one valid `pollfd` that outlives the call.
            match unsafe { libc::poll(&mut descriptor, 1, milliseconds) } {
                -1 => Err(std::io::Error::last_os_error()),
                0 => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("socket not ready after {milliseconds} ms"),
                )),
                _ => io(),
            }
        }
        other => other,
    }
}

fn is_poll_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
    )
}

/// Write all of `bytes`, treating a write that times out as a peer that is
/// still draining, until `deadline`.
fn write_all<S: Transport>(stream: &mut S, bytes: &[u8], deadline: Instant) -> std::io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "daemon did not drain the socket in time (wrote {written}/{} bytes)",
                    bytes.len()
                ),
            ));
        }
        match stream.write_within(&bytes[written..], remaining.min(WRITE_POLL)) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "daemon closed the connection mid-request",
                ));
            }
            Ok(sent) => written += sent,
            Err(error) if is_poll_timeout(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Read up to the next newline, keeping any bytes after it in `pending` for
/// the next line.
fn read_line<S: Transport>(
    stream: &mut S,
    pending: &mut Vec<u8>,
    deadline: Instant,
    poll: Duration,
) -> Result<String, ExchangeError> {
    let mut chunk = vec![0_u8; 64 * 1024];
    let mut scanned = 0;
    let line = loop {
        if let Some(newline) = pending[scanned..].iter().position(|&byte| byte == b'\n') {
            let rest = pending.split_off(scanned + newline + 1);
            let mut line = std::mem::replace(pending, rest);
            line.truncate(scanned + newline);
            break line;
        }
        scanned = pending.len();
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ExchangeError::TimedOut {
                received: pending.len(),
            });
        }
        match stream.read_within(&mut chunk, remaining.min(poll)) {
            Ok(0) if pending.is_empty() => return Err(ExchangeError::Closed),
            Ok(0) => break std::mem::take(pending),
            Ok(read) => pending.extend_from_slice(&chunk[..read]),
            Err(error) if is_poll_timeout(&error) => {}
            Err(error) => return Err(ExchangeError::Read(error)),
        }
    };
    String::from_utf8(line).map_err(|error| {
        ExchangeError::Read(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

#[cfg(unix)]
type AsyncStream = tokio::net::UnixStream;
#[cfg(windows)]
type AsyncStream = tokio::net::windows::named_pipe::NamedPipeClient;

/// An async connection to a Unix socket or named pipe.
#[derive(Debug)]
pub struct LocalIpcStream(AsyncStream);

impl LocalIpcStream {
    pub async fn connect(path: &str, policy: ConnectPolicy) -> std::io::Result<Self> {
        let give_up = Instant::now() + policy.budget;
        loop {
            #[cfg(unix)]
            let attempt = AsyncStream::connect(path).await;
            #[cfg(windows)]
            let attempt = tokio::net::windows::named_pipe::ClientOptions::new().open(path);
            match attempt {
                Ok(stream) => return Ok(Self(stream)),
                Err(error) => match policy.pause(give_up, &error) {
                    Some(pause) => tokio::time::sleep(pause).await,
                    None => return Err(error),
                },
            }
        }
    }
}

impl tokio::io::AsyncRead for LocalIpcStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for LocalIpcStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{read_line, write_all, ConnectPolicy, ExchangeError, Transport};
    use std::io::{Error, ErrorKind};
    use std::time::{Duration, Instant};

    /// Refuses its first `stalls` writes with `WouldBlock`, then takes at most
    /// `chunk` bytes per write.
    struct SlowPeer {
        stalls: usize,
        chunk: usize,
        written: Vec<u8>,
    }

    impl Transport for SlowPeer {
        fn write_within(&mut self, buf: &[u8], _: Duration) -> std::io::Result<usize> {
            if self.stalls > 0 {
                self.stalls -= 1;
                return Err(Error::new(ErrorKind::WouldBlock, "stalled"));
            }
            let taken = buf.len().min(self.chunk);
            self.written.extend_from_slice(&buf[..taken]);
            Ok(taken)
        }
        fn read_within(&mut self, _: &mut [u8], _: Duration) -> std::io::Result<usize> {
            Err(Error::new(ErrorKind::WouldBlock, "nothing yet"))
        }
    }

    #[test]
    fn a_retry_never_sleeps_past_its_budget() {
        let policy = ConnectPolicy {
            retry: Duration::from_secs(5),
            budget: Duration::from_millis(100),
            only_while_busy: false,
        };
        let started = Instant::now();
        let mut attempts = 0;
        let error = policy
            .retry(|| -> std::io::Result<()> {
                attempts += 1;
                Err(Error::new(ErrorKind::NotFound, "absent"))
            })
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert_eq!(attempts, 2);
        assert!(started.elapsed() < Duration::from_millis(600));
    }

    #[test]
    fn a_busy_only_retry_gives_up_at_once_on_anything_but_a_full_queue() {
        let policy = ConnectPolicy {
            retry: Duration::from_millis(10),
            budget: Duration::from_secs(3),
            only_while_busy: true,
        };
        let mut attempts = 0;
        let error = policy
            .retry(|| -> std::io::Result<()> {
                attempts += 1;
                Err(Error::new(ErrorKind::ConnectionRefused, "no daemon"))
            })
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ConnectionRefused);
        assert_eq!(attempts, 1);

        let mut attempts = 0;
        policy
            .retry(|| {
                attempts += 1;
                if attempts < 3 {
                    Err(Error::new(ErrorKind::WouldBlock, "queue full"))
                } else {
                    Ok(())
                }
            })
            .unwrap();
        assert_eq!(attempts, 3);
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn a_write_waits_out_a_peer_that_drains_slowly() {
        let mut peer = SlowPeer {
            stalls: 3,
            chunk: 2,
            written: Vec::new(),
        };
        write_all(&mut peer, b"abcdefg\n", soon()).unwrap();
        assert_eq!(peer.written, b"abcdefg\n");
    }

    #[test]
    fn a_write_to_a_peer_that_never_drains_stops_at_the_deadline() {
        let mut peer = SlowPeer {
            stalls: usize::MAX,
            chunk: 0,
            written: Vec::new(),
        };
        let error = write_all(
            &mut peer,
            b"hello\n",
            Instant::now() + Duration::from_millis(30),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert_eq!(
            error.to_string(),
            "daemon did not drain the socket in time (wrote 0/6 bytes)"
        );
    }

    #[test]
    fn a_read_that_never_ends_its_line_stops_at_the_deadline() {
        let mut peer = SlowPeer {
            stalls: 0,
            chunk: 0,
            written: Vec::new(),
        };
        let mut pending = b"{\"ok\":".to_vec();
        let error = read_line(
            &mut peer,
            &mut pending,
            Instant::now() + Duration::from_millis(30),
            Duration::from_millis(5),
        )
        .unwrap_err();
        assert!(
            matches!(error, ExchangeError::TimedOut { received: 6 }),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    mod unix {
        use super::super::{ConnectPolicy, ExchangeError, LocalIpcClient, LocalIpcStream};
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::time::{Duration, Instant};

        fn socket_dir() -> tempfile::TempDir {
            tempfile::Builder::new()
                .prefix("cua-ipc-")
                .tempdir_in("/tmp")
                .unwrap()
        }

        /// Accept one client, read its request line and hand both to `serve`.
        fn serve_one(
            listener: UnixListener,
            serve: impl FnOnce(String, UnixStream) + Send + 'static,
        ) -> std::thread::JoinHandle<()> {
            std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                serve(request, stream);
            })
        }

        fn connected(
            serve: impl FnOnce(String, UnixStream) + Send + 'static,
        ) -> (
            LocalIpcClient,
            std::thread::JoinHandle<()>,
            tempfile::TempDir,
        ) {
            let directory = socket_dir();
            let path = directory.path().join("ipc.sock");
            let server = serve_one(UnixListener::bind(&path).unwrap(), serve);
            let client =
                LocalIpcClient::connect(path.to_str().unwrap(), ConnectPolicy::ONCE).unwrap();
            (client, server, directory)
        }

        #[test]
        fn an_exchange_sends_one_line_and_keeps_what_follows_the_reply() {
            let (mut client, server, _directory) = connected(|request, mut stream| {
                assert_eq!(request, "ping\n");
                stream.write_all(b"first\nsecond\n").unwrap();
                std::thread::sleep(Duration::from_millis(200));
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            assert_eq!(client.exchange_line(b"ping", deadline).unwrap(), "first");
            assert_eq!(client.exchange_line(b"next", deadline).unwrap(), "second");
            server.join().unwrap();
        }

        #[test]
        fn a_poll_slice_that_expires_keeps_waiting_and_keeps_split_utf8() {
            let (mut client, server, _directory) = connected(|_, mut stream| {
                stream.write_all(&[0xe2]).unwrap();
                std::thread::sleep(Duration::from_millis(120));
                stream.write_all(&[0x82, 0xac, b'\n']).unwrap();
            });
            let line = client
                .exchange_polling(
                    b"ping",
                    Instant::now() + Duration::from_secs(5),
                    Duration::from_millis(20),
                )
                .unwrap();
            assert_eq!(line, "€");
            server.join().unwrap();
        }

        #[test]
        fn a_stalling_server_hits_the_deadline() {
            let (release, released) = std::sync::mpsc::channel::<()>();
            let (mut client, server, _directory) = connected(move |_, mut stream| {
                stream.write_all(b"{\"ok\":").unwrap();
                let _ = released.recv();
            });
            let started = Instant::now();
            let error = client
                .exchange_line(b"ping", Instant::now() + Duration::from_millis(300))
                .unwrap_err();
            assert!(
                matches!(error, ExchangeError::TimedOut { received: 6 }),
                "{error:?}"
            );
            assert!(started.elapsed() >= Duration::from_millis(300));
            assert!(started.elapsed() < Duration::from_millis(500));
            release.send(()).unwrap();
            server.join().unwrap();
        }

        #[test]
        fn a_server_that_never_reads_hits_the_write_deadline() {
            let directory = socket_dir();
            let path = directory.path().join("full.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let (release, released) = std::sync::mpsc::channel::<()>();
            let server = std::thread::spawn(move || {
                let (_stream, _) = listener.accept().unwrap();
                let _ = released.recv();
            });
            let mut client =
                LocalIpcClient::connect(path.to_str().unwrap(), ConnectPolicy::ONCE).unwrap();
            let request = vec![b'x'; 16 * 1024 * 1024];
            let started = Instant::now();
            let error = client
                .exchange_line(&request, Instant::now() + Duration::from_millis(200))
                .unwrap_err();
            let ExchangeError::Write(error) = error else {
                panic!("{error:?}");
            };
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(
                started.elapsed() < Duration::from_millis(400),
                "{:?}",
                started.elapsed()
            );
            release.send(()).unwrap();
            server.join().unwrap();
        }

        #[test]
        fn a_reply_that_trickles_without_a_newline_hits_the_deadline() {
            let (mut client, server, _directory) = connected(|_, mut stream| {
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(3) && stream.write_all(b"x").is_ok() {
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            let started = Instant::now();
            let error = client
                .exchange_polling(
                    b"ping",
                    Instant::now() + Duration::from_millis(300),
                    Duration::from_millis(50),
                )
                .unwrap_err();
            assert!(matches!(error, ExchangeError::TimedOut { .. }), "{error:?}");
            assert!(started.elapsed() < Duration::from_secs(2));
            drop(client);
            server.join().unwrap();
        }

        #[test]
        fn a_close_before_any_reply_reads_as_closed_and_after_some_as_the_reply() {
            let (mut client, server, _directory) = connected(|_, _| {});
            let deadline = Instant::now() + Duration::from_secs(5);
            assert!(matches!(
                client.exchange_line(b"ping", deadline),
                Err(ExchangeError::Closed)
            ));
            server.join().unwrap();

            let (mut client, server, _directory) = connected(|_, mut stream| {
                stream.write_all(b"{\"ok\":true}").unwrap();
            });
            assert_eq!(
                client.exchange_line(b"ping", deadline).unwrap(),
                "{\"ok\":true}"
            );
            server.join().unwrap();
        }

        #[test]
        fn a_connect_retries_only_within_its_budget() {
            let directory = socket_dir();
            let path = directory.path().join("late.sock");
            let started = Instant::now();
            let error =
                LocalIpcClient::connect(path.to_str().unwrap(), ConnectPolicy::ONCE).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            assert!(started.elapsed() < Duration::from_millis(100));

            let late = path.clone();
            let binder = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                let listener = UnixListener::bind(late).unwrap();
                let _ = listener.accept().unwrap();
            });
            let policy = ConnectPolicy {
                retry: Duration::from_millis(20),
                budget: Duration::from_secs(3),
                only_while_busy: false,
            };
            LocalIpcClient::connect(path.to_str().unwrap(), policy).unwrap();
            binder.join().unwrap();
        }

        #[tokio::test]
        async fn an_async_connect_retries_only_within_its_budget() {
            let directory = socket_dir();
            let path = directory.path().join("late-async.sock");
            let error = LocalIpcStream::connect(path.to_str().unwrap(), ConnectPolicy::ONCE)
                .await
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

            let late = path.clone();
            let binder = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                let listener = UnixListener::bind(late).unwrap();
                let _ = listener.accept().unwrap();
            });
            let policy = ConnectPolicy {
                retry: Duration::from_millis(20),
                budget: Duration::from_secs(3),
                only_while_busy: false,
            };
            LocalIpcStream::connect(path.to_str().unwrap(), policy)
                .await
                .unwrap();
            binder.join().unwrap();
        }
    }
}
