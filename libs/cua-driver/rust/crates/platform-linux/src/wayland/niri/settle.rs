//! Layout settling: the event-stream history and the wait for niri's geometry
//! to stop changing.

use super::*;

/// How long the layout must stay unchanged before IPC geometry is taken to
/// be what niri renders. This is an assumption, not proof: niri's IPC
/// positions exclude animation offsets and niri reports no animation state,
/// so an animation longer than this (a slow `animations` config) is not
/// detected, and input or a window capture can then use the destination
/// while the window is still drawn elsewhere. niri's default animations
/// finish well inside it.
const SETTLE_QUIET: Duration = Duration::from_secs(1);
/// How long a geometry-bound route waits for the layout to go quiet.
const SETTLE_LIMIT: Duration = Duration::from_secs(4);

// ── Settled geometry ─────────────────────────────────────────────────────────

/// What an event-stream event did to the shown layout.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    /// Anything that can move, uncover or animate a window, and any event
    /// this adapter does not know, so a newer niri fails closed.
    Moved,
    /// Keyboard focus went to this window, or to none.
    Focus(Option<u64>),
    /// This workspace became active on its output.
    Activated(Option<u64>),
    /// This window became its workspace's active window.
    Active(Option<u64>),
}

/// The focus change a pointer action makes itself. Focus moves no floating
/// window, so the events naming the window it focuses are expected.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Expected {
    pub(super) window: Option<u64>,
    /// The window's workspace, already active on its output: when focus
    /// moves there from another output, niri reports it activated.
    pub(super) workspace: Option<u64>,
}

impl Change {
    fn counts(self, expected: Expected) -> bool {
        match self {
            Self::Moved => true,
            Self::Focus(id) | Self::Active(id) => {
                expected.window.is_none() || id != expected.window
            }
            Self::Activated(id) => expected.workspace.is_none() || id != expected.workspace,
        }
    }
}

/// Classifies event-stream events. A window event that only changes a title
/// or similar metadata changes nothing.
#[derive(Default)]
struct LayoutWatch {
    layouts: HashMap<u64, serde_json::Value>,
}

impl LayoutWatch {
    /// The parts of a window that place it on screen.
    fn placing(window: &serde_json::Value) -> serde_json::Value {
        serde_json::json!([
            window.get("workspace_id"),
            window.get("is_floating"),
            window.get("layout")
        ])
    }

    fn change(&mut self, event: &serde_json::Value) -> Option<Change> {
        let Some((name, body)) = event.as_object().and_then(|event| event.iter().next()) else {
            return Some(Change::Moved);
        };
        let id = |key: &str| body.get(key).and_then(serde_json::Value::as_u64);
        match name.as_str() {
            "WindowFocusTimestampChanged"
            | "WindowUrgencyChanged"
            | "WorkspaceUrgencyChanged"
            | "KeyboardLayoutsChanged"
            | "KeyboardLayoutSwitched"
            | "ScreenshotCaptured"
            | "CastsChanged"
            | "CastStartedOrChanged"
            | "CastStopped" => None,
            // A reload can move tiled columns (struts, gaps, centering) and
            // change animation lengths without any other event.
            "ConfigLoaded" => Some(Change::Moved),
            "WindowFocusChanged" => Some(Change::Focus(id("id"))),
            "WorkspaceActivated" => Some(Change::Activated(id("id"))),
            "WorkspaceActiveWindowChanged" => Some(Change::Active(id("active_window_id"))),
            "WindowsChanged" => {
                self.layouts = body
                    .get("windows")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|window| Some((window.get("id")?.as_u64()?, Self::placing(window))))
                    .collect();
                Some(Change::Moved)
            }
            "WindowOpenedOrChanged" => {
                let Some(window) = body.get("window") else {
                    return Some(Change::Moved);
                };
                let Some(id) = window.get("id").and_then(serde_json::Value::as_u64) else {
                    return Some(Change::Moved);
                };
                let placing = Self::placing(window);
                (self.layouts.insert(id, placing.clone()) != Some(placing)).then_some(Change::Moved)
            }
            _ => Some(Change::Moved),
        }
    }
}

