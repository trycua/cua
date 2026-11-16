// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Window capture on the nested cua-compositor.
//!
//! The compositor has no per-toplevel capture protocol, so a window shot is a
//! crop of its single output. The crop is used only when the compositor's
//! `w <pid>` attestation names the same unoccluded rectangle and scene epoch
//! before and after the screencopy, and the control socket and the Wayland
//! socket belong to the same compositor process. Anything else is refused;
//! capture never focuses or raises the window.

use std::io::{BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::compositor_peer::{peer_pid, wayland_socket_with_timeout};
use super::WaylandWindowCrop;

const TIMEOUT: Duration = Duration::from_secs(5);

/// One `w <pid>` reply: the target's rectangle in output pixels, the size of
/// the output it is drawn on, and the compositor's scene epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Attestation {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    epoch: u64,
}

impl Attestation {
    /// Parse a reply. A compositor refusal (`err <reason>`) is returned as
    /// that reason, so the caller's error names it.
    fn parse(line: &str) -> Result<Self> {
        let line = line.trim();
        if let Some(reason) = line.strip_prefix("err") {
            bail!("cua-compositor refused window capture: {}", reason.trim());
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let ["capture", x, y, width, height, output_width, output_height, epoch] = fields[..]
        else {
            bail!("unexpected cua-compositor capture reply: {line:?}");
        };
        Ok(Self {
            x: x.parse()?,
            y: y.parse()?,
            width: width.parse()?,
            height: height.parse()?,
            output_width: output_width.parse()?,
            output_height: output_height.parse()?,
            epoch: epoch.parse()?,
        })
    }
}

/// The crop for a capture bracketed by `before` and `after`, over an output
/// image `image` pixels in size. Refuses when the scene changed during the
/// copy, the image is not the attested output, or the window is not wholly
/// inside it (a clipped origin would shift the window's coordinate frame).
fn bracketed_crop(
    before: Attestation,
    after: Attestation,
    image: (u32, u32),
) -> Result<WaylandWindowCrop> {
    if before != after {
        bail!("the window or the compositor scene changed during capture");
    }
    if image != (before.output_width, before.output_height) {
        bail!(
            "captured output is {}x{}, but the compositor attested {}x{}",
            image.0,
            image.1,
            before.output_width,
            before.output_height
        );
    }
    let inside = |origin: i32, extent: u32, limit: u32| {
        origin >= 0 && extent > 0 && i64::from(origin) + i64::from(extent) <= i64::from(limit)
    };
    if !inside(before.x, before.width, image.0) || !inside(before.y, before.height, image.1) {
        bail!("the window is not wholly inside the captured output");
    }
    Ok(WaylandWindowCrop {
        x: before.x,
        y: before.y,
        width: before.width,
        height: before.height,
    })
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

/// A handshaken control connection, proven to reach `compositor`.
struct Control {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Control {
    fn open(stream: UnixStream, compositor: libc::pid_t) -> Result<Self> {
        if peer_pid(stream.as_raw_fd())? != compositor {
            bail!("CUA_INJECT_SOCKET and WAYLAND_DISPLAY name different compositor processes");
        }
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut control = Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        };
        writeln!(control.writer, "{}", super::INJECT_PROTO_HELLO)?;
        super::parse_inject_hello(&super::read_inject_line(&mut control.reader)?)?;
        Ok(control)
    }

    fn attest(&mut self, pid: u32) -> Result<Attestation> {
        writeln!(self.writer, "w {pid}")?;
        Attestation::parse(&super::read_inject_line(&mut self.reader)?)
    }
}

/// Capture window `xid` of `pid` (or of the one process it was listed for).
pub(super) fn capture(xid: u64, pid: Option<u32>) -> Result<Vec<u8>> {
    let pid = match pid {
        Some(pid) => pid,
        None => unique_listed_pid(xid, &super::list_windows_dispatch(None))?,
    };
    if !super::window_was_listed_for_pid(pid, xid) {
        bail!("window {xid} was not listed for process {pid}");
    }
    let wayland = wayland_socket_with_timeout(TIMEOUT)?;
    let compositor = peer_pid(wayland.as_raw_fd())?;
    let socket = super::inject_socket_path().context("CUA_INJECT_SOCKET is not set")?;
    // Both connections stay open through the copy, so the pixels and the
    // attestations come from one compositor process.
    let mut control = Control::open(UnixStream::connect(socket)?, compositor)?;
    let before = control.attest(pid)?;
    let output =
        super::capture_via_screencopy_on(wayland_client::Connection::from_socket(wayland)?)?;
    let after = control.attest(pid)?;
    let crop = bracketed_crop(before, after, crate::capture::png_dimensions_pub(&output)?)?;
    super::crop_png_to_rect(
        &output,
        crop.x,
        crop.y,
        crop.width,
        crop.height,
        &format!("cua-compositor window {xid}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATTESTED: Attestation = Attestation {
        x: 10,
        y: 20,
        width: 600,
        height: 800,
        output_width: 1280,
        output_height: 1024,
        epoch: 7,
    };

    #[test]
    fn parses_the_attestation_and_names_refusals() {
        assert_eq!(
            Attestation::parse("capture 10 20 600 800 1280 1024 7\n").unwrap(),
            ATTESTED
        );
        let refused = Attestation::parse("err target-occluded").unwrap_err();
        assert!(refused.to_string().contains("target-occluded"), "{refused}");
        for line in [
            "ok",
            "geometry 0 0 0 0",
            "capture 10 20 600 800 1280 1024",
            "capture 10 20 600 800 1280 1024 7 extra",
            "capture 10 20 -600 800 1280 1024 7",
        ] {
            assert!(Attestation::parse(line).is_err(), "accepted {line:?}");
        }
    }

    #[test]
    fn crops_to_the_attested_window_when_nothing_changed() {
        assert_eq!(
            bracketed_crop(ATTESTED, ATTESTED, (1280, 1024)).unwrap(),
            WaylandWindowCrop {
                x: 10,
                y: 20,
                width: 600,
                height: 800
            }
        );
    }

    #[test]
    fn refuses_a_scene_change_during_capture() {
        for after in [
            Attestation {
                epoch: 8,
                ..ATTESTED
            },
            Attestation { x: 11, ..ATTESTED },
            Attestation {
                width: 599,
                ..ATTESTED
            },
        ] {
            assert!(bracketed_crop(ATTESTED, after, (1280, 1024)).is_err());
        }
    }

    #[test]
    fn refuses_an_image_that_is_not_the_attested_output() {
        assert!(bracketed_crop(ATTESTED, ATTESTED, (2560, 2048)).is_err());
    }

    #[test]
    fn refuses_a_window_not_wholly_inside_the_output() {
        for window in [
            Attestation { x: -1, ..ATTESTED },
            Attestation { y: -1, ..ATTESTED },
            Attestation { x: 681, ..ATTESTED },
            Attestation { y: 225, ..ATTESTED },
            Attestation {
                width: 0,
                ..ATTESTED
            },
        ] {
            assert!(
                bracketed_crop(window, window, (1280, 1024)).is_err(),
                "accepted {window:?}"
            );
        }
        let flush = Attestation {
            x: 680,
            y: 224,
            ..ATTESTED
        };
        assert!(bracketed_crop(flush, flush, (1280, 1024)).is_ok());
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

    #[test]
    fn refuses_a_control_socket_from_another_process() {
        let (client, _server) = UnixStream::pair().unwrap();
        let ours = peer_pid(client.as_raw_fd()).unwrap();
        let error = Control::open(client, ours + 1).err().unwrap();
        assert!(
            error.to_string().contains("different compositor"),
            "{error}"
        );
    }

    #[test]
    fn attests_over_the_handshaken_control_socket() {
        let (client, server) = UnixStream::pair().unwrap();
        let ours = peer_pid(client.as_raw_fd()).unwrap();
        let compositor = std::thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            let mut writer = server;
            let hello = super::super::read_inject_line(&mut reader).unwrap();
            writeln!(writer, "{}", hello.trim()).unwrap();
            let query = super::super::read_inject_line(&mut reader).unwrap();
            assert_eq!(query.trim(), "w 42");
            writeln!(writer, "capture 10 20 600 800 1280 1024 7").unwrap();
        });
        let mut control = Control::open(client, ours).unwrap();
        assert_eq!(control.attest(42).unwrap(), ATTESTED);
        compositor.join().unwrap();
    }
}
