// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The permission guide's logic, kept free of AppKit so it can be tested.
//!
//! The guide walks a newcomer through the two macOS switches Cua Driver
//! needs, one at a time:
//!
//! 1. Accessibility ("use your mouse and keyboard"), then
//! 2. Screen Recording ("see your screen").
//!
//! [`GuideMachine`] decides which step shows for a status, when to ask macOS
//! for the next permission (strictly one at a time, Accessibility first) and
//! when to open the next System Settings pane. [`ResumeMarker`] carries the
//! guide across macOS's "Quit & Reopen" for Screen Recording, which quits the
//! app and starts it again with no arguments.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::permissions::gate::MissingPermission;
use crate::permissions::status::PermissionsStatus;

/// The step the guide shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideStep {
    Accessibility,
    ScreenRecording,
    Done,
}

impl GuideStep {
    /// The first switch that is still off, Accessibility first.
    pub fn for_status(status: PermissionsStatus) -> Self {
        if !status.accessibility {
            Self::Accessibility
        } else if !status.screen_recording {
            Self::ScreenRecording
        } else {
            Self::Done
        }
    }

    /// The permission this step asks for, or `None` once done.
    pub fn permission(self) -> Option<MissingPermission> {
        match self {
            Self::Accessibility => Some(MissingPermission::Accessibility),
            Self::ScreenRecording => Some(MissingPermission::ScreenRecording),
            Self::Done => None,
        }
    }

    pub fn copy(self) -> StepCopy {
        match self {
            Self::Accessibility => StepCopy {
                counter: "Step 1 of 2",
                title: "Let Cua Driver use your mouse and keyboard",
                why: "This lets Cua Driver click and type in your apps for your agents.",
                how: "In System Settings, turn on the switch next to Cua Driver.",
                note: "",
            },
            Self::ScreenRecording => StepCopy {
                counter: "Step 2 of 2",
                title: "Let Cua Driver see your screen",
                why: "This lets Cua Driver take screenshots, so an agent can see what it is doing.",
                how: "In System Settings, turn on the switch next to Cua Driver.",
                note: "If macOS asks to quit and reopen Cua Driver, click Quit & Reopen. \
                       This window comes back to finish.",
            },
            Self::Done => StepCopy {
                counter: "",
                title: DONE_TITLE,
                why: "You can close System Settings.",
                how: "",
                note: "",
            },
        }
    }
}

/// Words a newcomer reads for one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepCopy {
    pub counter: &'static str,
    pub title: &'static str,
    pub why: &'static str,
    pub how: &'static str,
    pub note: &'static str,
}

pub const APP_NAME: &str = "Cua Driver";
pub const WINDOW_TITLE: &str = "Set up Cua Driver";
pub const DONE_TITLE: &str = "All set. Cua Driver can now use this Mac.";
pub const OPEN_SETTINGS_BUTTON: &str = "Open System Settings";
pub const NOT_NOW_BUTTON: &str = "Not Now";
pub const DRAG_HINT: &str = "If Cua Driver isn't in the list, drag this into it:";
pub const TILE_NAME: &str = "Cua Driver";
pub const TILE_DRAG_LABEL: &str = "Drag";
pub const WAITING_LINE: &str = "Waiting for the switch to turn on…";
pub const TURNED_ON_LINE: &str = "Turned on.";

pub fn for_app(text: &str, app: &str) -> String {
    text.replace(APP_NAME, app)
}

/// What the guide must do after reading a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideEffect {
    /// Ask macOS for this permission (its prompt also adds Cua Driver to
    /// the pane's list).
    Request(MissingPermission),
    /// Open this permission's pane in System Settings.
    OpenSettings(MissingPermission),
    /// Redraw the window for this step.
    Show(GuideStep),
}

/// One guide session: which step shows, and which permissions have already
/// been asked for, so each is asked once and never two at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideMachine {
    step: GuideStep,
    asked_accessibility: bool,
    asked_screen_recording: bool,
}

impl GuideMachine {
    /// Start a session. `asked` lists permissions already requested before
    /// the guide opened (the gate asks for the first one up front).
    pub fn new(status: PermissionsStatus, asked: &[MissingPermission]) -> Self {
        Self {
            step: GuideStep::for_status(status),
            asked_accessibility: asked.contains(&MissingPermission::Accessibility),
            asked_screen_recording: asked.contains(&MissingPermission::ScreenRecording),
        }
    }

    pub fn step(&self) -> GuideStep {
        self.step
    }

