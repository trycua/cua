// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The guest end of a macOS VM's host channel: a listener on the VM's
//! virtio socket (vsock) that carries a host connection to one of the
//! guest's own loopback ports.
//!
//! A macOS VM on a Mac sits on Apple's shared VM network, and a connect
//! from the host to its address is subject to macOS Local Network privacy
//! (per app) and to a VPN's routes. The host process that runs the VM
//! (Lume) can instead reach the guest over vsock, which neither touches:
//! it publishes `127.0.0.1:<port>` on the host and, for each connection,
//! asks this agent to connect to the guest's `127.0.0.1:<guest port>`.
//!
//! The protocol is the Linux microVM init's (`cua_microvm_init`): one JSON
//! line from the host, one JSON line back, then (for `forward`) raw bytes
//! both ways. Only the host side of the VM can reach a guest's vsock.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use serde::{Deserialize, Serialize};

/// The vsock port the agent listens on (`cua_microvm_init::PORT`).
pub const PORT: u32 = 1024;

/// Longest request line read.
const MAX_LINE: usize = 4096;

/// A request from the host.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Liveness: answered `{"ok":true}`.
    Ping,
    /// Connect to the guest's `127.0.0.1:port`, answer, then carry bytes.
    Forward { port: u16 },
}

/// The answer line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    fn ok() -> Self {
        Self {
            ok: true,
            error: None,
        }
    }

    fn err(e: impl std::fmt::Display) -> Self {
        Self {
            ok: false,
            error: Some(e.to_string()),
        }
    }

    fn line(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec(self).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
        out.push(b'\n');
        out
    }
}

/// Whether this Mac is a virtual machine (`kern.hv_vmm_present`): only a
/// guest has a host to listen for.
pub fn in_vm() -> bool {
    let mut value: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let name = c"kern.hv_vmm_present";
    // SAFETY: a plain sysctl read into a c_int-sized buffer.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut libc::c_int).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    rc == 0 && value == 1
}

/// Starts the agent on a thread of its own when this Mac is a VM with a
/// virtio socket device. Without one (an older host, or a VM started
/// without it) the listen fails and the host reaches the guest over its
/// network address, as before.
pub fn start() {
    if !in_vm() {
        return;
    }
    let listener = match listen(PORT) {
        Ok(fd) => fd,
        Err(e) => {
            tracing::debug!(error = %e, "no virtio socket; the host reaches this VM over its network");
            return;
        }
    };
    tracing::info!(port = PORT, "host channel listening on vsock");
    let spawned = std::thread::Builder::new()
        .name("vsock-agent".into())
        .spawn(move || accept_loop(listener));
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the vsock agent");
    }
}

fn listen(port: u32) -> std::io::Result<OwnedFd> {
    // SAFETY: socket/bind/listen on a fresh fd that OwnedFd then owns.
    unsafe {
        let fd = libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(fd);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        let mut addr: libc::sockaddr_vm = std::mem::zeroed();
        addr.svm_len = std::mem::size_of::<libc::sockaddr_vm>() as u8;
        addr.svm_family = libc::AF_VSOCK as libc::sa_family_t;
        addr.svm_port = port;
        addr.svm_cid = libc::VMADDR_CID_ANY;
        if libc::bind(
            fd.as_raw_fd(),
            (&addr as *const libc::sockaddr_vm).cast(),
            std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
        ) != 0
            || libc::listen(fd.as_raw_fd(), 64) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(fd)
    }
}

fn accept_loop(listener: OwnedFd) {
    loop {
        // SAFETY: accept on a listening socket; the new fd is owned below.
        let fd = unsafe {
            libc::accept(
                listener.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if fd < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            tracing::warn!(error = %e, "vsock accept failed; the agent stops");
            return;
        }
        // SAFETY: `fd` is a fresh connected socket nothing else owns.
        let conn = unsafe { File::from_raw_fd(fd) };
        let _ = std::thread::Builder::new()
            .name("vsock-conn".into())
            .spawn(move || handle(conn));
    }
}

/// Serves one host connection (any stream socket; tests use a Unix pair).
pub fn handle(conn: File) {
    let Ok(writer) = conn.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(conn);
    let mut line = Vec::new();
    match (&mut reader)
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut line)
    {
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }
    let request = serde_json::from_slice::<Request>(trim(&line));
    match request {
        Ok(Request::Ping) => {
            let _ = (&writer).write_all(&Reply::ok().line());
        }
        Ok(Request::Forward { port }) => forward(port, reader, writer),
        Err(e) => {
            let _ = (&writer).write_all(&Reply::err(format!("bad request: {e}")).line());
        }
    }
}

