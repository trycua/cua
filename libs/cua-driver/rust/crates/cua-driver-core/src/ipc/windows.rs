// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Named-pipe client with socket-style read and write timeouts, and the
//! server's pipe security descriptor and client identity.
//!
//! A pipe opened through `std::fs` blocks in `ReadFile` until the daemon
//! answers, so a stalled daemon hangs its client forever. This client opens the
//! pipe for overlapped I/O and waits on each operation for at most its timeout.
//! An operation that runs out of time is cancelled with `CancelIoEx` and
//! awaited, so no I/O is left in flight and no thread is left behind. It then
//! fails with `TimedOut`, as a socket read past `SO_RCVTIMEO` does, which lets
//! the shared poll loops run unchanged over it.

use std::fs::File;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::time::Duration;

use windows::core::{HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    LocalFree, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, HLOCAL,
    WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile, FILE_FLAG_OVERLAPPED};
use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, WaitNamedPipeW, NMPWAIT_NOWAIT};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
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
}

impl PipeStream {
    /// Open `path` once.
    pub(super) fn connect(path: &str) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED.0)
            .open(path)?;
        Ok(Self { file })
    }

    /// Start one overlapped operation with `start` and wait at most `timeout`
    /// for it. Returns the bytes it transferred.
    fn transfer(
        &self,
        timeout: Duration,
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

        let milliseconds = timeout
            .as_nanos()
            .div_ceil(1_000_000)
            .clamp(1, u128::from(INFINITE - 1)) as u32;
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

impl super::Transport for PipeStream {
    fn read_within(&mut self, buffer: &mut [u8], timeout: Duration) -> std::io::Result<usize> {
        let len = buffer.len().min(u32::MAX as usize);
        let buffer = &mut buffer[..len];
        let result = self.transfer(timeout, |file, overlapped| {
            // SAFETY: `transfer` keeps the buffer borrowed until the read settles.
            unsafe { ReadFile(file, Some(buffer), None, Some(overlapped)) }
        });
        match result {
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
            other => other,
        }
    }

    fn write_within(&mut self, buffer: &[u8], timeout: Duration) -> std::io::Result<usize> {
        let buffer = &buffer[..buffer.len().min(u32::MAX as usize)];
        self.transfer(timeout, |file, overlapped| {
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
}

/// A current-user pipe security descriptor, built once and passed to every
/// pipe instance.
///
/// The DACL grants only the daemon's own user, and the low mandatory label
/// lets a host at a different integrity level still connect. Every accepted
/// client is checked against its process token as well.
pub(super) struct PipeSecurity {
    attributes: SECURITY_ATTRIBUTES,
    user_sid: String,
}

// SAFETY: the descriptor is never written after it is built, and the pipe
// copies it at creation, so sharing or moving it across threads is sound.
unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

impl PipeSecurity {
    pub(super) fn current_user() -> std::io::Result<Self> {
        let user_sid = current_user_sid()?;
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{user_sid})S:(ML;;NW;;;LW)\0")
            .encode_utf16()
            .collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; the
        // descriptor it returns is owned here and freed in `drop`.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(os_error)?;
        Ok(Self {
            attributes: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            },
            user_sid,
        })
    }

    /// The `SECURITY_ATTRIBUTES` to pass when creating a pipe instance.
    pub(super) fn attributes(&self) -> *mut std::ffi::c_void {
        std::ptr::addr_of!(self.attributes).cast_mut().cast()
    }

    pub(super) fn user_sid(&self) -> &str {
        &self.user_sid
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        // SAFETY: the descriptor came from LocalAlloc and nothing uses it now.
        unsafe { LocalFree(Some(HLOCAL(self.attributes.lpSecurityDescriptor))) };
    }
}

/// Whether a client SID names the daemon's own user. SIDs compare without
/// regard to case.
pub(super) fn sid_is_authorized(expected: &str, actual: &str) -> bool {
    expected.eq_ignore_ascii_case(actual)
}

/// The process id and user SID of the client connected to `pipe`.
pub(super) fn pipe_client_identity(pipe: HANDLE) -> std::io::Result<(u32, String)> {
    let mut pid = 0_u32;
    // SAFETY: `pipe` is a connected server instance the caller keeps open.
    unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) }.map_err(os_error)?;
    if pid == 0 {
        return Err(std::io::Error::other(
            "named pipe client process id is unavailable",
        ));
    }
    // SAFETY: the handle it returns is owned right away.
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map_err(os_error)?;
    // SAFETY: `process` is a fresh handle this function alone owns.
    let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    Ok((pid, process_user_sid(HANDLE(process.as_raw_handle()))?))
}

