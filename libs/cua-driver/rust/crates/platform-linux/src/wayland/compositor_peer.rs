//! Compositor peer attestation shared by the IPC adapters.
//!
//! An IPC socket and `WAYLAND_DISPLAY` must name the same compositor process
//! before IPC identity or geometry is applied to Wayland input or pixels.

use anyhow::{bail, Context, Result};
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::Duration;

/// Process id of a same-user socket peer.
pub(super) fn peer_pid(fd: RawFd) -> Result<libc::pid_t> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0
        || len as usize != std::mem::size_of::<libc::ucred>()
        || cred.pid <= 0
        || cred.uid != unsafe { libc::geteuid() }
    {
        bail!("could not verify same-user compositor peer");
    }
    Ok(cred.pid)
}

/// Bound the socket connection as well as protocol dispatch. Inherited
/// WAYLAND_SOCKET descriptors are deliberately unsupported here: adapters
/// open independent, attested connections for each observation.
pub(super) fn wayland_connection_with_timeout(
    timeout: Duration,
) -> Result<wayland_client::Connection> {
    wayland_client::Connection::from_socket(wayland_socket_with_timeout(timeout)?)
        .context("Wayland connection failed")
}

/// A raw client socket to the `WAYLAND_DISPLAY` compositor, for a peer check
/// before a Wayland client (in or out of process) uses it.
pub(super) fn wayland_socket_with_timeout(
    timeout: Duration,
) -> Result<std::os::unix::net::UnixStream> {
    if std::env::var_os("WAYLAND_SOCKET").is_some() {
        bail!("compositor observation requires a named WAYLAND_DISPLAY socket");
    }
    let display =
        PathBuf::from(std::env::var_os("WAYLAND_DISPLAY").context("missing WAYLAND_DISPLAY")?);
    let path = if display.is_absolute() {
        display
    } else {
        if display.components().count() != 1 {
            bail!("invalid WAYLAND_DISPLAY socket name");
        }
        let runtime =
            PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").context("missing XDG_RUNTIME_DIR")?);
        if !runtime.is_absolute() {
            bail!("XDG_RUNTIME_DIR must be absolute");
        }
        runtime.join(display)
    };
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(path)?, timeout)?;
    Ok(socket.into())
}
