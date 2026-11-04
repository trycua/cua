// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! In-process `wlr-screencopy-unstable-v1` capture for the Hyprland backend.
//!
//! One persistent Wayland connection per stream replaces a `grim` process per
//! frame. The compositor copies the output region straight into shared-memory
//! buffers this module owns, so a frame costs one copy on each side and no
//! process spawn, PPM encode or parse.
//!
//! Damage-aware: after the first frame every request is `copy_with_damage`,
//! which the compositor answers only once it renders that output again. An
//! idle desktop therefore costs nothing. A request is always outstanding while
//! the stream runs (two buffers alternate), so a change that renders between
//! two paced frames is never missed: its frame waits in the spare buffer until
//! the stream is due for the next one.
//!
//! Only `wl_shm` buffers are used: frames end up in CPU memory for the encoder
//! anyway, and a dmabuf would need a GPU readback here instead.

use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::time::{Duration, Instant};

use memmap2::MmapMut;
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

/// Upper bound on one shm buffer (a 8K output is about 132 MB).
const MAX_BUFFER_BYTES: usize = 256 * 1024 * 1024;

/// A rectangle in an output's logical coordinates (what
/// `capture_output_region` takes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputRegion {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// `wl_shm` buffer parameters announced by the compositor for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShmParams {
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

