//! Prove the person gave no physical input across one short desktop
//! transaction, using `ext-idle-notify-v1` on the person's own seat.
//!
//! A barrier `idled` must arrive before the transaction starts. After it, any
//! `resumed` is sticky: a later `idled` never clears it, because the person's
//! input would otherwise be erased by the next quiet stretch. A missing
//! protocol, seat, barrier or a watcher failure means "not proven".

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use wayland_client::{
    protocol::{wl_callback, wl_registry, wl_seat},
    Connection, Dispatch, EventQueue, QueueHandle,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

use super::primary_seat::Seats;

/// Shortest quiet stretch that counts as the barrier. Any physical input
/// after it produces `resumed`.
const BARRIER_TIMEOUT_MS: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Activity {
    Idled,
    Resumed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Marks {
    barrier: bool,
    resumed: bool,
}

impl Marks {
    fn observe(&mut self, activity: Activity) {
        match activity {
            Activity::Idled => self.barrier = true,
            // Sticky: once the person has acted, no later idle erases it.
            Activity::Resumed => self.resumed = true,
        }
    }

    fn quiet(self) -> bool {
        self.barrier && !self.resumed
    }
}

#[derive(Default)]
struct State {
    seats: Seats<wl_seat::WlSeat>,
    notifier: Option<(ExtIdleNotifierV1, u32)>,
    marks: Marks,
    synced: bool,
    /// When the first `resumed` after the barrier was handled; timing only,
    /// for diagnosing a skipped hand-back.
    first_resumed: Option<Instant>,
}

/// Physical-input watch over one transaction. Hold it from before the first
/// change until the decision to hand anything back.
pub(crate) struct InputQuiet {
    begun: Instant,
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    notification: Option<ExtIdleNotificationV1>,
}

impl InputQuiet {
    /// Start watching and wait for the person's input to go quiet. Every
    /// compositor exchange shares the one `budget`; nothing here blocks past
    /// it. Refuses when the barrier does not arrive in time.
    pub(crate) fn begin(budget: Duration) -> Result<Self> {
        let deadline = Instant::now() + budget;
        let connection = super::hyprland::wayland_connection()?;
        super::hyprland::verify_capture_peer(&connection)?;
        let queue = connection.new_event_queue();
        let qh = queue.handle();
        connection.display().get_registry(&qh, ());
        let mut watch = Self {
            begun: Instant::now(),
            connection,
            queue,
            state: State::default(),
            notification: None,
        };
        watch.sync_by(deadline).context("Wayland registry")?;
        // Seat names arrive after their bind.
        watch.sync_by(deadline).context("Wayland seat names")?;
        let seat = watch
            .state
            .seats
            .selected()
            .context("no person-owned Wayland seat")?;
        let (notifier, version) = watch
            .state
            .notifier
            .clone()
            .context("the compositor does not offer ext-idle-notify")?;
        // Version 2 ignores idle inhibitors (a playing video), which would
        // otherwise hold back the barrier indefinitely.
        watch.notification = Some(if version >= 2 {
            notifier.get_input_idle_notification(BARRIER_TIMEOUT_MS, &seat, &qh, ())
        } else {
            notifier.get_idle_notification(BARRIER_TIMEOUT_MS, &seat, &qh, ())
        });
        while !watch.state.marks.barrier {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("the person's input did not go quiet before the transaction");
            }
            watch.dispatch_for(remaining)?;
        }
        if watch.state.marks.resumed {
            bail!("physical input arrived while the watch was starting");
        }
        Ok(watch)
    }

    /// Whether no physical input arrived since the barrier, after draining
    /// everything the compositor sent up to now. Any failure is "no". Call it
    /// again immediately before each change made on the person's behalf.
    pub(crate) fn quiet_since_begin(&mut self, budget: Duration) -> bool {
        self.sync_by(Instant::now() + budget).is_ok() && self.state.marks.quiet()
    }

    /// Milliseconds from the start of the watch to the first input after the
    /// barrier, when there was any. A content-free diagnostic.
    pub(crate) fn first_input_after_ms(&self) -> Option<u128> {
        self.state
            .first_resumed
            .map(|at| at.saturating_duration_since(self.begun).as_millis())
    }

    /// One bounded `wl_display.sync`: every event sent before it is handled.
    fn sync_by(&mut self, deadline: Instant) -> Result<()> {
        let qh = self.queue.handle();
        self.state.synced = false;
        self.connection.display().sync(&qh, ());
        while !self.state.synced {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("the compositor did not answer the idle watch in time");
            }
            self.dispatch_for(remaining)?;
        }
        Ok(())
    }

    fn dispatch_for(&mut self, timeout: Duration) -> Result<()> {
        self.queue.dispatch_pending(&mut self.state)?;
        self.queue.flush()?;
        let Some(guard) = self.queue.prepare_read() else {
            self.queue.dispatch_pending(&mut self.state)?;
            return Ok(());
        };
        let mut fds = [libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&guard.connection_fd().as_fd()),
            events: libc::POLLIN,
            revents: 0,
        }];
        let millis = i32::try_from(timeout.as_millis().max(1)).unwrap_or(i32::MAX);
        // SAFETY: one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, millis) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        } else if ready > 0 {
            guard.read()?;
        }
        self.queue.dispatch_pending(&mut self.state)?;
        Ok(())
    }
}