fn trim(line: &[u8]) -> &[u8] {
    let end = line
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    &line[..end]
}

/// Connects to the guest's `127.0.0.1:port`, answers, then copies bytes
/// both ways until either side closes. Bytes the host sent after its
/// request line (already in `reader`'s buffer) go first.
fn forward(port: u16, mut reader: BufReader<File>, mut writer: File) {
    let tcp = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(t) => t,
        Err(e) => {
            let _ = writer.write_all(&Reply::err(format!("127.0.0.1:{port}: {e}")).line());
            return;
        }
    };
    let _ = tcp.set_nodelay(true);
    if writer.write_all(&Reply::ok().line()).is_err() {
        return;
    }
    let (Ok(mut to_guest), Ok(mut from_guest)) = (tcp.try_clone(), tcp.try_clone()) else {
        return;
    };
    let pending = reader.buffer().to_vec();
    let up = std::thread::spawn(move || {
        if to_guest.write_all(&pending).is_ok() {
            let _ = std::io::copy(reader.get_mut(), &mut to_guest);
        }
        let _ = to_guest.shutdown(Shutdown::Write);
    });
    let _ = std::io::copy(&mut from_guest, &mut writer);
    // Half-close toward the host; the other direction may still be open.
    // SAFETY: shutdown on a socket fd this function holds.
    unsafe {
        libc::shutdown(writer.as_raw_fd(), libc::SHUT_WR);
    }
    let _ = up.join();
    let _ = tcp.shutdown(Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;

    fn pair() -> (UnixStream, File) {
        let (host, guest) = UnixStream::pair().unwrap();
        (host, File::from(OwnedFd::from(guest)))
    }

    fn reply(host: &UnixStream) -> String {
        let mut line = String::new();
        BufReader::new(host).read_line(&mut line).unwrap();
        line
    }

    #[test]
    fn requests_parse_as_the_microvm_init_protocol() {
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"op":"forward","port":3211}"#).unwrap(),
            Request::Forward { port: 3211 }
        );
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"op":"ping"}"#).unwrap(),
            Request::Ping
        );
        assert!(serde_json::from_str::<Request>(r#"{"op":"exec"}"#).is_err());
        assert_eq!(Reply::ok().line(), b"{\"ok\":true}\n");
    }

    #[test]
    fn ping_answers_ok() {
        let (mut host, guest) = pair();
        let t = std::thread::spawn(move || handle(guest));
        host.write_all(b"{\"op\":\"ping\"}\n").unwrap();
        assert_eq!(reply(&host), "{\"ok\":true}\n");
        t.join().unwrap();
    }

    #[test]
    fn an_unknown_request_is_refused() {
        let (mut host, guest) = pair();
        let t = std::thread::spawn(move || handle(guest));
        host.write_all(b"{\"op\":\"exec\"}\n").unwrap();
        assert!(reply(&host).starts_with("{\"ok\":false"));
        t.join().unwrap();
    }

    #[test]
    fn forward_carries_bytes_both_ways_including_early_ones() {
        let service = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = service.local_addr().unwrap().port();
        let echo = std::thread::spawn(move || {
            let (mut s, _) = service.accept().unwrap();
            let mut got = Vec::new();
            s.read_to_end(&mut got).unwrap();
            s.write_all(&got).unwrap();
        });
        let (mut host, guest) = pair();
        let t = std::thread::spawn(move || handle(guest));
        // The request and the first payload bytes in one write.
        host.write_all(format!("{{\"op\":\"forward\",\"port\":{port}}}\nhello").as_bytes())
            .unwrap();
        host.shutdown(Shutdown::Write).unwrap();
        let mut all = String::new();
        host.read_to_string(&mut all).unwrap();
        assert_eq!(all, "{\"ok\":true}\nhello");
        echo.join().unwrap();
        t.join().unwrap();
    }

    #[test]
    fn forward_to_a_closed_port_says_why() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let (mut host, guest) = pair();
        let t = std::thread::spawn(move || handle(guest));
        host.write_all(format!("{{\"op\":\"forward\",\"port\":{port}}}\n").as_bytes())
            .unwrap();
        assert!(reply(&host).starts_with("{\"ok\":false"));
        t.join().unwrap();
    }
}
