//! The Wayland client half of the fixture.
//!
//! The fixture owns its own `wl_surface`, so one input event maps to exactly
//! one content update and `wp_presentation.feedback` is requested for that
//! update and no other. A toolkit-hosted surface cannot make that claim: the
//! toolkit owns the commits and may coalesce several state changes into one.
//!
//! Every timestamp taken here is `CLOCK_MONOTONIC`, which is the clock the
//! fixture reports as its own. The compositor's presentation clock is recorded
//! as advertised and compared explicitly; it is never assumed.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::PathBuf;

use serde_json::json;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_compositor::WlCompositor,
    wl_keyboard::{self, WlKeyboard},
    wl_pointer::{self, WlPointer},
    wl_registry,
    wl_seat::{self, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::{self, WpPresentation},
    wp_presentation_feedback::{self, WpPresentationFeedback},
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

use crate::journal::{write_state, Journal};
use crate::layout::{Layout, Rect};
use crate::sample::{finalize, Feedback, Outcome, Pending, Region, CLOCK_MONOTONIC_ID};
use crate::Config;

/// How long one poll waits before the loop re-checks feedback deadlines.
const POLL_INTERVAL_MS: i32 = 25;

/// Slots in the fixture's `wl_shm` pool. Two is the minimum that lets a new
/// content update be drawn while the compositor still holds the previous one.
const SLOTS: usize = 2;

const APP_ID: &str = "com.trycua.CuaTestHarness.WaylandPresentation";

#[derive(Debug)]
pub enum RunError {
    /// The compositor does not implement stable presentation-time.
    NoPresentationSupport,
    Missing(&'static str),
    Protocol(String),
    Io(io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPresentationSupport => write!(formatter, "wp_presentation is unavailable"),
            Self::Missing(global) => write!(formatter, "required global is unavailable: {global}"),
            Self::Protocol(message) => write!(formatter, "{message}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<io::Error> for RunError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

type Result<T> = std::result::Result<T, RunError>;

/// `CLOCK_MONOTONIC`, the fixture's own clock for every timestamp it takes.
fn now_ns() -> u64 {
    let mut stamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` only writes through the provided pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut stamp) };
    (stamp.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(stamp.tv_nsec as u64)
}

/// Press decoding that compiles whether the generated event exposes the enum
/// directly or wrapped in [`WEnum`].
trait Pressed {
    fn is_pressed(self) -> bool;
}

impl Pressed for wl_pointer::ButtonState {
    fn is_pressed(self) -> bool {
        matches!(self, wl_pointer::ButtonState::Pressed)
    }
}

impl Pressed for WEnum<wl_pointer::ButtonState> {
    fn is_pressed(self) -> bool {
        matches!(self, WEnum::Value(wl_pointer::ButtonState::Pressed))
    }
}

impl Pressed for wl_keyboard::KeyState {
    fn is_pressed(self) -> bool {
        matches!(self, wl_keyboard::KeyState::Pressed)
    }
}

impl Pressed for WEnum<wl_keyboard::KeyState> {
    fn is_pressed(self) -> bool {
        matches!(self, WEnum::Value(wl_keyboard::KeyState::Pressed))
    }
}

/// Bitfield decoding for the same two generated shapes, tolerant of bits this
/// fixture does not know about.
trait Bits {
    fn raw_bits(self) -> u32;
}

impl Bits for wp_presentation_feedback::Kind {
    fn raw_bits(self) -> u32 {
        self.bits()
    }
}

impl Bits for WEnum<wp_presentation_feedback::Kind> {
    fn raw_bits(self) -> u32 {
        match self {
            WEnum::Value(kind) => kind.bits(),
            WEnum::Unknown(bits) => bits,
        }
    }
}

impl Bits for wl_seat::Capability {
    fn raw_bits(self) -> u32 {
        self.bits()
    }
}

impl Bits for WEnum<wl_seat::Capability> {
    fn raw_bits(self) -> u32 {
        match self {
            WEnum::Value(capability) => capability.bits(),
            WEnum::Unknown(bits) => bits,
        }
    }
}

/// One `wl_shm` slot inside the fixture's pool.
struct Slot {
    buffer: WlBuffer,
    offset: usize,
    busy: bool,
}

/// An `mmap`ed memfd pool holding [`SLOTS`] slots, so a new content update
/// never overwrites a buffer the compositor still holds.
struct Pool {
    pool: WlShmPool,
    pointer: *mut libc::c_void,
    size: usize,
    fd: i32,
    slots: Vec<Slot>,
    width: i32,
    height: i32,
    stride: usize,
}

impl Pool {
    fn slot_bytes(&self) -> usize {
        self.stride * (self.height as usize)
    }

    fn pixels(&mut self, index: usize) -> &mut [u8] {
        let offset = self.slots[index].offset;
        let length = self.slot_bytes();
        // SAFETY: `pointer` maps `size` bytes MAP_SHARED and
        // `offset + length <= size` by construction in `App::allocate`.
        unsafe { std::slice::from_raw_parts_mut((self.pointer as *mut u8).add(offset), length) }
    }

    /// Release the protocol objects and the backing memory. Only safe while
    /// the connection is still usable, so it is not what `Drop` does.
    fn release(&mut self) {
        for slot in &self.slots {
            slot.buffer.destroy();
        }
        self.slots.clear();
        self.pool.destroy();
        self.free_memory();
    }

    /// Idempotent: the mapping and descriptor are created together in
    /// `App::allocate` and released at most once.
    fn free_memory(&mut self) {
        if self.pointer.is_null() {
            return;
        }
        // SAFETY: `pointer` maps `size` bytes and `fd` owns that mapping;
        // both are cleared below so this cannot run twice.
        unsafe {
            libc::munmap(self.pointer, self.size);
            if self.fd >= 0 {
                libc::close(self.fd);
            }
        }
        self.pointer = std::ptr::null_mut();
        self.fd = -1;
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // Memory only. An early return may leave the connection unusable, and
        // sending protocol requests from a destructor is not worth the risk.
        self.free_memory();
    }
}

struct App {
    title_prefix: String,
    deadline_ns: u64,
    exit_after: u64,
    state_path: Option<PathBuf>,

    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    wm_base: Option<XdgWmBase>,
    presentation: Option<WpPresentation>,
    surface: Option<WlSurface>,
    toplevel: Option<XdgToplevel>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    pool: Option<Pool>,

    presentation_clock_id: Option<u32>,
    configured: bool,
    closed: bool,
    width: i32,
    height: i32,
    layout: Layout,
    counter: u64,
    sequence: u64,
    accounted: u64,
    presented: u64,
    pointer_position: (f64, f64),
    pending: BTreeMap<u64, Pending>,
    journal: Journal,
    failure: Option<String>,
}

// SAFETY: the raw pointer in `Pool` refers to an mmap region owned
// exclusively by the thread that built this `App`, and the fixture never
// shares its state across threads: `EventQueue<State>` keeps it pinned to this
// thread. The Send bound wayland-client requires of a dispatch state applies
// to the struct as a whole, hence the explicit assertion. This mirrors
// `platform-linux`'s overlay state.
unsafe impl Send for App {}

impl App {
    fn colour(&self) -> [u8; 3] {
        // Deterministic palette, so an independent pixel oracle can be added
        // later without renegotiating fixture behavior.
        const PALETTE: [[u8; 3]; 6] = [
            [0x1e, 0x6f, 0xb8],
            [0xb8, 0x54, 0x1e],
            [0x2f, 0x9e, 0x63],
            [0x8e, 0x44, 0xad],
            [0xc9, 0xa2, 0x27],
            [0x24, 0x8b, 0x8b],
        ];
        PALETTE[(self.counter % PALETTE.len() as u64) as usize]
    }

    fn title(&self) -> String {
        format!("{} [n={}]", self.title_prefix, self.counter)
    }

    fn state_value(&self, action: &str, region: Option<Region>, time_ns: u64) -> serde_json::Value {
        let [red, green, blue] = self.colour();
        json!({
            "schema": crate::sample::JOURNAL_SCHEMA,
            "counter": self.counter,
            "colour": format!("#{red:02x}{green:02x}{blue:02x}"),
            "last_action": action,
            "last_region": region,
            "updated_ns": time_ns,
            "clock_id": CLOCK_MONOTONIC_ID,
        })
    }

    /// Publish application-owned state on a channel separate from the timing
    /// rows, so the runner's mutation assertion never reads the evidence the
    /// presentation path produced.
    fn publish_state(&mut self, action: &str, region: Option<Region>, time_ns: u64) {
        let value = self.state_value(action, region, time_ns);
        if let Some(path) = self.state_path.clone() {
            if let Err(error) = write_state(&path, &value) {
                self.failure = Some(format!("state file write failed: {error}"));
            }
        }
        let _ = self.journal.record("state", time_ns, value);
    }

    fn allocate(&mut self, shm: &WlShm, queue: &QueueHandle<Self>) -> Result<()> {
        if let Some(mut previous) = self.pool.take() {
            previous.release();
        }
        let stride = (self.width as usize) * 4;
        let slot_bytes = stride * (self.height as usize);
        let size = slot_bytes * SLOTS;
        let name = b"cua-wayland-presentation\0";
        // SAFETY: `name` is NUL terminated and the flag is a constant.
        let fd =
            unsafe { libc::memfd_create(name.as_ptr() as *const libc::c_char, libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(RunError::Io(io::Error::last_os_error()));
        }
        // SAFETY: `fd` is the descriptor just created.
        if unsafe { libc::ftruncate(fd, size as libc::off_t) } != 0 {
            let error = io::Error::last_os_error();
            // SAFETY: `fd` is owned here and not yet shared with the compositor.
            unsafe { libc::close(fd) };
            return Err(RunError::Io(error));
        }
        // SAFETY: `fd` is a memfd sized to exactly `size` bytes.
        let pointer = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if pointer == libc::MAP_FAILED {
            let error = io::Error::last_os_error();
            // SAFETY: `fd` is still owned here.
            unsafe { libc::close(fd) };
            return Err(RunError::Io(error));
        }
        // SAFETY: `fd` remains owned by this pool; the borrow lives only for
        // the duration of the `create_pool` request.
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let pool = shm.create_pool(borrowed, size as i32, queue, ());
        let mut slots = Vec::with_capacity(SLOTS);
        for index in 0..SLOTS {
            let offset = index * slot_bytes;
            let buffer = pool.create_buffer(
                offset as i32,
                self.width,
                self.height,
                stride as i32,
                wl_shm::Format::Argb8888,
                queue,
                index,
            );
            slots.push(Slot {
                buffer,
                offset,
                busy: false,
            });
        }
        self.pool = Some(Pool {
            pool,
            pointer,
            size,
            fd,
            slots,
            width: self.width,
            height: self.height,
            stride,
        });
        Ok(())
    }

    /// Fill a free slot with the current state and return its index.
    fn paint(&mut self) -> Result<usize> {
        let layout = self.layout;
        let colour = self.colour();
        let pool = self.pool.as_mut().ok_or(RunError::Missing("wl_shm pool"))?;
        let index = pool
            .slots
            .iter()
            .position(|slot| !slot.busy)
            // Both slots are still held by the compositor. Reusing a held
            // buffer would corrupt an in-flight frame, so report it instead of
            // producing a timing row nobody can trust.
            .ok_or_else(|| RunError::Protocol("no free wl_shm slot".to_owned()))?;
        let stride = pool.stride;
        let width = pool.width;
        let height = pool.height;
        let pixels = pool.pixels(index);
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[0x14, 0x14, 0x14, 0xff]);
        }
        // Argb8888 is little-endian, so the in-memory byte order is B, G, R, A.
        let fill = |pixels: &mut [u8], rect: Rect, rgb: [u8; 3]| {
            let last_row = (rect.y + rect.height).min(height);
            let last_column = (rect.x + rect.width).min(width);
            for row in rect.y..last_row {
                for column in rect.x..last_column {
                    let offset = (row as usize) * stride + (column as usize) * 4;
                    pixels[offset] = rgb[2];
                    pixels[offset + 1] = rgb[1];
                    pixels[offset + 2] = rgb[0];
                    pixels[offset + 3] = 0xff;
                }
            }
        };
        fill(&mut pixels[..], layout.active, colour);
        fill(&mut pixels[..], layout.inert, [0x5a, 0x5a, 0x5a]);
        fill(&mut pixels[..], layout.supersede, [0x2a, 0x3a, 0x4a]);
        Ok(index)
    }

    /// Attach the painted slot, request feedback for the update it is about to
    /// become, and commit. Returns the commit timestamp.
    fn attach_and_commit(
        &mut self,
        index: usize,
        feedback_for: Option<u64>,
        connection: &Connection,
        queue: &QueueHandle<Self>,
    ) -> Result<u64> {
        let surface = self
            .surface
            .clone()
            .ok_or(RunError::Missing("wl_surface"))?;
        let (buffer, width, height) = {
            let pool = self.pool.as_mut().ok_or(RunError::Missing("wl_shm pool"))?;
            pool.slots[index].busy = true;
            (pool.slots[index].buffer.clone(), pool.width, pool.height)
        };
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        if let Some(sequence) = feedback_for {
            let presentation = self
                .presentation
                .clone()
                .ok_or(RunError::Missing("wp_presentation"))?;
            // Requested before the commit it refers to: presentation feedback
            // is bound to the next content update on this surface.
            presentation.feedback(&surface, queue, sequence);
        }
        surface.commit();
        connection
            .flush()
            .map_err(|error| RunError::Protocol(format!("flush after commit failed: {error}")))?;
        Ok(now_ns())
    }

    /// Mutate state and submit exactly one content update whose presentation
    /// feedback is accounted for on its own.
    fn submit(
        &mut self,
        action: &str,
        region: Region,
        input_ns: u64,
        supersede_probe: bool,
        connection: &Connection,
        queue: &QueueHandle<Self>,
    ) -> Result<()> {
        let counter_before = self.counter;
        self.counter += 1;
        let state_changed_ns = now_ns();
        self.publish_state(action, Some(region), state_changed_ns);
        let title = self.title();
        if let Some(toplevel) = self.toplevel.as_ref() {
            // An application-owned mirror of the counter that the Driver can
            // read back through `list_windows`, independently of the fixture's
            // own evidence files.
            toplevel.set_title(title);
        }
        let index = self.paint()?;
        let sequence = self.sequence;
        self.sequence += 1;
        let surface_commit_ns =
            self.attach_and_commit(index, Some(sequence), connection, queue)?;
        self.pending.insert(
            sequence,
            Pending {
                action: action.to_owned(),
                sequence,
                region,
                input_received_ns: input_ns,
                state_changed_ns: Some(state_changed_ns),
                surface_commit_ns: Some(surface_commit_ns),
                counter_before,
                counter_after: self.counter,
                supersede_probe,
            },
        );
        Ok(())
    }

    /// One delivered input event, routed by the region it landed in.
    fn on_input(
        &mut self,
        action: &str,
        region: Option<Region>,
        detail: serde_json::Value,
        connection: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let input_ns = now_ns();
        let _ = self.journal.record(
            "input",
            input_ns,
            json!({
                "action": action,
                "region": region,
                "pointer": [self.pointer_position.0, self.pointer_position.1],
                "counter": self.counter,
                "detail": detail,
            }),
        );
        let Some(region) = region else {
            // Delivered outside every region: no state change, no content
            // update, and no row, so it can never read as a presented
            // mutation.
            return;
        };
        let result = match region {
            Region::Inert => {
                self.record_inert(action, input_ns);
                Ok(())
            }
            Region::Active => self.submit(action, region, input_ns, false, connection, queue),
            Region::Supersede => {
                // Two content updates back to back. The compositor may present
                // both; when it supersedes the first, the discarded path is
                // exercised against a real compositor.
                match self.submit(action, region, input_ns, true, connection, queue) {
                    Ok(()) => self.submit(action, region, input_ns, false, connection, queue),
                    Err(error) => Err(error),
                }
            }
        };
        if let Err(error) = result {
            self.failure = Some(format!("submit failed: {error}"));
        }
    }

    /// A real delivered input that intentionally changes nothing. It is
    /// retained as a typed row so the run can prove the Driver reached the
    /// fixture without a presented mutation being invented for it.
    fn record_inert(&mut self, action: &str, input_ns: u64) {
        let sequence = self.sequence;
        self.sequence += 1;
        let pending = Pending {
            action: action.to_owned(),
            sequence,
            region: Region::Inert,
            input_received_ns: input_ns,
            state_changed_ns: None,
            surface_commit_ns: None,
            counter_before: self.counter,
            counter_after: self.counter,
            supersede_probe: false,
        };
        // No content update exists, so no feedback can ever arrive for it.
        let sample = finalize(&pending, Feedback::Timeout, self.deadline_ns);
        debug_assert_eq!(sample.fixture_outcome, Outcome::NoMutation);
        let _ = self.journal.sample(&sample);
        self.accounted += 1;
    }

    fn complete(&mut self, sequence: u64, feedback: Feedback) {
        let Some(pending) = self.pending.remove(&sequence) else {
            return;
        };
        let sample = finalize(&pending, feedback, self.deadline_ns);
        if sample.fixture_outcome.is_presented_mutation() {
            self.presented += 1;
        }
        let _ = self.journal.sample(&sample);
        self.accounted += 1;
    }

    /// Give up on content updates whose feedback never arrived, so a silent
    /// compositor produces a typed `timeout` row instead of a missing row. The
    /// give-up point is deliberately later than the reporting deadline: a late
    /// presentation is still evidence and is recorded as a deadline miss.
    fn expire(&mut self) {
        let now = now_ns();
        let give_up_ns = self.deadline_ns.saturating_mul(2);
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, pending)| now.saturating_sub(pending.input_received_ns) > give_up_ns)
            .map(|(sequence, _)| *sequence)
            .collect();
        for sequence in expired {
            self.complete(sequence, Feedback::Timeout);
        }
    }

    fn finished(&self) -> bool {
        self.closed
            || self.failure.is_some()
            || (self.exit_after > 0 && self.accounted >= self.exit_after)
    }
}

pub fn run(config: &Config) -> Result<()> {
    let connection = Connection::connect_to_env()
        .map_err(|error| RunError::Protocol(format!("cannot connect to a compositor: {error}")))?;
    let (globals, mut queue): (_, EventQueue<App>) = registry_queue_init(&connection)
        .map_err(|error| RunError::Protocol(format!("registry init failed: {error}")))?;
    let handle = queue.handle();

    let journal = Journal::create(&config.journal)?;
    let mut app = App {
        title_prefix: config.title.clone(),
        deadline_ns: config.deadline_ns(),
        exit_after: config.exit_after,
        state_path: config.state.clone(),
        compositor: None,
        shm: None,
        wm_base: None,
        presentation: None,
        surface: None,
        toplevel: None,
        pointer: None,
        keyboard: None,
        pool: None,
        presentation_clock_id: None,
        configured: false,
        closed: false,
        width: config.width,
        height: config.height,
        layout: Layout::for_size(config.width, config.height),
        counter: 0,
        sequence: 0,
        accounted: 0,
        presented: 0,
        pointer_position: (-1.0, -1.0),
        pending: BTreeMap::new(),
        journal,
        failure: None,
    };

    // `wl_surface.damage_buffer` needs wl_compositor v4. The fixture requires
    // it rather than falling back to deprecated surface-local damage.
    app.compositor = Some(
        globals
            .bind::<WlCompositor, _, _>(&handle, 4..=6, ())
            .map_err(|_| RunError::Missing("wl_compositor v4"))?,
    );
    app.shm = Some(
        globals
            .bind::<WlShm, _, _>(&handle, 1..=1, ())
            .map_err(|_| RunError::Missing("wl_shm"))?,
    );
    app.wm_base = Some(
        globals
            .bind::<XdgWmBase, _, _>(&handle, 1..=6, ())
            .map_err(|_| RunError::Missing("xdg_wm_base"))?,
    );
    // An absent wp_presentation is an environment limitation, not a failure to
    // measure: the fixture reports it as such and never invents a timestamp.
    app.presentation = Some(
        globals
            .bind::<WpPresentation, _, _>(&handle, 1..=1, ())
            .map_err(|_| RunError::NoPresentationSupport)?,
    );
    let _seat = globals
        .bind::<WlSeat, _, _>(&handle, 3..=7, ())
        .map_err(|_| RunError::Missing("wl_seat"))?;

    if config.probe {
        // Support answered without mapping a window. Binding wp_presentation
        // above already failed with NoPresentationSupport when the compositor
        // does not implement it, so reaching here means the protocol is there.
        queue
            .roundtrip(&mut app)
            .map_err(|error| RunError::Protocol(format!("probe roundtrip failed: {error}")))?;
        app.journal.record(
            "probe",
            now_ns(),
            json!({
                "presentation_supported": true,
                "presentation_clock_id": app.presentation_clock_id,
                "presentation_clock_comparable":
                    app.presentation_clock_id == Some(CLOCK_MONOTONIC_ID),
            }),
        )?;
        return Ok(());
    }

    let compositor = app
        .compositor
        .clone()
        .ok_or(RunError::Missing("wl_compositor"))?;
    let wm_base = app
        .wm_base
        .clone()
        .ok_or(RunError::Missing("xdg_wm_base"))?;
    let surface = compositor.create_surface(&handle, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
    let toplevel = xdg_surface.get_toplevel(&handle, ());
    toplevel.set_title(app.title());
    toplevel.set_app_id(APP_ID.to_owned());
    toplevel.set_min_size(config.width, config.height);
    surface.commit();
    app.surface = Some(surface);
    app.toplevel = Some(toplevel);

    // Two roundtrips: the first delivers the presentation clock id and seat
    // capabilities, the second the initial xdg_surface configure.
    queue
        .roundtrip(&mut app)
        .map_err(|error| RunError::Protocol(format!("initial roundtrip failed: {error}")))?;
    queue
        .roundtrip(&mut app)
        .map_err(|error| RunError::Protocol(format!("configure roundtrip failed: {error}")))?;
    if !app.configured {
        return Err(RunError::Protocol(
            "compositor never configured the fixture toplevel".to_owned(),
        ));
    }
    if let Some(message) = app.failure.clone() {
        return Err(RunError::Protocol(message));
    }

    let startup_ns = now_ns();
    app.journal.record(
        "startup",
        startup_ns,
        json!({
            "pid": std::process::id(),
            "title": app.title(),
            "app_id": APP_ID,
            "clock_id": CLOCK_MONOTONIC_ID,
            "presentation_clock_id": app.presentation_clock_id,
            "presentation_clock_comparable":
                app.presentation_clock_id == Some(CLOCK_MONOTONIC_ID),
            "deadline_ns": app.deadline_ns,
            "layout": app.layout,
            "wayland_display": std::env::var("WAYLAND_DISPLAY").ok(),
            "desktop": std::env::var("XDG_CURRENT_DESKTOP").ok(),
        }),
    )?;
    app.publish_state("startup", None, startup_ns);

    // The first paint maps the surface with content so it is visible and
    // clickable. It requests no feedback: it answers no Driver action.
    let shm = app.shm.clone().ok_or(RunError::Missing("wl_shm"))?;
    if app.pool.is_none() {
        app.allocate(&shm, &handle)?;
    }
    let index = app.paint()?;
    app.attach_and_commit(index, None, &connection, &handle)?;
    app.journal.record(
        "mapped",
        now_ns(),
        json!({"width": app.width, "height": app.height}),
    )?;

    while !app.finished() {
        queue
            .dispatch_pending(&mut app)
            .map_err(|error| RunError::Protocol(format!("dispatch failed: {error}")))?;
        app.expire();
        if app.finished() {
            break;
        }
        queue
            .flush()
            .map_err(|error| RunError::Protocol(format!("flush failed: {error}")))?;
        let Some(guard) = queue.prepare_read() else {
            // Events are already queued; dispatch them on the next turn.
            continue;
        };
        let mut poll_fd = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd describing the connection descriptor.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, POLL_INTERVAL_MS) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(RunError::Io(error));
        }
        if ready == 0 {
            // A quiet tick. The loop re-checks feedback deadlines.
            continue;
        }
        match guard.read() {
            Ok(_) => {}
            Err(wayland_client::backend::WaylandError::Io(error))
                if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => {
                return Err(RunError::Protocol(format!(
                    "reading events failed: {error}"
                )))
            }
        }
    }

    let stop_ns = now_ns();
    // Nothing still in flight may be left unaccounted for.
    let outstanding: Vec<u64> = app.pending.keys().copied().collect();
    for sequence in outstanding {
        app.complete(sequence, Feedback::Timeout);
    }
    let failure = app.failure.clone();
    app.journal.record(
        "shutdown",
        stop_ns,
        json!({
            "counter": app.counter,
            "accounted": app.accounted,
            "presented": app.presented,
            "closed": app.closed,
            "failure": failure,
        }),
    )?;
    if let Some(mut pool) = app.pool.take() {
        pool.release();
    }
    let _ = connection.flush();
    match app.failure.take() {
        Some(message) => Err(RunError::Protocol(message)),
        None => Ok(()),
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpPresentation, ()> for App {
    fn event(
        state: &mut Self,
        _: &WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.presentation_clock_id = Some(clk_id);
        }
    }
}

impl Dispatch<WpPresentationFeedback, u64> for App {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        sequence: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let presented_ns = seconds
                    .saturating_mul(1_000_000_000)
                    .saturating_add(u64::from(tv_nsec));
                let clock_id = state.presentation_clock_id;
                state.complete(
                    *sequence,
                    Feedback::Presented {
                        presented_ns,
                        refresh_ns: refresh,
                        sequence: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                        flags: flags.raw_bits(),
                        clock_id,
                    },
                );
            }
            wp_presentation_feedback::Event::Discarded => {
                state.complete(*sequence, Feedback::Discarded);
            }
            // `sync_output` names an output; it is not a completion event.
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for App {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for App {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<XdgToplevel, ()> for App {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                // The canonical Sway lane resizes `CuaTestHarness` windows by
                // title, so honor the compositor's size instead of assuming
                // the requested one.
                if width > 0 && height > 0 && (width != state.width || height != state.height) {
                    state.width = width;
                    state.height = height;
                    state.layout = Layout::for_size(width, height);
                    if let Some(shm) = state.shm.clone() {
                        if let Err(error) = state.allocate(&shm, queue) {
                            state.failure = Some(format!("resize failed: {error}"));
                        }
                    }
                }
            }
            xdg_toplevel::Event::Close => state.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for App {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            let bits = capabilities.raw_bits();
            if bits & wl_seat::Capability::Pointer.bits() != 0 && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(queue, ()));
            }
            if bits & wl_seat::Capability::Keyboard.bits() != 0 && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(queue, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for App {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        connection: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_position = (surface_x, surface_y);
            }
            wl_pointer::Event::Leave { .. } => {
                state.pointer_position = (-1.0, -1.0);
            }
            wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } => {
                if button_state.is_pressed() {
                    let (x, y) = state.pointer_position;
                    let region = state.layout.region_at(x, y);
                    state.on_input(
                        "click",
                        region,
                        json!({"button": button}),
                        connection,
                        queue,
                    );
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        connection: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: key_state,
            ..
        } = event
        {
            if key_state.is_pressed() {
                // A keyboard event carries no coordinates, so it is always
                // attributed to the active region.
                state.on_input(
                    "key",
                    Some(Region::Active),
                    json!({"keycode": key}),
                    connection,
                    queue,
                );
            }
        }
    }
}

impl Dispatch<WlBuffer, usize> for App {
    fn event(
        state: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release) {
            if let Some(pool) = state.pool.as_mut() {
                if let Some(slot) = pool.slots.get_mut(*index) {
                    slot.busy = false;
                }
            }
        }
    }
}

macro_rules! ignore_events {
    ($($proxy:ty),* $(,)?) => {
        $(
            impl Dispatch<$proxy, ()> for App {
                fn event(
                    _: &mut Self,
                    _: &$proxy,
                    _: <$proxy as Proxy>::Event,
                    _: &(),
                    _: &Connection,
                    _: &QueueHandle<Self>,
                ) {
                }
            }
        )*
    };
}

ignore_events!(WlCompositor, WlShm, WlShmPool, WlSurface);