impl Drop for InputQuiet {
    fn drop(&mut self) {
        if let Some(notification) = self.notification.take() {
            notification.destroy();
        }
        let _ = self.connection.flush();
    }
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
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" => state.seats.add(registry.bind::<wl_seat::WlSeat, _, _>(
                    name,
                    version.min(2),
                    qh,
                    (),
                )),
                "ext_idle_notifier_v1" => {
                    let version = version.min(2);
                    state.notifier = Some((
                        registry.bind::<ExtIdleNotifierV1, _, _>(name, version, qh, ()),
                        version,
                    ));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Name { name } = event {
            state.seats.name(seat, name);
        }
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.marks.observe(Activity::Idled),
            ext_idle_notification_v1::Event::Resumed => {
                if state.marks.barrier && state.first_resumed.is_none() {
                    state.first_resumed = Some(Instant::now());
                }
                state.marks.observe(Activity::Resumed)
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(events: &[Activity]) -> Marks {
        let mut marks = Marks::default();
        for event in events {
            marks.observe(*event);
        }
        marks
    }

    /// Read-only probe of the live desktop: run with
    /// `cargo test -- --ignored input_quiet_live_probe --nocapture`.
    #[test]
    #[ignore = "reads the live compositor"]
    fn input_quiet_live_probe() {
        let started = Instant::now();
        match InputQuiet::begin(Duration::from_millis(400)) {
            Ok(mut watch) => {
                println!(
                    "begin ok in {:?}; notifier version {:?}",
                    started.elapsed(),
                    watch.state.notifier.as_ref().map(|(_, version)| *version)
                );
                println!(
                    "quiet now: {}",
                    watch.quiet_since_begin(Duration::from_millis(500))
                );
            }
            Err(error) => println!("begin failed in {:?}: {error:#}", started.elapsed()),
        }
        println!(
            "active={:?} workspace={:?} cursor={:?} locked={:?}",
            super::super::hyprland::active_window_address().map_err(|e| e.to_string()),
            super::super::hyprland::single_output_workspace().map_err(|e| e.to_string()),
            super::super::hyprland::cursor_position().map_err(|e| e.to_string()),
            super::super::hyprland::session_locked().map_err(|e| e.to_string()),
        );
    }

    /// Read-only: record only the times of idle/resume transitions for
    /// `CUA_IDLE_TRACE_SECS` seconds (no input content is observable here).
    #[test]
    #[ignore = "reads the live compositor"]
    fn input_quiet_live_activity_trace() {
        let secs: u64 = std::env::var("CUA_IDLE_TRACE_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(30);
        let mut watch = match InputQuiet::begin(Duration::from_millis(2000)) {
            Ok(watch) => watch,
            Err(error) => return println!("begin failed: {error:#}"),
        };
        let start = Instant::now();
        let mut last = watch.state.marks;
        println!("t=0.000 barrier");
        while start.elapsed() < Duration::from_secs(secs) {
            watch.state.marks = Marks {
                barrier: true,
                resumed: false,
            };
            let _ = watch.dispatch_for(Duration::from_millis(200));
            if watch.state.marks.resumed {
                println!("t={:.3} resumed", start.elapsed().as_secs_f64());
            }
            last = watch.state.marks;
        }
        let _ = last;
    }

    #[test]
    fn only_an_uninterrupted_quiet_stretch_after_the_barrier_is_quiet() {
        assert!(marks(&[Activity::Idled]).quiet());
        // No barrier: the person may have been acting all along.
        assert!(!marks(&[]).quiet());
        // Input after the barrier stays recorded even when the person goes
        // quiet again before the decision.
        assert!(!marks(&[Activity::Idled, Activity::Resumed]).quiet());
        assert!(!marks(&[Activity::Idled, Activity::Resumed, Activity::Idled]).quiet());
        assert!(!marks(&[
            Activity::Idled,
            Activity::Resumed,
            Activity::Idled,
            Activity::Resumed,
            Activity::Idled
        ])
        .quiet());
    }
}
