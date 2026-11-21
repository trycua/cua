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
//! the shared poll loops run unchanged over it.

use std::cell::Cell;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::time::Duration;

use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_OBJECT_0,
    WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile, FILE_FLAG_OVERLAPPED};
use windows::Win32::System::Pipes::{WaitNamedPipeW, NMPWAIT_NOWAIT};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// `WaitNamedPipeW` without waiting: `Ok` when an instance of `path` is free
/// to connect, without consuming it.
pub(crate) fn wait_named_pipe(path: &str) -> std::io::Result<()> {
    let wide: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    if unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), NMPWAIT_NOWAIT) }.as_bool() {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// One client end of a byte-mode named pipe.
#[derive(Debug)]
pub(super) struct PipeStream {
    file: File,
    read_timeout: Cell<Option<Duration>>,
    write_timeout: Cell<Option<Duration>>,
}

impl PipeStream {
    /// Open `path` once. Reads and writes start with no timeout.
    pub(super) fn connect(path: &str) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED.0)
            .open(path)?;
        Ok(Self {
            file,
            read_timeout: Cell::new(None),
            write_timeout: Cell::new(None),
        })
    }

    /// Start one overlapped operation with `start` and wait at most `timeout`
    /// for it. Returns the bytes it transferred.
    fn transfer(
        &self,
        timeout: Option<Duration>,
        start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> windows::core::Result<()>,
    ) -> std::io::Result<usize> {
        let file = HANDLE(self.file.as_raw_handle());
        // SAFETY: a manual-reset, unnamed event with default security.
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(os_error)?;
        // SAFETY: `event` is a fresh handle this function alone owns.
        let event = unsafe { OwnedHandle::from_raw_handle(event.0) };
        let mut overlapped = OVERLAPPED {
            hEvent: HANDLE(event.as_raw_handle()),
            ..Default::default()
        };
        // The kernel writes through this pointer until the operation settles,
        // so every call below uses it and none takes a reference.
        let overlapped = std::ptr::addr_of_mut!(overlapped);
        if let Err(error) = start(file, overlapped) {
            if !is_win32(&error, ERROR_IO_PENDING) {
                return Err(broken_pipe_as_eof(os_error(error)));
            }
        }

        let milliseconds = timeout.map_or(INFINITE, |timeout| {
            timeout.as_millis().clamp(1, u128::from(INFINITE - 1)) as u32
        });
        // SAFETY: `event` stays open for the whole wait.
        let waited = unsafe { WaitForSingleObject(HANDLE(event.as_raw_handle()), milliseconds) };
        let timed_out = if waited == WAIT_OBJECT_0 {
            false
        } else if waited == WAIT_TIMEOUT {
            // SAFETY: cancels only this operation; the wait below keeps
            // `overlapped` and the buffer alive until it settles.
            let _ = unsafe { CancelIoEx(file, Some(overlapped)) };
            true
        } else {
            let error = std::io::Error::last_os_error();
            // SAFETY: as above; the operation must settle before return.
            unsafe {
                let _ = CancelIoEx(file, Some(overlapped));
                let _ = GetOverlappedResult(file, overlapped, &mut 0, true);
            }
            return Err(error);
        };

        let mut transferred = 0_u32;
        // SAFETY: waits for this operation to settle, cancelled or not.
        match unsafe { GetOverlappedResult(file, overlapped, &mut transferred, true) } {
            Ok(()) => Ok(transferred as usize),
            Err(error) if timed_out && is_win32(&error, ERROR_OPERATION_ABORTED) => {
                if transferred > 0 {
                    return Ok(transferred as usize);
                }
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("named pipe operation timed out after {milliseconds} ms"),
                ))
            }
            Err(error) => Err(broken_pipe_as_eof(os_error(error))),
        }
    }
}

impl super::Timeouts for PipeStream {
    fn set_read_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.read_timeout.set(Some(timeout));
        Ok(())
    }

    fn set_write_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.write_timeout.set(Some(timeout));
        Ok(())
    }
}

fn is_win32(error: &windows::core::Error, code: WIN32_ERROR) -> bool {
    error.code() == HRESULT::from_win32(code.0)
}

/// The Win32 error inside `error`'s `HRESULT`, as `raw_os_error` reports it.
fn os_error(error: windows::core::Error) -> std::io::Error {
    let HRESULT(code) = error.code();
    let code = code as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        std::io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        std::io::Error::from_raw_os_error(code as i32)
    }
}

/// The server closing its end reads as EOF, as it does for `std::fs::File`.
fn broken_pipe_as_eof(error: std::io::Error) -> std::io::Error {
    let WIN32_ERROR(broken_pipe) = ERROR_BROKEN_PIPE;
    if error.raw_os_error() == Some(broken_pipe as i32) {
        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, error)
    } else {
        error
    }
}

impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let len = buffer.len().min(u32::MAX as usize);
        let buffer = &mut buffer[..len];
        let result = self.transfer(self.read_timeout.get(), |file, overlapped| {
            // SAFETY: `transfer` keeps the buffer borrowed until the read settles.
            unsafe { ReadFile(file, Some(buffer), None, Some(overlapped)) }
        });
        match result {
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
            other => other,
        }
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let buffer = &buffer[..buffer.len().min(u32::MAX as usize)];
        self.transfer(self.write_timeout.get(), |file, overlapped| {
            // SAFETY: `transfer` keeps the buffer borrowed until the write settles.
            unsafe { WriteFile(file, Some(buffer), None, Some(overlapped)) }
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
    use super::super::{ConnectPolicy, ExchangeError, LocalIpcClient, LocalIpcStream, Timeouts};
    use super::PipeStream;
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    fn pipe_name(label: &str) -> String {
        format!(r"\\.\pipe\cua-driver-ipc-{label}-{}", std::process::id())
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

        tokio::task::spawn_blocking(move || {
            let mut client = PipeStream::connect(&name).unwrap();
            client.set_read_timeout(Duration::from_millis(200)).unwrap();
            let mut buffer = [0_u8; 16];
            let started = Instant::now();
            let error = client.read(&mut buffer).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(started.elapsed() >= Duration::from_millis(200));
            assert!(started.elapsed() < Duration::from_secs(5));

            release_tx.send(()).unwrap();
            client.set_read_timeout(Duration::from_secs(5)).unwrap();
            let read = client.read(&mut buffer).unwrap();
            assert_eq!(&buffer[..read], b"late\n");
        })
        .await
        .unwrap();
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
        });

        tokio::task::spawn_blocking(move || {
            let mut client = PipeStream::connect(&name).unwrap();
            client.set_read_timeout(Duration::from_secs(5)).unwrap();
            let mut received = Vec::new();
            client.read_to_end(&mut received).unwrap();
            assert_eq!(received, b"bye\n");
            assert!(client.write(b"x").is_err());
        })
        .await
        .unwrap();
        served.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stalling_server_hits_the_deadline() {
        let name = pipe_name("stalled-reply");
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let served = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut server = BufReader::new(server);
            let mut request = String::new();
            server.read_line(&mut request).await.unwrap();
            assert_eq!(request, "ping\n");
            server.get_mut().write_all(b"{\"ok\":").await.unwrap();
            release_rx.await.unwrap();
        });

        let (error, elapsed) = tokio::task::spawn_blocking(move || {
            let mut client = LocalIpcClient::connect(&name, ConnectPolicy::REQUEST).unwrap();
            let started = Instant::now();
            let error = client
                .exchange_line(b"ping", Instant::now() + Duration::from_millis(300))
                .unwrap_err();
            (error, started.elapsed())
        })
        .await
        .unwrap();
        assert!(
            matches!(error, ExchangeError::TimedOut { received: 6 }),
            "{error:?}"
        );
        assert!(elapsed >= Duration::from_millis(300));
        assert!(elapsed < Duration::from_secs(3));
        release_tx.send(()).unwrap();
        served.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_poll_slice_that_expires_keeps_waiting_and_keeps_what_follows_the_reply() {
        let name = pipe_name("poll-slice");
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let served = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut server = BufReader::new(server);
            let mut request = String::new();
            server.read_line(&mut request).await.unwrap();
            tokio::time::sleep(Duration::from_millis(120)).await;
            server
                .get_mut()
                .write_all(b"first\nsecond\n")
                .await
                .unwrap();
            let mut rest = Vec::new();
            server.read_to_end(&mut rest).await.unwrap();
        });

        tokio::task::spawn_blocking(move || {
            let mut client = LocalIpcClient::connect(&name, ConnectPolicy::REQUEST).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let first = client
                .exchange_polling(b"ping", deadline, Duration::from_millis(20))
                .unwrap();
            assert_eq!(first, "first");
            assert_eq!(client.exchange_line(b"next", deadline).unwrap(), "second");
        })
        .await
        .unwrap();
        served.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_connect_retries_only_within_its_budget() {
        let name = pipe_name("late");
        let error = LocalIpcStream::connect(&name, ConnectPolicy::ONCE)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let late = name.clone();
        let served = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&late)
                .unwrap();
            server.connect().await.unwrap();
        });
        let policy = ConnectPolicy {
            retry: Duration::from_millis(20),
            budget: Duration::from_secs(3),
        };
        let blocking = name.clone();
        tokio::task::spawn_blocking(move || LocalIpcClient::connect(&blocking, policy).unwrap())
            .await
            .unwrap();
        served.await.unwrap();
    }
}