/// How many recent changes a history keeps for checks still in flight; a
/// check older than that counts as changed.
const MAX_CHANGES: usize = 1024;

/// How long one catch-up may read: a stream that keeps sending for longer
/// fails, and the history reconnects, rather than holding its lock.
const CATCH_UP_LIMIT: Duration = Duration::from_secs(1);

/// niri's layout changes as one long-lived, attested event stream reports
/// them. niri reports a new place at once and animates to it, and the stream
/// does not report the animation's end, so the layout counts as settled once
/// it has not changed for [`SETTLE_QUIET`]; that covers only animations no
/// longer than it (see [`SETTLE_QUIET`]).
struct Replica {
    lines: Lines,
    generation: u64,
    watch: LayoutWatch,
    /// When the layout last changed. Connecting counts: an animation may
    /// have started before the stream did.
    changed_at: Instant,
    /// Recent changes, the first numbered `first`.
    changes: VecDeque<Change>,
    first: u64,
}

impl Replica {
    fn start(stream: UnixStream, generation: u64) -> Result<Self> {
        let deadline = Instant::now() + QUERY_TIMEOUT;
        let mut lines = Lines::new(stream);
        lines.send("\"EventStream\"", deadline)?;
        let first = lines
            .next(deadline)?
            .context("niri event stream did not start")?;
        reply_ok(
            "EventStream",
            serde_json::from_slice(&first).context("invalid niri IPC JSON")?,
        )?;
        // From here on, reads take only what niri has already sent.
        lines.stream.set_nonblocking(true)?;
        Ok(Self {
            lines,
            generation,
            watch: LayoutWatch::default(),
            changed_at: Instant::now(),
            changes: VecDeque::new(),
            first: 0,
        })
    }

    fn mark(&self) -> Mark {
        Mark {
            generation: self.generation,
            at: self.first + self.changes.len() as u64,
        }
    }

    /// Apply every event niri has sent so far.
    fn catch_up(&mut self) -> Result<()> {
        self.catch_up_within(CATCH_UP_LIMIT)
    }

    fn catch_up_within(&mut self, limit: Duration) -> Result<()> {
        let stop = Instant::now() + limit;
        while let Some(line) = self.lines.next(Instant::now() + QUERY_TIMEOUT)? {
            if Instant::now() > stop {
                bail!("niri sent layout events for over {limit:?} without a pause");
            }
            let event: serde_json::Value =
                serde_json::from_slice(&line).context("invalid niri event")?;
            if let Some(change) = self.watch.change(&event) {
                self.changed_at = Instant::now();
                self.changes.push_back(change);
                if self.changes.len() > MAX_CHANGES {
                    self.changes.pop_front();
                    self.first += 1;
                }
            }
        }
        Ok(())
    }
}

/// Where a layout history stood when a check passed: later changes are new.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub(super) generation: u64,
    pub(super) at: u64,
}

/// The process's niri layout history: one event stream, kept open between
/// actions so an action on a layout that has been quiet for
/// [`SETTLE_QUIET`] need not wait. A background thread reads it as events
/// arrive, which dates each change and keeps niri from dropping a stream
/// that reads too slowly; every check also reads what has arrived itself.
/// A stream that fails is dropped and the next settle connects again; a
/// check made against an earlier connection refuses.
pub(super) struct History {
    replica: Mutex<Option<Replica>>,
    generations: AtomicU64,
}