    /// The permission to ask macOS for now, if any. Accessibility comes
    /// first; Screen Recording is asked only once Accessibility is on, so
    /// macOS never has two prompts for Cua Driver up at once (it shows one
    /// and drops the other, which left Cua Driver out of the Screen
    /// Recording list).
    pub fn pending_request(&self, status: PermissionsStatus) -> Option<MissingPermission> {
        next_request(
            status,
            self.asked_accessibility,
            self.asked_screen_recording,
        )
    }

    /// Record that `permission` was requested.
    pub fn mark_requested(&mut self, permission: MissingPermission) {
        match permission {
            MissingPermission::Accessibility => self.asked_accessibility = true,
            MissingPermission::ScreenRecording => self.asked_screen_recording = true,
        }
    }

    /// Feed a fresh status (see [`PollSchedule`]). Returns what to
    /// do, in order. Requests are marked as made.
    pub fn on_status(&mut self, status: PermissionsStatus) -> Vec<GuideEffect> {
        let mut effects = Vec::new();
        let next = GuideStep::for_status(status);
        let advanced = next != self.step;
        if advanced {
            self.step = next;
            effects.push(GuideEffect::Show(next));
        }
        if let Some(permission) = self.pending_request(status) {
            self.mark_requested(permission);
            effects.push(GuideEffect::Request(permission));
        }
        // Moving on by itself: the person is already in System Settings, so
        // take them straight to the next pane.
        if advanced {
            if let Some(permission) = next.permission() {
                effects.push(GuideEffect::OpenSettings(permission));
            }
        }
        effects
    }
}

/// Which permission to request next, strictly one at a time: Accessibility
/// while it is off and not yet asked; Screen Recording only once
/// Accessibility is on.
pub fn next_request(
    status: PermissionsStatus,
    asked_accessibility: bool,
    asked_screen_recording: bool,
) -> Option<MissingPermission> {
    if !status.accessibility {
        return (!asked_accessibility).then_some(MissingPermission::Accessibility);
    }
    if !status.screen_recording && !asked_screen_recording {
        return Some(MissingPermission::ScreenRecording);
    }
    None
}

// ── When to check the switches again ───────────────────────────────────────

/// How often the guide and the terminal wait check the switches. Each check
/// starts a fresh helper process, because macOS caches a "no" in the process
/// that asked. So checks are quick only for a while after something happens
/// (the wait starts, a switch turns on, the person opens System Settings) and
/// slower the rest of the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollCadence {
    /// Time between checks right after something happens.
    pub quick: Duration,
    /// How long checks stay quick after something happens.
    pub quick_for: Duration,
    /// Time between checks the rest of the time.
    pub settled: Duration,
}

impl PollCadence {
    /// While the guide or the terminal text is up and someone may be in
    /// System Settings.
    pub const INTERACTIVE: Self = Self {
        quick: Duration::from_secs(1),
        quick_for: Duration::from_secs(30),
        settled: Duration::from_secs(5),
    };

    /// One check every `every`, whatever happens.
    pub const fn steady(every: Duration) -> Self {
        Self {
            quick: every,
            quick_for: Duration::ZERO,
            settled: every,
        }
    }
}

/// When the next check is due under a [`PollCadence`].
#[derive(Debug, Clone, Copy)]
pub struct PollSchedule {
    cadence: PollCadence,
    active_since: Instant,
    last_check: Option<Instant>,
}

impl PollSchedule {
    pub fn new(cadence: PollCadence, now: Instant) -> Self {
        Self {
            cadence,
            active_since: now,
            last_check: None,
        }
    }

    /// Something happened: check quickly again for a while.
    pub fn note_activity(&mut self, now: Instant) {
        self.active_since = now;
    }

    /// Record a check made at `now`.
    pub fn checked(&mut self, now: Instant) {
        self.last_check = Some(now);
    }

    /// How long until the next check is due; zero when it is due now.
    pub fn wait(&self, now: Instant) -> Duration {
        let Some(last) = self.last_check else {
            return Duration::ZERO;
        };
        let quick = now.saturating_duration_since(self.active_since) < self.cadence.quick_for;
        let interval = if quick {
            self.cadence.quick
        } else {
            self.cadence.settled
        };
        interval.saturating_sub(now.saturating_duration_since(last))
    }
}

// ── Resuming after "Quit & Reopen" ─────────────────────────────────────────

/// How long a marker stays valid. "Quit & Reopen" happens within seconds of
/// the switch turning on; a marker older than this is stale.
pub const RESUME_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Flags never carried over to an automatic relaunch: the person must pass
/// them again on purpose.
const NOT_REPLAYED_FLAGS: &[&str] = &[
    "--dangerously-bypass-approvals",
    "--approve-capability-manifest",
    "--approve-session-policy",
];
const NOT_REPLAYED_VALUE_FLAGS: &[&str] = &["--grant"];

