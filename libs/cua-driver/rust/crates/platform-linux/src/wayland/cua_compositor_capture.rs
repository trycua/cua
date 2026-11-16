// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Window capture on the nested cua-compositor.
//!
//! The compositor renders the target toplevel's own scene subtree (its
//! surface, subsurfaces and popups) into an offscreen buffer and passes the
//! pixels back over the control socket as a memfd. Nothing outside that
//! subtree is drawn, so a window stacked above the target never shows and the
//! target is never focused or raised to be captured. The control socket must
//! belong to the same compositor process as `WAYLAND_DISPLAY`, the one the
//! window was listed on.

use std::io::{IoSliceMut, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::compositor_peer::{peer_pid, wayland_socket_with_timeout};

const TIMEOUT: Duration = Duration::from_secs(5);

/// The compositor's `s <pid>` reply: the window's size in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Image {
    width: u32,
    height: u32,
}

impl Image {
    /// Parse a reply. A compositor refusal (`err <reason>`) is returned as
    /// that reason, so the caller's error names it.
    fn parse(line: &str) -> Result<Self> {
        let line = line.trim();
        if let Some(reason) = line.strip_prefix("err") {
            bail!("cua-compositor refused window capture: {}", reason.trim());
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let ["image", width, height] = fields[..] else {
            bail!("unexpected cua-compositor capture reply: {line:?}");
        };
        let image = Self {
            width: width.parse()?,
            height: height.parse()?,
        };
        if image.width == 0 || image.height == 0 {
            bail!("cua-compositor returned an empty window image");
        }
        Ok(image)
    }

    fn byte_len(self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

/// Read one reply line and the descriptor sent with it, if any.
fn recv_line(stream: &UnixStream) -> Result<(String, Option<OwnedFd>)> {
    let mut line = Vec::new();
    let mut received = None;
    while !line.ends_with(b"\n") {
        let mut chunk = [0u8; 256];
        let (read, fd) = recv_with_fd(stream.as_raw_fd(), &mut chunk)?;
        if read == 0 {
            bail!("cua-compositor closed the connection before responding");
        }
        if let Some(fd) = fd {
            if received.replace(fd).is_some() {
                bail!("cua-compositor sent more than one descriptor");
            }
        }
        line.extend_from_slice(&chunk[..read]);
    }
    Ok((String::from_utf8(line)?, received))
}

fn recv_with_fd(socket: RawFd, buffer: &mut [u8]) -> Result<(usize, Option<OwnedFd>)> {
    let mut iov = [IoSliceMut::new(buffer)];
    let mut control = [0u8; 64];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = iov.as_mut_ptr().cast();
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let read = unsafe { libc::recvmsg(socket, &mut message, libc::MSG_CMSG_CLOEXEC) };
    if read < 0 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            bail!("cua-compositor did not respond within the timeout");
        }
        return Err(error).context("cua-compositor read failed");
    }
    let mut fds = Vec::new();
    let mut header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    while !header.is_null() {
        let cmsg = unsafe { &*header };
        if cmsg.cmsg_level == libc::SOL_SOCKET && cmsg.cmsg_type == libc::SCM_RIGHTS {
            let data = unsafe { libc::CMSG_DATA(header) }.cast::<RawFd>();
            let count = (cmsg.cmsg_len as usize - unsafe { libc::CMSG_LEN(0) } as usize)
                / std::mem::size_of::<RawFd>();
            for index in 0..count {
                let fd = unsafe { data.add(index).read_unaligned() };
                fds.push(unsafe { OwnedFd::from_raw_fd(fd) });
            }
        }
        header = unsafe { libc::CMSG_NXTHDR(&message, header) };
    }
    if message.msg_flags & libc::MSG_CTRUNC != 0 {
        bail!("cua-compositor sent more descriptors than expected");
    }
    if fds.len() > 1 {
        bail!("cua-compositor sent more than one descriptor");
    }
    Ok((read as usize, fds.pop()))
}

/// Request window pixels for `pid` over a control connection whose peer must
/// be the `compositor` process.
fn request(stream: UnixStream, compositor: libc::pid_t, pid: u32) -> Result<(Image, OwnedFd)> {
    if peer_pid(stream.as_raw_fd())? != compositor {
        bail!("CUA_INJECT_SOCKET and WAYLAND_DISPLAY name different compositor processes");
    }
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut writer = &stream;
    writeln!(writer, "{}", super::INJECT_PROTO_HELLO)?;
    super::parse_inject_hello(&recv_line(&stream)?.0)?;
    writeln!(writer, "s {pid}")?;
    let (line, fd) = recv_line(&stream)?;
    let image = Image::parse(&line)?;
    let fd = fd.context("cua-compositor sent no window pixels")?;
    Ok((image, fd))
}

/// Encode the ARGB8888 (B, G, R, A in memory) rows the compositor wrote.
fn encode(image: Image, fd: &OwnedFd) -> Result<Vec<u8>> {
    let file = std::fs::File::from(fd.try_clone()?);
    if (file.metadata()?.len() as usize) < image.byte_len() {
        bail!("cua-compositor window pixels are shorter than the reported size");
    }
    let pixels = unsafe {
        memmap2::MmapOptions::new()
            .len(image.byte_len())
            .map(&file)?
    };
    cua_driver_core::image_utils::encode_bgra_to_png(&pixels, image.width, image.height)
}

/// The one process a window id was listed for, for callers that pass no pid.
fn unique_listed_pid(xid: u64, windows: &[crate::x11::WindowInfo]) -> Result<u32> {
    let mut pids = windows
        .iter()
        .filter(|window| window.xid == xid)
        .map(|window| window.pid.filter(|pid| *pid > 0));
    match (pids.next(), pids.next()) {
        (Some(Some(pid)), None) => Ok(pid),
        (None, _) => bail!("window {xid} is not listed"),
        (Some(None), None) => bail!("window {xid} is listed without a process"),
        (Some(_), Some(_)) => bail!("window {xid} is listed more than once"),
    }
}

/// Capture window `xid` of `pid` (or of the one process it was listed for),
/// whether or not another window covers it.
pub(super) fn capture(xid: u64, pid: Option<u32>) -> Result<Vec<u8>> {
    let pid = match pid {
        Some(pid) => pid,
        None => unique_listed_pid(xid, &super::list_windows_dispatch(None))?,
    };
    if !super::window_was_listed_for_pid(pid, xid) {
        bail!("window {xid} was not listed for process {pid}");
    }
    let compositor = peer_pid(wayland_socket_with_timeout(TIMEOUT)?.as_raw_fd())?;
    let socket = super::inject_socket_path().context("CUA_INJECT_SOCKET is not set")?;
    let (image, fd) = request(UnixStream::connect(socket)?, compositor, pid)?;
    encode(image, &fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn parses_the_image_size_and_names_refusals() {
        assert_eq!(
            Image::parse("image 560 757\n").unwrap(),
            Image {
                width: 560,
                height: 757
            }
        );
        let refused = Image::parse("err ambiguous-pid").unwrap_err();
        assert!(refused.to_string().contains("ambiguous-pid"), "{refused}");
        for line in [
            "ok",
            "image 560",
            "image 560 757 1",
            "image -1 757",
            "image 0 757",
            "image 560 0",
            "capture 0 0 560 757 1280 1024 7",
        ] {
            assert!(Image::parse(line).is_err(), "accepted {line:?}");
        }
    }

    #[test]
    fn an_id_only_capture_needs_one_listed_process() {
        let window = |xid, pid| crate::x11::WindowInfo {
            xid,
            pid,
            app_name: String::new(),
            title: String::new(),
            is_on_screen: true,
            z_index: None,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        assert_eq!(
            unique_listed_pid(9, &[window(8, Some(3)), window(9, Some(4))]).unwrap(),
            4
        );
        assert!(unique_listed_pid(9, &[window(8, Some(3))]).is_err());
        assert!(unique_listed_pid(9, &[window(9, None)]).is_err());
        assert!(unique_listed_pid(9, &[window(9, Some(0))]).is_err());
        assert!(unique_listed_pid(9, &[window(9, Some(3)), window(9, Some(4))]).is_err());
    }

    /// Plays the compositor's side of one `s` request: echo the handshake,
    /// then send `reply` with `pixels` (if any) as SCM_RIGHTS.
    fn compositor(
        server: UnixStream,
        reply: &'static str,
        pixels: Option<Vec<u8>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            writeln!(&server, "{}", line.trim()).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim(), "s 42");
            let Some(pixels) = pixels else {
                writeln!(&server, "{reply}").unwrap();
                return;
            };
            let mut memfd = tempfile::tempfile().unwrap();
            memfd.write_all(&pixels).unwrap();
            let fd = memfd.as_raw_fd();
            let payload = format!("{reply}\n");
            let iov = libc::iovec {
                iov_base: payload.as_ptr() as *mut _,
                iov_len: payload.len(),
            };
            let mut control = [0u8; 64];
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            message.msg_iov = &iov as *const _ as *mut _;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen =
                unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as _;
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
                libc::CMSG_DATA(header).cast::<RawFd>().write_unaligned(fd);
                assert!(libc::sendmsg(server.as_raw_fd(), &message, 0) > 0);
            }
        })
    }

    fn ours(client: &UnixStream) -> libc::pid_t {
        peer_pid(client.as_raw_fd()).unwrap()
    }

    #[test]
    fn refuses_a_control_socket_from_another_process() {
        let (client, _server) = UnixStream::pair().unwrap();
        let other = ours(&client) + 1;
        let error = request(client, other, 42).unwrap_err();
        assert!(
            error.to_string().contains("different compositor"),
            "{error}"
        );
    }

    #[test]
    fn decodes_the_window_pixels_the_compositor_sends() {
        // Two pixels, B,G,R,A in memory: opaque red, then opaque blue.
        let (client, server) = UnixStream::pair().unwrap();
        let peer = compositor(
            server,
            "image 2 1",
            Some(vec![0, 0, 255, 255, 255, 0, 0, 255]),
        );
        let (image, fd) = {
            let compositor = ours(&client);
            request(client, compositor, 42)
        }
        .unwrap();
        peer.join().unwrap();
        let png = encode(image, &fd).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(1, 0).0, [0, 0, 255, 255]);
    }

    #[test]
    fn surfaces_the_compositor_refusal() {
        let (client, server) = UnixStream::pair().unwrap();
        let peer = compositor(server, "err unmapped-target", None);
        let error = {
            let compositor = ours(&client);
            request(client, compositor, 42)
        }
        .unwrap_err();
        peer.join().unwrap();
        assert!(error.to_string().contains("unmapped-target"), "{error}");
    }

    #[test]
    fn refuses_a_reply_without_pixels() {
        let (client, server) = UnixStream::pair().unwrap();
        let peer = compositor(server, "image 2 1", None);
        let error = {
            let compositor = ours(&client);
            request(client, compositor, 42)
        }
        .unwrap_err();
        peer.join().unwrap();
        assert!(error.to_string().contains("no window pixels"), "{error}");
    }

    #[test]
    fn refuses_pixels_shorter_than_the_reported_size() {
        let (client, server) = UnixStream::pair().unwrap();
        let peer = compositor(server, "image 2 2", Some(vec![0; 8]));
        let (image, fd) = {
            let compositor = ours(&client);
            request(client, compositor, 42)
        }
        .unwrap();
        peer.join().unwrap();
        let error = encode(image, &fd).unwrap_err();
        assert!(error.to_string().contains("shorter"), "{error}");
    }
}