fn current_user_sid() -> std::io::Result<String> {
    // SAFETY: the pseudo-handle needs no closing.
    process_user_sid(unsafe { GetCurrentProcess() })
}

/// The user SID of `process`'s token, in its string form.
fn process_user_sid(process: HANDLE) -> std::io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: `process` stays open for the call; the token is owned below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.map_err(os_error)?;
    // SAFETY: `token` is a fresh handle this function alone owns.
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let token = HANDLE(token.as_raw_handle());

    let mut required = 0_u32;
    // SAFETY: a size query; it fails with the length it needs.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut required) };
    if required == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buffer = vec![0_u8; required as usize];
    // SAFETY: `buffer` holds `required` writable bytes.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            required,
            &mut required,
        )
    }
    .map_err(os_error)?;
    // The byte buffer need not be aligned for TOKEN_USER, so copy it out.
    // SAFETY: GetTokenInformation wrote a TOKEN_USER at the buffer's start,
    // and its SID points into `buffer`, which lives until after the convert.
    let user = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>()) };
    let mut text = PWSTR::null();
    // SAFETY: the SID is valid; the string it returns is freed below.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }.map_err(os_error)?;
    // SAFETY: `text` is a NUL-terminated string from LocalAlloc.
    let sid = unsafe { text.to_string() };
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    let sid = sid.map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if sid.is_empty() {
        return Err(std::io::Error::other("token user SID is empty"));
    }
    Ok(sid)
}

#[cfg(test)]
mod tests {
    use super::super::{ConnectPolicy, ExchangeError, LocalIpcClient, LocalIpcStream, Transport};
    use super::{sid_is_authorized, PipeStream};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    fn pipe_name(label: &str) -> String {
        format!(r"\\.\pipe\cua-driver-ipc-{label}-{}", std::process::id())
    }

    #[test]
    fn only_the_daemon_users_sid_is_authorized() {
        assert!(!sid_is_authorized("S-1-5-21-1", "S-1-5-21-2"));
        assert!(!sid_is_authorized("S-1-5-21-1", ""));
        assert!(sid_is_authorized("S-1-5-21-1", "s-1-5-21-1"));
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
            let mut buffer = [0_u8; 16];
            let started = Instant::now();
            let error = client
                .read_within(&mut buffer, Duration::from_millis(200))
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(started.elapsed() >= Duration::from_millis(200));
            assert!(started.elapsed() < Duration::from_secs(5));

            release_tx.send(()).unwrap();
            let read = client
                .read_within(&mut buffer, Duration::from_secs(5))
                .unwrap();
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
            let mut received = Vec::new();
            let mut buffer = [0_u8; 16];
            loop {
                match client
                    .read_within(&mut buffer, Duration::from_secs(5))
                    .unwrap()
                {
                    0 => break,
                    read => received.extend_from_slice(&buffer[..read]),
                }
            }
            assert_eq!(received, b"bye\n");
            assert!(client.write_within(b"x", Duration::from_secs(5)).is_err());
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
        assert!(elapsed < Duration::from_millis(500));
        release_tx.send(()).unwrap();
        served.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_that_never_reads_hits_the_write_deadline() {
        let name = pipe_name("full");
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let served = tokio::spawn(async move {
            server.connect().await.unwrap();
            release_rx.await.unwrap();
        });

        let (error, elapsed) = tokio::task::spawn_blocking(move || {
            let mut client = LocalIpcClient::connect(&name, ConnectPolicy::REQUEST).unwrap();
            let request = vec![b'x'; 16 * 1024 * 1024];
            let started = Instant::now();
            let error = client
                .exchange_line(&request, Instant::now() + Duration::from_millis(200))
                .unwrap_err();
            (error, started.elapsed())
        })
        .await
        .unwrap();
        let ExchangeError::Write(error) = error else {
            panic!("{error:?}");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(elapsed < Duration::from_millis(400), "{elapsed:?}");
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