/// A small file written while the Screen Recording step is open (and kept
/// briefly after it finishes). When macOS's "Quit & Reopen" starts the app
/// again with no arguments, the new process finds it, starts serving with
/// the same arguments, and shows the finished state instead of nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResumeMarker {
    /// Unix seconds when written.
    pub written_at: u64,
    /// The `serve` arguments to start again with (first item is `serve`).
    pub serve_args: Vec<String>,
}

impl ResumeMarker {
    pub fn new(serve_args: Vec<String>, now: SystemTime) -> Self {
        Self {
            written_at: unix_secs(now),
            serve_args: replayable_serve_args(&serve_args),
        }
    }

    pub fn is_fresh(&self, now: SystemTime) -> bool {
        let now = unix_secs(now);
        now >= self.written_at && now - self.written_at <= RESUME_WINDOW.as_secs()
    }
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The `serve` arguments safe to replay, without approval-bypass flags.
/// Anything that isn't a `serve` command becomes plain `serve`.
pub fn replayable_serve_args(args: &[String]) -> Vec<String> {
    if args.first().map(String::as_str) != Some("serve") {
        return vec!["serve".to_owned()];
    }
    let mut out = Vec::with_capacity(args.len());
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if NOT_REPLAYED_FLAGS.contains(&arg.as_str()) {
            continue;
        }
        if NOT_REPLAYED_VALUE_FLAGS.contains(&arg.as_str()) {
            skip_value = true;
            continue;
        }
        if NOT_REPLAYED_VALUE_FLAGS
            .iter()
            .any(|flag| arg.starts_with(&format!("{flag}=")))
        {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

/// Where the marker lives for the app this executable belongs to, e.g.
/// `~/Library/Caches/CuaDriver/permission-guide.json`. `None` outside an
/// app bundle: only the bundled app is reopened by macOS.
pub fn marker_path_for(exe: &Path, home: &Path) -> Option<PathBuf> {
    let app = exe
        .ancestors()
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))?;
    let stem = app.file_stem()?.to_str()?;
    Some(
        home.join("Library/Caches")
            .join(stem)
            .join("permission-guide.json"),
    )
}

fn marker_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let home = std::env::var_os("HOME")?;
    marker_path_for(&exe, Path::new(&home))
}

/// Remember that the guide is on (or just finished) the Screen Recording
/// step, so a "Quit & Reopen" comes back to it.
pub fn write_resume_marker() {
    let Some(path) = marker_path() else { return };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let marker = ResumeMarker::new(args, SystemTime::now());
    let Ok(body) = serde_json::to_vec(&marker) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = write_private(&path, &body);
}

fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(body)
}

