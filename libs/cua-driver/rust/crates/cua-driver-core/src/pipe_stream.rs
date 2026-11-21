// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Named-pipe client with socket-style read and write timeouts.
//!
//! A pipe opened through `std::fs` blocks in `ReadFile` until the daemon
//! answers, so a stalled daemon hangs its client forever. This client opens the
//! pipe for overlapped I/O and waits on each operation for at most its timeout.
//! An operation that runs out of time is cancelled with `CancelIoEx` and
//! awaited, so no I/O is left in flight and no thread is left behind. It then
//! fails with `TimedOut`, as a socket read past `SO_RCVTIMEO` does, which lets
//! the Unix deadline loops run unchanged over it.

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle, RawHandle};
use std::time::{Duration, Instant};

/// How long [`PipeStream::connect`] retries a pipe whose instances are all
/// busy or not yet created.
pub const OPEN_RETRY: Duration = Duration::from_secs(3);

const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const ERROR_BROKEN_PIPE: i32 = 109;
const ERROR_OPERATION_ABORTED: i32 = 995;
const ERROR_IO_PENDING: i32 = 997;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const INFINITE: u32 = u32::MAX;

/// `OVERLAPPED`. Only the kernel reads its fields.
#[repr(C)]
#[allow(dead_code)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: RawHandle,
}

#[link(name = "kernel32")]
extern "system" {
    fn ReadFile(
        file: RawHandle,
        buffer: *mut u8,
        len: u32,
        read: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn WriteFile(
        file: RawHandle,
        buffer: *const u8,
        len: u32,
        written: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn GetOverlappedResult(
        file: RawHandle,
        overlapped: *const Overlapped,
        transferred: *mut u32,
        wait: i32,
    ) -> i32;
    fn CancelIoEx(file: RawHandle, overlapped: *const Overlapped) -> i32;
    fn CreateEventW(
        attributes: *const c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> RawHandle;
    fn WaitForSingleObject(handle: RawHandle, milliseconds: u32) -> u32;
}

/// One client end of a byte-mode named pipe.
#[derive(Debug)]
pub struct PipeStream {
    file: File,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    deadline: Option<Instant>,
}

impl PipeStream {
    /// Open `path`, retrying for up to `open_for` while the server has no free
    /// instance. Reads and writes start with no timeout.
    pub fn connect(path: &str, open_for: Duration) -> std::io::Result<Self> {
        let deadline = Instant::now() + open_for;
        let file = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .open(path)
            {
                Ok(file) => break file,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error),
            }
        };
        Ok(Self {
            file,
            read_timeout: None,
            write_timeout: None,
            deadline: None,
        })
    }

    /// A second handle to the same pipe, with the same timeouts and deadline.
    pub fn try_clone(&self) -> std::io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            read_timeout: self.read_timeout,
            write_timeout: self.write_timeout,
            deadline: self.deadline,
        })
    }

    /// No read or write on this handle waits past `deadline`, whatever its
    /// timeout; once it has passed they fail with `TimedOut` at once.
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }

    /// Longest one read waits; `None` waits until data or EOF arrives.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.read_timeout = timeout;
        Ok(())
    }

    /// Longest one write waits; `None` waits until the server drains it.
    pub fn set_write_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.write_timeout = timeout;
        Ok(())
    }

    /// Start one overlapped operation with `start` and wait at most `timeout`
    /// for it. Returns the bytes it transferred.
    fn transfer(
        &self,
        timeout: Option<Duration>,
        start: impl FnOnce(RawHandle, *mut Overlapped) -> i32,
    ) -> std::io::Result<usize> {
        let timeout = match self.deadline {
            None => timeout,
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "named pipe deadline has passed",
                    ));
                }
                Some(timeout.map_or(remaining, |timeout| timeout.min(remaining)))
            }
        };
        let file = self.file.as_raw_handle();
        // SAFETY: a manual-reset, unnamed event with default security.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `event` is a fresh handle this function alone owns.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        let mut overlapped = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event: event.as_raw_handle(),
        };
        // The kernel writes through this pointer until the operation settles,
        // so every call below uses it and none takes a reference.
        let overlapped = std::ptr::addr_of_mut!(overlapped);
        if start(file, overlapped) == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING) {
                return Err(broken_pipe_as_eof(error));
            }
        }

        let milliseconds = timeout.map_or(INFINITE, |timeout| {
            timeout.as_millis().clamp(1, u128::from(INFINITE - 1)) as u32
        });
        // SAFETY: `event` stays open for the whole wait.
        let timed_out = match unsafe { WaitForSingleObject(event.as_raw_handle(), milliseconds) } {
            WAIT_OBJECT_0 => false,
            WAIT_TIMEOUT => {
                // SAFETY: cancels only this operation; the wait below keeps
                // `overlapped` and the buffer alive until it settles.
                unsafe { CancelIoEx(file, overlapped) };
                true
            }
            _ => {
                let error = std::io::Error::last_os_error();
                // SAFETY: as above; the operation must settle before return.
                unsafe {
                    CancelIoEx(file, overlapped);
                    GetOverlappedResult(file, overlapped, &mut 0, 1);
                }
                return Err(error);
            }
        };

        let mut transferred = 0_u32;
        // SAFETY: waits for this operation to settle, cancelled or not.
        if unsafe { GetOverlappedResult(file, overlapped, &mut transferred, 1) } != 0 {
            return Ok(transferred as usize);
        }
        let error = std::io::Error::last_os_error();
        if timed_out && error.raw_os_error() == Some(ERROR_OPERATION_ABORTED) {
            if transferred > 0 {
                return Ok(transferred as usize);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("named pipe operation timed out after {milliseconds} ms"),
            ));
        }
        Err(broken_pipe_as_eof(error))
    }
}