impl History {
    pub(super) const fn new() -> Self {
        Self {
            replica: Mutex::new(None),
            generations: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Option<Replica>>> {
        self.replica
            .lock()
            .map_err(|_| anyhow!("niri layout history is unavailable"))
    }

    /// Wait until the layout has not changed for `quiet`, at most `limit`,
    /// connecting through `connect` when no stream is open.
    pub(super) fn settle(
        &'static self,
        connect: impl FnOnce() -> Result<UnixStream>,
        quiet: Duration,
        limit: Duration,
    ) -> Result<Mark> {
        let started = Instant::now();
        let mut connect = Some(connect);
        loop {
            let settled = {
                let mut replica = self.lock()?;
                if replica.is_none() {
                    let connect = connect
                        .take()
                        .context("niri event stream closed while settling")?;
                    *replica = Some(self.start(connect()?)?);
                }
                let current = replica.as_mut().expect("connected above");
                if let Err(error) = current.catch_up() {
                    *replica = None;
                    return Err(error.context("niri event stream failed"));
                }
                let settled = current.changed_at + quiet;
                if settled <= Instant::now() {
                    return Ok(current.mark());
                }
                settled
            };
            if settled > started + limit {
                return Err(foreground_unavailable(format!(
                    "the niri layout did not stay unchanged for {} ms within {} ms, so window geometry may not match the screen",
                    quiet.as_millis(),
                    limit.as_millis()
                )));
            }
            std::thread::sleep(settled.saturating_duration_since(Instant::now()));
        }
    }

    fn start(&'static self, stream: UnixStream) -> Result<Replica> {
        let generation = self.generations.fetch_add(1, Ordering::Relaxed) + 1;
        let replica = Replica::start(stream, generation)?;
        let watched = replica.lines.stream.try_clone()?;
        std::thread::Builder::new()
            .name("cua-niri-events".into())
            .spawn(move || self.watch(generation, &watched))?;
        Ok(replica)
    }

    /// Read events as they arrive until this generation's stream is gone.
    fn watch(&self, generation: u64, stream: &UnixStream) {
        loop {
            let mut poll = libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd that outlives the call.
            if unsafe { libc::poll(&mut poll, 1, 1000) } < 0
                && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
            {
                return;
            }
            let Ok(mut replica) = self.replica.lock() else {
                return;
            };
            match replica.as_mut() {
                Some(current) if current.generation == generation => {
                    if let Err(error) = current.catch_up() {
                        tracing::warn!("niri event stream failed: {error:#}");
                        *replica = None;
                        return;
                    }
                }
                _ => return,
            }
        }
    }

    /// Whether the layout changed after `mark`, other than as `expected`.
    /// A stream that failed or reconnected since cannot tell, so it refuses.
    pub(super) fn changed_since(&self, mark: Mark, expected: Expected) -> Result<bool> {
        let mut replica = self.lock()?;
        let Some(current) = replica
            .as_mut()
            .filter(|current| current.generation == mark.generation)
        else {
            bail!("niri's event stream reconnected after the layout was checked");
        };
        if let Err(error) = current.catch_up() {
            *replica = None;
            return Err(error.context("niri event stream failed"));
        }
        let Some(skip) = mark.at.checked_sub(current.first) else {
            return Ok(true);
        };
        Ok(current
            .changes
            .iter()
            .skip(usize::try_from(skip)?)
            .any(|change| change.counts(expected)))
    }
}

pub(super) static HISTORY: History = History::new();

/// Wait until niri's layout has been quiet for [`SETTLE_QUIET`]: at once
/// when it already has been.
pub(super) fn settled() -> Result<Mark> {
    HISTORY.settle(
        || attested_connection(Instant::now() + QUERY_TIMEOUT),
        SETTLE_QUIET,
        SETTLE_LIMIT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // ── Settling ────────────────────────────────────────────────────────────

    #[test]
    fn a_config_reload_counts_as_a_layout_change() {
        // A reload can move tiled columns (struts, gaps, centering) or
        // lengthen animations past the settle period, with no other event.
        let mut watch = LayoutWatch::default();
        for event in [
            json!({"ConfigLoaded": {"failed": false}}),
            json!({"ConfigLoaded": {"failed": true}}),
        ] {
            assert_eq!(watch.change(&event), Some(Change::Moved), "{event}");
        }
    }

    #[test]
    fn title_changes_do_not_count_as_layout_changes() {
        let mut watch = LayoutWatch::default();
        let window = floating(7, 1, (100.0, 60.0), (300, 200));
        let mut change = |event: Value| watch.change(&event);
        assert_eq!(
            change(json!({"WindowsChanged": {"windows": [window.clone()]}})),
            Some(Change::Moved)
        );
        let mut retitled = window.clone();
        retitled["title"] = json!("Float 7 clicked 1");
        assert_eq!(
            change(json!({"WindowOpenedOrChanged": {"window": retitled}})),
            None
        );
        let mut moved = window;
        moved["layout"]["tile_pos_in_workspace_view"] = json!([200.0, 100.0]);
        assert_eq!(
            change(json!({"WindowOpenedOrChanged": {"window": moved}})),
            Some(Change::Moved)
        );
        assert_eq!(
            change(
                json!({"WindowOpenedOrChanged": {"window": floating(9, 1, (0.0, 0.0), (10, 10))}})
            ),
            Some(Change::Moved)
        );
        assert_eq!(
            change(json!({"WindowFocusTimestampChanged": {"id": 7}})),
            None
        );
        assert_eq!(
            change(json!({"SomethingFromALaterNiri": {}})),
            Some(Change::Moved)
        );
    }

    #[test]
    fn only_the_drivers_own_focus_change_is_expected() {
        let mut watch = LayoutWatch::default();
        let mut counts =
            |event: Value, expected: Expected| watch.change(&event).unwrap().counts(expected);
        let none = Expected::default();
        assert!(counts(json!({"WindowFocusChanged": {"id": 7}}), none));
        let focusing = Expected {
            window: Some(7),
            workspace: Some(4),
        };
        assert!(!counts(json!({"WindowFocusChanged": {"id": 7}}), focusing));
        assert!(!counts(
            json!({"WorkspaceActiveWindowChanged": {"workspace_id": 1, "active_window_id": 7}}),
            focusing
        ));
        assert!(counts(json!({"WindowFocusChanged": {"id": 8}}), focusing));
        assert!(counts(
            json!({"WindowFocusChanged": {"id": null}}),
            focusing
        ));
        assert!(counts(
            json!({"WorkspaceActiveWindowChanged": {"workspace_id": 1, "active_window_id": 8}}),
            focusing
        ));
        // Focus moving to the target's (already active) workspace on another
        // output is expected; another workspace is not.
        assert!(!counts(
            json!({"WorkspaceActivated": {"id": 4, "focused": true}}),
            focusing
        ));
        assert!(counts(
            json!({"WorkspaceActivated": {"id": 3, "focused": true}}),
            focusing
        ));
        assert!(counts(
            json!({"WorkspaceActivated": {"id": 4, "focused": true}}),
            none
        ));
    }

    fn workspaces_changed() -> Value {
        json!({"WorkspacesChanged": {"workspaces": []}})
    }

    #[test]
    fn a_layout_that_stayed_quiet_settles_at_once_for_the_next_action() {
        let history = history();
        let quiet = Duration::from_millis(200);
        let started = Instant::now();
        let first = history
            .settle(
                || Ok(event_stream(vec![(Duration::ZERO, workspaces_changed())])),
                quiet,
                Duration::from_secs(2),
            )
            .unwrap();
        // Connecting counts as a change: an animation may be running.
        assert!(started.elapsed() >= quiet);
        let again = Instant::now();
        let second = history
            .settle(
                || bail!("the stream stays open between actions"),
                quiet,
                Duration::from_secs(2),
            )
            .unwrap();
        assert!(
            again.elapsed() < Duration::from_millis(100),
            "{:?}",
            again.elapsed()
        );
        assert_eq!(first, second);
    }

    #[test]
    fn a_change_is_dated_when_niri_sends_it_not_when_an_action_looks() {
        let history = history();
        let quiet = Duration::from_millis(200);
        let started = Instant::now();
        // The change comes 50 ms in, while the first wait sleeps: read then,
        // the layout is quiet 250 ms in; read when the wait wakes, 400 ms.
        history
            .settle(
                || {
                    Ok(event_stream(vec![(
                        Duration::from_millis(50),
                        json!({"WindowLayoutsChanged": {"changes": []}}),
                    )]))
                },
                quiet,
                Duration::from_secs(2),
            )
            .unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(250), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(380), "{elapsed:?}");
    }

    #[test]
    fn a_recent_change_still_waits_out_the_quiet_period() {
        let history = history();
        let quiet = Duration::from_millis(200);
        history
            .settle(
                || {
                    Ok(event_stream(vec![(
                        Duration::from_millis(400),
                        json!({"WindowLayoutsChanged": {"changes": []}}),
                    )]))
                },
                quiet,
                Duration::from_secs(2),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(500).saturating_sub(quiet));
        let again = Instant::now();
        history
            .settle(|| bail!("still connected"), quiet, Duration::from_secs(2))
            .unwrap();
        assert!(
            again.elapsed() >= Duration::from_millis(80),
            "{:?}",
            again.elapsed()
        );
    }

    #[test]
    fn a_layout_that_keeps_moving_refuses() {
        let events = (0..40)
            .map(|_| {
                (
                    Duration::from_millis(20),
                    json!({"WindowLayoutsChanged": {"changes": []}}),
                )
            })
            .collect();
        let error = history()
            .settle(
                || Ok(event_stream(events)),
                Duration::from_millis(100),
                Duration::from_millis(300),
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: the niri layout did not stay unchanged for 100 ms within 300 ms, so window geometry may not match the screen"
        );
    }

    #[test]
    fn a_stream_that_keeps_sending_fails_instead_of_holding_the_history() {
        let events = (0..64)
            .map(|_| {
                (
                    Duration::ZERO,
                    json!({"WindowLayoutsChanged": {"changes": []}}),
                )
            })
            .collect();
        let mut replica = Replica::start(event_stream(events), 1).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let error = replica.catch_up_within(Duration::ZERO).unwrap_err();
        assert!(error.to_string().contains("without a pause"), "{error}");
    }

    #[test]
    fn a_change_after_settling_is_detected() {
        let history = history();
        let mark = history
            .settle(
                || {
                    Ok(event_stream(vec![(
                        Duration::from_millis(150),
                        json!({"WorkspaceActivated": {"id": 2, "focused": true}}),
                    )]))
                },
                Duration::from_millis(100),
                Duration::from_secs(1),
            )
            .unwrap();
        assert!(!history.changed_since(mark, Expected::default()).unwrap());
        std::thread::sleep(Duration::from_millis(150));
        assert!(history.changed_since(mark, Expected::default()).unwrap());
    }

    #[test]
    fn a_lost_or_reconnected_stream_cannot_vouch_for_an_earlier_check() {
        let history = history();
        let quiet = Duration::from_millis(50);
        let mark = history
            .settle(
                || Ok(event_stream_open(vec![], Duration::from_millis(100))),
                quiet,
                Duration::from_secs(1),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        // niri closed the stream: whatever happened since is unknown.
        assert!(history.changed_since(mark, Expected::default()).is_err());
        let again = history
            .settle(|| Ok(event_stream(vec![])), quiet, Duration::from_secs(1))
            .unwrap();
        assert_ne!(again, mark);
        assert_eq!(
            history
                .changed_since(mark, Expected::default())
                .unwrap_err()
                .to_string(),
            "niri's event stream reconnected after the layout was checked"
        );
        assert!(!history.changed_since(again, Expected::default()).unwrap());
    }
}