fn read_marker() -> Option<ResumeMarker> {
    let path = marker_path()?;
    let body = std::fs::read(&path).ok()?;
    let marker: ResumeMarker = serde_json::from_slice(&body).ok()?;
    if marker.is_fresh(SystemTime::now()) {
        Some(marker)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}

/// Remove the marker. Returns whether a fresh one was there.
pub fn take_resume_marker() -> bool {
    let fresh = read_marker().is_some();
    if let Some(path) = marker_path() {
        let _ = std::fs::remove_file(path);
    }
    fresh
}

/// True when this process was started by macOS with no arguments (a
/// "Quit & Reopen", or a double-click in Finder): no arguments other than
/// an old-style `-psn_` one, and stdin is not a terminal, pipe or socket.
pub fn started_by_launch_services_without_args() -> bool {
    let bare = std::env::args().skip(1).all(|arg| arg.starts_with("-psn_"));
    bare && stdin_is_null_device()
}

fn stdin_is_null_device() -> bool {
    use std::io::IsTerminal as _;
    if std::io::stdin().is_terminal() {
        return false;
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(0, &mut stat) } != 0 {
        // A closed stdin is what LaunchServices may give too.
        return true;
    }
    (stat.st_mode & libc::S_IFMT) == libc::S_IFCHR
}

/// The `serve` arguments to start again with when macOS reopened the app
/// mid-guide. `None` for every ordinary launch.
pub fn resume_serve_args() -> Option<Vec<String>> {
    if !started_by_launch_services_without_args() {
        return None;
    }
    read_marker().map(|marker| replayable_serve_args(&marker.serve_args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(accessibility: bool, screen_recording: bool) -> PermissionsStatus {
        PermissionsStatus {
            accessibility,
            screen_recording,
        }
    }

    /// Seconds between the checks a schedule makes over `span`, with
    /// `activity_at` seconds where something happens.
    fn check_gaps(cadence: PollCadence, span: u64, activity_at: &[u64]) -> Vec<u64> {
        let start = Instant::now();
        let mut schedule = PollSchedule::new(cadence, start);
        let mut checks = Vec::new();
        for second in 0..=span {
            let now = start + Duration::from_secs(second);
            if activity_at.contains(&second) {
                schedule.note_activity(now);
            }
            if schedule.wait(now).is_zero() {
                schedule.checked(now);
                checks.push(second);
            }
        }
        checks.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }

    #[test]
    fn checks_are_quick_only_for_a_while_after_something_happens() {
        let gaps = check_gaps(PollCadence::INTERACTIVE, 120, &[60]);
        // Every second for the first 30 s, then every 5 s until the activity
        // at 60 s makes them quick again for 30 s, then every 5 s.
        let mut expected = vec![1; 29];
        expected.extend([5; 6]);
        expected.extend([1; 30]);
        expected.extend([5; 6]);
        assert_eq!(gaps, expected);

        let gaps = check_gaps(PollCadence::steady(Duration::from_secs(30)), 120, &[45]);
        assert_eq!(gaps, vec![30, 30, 30, 30]);
    }

    #[test]
    fn step_follows_the_first_switch_that_is_off() {
        assert_eq!(
            GuideStep::for_status(st(false, false)),
            GuideStep::Accessibility
        );
        assert_eq!(
            GuideStep::for_status(st(false, true)),
            GuideStep::Accessibility
        );
        assert_eq!(
            GuideStep::for_status(st(true, false)),
            GuideStep::ScreenRecording
        );
        assert_eq!(GuideStep::for_status(st(true, true)), GuideStep::Done);
    }

    #[test]
    fn requests_are_sequential_accessibility_first() {
        // Nothing granted: only Accessibility, never both at once.
        assert_eq!(
            next_request(st(false, false), false, false),
            Some(MissingPermission::Accessibility)
        );
        // Accessibility asked but still off: wait, don't ask Screen Recording.
        assert_eq!(next_request(st(false, false), true, false), None);
        // Accessibility on: now Screen Recording.
        assert_eq!(
            next_request(st(true, false), true, false),
            Some(MissingPermission::ScreenRecording)
        );
        // Each is asked once.
        assert_eq!(next_request(st(true, false), true, true), None);
        assert_eq!(next_request(st(true, true), false, false), None);
    }

    #[test]
    fn machine_requests_in_order_and_advances_by_itself() {
        let mut m = GuideMachine::new(st(false, false), &[]);
        assert_eq!(m.step(), GuideStep::Accessibility);

        // First tick: ask for Accessibility only.
        assert_eq!(
            m.on_status(st(false, false)),
            vec![GuideEffect::Request(MissingPermission::Accessibility)]
        );
        // Still off: nothing to do.
        assert!(m.on_status(st(false, false)).is_empty());

        // Switch turned on: move to step 2, ask for Screen Recording, open
        // its pane.
        assert_eq!(
            m.on_status(st(true, false)),
            vec![
                GuideEffect::Show(GuideStep::ScreenRecording),
                GuideEffect::Request(MissingPermission::ScreenRecording),
                GuideEffect::OpenSettings(MissingPermission::ScreenRecording),
            ]
        );
        assert!(m.on_status(st(true, false)).is_empty());

        // Second switch on: done, nothing more to open or ask.
        assert_eq!(
            m.on_status(st(true, true)),
            vec![GuideEffect::Show(GuideStep::Done)]
        );
        assert_eq!(m.step(), GuideStep::Done);
    }

    #[test]
    fn machine_skips_a_request_made_before_it_opened() {
        let mut m = GuideMachine::new(st(false, false), &[MissingPermission::Accessibility]);
        assert!(m.on_status(st(false, false)).is_empty());
    }

    #[test]
    fn machine_starting_on_step_two_asks_screen_recording_once() {
        let mut m = GuideMachine::new(st(true, false), &[]);
        assert_eq!(m.step(), GuideStep::ScreenRecording);
        assert_eq!(
            m.on_status(st(true, false)),
            vec![GuideEffect::Request(MissingPermission::ScreenRecording)]
        );
        assert!(m.on_status(st(true, false)).is_empty());
    }

    #[test]
    fn machine_goes_back_if_accessibility_is_turned_off() {
        let mut m = GuideMachine::new(st(true, false), &[MissingPermission::ScreenRecording]);
        assert_eq!(
            m.on_status(st(false, false)),
            vec![
                GuideEffect::Show(GuideStep::Accessibility),
                GuideEffect::Request(MissingPermission::Accessibility),
                GuideEffect::OpenSettings(MissingPermission::Accessibility),
            ]
        );
    }

    #[test]
    fn copy_is_plain_words() {
        let steps = [
            GuideStep::Accessibility,
            GuideStep::ScreenRecording,
            GuideStep::Done,
        ];
        let mut all: Vec<&str> = vec![
            WINDOW_TITLE,
            DONE_TITLE,
            OPEN_SETTINGS_BUTTON,
            NOT_NOW_BUTTON,
            DRAG_HINT,
            TILE_NAME,
            WAITING_LINE,
            TURNED_ON_LINE,
        ];
        for step in steps {
            let c = step.copy();
            all.extend([c.counter, c.title, c.why, c.how, c.note]);
        }
        for text in all {
            for banned in [
                "TCC",
                "AX ",
                "AX)",
                "accessibility tree",
                "daemon",
                "CuaDriver",
                "cua-driver",
            ] {
                assert!(!text.contains(banned), "{text:?} contains {banned:?}");
            }
        }
        assert_eq!(GuideStep::Accessibility.copy().counter, "Step 1 of 2");
        assert_eq!(
            GuideStep::Accessibility.copy().title,
            "Let Cua Driver use your mouse and keyboard"
        );
        assert_eq!(GuideStep::ScreenRecording.copy().counter, "Step 2 of 2");
        assert_eq!(
            GuideStep::ScreenRecording.copy().title,
            "Let Cua Driver see your screen"
        );
        assert_eq!(
            GuideStep::Done.copy().title,
            "All set. Cua Driver can now use this Mac."
        );
        assert!(GuideStep::ScreenRecording
            .copy()
            .note
            .contains("Quit & Reopen"));
    }

    #[test]
    fn every_name_in_the_copy_follows_the_host_app() {
        let mut all = vec![WINDOW_TITLE, DONE_TITLE, DRAG_HINT, TILE_NAME];
        for step in [
            GuideStep::Accessibility,
            GuideStep::ScreenRecording,
            GuideStep::Done,
        ] {
            let c = step.copy();
            all.extend([c.title, c.why, c.how, c.note]);
        }
        for text in all {
            let named = for_app(text, "Host App");
            assert!(!named.contains(APP_NAME), "{named:?}");
            assert_eq!(for_app(text, APP_NAME), text);
        }
        assert_eq!(
            for_app(GuideStep::Accessibility.copy().how, "Host App"),
            "In System Settings, turn on the switch next to Host App."
        );
        assert_eq!(for_app(TILE_NAME, "Host App"), "Host App");
    }

    #[test]
    fn marker_path_is_per_app_bundle() {
        let home = Path::new("/Users/u");
        assert_eq!(
            marker_path_for(
                Path::new("/Applications/CuaDriver.app/Contents/MacOS/cua-driver"),
                home
            ),
            Some(PathBuf::from(
                "/Users/u/Library/Caches/CuaDriver/permission-guide.json"
            ))
        );
        assert_eq!(
            marker_path_for(
                Path::new("/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local"),
                home
            ),
            Some(PathBuf::from(
                "/Users/u/Library/Caches/CuaDriverLocal/permission-guide.json"
            ))
        );
        assert_eq!(
            marker_path_for(Path::new("/usr/local/bin/cua-driver"), home),
            None
        );
    }

    #[test]
    fn resume_marker_expires() {
        let now = SystemTime::now();
        let marker = ResumeMarker::new(vec!["serve".into()], now);
        assert!(marker.is_fresh(now));
        assert!(marker.is_fresh(now + Duration::from_secs(60)));
        assert!(!marker.is_fresh(now + RESUME_WINDOW + Duration::from_secs(1)));
        // A marker from the future (clock change) is not trusted.
        assert!(!marker.is_fresh(now - Duration::from_secs(60)));
    }

    #[test]
    fn relaunch_replays_serve_args_without_approval_bypass() {
        let args: Vec<String> = [
            "serve",
            "--socket",
            "/tmp/s.sock",
            "--dangerously-bypass-approvals",
            "--grant",
            "existing-profile",
            "--grant=other",
            "--approve-capability-manifest",
            "--no-automation-prompts",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            replayable_serve_args(&args),
            vec![
                "serve",
                "--socket",
                "/tmp/s.sock",
                "--no-automation-prompts"
            ]
        );
        // Anything but `serve` relaunches as plain `serve`.
        assert_eq!(
            replayable_serve_args(&["mcp".to_owned()]),
            vec!["serve".to_owned()]
        );
        assert_eq!(replayable_serve_args(&[]), vec!["serve".to_owned()]);
    }
}