/// The server closing its end reads as EOF, as it does for `std::fs::File`.
fn broken_pipe_as_eof(error: std::io::Error) -> std::io::Error {
    if error.raw_os_error() == Some(ERROR_BROKEN_PIPE) {
        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, error)
    } else {
        error
    }
}

fn eof_as_zero(result: std::io::Result<usize>) -> std::io::Result<usize> {
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
        other => other,
    }
}

impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let len = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let pointer = buffer.as_mut_ptr();
        eof_as_zero(self.transfer(self.read_timeout, |file, overlapped| {
            // SAFETY: `transfer` keeps the buffer borrowed until the read settles.
            unsafe { ReadFile(file, pointer, len, std::ptr::null_mut(), overlapped) }
        }))
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let len = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let pointer = buffer.as_ptr();
        self.transfer(self.write_timeout, |file, overlapped| {
            // SAFETY: `transfer` keeps the buffer borrowed until the write settles.
            unsafe { WriteFile(file, pointer, len, std::ptr::null_mut(), overlapped) }
        })
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, error)
            } else {
                error
            }
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PipeStream;
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::windows::named_pipe::ServerOptions;

    fn pipe_name(label: &str) -> String {
        format!(
            r"\\.\pipe\cua-driver-pipe-stream-{label}-{}",
            std::process::id()
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stalled_read_times_out_and_a_later_read_still_gets_the_reply() {
        let name = pipe_name("stalled-read");
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let served = tokio::spawn(async move {
            let mut server = server;
            server.connect().await.unwrap();
            release_rx.await.unwrap();
            server.write_all(b"late\n").await.unwrap();
            let mut rest = Vec::new();
            server.read_to_end(&mut rest).await.unwrap();
        });

        let client = tokio::task::spawn_blocking(move || {
            let mut client = PipeStream::connect(&name, Duration::from_secs(3)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let mut buffer = [0_u8; 16];
            let started = Instant::now();
            let error = client.read(&mut buffer).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(started.elapsed() >= Duration::from_millis(200));
            assert!(started.elapsed() < Duration::from_secs(5));

            release_tx.send(()).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let read = client.read(&mut buffer).unwrap();
            assert_eq!(&buffer[..read], b"late\n");
            drop(client);
        });
        client.await.unwrap();
        served.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_deadline_caps_a_longer_timeout() {
        let name = pipe_name("deadline");
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let served = tokio::spawn(async move {
            server.connect().await.unwrap();
            release_rx.await.unwrap();
        });

        tokio::task::spawn_blocking(move || {
            let mut client = PipeStream::connect(&name, Duration::from_secs(3)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            client.set_deadline(Some(Instant::now() + Duration::from_millis(200)));
            let mut buffer = [0_u8; 16];
            let started = Instant::now();
            let error = client.read(&mut buffer).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_secs(5));

            let started = Instant::now();
            let error = client.read(&mut buffer).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_millis(100));
        })
        .await
        .unwrap();
        release_tx.send(()).unwrap();
        served.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_closed_server_reads_as_eof_and_fails_writes() {
        let name = pipe_name("closed");
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let served = tokio::spawn(async move {
            let mut server = server;
            server.connect().await.unwrap();
            server.write_all(b"bye\n").await.unwrap();
            drop(server);
        });

        tokio::task::spawn_blocking(move || {
            let mut client = PipeStream::connect(&name, Duration::from_secs(3)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            client.read_to_end(&mut received).unwrap();
            assert_eq!(received, b"bye\n");
            assert!(client.write(b"x").is_err());
        })
        .await
        .unwrap();
        served.await.unwrap();
    }
}
