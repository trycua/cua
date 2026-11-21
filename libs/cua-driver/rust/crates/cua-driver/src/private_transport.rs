// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

#![cfg(target_os = "macos")]

use anyhow::{bail, Context};
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement,
};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::str::FromStr;
use std::sync::Mutex;

static CHANNEL: Mutex<Option<UnixStream>> = Mutex::new(None);
static CLOSED: tokio::sync::Notify = tokio::sync::Notify::const_new();

pub fn guard() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(requirement) = option_env!("CUA_DRIVER_EMBEDDED_PARENT_REQUIREMENT") else {
        if args.iter().any(|arg| arg.starts_with("--transport-fd")) {
            bail!("private embedded entry requires a build-bound parent requirement");
        }
        return Ok(());
    };
    if args.len() != 4
        || args[0] != "__private-worker"
        || args[1] != "--generation"
        || crate::private_worker::requested_generation().is_none_or(|value| value.is_empty())
        || !args[3].starts_with("--transport-fd=")
    {
        bail!("private embedded entry required");
    }
    if crate::mcp_http::configured_port()?.is_some()
        || crate::driver_service_http::configured_port()?.is_some()
        || std::env::var_os(cua_driver_core::PARENT_LIVENESS_STDIN_ENV).is_some()
    {
        bail!("alternate embedded endpoint is forbidden");
    }
    let parent = unsafe { libc::getppid() };
    let mut attributes = GuestAttributes::new();
    attributes.set_pid(parent);
    let trusted = SecRequirement::from_str(requirement).context("invalid parent requirement")?;
    let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .context("untrusted embedded parent")?;
    if parent <= 1
        || code
            .check_validity(Flags::STRICT_VALIDATE, &trusted)
            .is_err()
        || unsafe { libc::getppid() } != parent
    {
        bail!("untrusted embedded parent");
    }
    let fd: i32 = args[3]["--transport-fd=".len()..]
        .parse()
        .context("invalid inherited stream")?;
    let mut kind: libc::c_int = 0;
    let mut size = std::mem::size_of_val(&kind) as libc::socklen_t;
    let mut peer: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut peer_size = std::mem::size_of_val(&peer) as libc::socklen_t;
    if fd < 3
        || unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut libc::c_int).cast(),
                &mut size,
            )
        } != 0
        || kind != libc::SOCK_STREAM
        || unsafe {
            libc::getpeername(
                fd,
                (&mut peer as *mut libc::sockaddr_storage).cast(),
                &mut peer_size,
            )
        } != 0
        || peer.ss_family as libc::c_int != libc::AF_UNIX
        || unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0
    {
        bail!("invalid inherited stream");
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    watch(stream.try_clone()?)?;
    *CHANNEL
        .lock()
        .map_err(|_| anyhow::anyhow!("embedded channel unavailable"))? = Some(stream);
    Ok(())
}

fn watch(stream: UnixStream) -> anyhow::Result<std::thread::JoinHandle<()>> {
    let fd = unsafe { libc::kqueue() };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let queue = unsafe { std::fs::File::from_raw_fd(fd) };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let registration = libc::kevent {
        ident: stream.as_raw_fd() as libc::uintptr_t,
        filter: libc::EVFILT_READ,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    if unsafe {
        libc::kevent(
            fd,
            &registration,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(std::thread::Builder::new()
        .name("embedded-lifetime".into())
        .spawn(move || {
            let _stream = stream;
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            loop {
                let result = unsafe {
                    libc::kevent(
                        queue.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        std::ptr::null(),
                    )
                };
                if result < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                }
                if result < 0 || event.flags & (libc::EV_EOF | libc::EV_ERROR) != 0 {
                    CLOSED.notify_one();
                    return;
                }
            }
        })?)
}

pub fn take() -> anyhow::Result<Option<UnixStream>> {
    Ok(CHANNEL
        .lock()
        .map_err(|_| anyhow::anyhow!("embedded channel unavailable"))?
        .take())
}

pub async fn closed() {
    if option_env!("CUA_DRIVER_EMBEDDED_PARENT_REQUIREMENT").is_some() {
        CLOSED.notified().await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn observes_eof_without_consuming_requests() {
        use std::io::{Read, Write};
        for half_close in [true, false] {
            let (mut host, child) = UnixStream::pair().unwrap();
            let mut reader = child.try_clone().unwrap();
            let observed = CLOSED.notified();
            let monitor = watch(child).unwrap();
            host.write_all(b"queued request").unwrap();
            if half_close {
                host.shutdown(std::net::Shutdown::Write).unwrap();
            } else {
                drop(host);
            }
            tokio::time::timeout(std::time::Duration::from_secs(3), observed)
                .await
                .unwrap();
            monitor.join().unwrap();
            let mut request = Vec::new();
            reader.read_to_end(&mut request).unwrap();
            assert_eq!(request, b"queued request");
        }
    }
}
