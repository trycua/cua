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
}

/// Physical-input watch over one transaction. Hold it from before the first
/// change until the decision to hand anything back.
pub(crate) struct InputQuiet {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    notification: ExtIdleNotificationV1,
}

impl InputQuiet {
    /// Start watching and wait up to `barrier_wait` for the person's input
    /// to go quiet. Refuses when it does not.
    pub(crate) fn begin(barrier_wait: Duration) -> Result<Self> {
        let connection = super::hyprland::wayland_connection()?;
        super::hyprland::verify_capture_peer(&connection)?;
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        connection.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).context("Wayland registry")?;
        queue.roundtrip(&mut state).context("Wayland seat names")?;
        let seat = state
            .seats
            .selected()
            .context("no person-owned Wayland seat")?;
        let (notifier, version) = state
            .notifier
            .clone()
            .context("the compositor does not offer ext-idle-notify")?;
        // Version 2 ignores idle inhibitors (a playing video), which would
        // otherwise hold back the barrier indefinitely.
        let notification = if version >= 2 {
            notifier.get_input_idle_notification(BARRIER_TIMEOUT_MS, &seat, &qh, ())
        } else {
            notifier.get_idle_notification(BARRIER_TIMEOUT_MS, &seat, &qh, ())
        };
        let mut watch = Self {
            connection,
            queue,
            state,
            notification,
        };
        let deadline = Instant::now() + barrier_wait;
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
    /// everything the compositor sent up to now. Any failure is "no".
    pub(crate) fn quiet_since_begin(&mut self, budget: Duration) -> bool {
        self.drain(budget).is_ok() && self.state.marks.quiet()
    }

    fn drain(&mut self, budget: Duration) -> Result<()> {
        let qh = self.queue.handle();
        self.state.synced = false;
        self.connection.display().sync(&qh, ());
        let deadline = Instant::now() + budget;
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
        self.notification.destroy();
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
            ext_idle_notification_v1::Event::Resumed => state.marks.observe(Activity::Resumed),
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