/// Progress of the one outstanding frame request.
#[derive(Debug, Default)]
struct FrameProgress {
    params: Option<ShmParams>,
    buffer_done: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct State {
    shm: Option<wl_shm::WlShm>,
    manager: Option<ZwlrScreencopyManagerV1>,
    manager_version: u32,
    /// Every output with the name it announced (`wl_output` v4 `name`).
    outputs: Vec<(wl_output::WlOutput, Option<String>)>,
    /// Serial of the request whose events are current; events of an
    /// abandoned request (a region change) are ignored.
    serial: u64,
    frame: FrameProgress,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        if interface == wl_shm::WlShm::interface().name {
            state.shm = Some(registry.bind(name, version.min(1), qh, ()));
        } else if interface == ZwlrScreencopyManagerV1::interface().name {
            let version = version.min(3);
            state.manager_version = version;
            state.manager = Some(registry.bind(name, version, qh, ()));
        } else if interface == wl_output::WlOutput::interface().name {
            let output = registry.bind(name, version.min(4), qh, ());
            state.outputs.push((output, None));
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some(entry) = state.outputs.iter_mut().find(|(o, _)| o == output) {
                entry.1 = Some(name);
            }
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, u64> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        serial: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if *serial != state.serial {
            return;
        }
        let frame = &mut state.frame;
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let format = match format {
                    WEnum::Value(format) => format as u32,
                    WEnum::Unknown(raw) => raw,
                };
                frame.params = Some(ShmParams {
                    format,
                    width,
                    height,
                    stride,
                });
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => frame.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                frame.y_invert = matches!(
                    flags,
                    WEnum::Value(flags) if flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert)
                );
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => frame.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => frame.failed = true,
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(State: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(State: ignore ZwlrScreencopyManagerV1);

/// One shared-memory buffer the compositor copies frames into.
struct Slot {
    params: ShmParams,
    map: MmapMut,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    /// The fd stays open for the pool's lifetime.
    _file: File,
}

impl Slot {
    fn new(
        shm: &wl_shm::WlShm,
        params: ShmParams,
        qh: &QueueHandle<State>,
    ) -> Result<Self, String> {
        let size = params.stride as usize * params.height as usize;
        if size == 0 || size > MAX_BUFFER_BYTES || params.stride < params.width * 4 {
            return Err(format!("unusable screencopy buffer {params:?}"));
        }
        let format = wl_shm::Format::try_from(params.format)
            .map_err(|_| format!("unknown wl_shm format {:#x}", params.format))?;
        let file = memfd(size)?;
        // SAFETY: the memfd is private to this process and the compositor,
        // which only writes it between `copy` and `ready`; it is read here
        // only after `ready`, while no request targets this slot.
        let map = unsafe { MmapMut::map_mut(&file) }.map_err(|e| format!("mmap: {e}"))?;
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            params.width as i32,
            params.height as i32,
            params.stride as i32,
            format,
            qh,
            (),
        );
        Ok(Self {
            params,
            map,
            pool,
            buffer,
            _file: file,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

fn memfd(size: usize) -> Result<File, String> {
    // SAFETY: plain syscalls on a NUL-terminated name; the fd is owned by the
    // returned File.
    let fd = unsafe { libc::memfd_create(c"cua-screencopy".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    file.set_len(size as u64)
        .map_err(|e| format!("memfd resize: {e}"))?;
    Ok(file)
}

/// A captured frame: tightly packed, top-down BGRA (alpha forced opaque).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Captured {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A persistent screencopy session for one output.
pub(crate) struct Screencopy {
    _conn: Connection,
    queue: EventQueue<State>,
    state: State,
    output: wl_output::WlOutput,
    region: OutputRegion,
    /// The outstanding request, if any.
    pending: Option<ZwlrScreencopyFrameV1>,
    /// Whether `copy` was already sent for the outstanding request.
    copied: bool,
    /// The next request must be a plain `copy` (first frame, new region or
    /// resume): `copy_with_damage` would wait for the next damage.
    force_full: bool,
    slots: [Option<Slot>; 2],
    /// Slot the outstanding request copies into.
    target: usize,
    /// A completed frame not yet taken: its slot and y-invert flag.
    completed: Option<(usize, bool)>,
    consecutive_failures: u32,
}

impl Screencopy {
    /// Connect to the session's compositor and bind the output named
    /// `output_name` (the first output when there is one and no name
    /// matches).
    pub(crate) fn connect(output_name: &str) -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("Wayland connect: {e}"))?;
        let mut queue = conn.new_event_queue::<State>();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = State::default();
        // Globals, then the outputs' names.
        for _ in 0..2 {
            queue
                .roundtrip(&mut state)
                .map_err(|e| format!("Wayland roundtrip: {e}"))?;
        }
        if state.manager.is_none() {
            return Err("the compositor does not offer zwlr_screencopy_manager_v1".into());
        }
        if state.shm.is_none() {
            return Err("the compositor does not offer wl_shm".into());
        }
        let output = pick_output(&state.outputs, output_name)
            .ok_or_else(|| format!("no Wayland output named {output_name:?}"))?;
        Ok(Self {
            _conn: conn,
            queue,
            state,
            output,
            region: OutputRegion {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            pending: None,
            copied: false,
            force_full: true,
            slots: [None, None],
            target: 0,
            completed: None,
            consecutive_failures: 0,
        })
    }

    /// Change the captured region. The next frame is a full copy of it.
    pub(crate) fn set_region(&mut self, region: OutputRegion) {
        if region != self.region {
            self.region = region;
            self.abandon();
        }
    }

    /// Stop capturing until the next [`Self::wait`] (pause); the next frame
    /// is a full copy.
    pub(crate) fn abandon(&mut self) {
        if let Some(frame) = self.pending.take() {
            frame.destroy();
        }
        self.state.serial += 1;
        self.state.frame = FrameProgress::default();
        self.copied = false;
        self.completed = None;
        self.force_full = true;
        let _ = self.queue.flush();
    }

    /// Keep the capture running until a frame is available and `due` has
    /// passed, or until `give_up`. Returns whether a frame is available.
    pub(crate) fn wait(&mut self, due: Instant, give_up: Instant) -> Result<bool, String> {
        loop {
            self.advance()?;
            let now = Instant::now();
            if self.completed.is_some() && now >= due {
                return Ok(true);
            }
            if now >= give_up {
                return Ok(self.completed.is_some());
            }
            let until = if self.completed.is_some() {
                due.min(give_up)
            } else {
                give_up
            };
            self.read_events(until.saturating_duration_since(now))?;
        }
    }

    /// Take the latest completed frame as packed BGRA.
    pub(crate) fn take(&mut self) -> Option<Captured> {
        let (index, y_invert) = self.completed.take()?;
        let slot = self.slots[index].as_ref()?;
        let params = slot.params;
        let bgra = to_bgra(&slot.map, params, y_invert)?;
        Some(Captured {
            bgra,
            width: params.width,
            height: params.height,
        })
    }

    /// Drive the outstanding request: issue one when none is outstanding,
    /// send `copy` once the buffer is known, and collect a finished frame.
    fn advance(&mut self) -> Result<(), String> {
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| format!("Wayland dispatch: {e}"))?;
        if self.pending.is_some() && self.state.frame.failed {
            self.consecutive_failures += 1;
            if self.consecutive_failures >= 3 {
                return Err("the compositor failed three screencopy frames in a row".into());
            }
            self.abandon();
        }
        if self.pending.is_some() && self.state.frame.ready {
            self.consecutive_failures = 0;
            if let Some(frame) = self.pending.take() {
                frame.destroy();
            }
            self.completed = Some((self.target, self.state.frame.y_invert));
            self.target = 1 - self.target;
            self.state.frame = FrameProgress::default();
            self.copied = false;
            self.force_full = false;
        }
        if self.pending.is_none() {
            if self.region.width <= 0 || self.region.height <= 0 {
                return Ok(());
            }
            let manager = self.state.manager.as_ref().expect("checked in connect");
            self.state.serial += 1;
            self.state.frame = FrameProgress::default();
            let qh = self.queue.handle();
            self.pending = Some(manager.capture_output_region(
                0,
                &self.output,
                self.region.x,
                self.region.y,
                self.region.width,
                self.region.height,
                &qh,
                self.state.serial,
            ));
        }
        let frame = &self.state.frame;
        let params_final = frame.buffer_done || self.state.manager_version < 3;
        if !self.copied && params_final {
            if let Some(params) = frame.params {
                let qh = self.queue.handle();
                let shm = self.state.shm.as_ref().expect("checked in connect");
                if self.slots[self.target].as_ref().map(|s| s.params) != Some(params) {
                    self.slots[self.target] = Some(Slot::new(shm, params, &qh)?);
                }
                let buffer = &self.slots[self.target].as_ref().expect("just set").buffer;
                let pending = self.pending.as_ref().expect("issued above");
                if self.force_full || self.state.manager_version < 2 {
                    pending.copy(buffer);
                } else {
                    pending.copy_with_damage(buffer);
                }
                self.copied = true;
            }
        }
        self.queue
            .flush()
            .map_err(|e| format!("Wayland flush: {e}"))?;
        Ok(())
    }

    /// Block for at most `timeout` reading events from the compositor.
    fn read_events(&mut self, timeout: Duration) -> Result<(), String> {
        let Some(guard) = self.queue.prepare_read() else {
            return Ok(()); // events already queued
        };
        let mut pollfd = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut pollfd, 1, millis) };
        if ready > 0 {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("Wayland read: {error}")),
            }
        }
        Ok(())
    }
}

impl Drop for Screencopy {
    fn drop(&mut self) {
        if let Some(frame) = self.pending.take() {
            frame.destroy();
        }
        self.slots = [None, None];
        let _ = self.queue.flush();
    }
}

fn pick_output(
    outputs: &[(wl_output::WlOutput, Option<String>)],
    name: &str,
) -> Option<wl_output::WlOutput> {
    outputs
        .iter()
        .find(|(_, n)| n.as_deref() == Some(name))
        .or_else(|| (outputs.len() == 1).then(|| &outputs[0]))
        .map(|(o, _)| o.clone())
}

/// Clip a global logical rectangle to the output at `origin` with logical
/// `size`, returning it in output-local coordinates (None when it does not
/// overlap the output).
pub(crate) fn output_region(
    rect: (i32, i32, u32, u32),
    origin: (i32, i32),
    size: (u32, u32),
) -> Option<OutputRegion> {
    let (x, y, w, h) = rect;
    let left = x.max(origin.0);
    let top = y.max(origin.1);
    let right = (x + w as i32).min(origin.0 + size.0 as i32);
    let bottom = (y + h as i32).min(origin.1 + size.1 as i32);
    (right > left && bottom > top).then(|| OutputRegion {
        x: left - origin.0,
        y: top - origin.1,
        width: right - left,
        height: bottom - top,
    })
}

/// Convert a mapped `wl_shm` buffer to packed, top-down, opaque BGRA.
/// `?RGB8888` is B,G,R,x in memory (little endian) and copies straight;
/// `?BGR8888` swaps red and blue. Other formats are refused.
pub(crate) fn to_bgra(data: &[u8], params: ShmParams, y_invert: bool) -> Option<Vec<u8>> {
    let argb = wl_shm::Format::Argb8888 as u32;
    let xrgb = wl_shm::Format::Xrgb8888 as u32;
    let abgr = wl_shm::Format::Abgr8888 as u32;
    let xbgr = wl_shm::Format::Xbgr8888 as u32;
    let swap = if params.format == argb || params.format == xrgb {
        false
    } else if params.format == abgr || params.format == xbgr {
        true
    } else {
        return None;
    };
    let (width, height, stride) = (
        params.width as usize,
        params.height as usize,
        params.stride as usize,
    );
    let row_bytes = width * 4;
    if stride < row_bytes || data.len() < stride * height {
        return None;
    }
    let mut out = vec![0u8; row_bytes * height];
    for (row, dst) in out.chunks_exact_mut(row_bytes).enumerate() {
        let src_row = if y_invert { height - 1 - row } else { row };
        let src = &data[src_row * stride..src_row * stride + row_bytes];
        dst.copy_from_slice(src);
        for pixel in dst.as_chunks_mut::<4>().0 {
            if swap {
                pixel.swap(0, 2);
            }
            pixel[3] = 255;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(format: wl_shm::Format, width: u32, height: u32, stride: u32) -> ShmParams {
        ShmParams {
            format: format as u32,
            width,
            height,
            stride,
        }
    }

    #[test]
    fn xrgb_copies_rows_and_forces_opaque_alpha() {
        // 2x2, stride 12 (4 bytes of row padding).
        let data = [
            1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99, //
            7, 8, 9, 0, 10, 11, 12, 0, 99, 99, 99, 99,
        ];
        let out = to_bgra(&data, params(wl_shm::Format::Xrgb8888, 2, 2, 12), false).unwrap();
        assert_eq!(
            out,
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }

    #[test]
    fn xbgr_swaps_red_and_blue_and_y_invert_flips_rows() {
        let data = [1, 2, 3, 0, 7, 8, 9, 0];
        let out = to_bgra(&data, params(wl_shm::Format::Xbgr8888, 1, 2, 4), true).unwrap();
        assert_eq!(out, [9, 8, 7, 255, 3, 2, 1, 255]);
    }

    #[test]
    fn unsupported_format_or_short_buffer_is_refused() {
        let data = [0u8; 16];
        assert!(to_bgra(&data, params(wl_shm::Format::Rgb565, 2, 2, 8), false).is_none());
        assert!(to_bgra(&data, params(wl_shm::Format::Xrgb8888, 2, 3, 8), false).is_none());
        assert!(to_bgra(&data, params(wl_shm::Format::Xrgb8888, 3, 1, 8), false).is_none());
    }

    /// Opt-in, against a real compositor (the Omarchy image's Hyprland, or a
    /// headless sway in CI): the first frame is a full copy, an idle output
    /// yields nothing, and damage yields a new frame.
    ///
    ///     CUA_SPACESD_WAYLAND_TEST=1 \
    ///     CUA_SPACESD_WAYLAND_TEST_DAMAGE='swaymsg "output * bg #336699 solid_color"' \
    ///         cargo test -p cua-spacesd-desktop --lib live_compositor
    #[test]
    fn live_compositor_capture_is_damage_driven() {
        if std::env::var_os("CUA_SPACESD_WAYLAND_TEST").is_none() {
            return;
        }
        let output = std::env::var("CUA_SPACESD_WAYLAND_TEST_OUTPUT").unwrap_or_default();
        let mut session = Screencopy::connect(&output).expect("screencopy session");
        let region = OutputRegion {
            x: 0,
            y: 0,
            width: 64,
            height: 48,
        };
        session.set_region(region);
        let now = Instant::now();
        assert!(session.wait(now, now + Duration::from_secs(5)).unwrap());
        let first = session.take().expect("first frame");
        assert_eq!((first.width, first.height), (64, 48));
        assert_eq!(first.bgra.len(), 64 * 48 * 4);

        // wlroots answers the first damage-tracked copy at once (all of it
        // counts as damaged); its pixels are unchanged. After that an idle
        // output yields nothing.
        let now = Instant::now();
        if session.wait(now, now + Duration::from_millis(700)).unwrap() {
            assert_eq!(session.take().unwrap(), first, "an idle output changed");
        }
        let now = Instant::now();
        assert!(
            !session.wait(now, now + Duration::from_millis(700)).unwrap(),
            "an idle output kept producing frames"
        );

        if let Ok(damage) = std::env::var("CUA_SPACESD_WAYLAND_TEST_DAMAGE") {
            let status = std::process::Command::new("sh")
                .args(["-c", &damage])
                .status()
                .expect("damage command");
            assert!(status.success());
            // The change may take a few renders (swaybg starts, then draws).
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut changed = false;
            while !changed && Instant::now() < deadline {
                if session.wait(Instant::now(), deadline).unwrap() {
                    changed = session.take().expect("damaged frame").bgra != first.bgra;
                }
            }
            assert!(changed, "no changed frame after damage");
        }

        // A new region is captured at once, without waiting for damage.
        session.set_region(OutputRegion {
            width: 32,
            height: 16,
            ..region
        });
        let now = Instant::now();
        assert!(session.wait(now, now + Duration::from_secs(5)).unwrap());
        let cropped = session.take().unwrap();
        assert_eq!((cropped.width, cropped.height), (32, 16));
    }

    #[test]
    fn region_is_clipped_to_the_output_and_made_output_local() {
        // Fully inside the second monitor at x=1280.
        assert_eq!(
            output_region((1300, 20, 400, 300), (1280, 0), (1920, 1080)),
            Some(OutputRegion {
                x: 20,
                y: 20,
                width: 400,
                height: 300
            })
        );
        // Hanging off the top-left corner.
        assert_eq!(
            output_region((-10, -5, 100, 50), (0, 0), (1280, 800)),
            Some(OutputRegion {
                x: 0,
                y: 0,
                width: 90,
                height: 45
            })
        );
        // Entirely on another output.
        assert_eq!(
            output_region((2000, 0, 100, 100), (0, 0), (1280, 800)),
            None
        );
    }
}
