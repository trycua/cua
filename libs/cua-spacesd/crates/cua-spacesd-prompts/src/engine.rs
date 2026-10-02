// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Scan, answer, and the background watcher.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::classify::{classify, Kind, Plan, Snapshot};
use crate::guard::{self, Guard, GuardReport};

/// Processes whose windows are security dialogs.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const DIALOG_PROCESSES: &[&str] = &["SecurityAgent"];

/// How long after pressing to wait before checking the dialog went away.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SETTLE: Duration = Duration::from_millis(1200);

/// What to do about a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Answered and the dialog went away.
    Answered,
    /// Dry run: this is what would be pressed.
    WouldAnswer,
    /// Left alone (kind not enabled, never answered, or nothing to press).
    Skipped,
    /// Answered but the dialog is still there (wrong password).
    Rejected,
    /// The accessibility calls failed.
    Failed,
}

/// One dialog, as reported to the caller.
#[derive(Debug, Clone, Serialize)]
pub struct DialogReport {
    /// Owning process.
    pub pid: i32,
    /// Classification.
    pub kind: Kind,
    /// The dialog's text.
    pub text: Vec<String>,
    /// Its buttons.
    pub buttons: Vec<String>,
    /// What was done.
    pub action: Action,
    /// The button pressed (or that would be).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressed: Option<String>,
    /// Why, in a sentence.
    pub detail: String,
}

/// The result of one [`scan`] or [`unblock`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    /// Dialogs seen, in the order handled.
    pub dialogs: Vec<DialogReport>,
    /// How many were answered.
    pub answered: usize,
    /// Dialogs still on screen that need a person (or a different call).
    pub remaining: usize,
    /// The guard that applied (`unblock` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<GuardReport>,
    /// Set when nothing could be done at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What [`unblock`] may do.
#[derive(Debug, Clone)]
pub struct Options {
    /// Enabled classes: `keychain` (default) and/or `authorization`.
    pub classes: Vec<String>,
    /// Report what would be pressed without touching anything.
    pub dry_run: bool,
    /// Poll this long for a dialog to appear when none is up yet.
    pub wait: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            classes: vec!["keychain".into()],
            dry_run: false,
            wait: Duration::ZERO,
        }
    }
}

impl Options {
    fn enabled(&self, kind: Kind) -> bool {
        kind.class()
            .is_some_and(|c| self.classes.iter().any(|x| x == c))
    }
}

/// A dialog found on screen, with what is needed to act on it.
struct Found {
    pid: i32,
    snapshot: Snapshot,
    plan: Plan,
    #[cfg(target_os = "macos")]
    window: crate::ax::Window,
}

impl Found {
    fn signature(&self) -> String {
        format!("{}|{}", self.pid, self.snapshot.texts.join("|"))
    }

    fn report(
        &self,
        action: Action,
        pressed: Option<String>,
        detail: impl Into<String>,
    ) -> DialogReport {
        DialogReport {
            pid: self.pid,
            kind: self.plan.kind,
            text: self.snapshot.texts.clone(),
            buttons: self.snapshot.buttons.clone(),
            action,
            pressed,
            detail: detail.into(),
        }
    }
}

#[cfg(target_os = "macos")]
fn find_dialogs() -> Result<Vec<Found>, String> {
    if !crate::ax::trusted() {
        return Err(
            "this process has no Accessibility grant; run it from the cua-spacesd app bundle \
             (it holds the grant in the Space images)"
                .into(),
        );
    }
    let mut out = Vec::new();
    for name in DIALOG_PROCESSES {
        for pid in crate::ax::pids_named(name) {
            let mut windows = crate::ax::windows(pid);
            // SecurityAgent leaves the windows of finished requests in its
            // tree; the live one is `AXMain`. With none marked, trust the
            // first (newest) rather than miss a dialog.
            if windows.iter().any(|w| w.main) {
                windows.retain(|w| w.main);
            } else {
                windows.truncate(1);
            }
            for window in windows {
                // An empty window is SecurityAgent idling, not a dialog.
                if window.snapshot.texts.is_empty() && window.snapshot.buttons.is_empty() {
                    continue;
                }
                let snapshot = window.snapshot.clone();
                let plan = classify(&snapshot);
                out.push(Found {
                    pid,
                    snapshot,
                    plan,
                    window,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(not(target_os = "macos"))]
fn find_dialogs() -> Result<Vec<Found>, String> {
    Err("security prompts exist on macOS only".into())
}

/// Lists the dialogs on screen and what [`unblock`] would do about each.
/// Read-only; allowed anywhere.
pub fn scan(options: &Options) -> Report {
    let mut report = Report::default();
    match find_dialogs() {
        Err(e) => report.error = Some(e),
        Ok(found) => {
            for f in &found {
                let (action, pressed, detail) = decide(f, options);
                report.remaining += 1;
                report.dialogs.push(f.report(action, pressed, detail));
            }
        }
    }
    report
}

fn decide(f: &Found, options: &Options) -> (Action, Option<String>, String) {
    match (&f.plan.press, options.enabled(f.plan.kind)) {
        (Some(button), true) => (
            Action::WouldAnswer,
            Some(button.clone()),
            format!("{} dialog: would press {button:?}", f.plan.kind.as_str()),
        ),
        (Some(_), false) => (
            Action::Skipped,
            None,
            format!(
                "{} dialogs are not enabled for this call (classes: {})",
                f.plan.kind.as_str(),
                options.classes.join(", ")
            ),
        ),
        (None, _) => (
            Action::Skipped,
            None,
            match f.plan.kind {
                Kind::KeychainBroken => "the login keychain is broken; every button here resets or replaces it, so it is never pressed. Repair the keychain instead (security create-keychain / the image's sanitize step)".into(),
                Kind::Authorization => "authorization panel with no recognised approve button".into(),
                _ => "not a dialog this tool knows how to answer".into(),
            },
        ),
    }
}

/// Answers every enabled dialog on screen. Polls up to `options.wait` for one
/// to appear, answers it, and keeps going while chained prompts follow
/// (Chrome asks once per Safe Storage item).
pub fn unblock(options: &Options) -> Report {
    let guard = match guard::guard() {
        Ok(g) => g,
        Err(e) => {
            return Report {
                error: Some(e),
                ..Report::default()
            }
        }
    };
    let mut report = Report {
        guard: Some(guard.report()),
        ..Report::default()
    };
    let deadline = Instant::now() + options.wait;
    let mut rounds = 0;
    loop {
        let found = match find_dialogs() {
            Ok(f) => f,
            Err(e) => {
                report.error = Some(e);
                return report;
            }
        };
        let actionable: Vec<&Found> = found
            .iter()
            .filter(|f| options.enabled(f.plan.kind))
            .collect();
        if actionable.is_empty() {
            if found.is_empty() && Instant::now() < deadline && rounds == 0 {
                std::thread::sleep(Duration::from_millis(400));
                continue;
            }
            // Report what is left that we will not touch.
            for f in &found {
                let (a, p, d) = decide(f, options);
                report.dialogs.push(f.report(a, p, d));
            }
            report.remaining = found.len();
            return report;
        }
        let mut progressed = false;
        for f in actionable {
            let r = answer(f, &guard, options);
            if r.action == Action::Answered {
                report.answered += 1;
                progressed = true;
            }
            report.dialogs.push(r);
        }
        rounds += 1;
        if !progressed || options.dry_run || rounds >= 6 {
            let left = find_dialogs().map(|f| f.len()).unwrap_or(0);
            report.remaining = left;
            return report;
        }
    }
}

#[cfg(target_os = "macos")]
fn answer(f: &Found, guard: &Guard, options: &Options) -> DialogReport {
    let Some(button_label) = f.plan.press.clone() else {
        let (a, p, d) = decide(f, options);
        return f.report(a, p, d);
    };
    if options.dry_run {
        let (a, p, d) = decide(f, options);
        return f.report(a, p, d);
    }
    let fail = |detail: String| f.report(Action::Failed, None, detail);

    if f.plan.fill_password {
        let Some(password) = guard.password.as_ref() else {
            return f.report(
                Action::Skipped,
                None,
                "no guest password is provisioned for this account (set CUA_SPACESD_PROMPT_PASSWORD)",
            );
        };
        let Some(field) = f.window.fields.iter().find(|x| x.secure) else {
            return fail("the dialog has no password field to fill".into());
        };
        if let Err(code) = field.element.set_value(password.expose()) {
            return fail(format!(
                "setting the password field failed (AX status {code})"
            ));
        }
    }
    if f.plan.fill_user {
        if let Some(field) = f.window.fields.iter().find(|x| !x.secure && x.empty) {
            let _ = field.element.set_value(&guard.user);
        }
    }
    let Some(button) = f
        .window
        .buttons
        .iter()
        .find(|b| b.label.trim().eq_ignore_ascii_case(button_label.trim()))
    else {
        return fail(format!(
            "button {button_label:?} vanished before it could be pressed"
        ));
    };
    if let Err(code) = button.element.press() {
        return fail(format!(
            "pressing {button_label:?} failed (AX status {code})"
        ));
    }
    tracing::info!(
        pid = f.pid,
        kind = f.plan.kind.as_str(),
        button = %button_label,
        text = %f.snapshot.texts.join(" / "),
        "answered a security prompt"
    );
    std::thread::sleep(SETTLE);
    // Same dialog still up: the password was wrong (or the app re-asked).
    let sig = f.signature();
    let still = find_dialogs()
        .map(|all| all.iter().any(|g| g.signature() == sig))
        .unwrap_or(false);
    if still {
        f.report(
            Action::Rejected,
            Some(button_label),
            "pressed, but the same dialog is still showing: the provisioned password was refused",
        )
    } else {
        f.report(Action::Answered, Some(button_label), "answered")
    }
}

#[cfg(not(target_os = "macos"))]
fn answer(f: &Found, _guard: &Guard, options: &Options) -> DialogReport {
    let (a, p, d) = decide(f, options);
    f.report(a, p, d)
}

/// Background watcher settings.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Time between scans.
    pub interval: Duration,
    /// A dialog must have been up this long before it is answered, so a
    /// person typing their own answer is never raced.
    pub debounce: Duration,
    /// Enabled classes.
    pub classes: Vec<String>,
    /// Attempts per dialog before it is left for a person.
    pub max_attempts: u32,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            debounce: Duration::from_secs(3),
            classes: vec!["keychain".into()],
            max_attempts: 2,
        }
    }
}

/// Watches until `stop` is set, answering enabled dialogs that have been up
/// for `debounce`. Blocking: run it on its own thread. Returns immediately,
/// with the reason logged, when the host guard refuses.
pub fn run_watcher(config: WatcherConfig, stop: &AtomicBool) {
    let guard = match guard::guard() {
        Ok(g) => g,
        Err(reason) => {
            tracing::info!("security prompt watcher off: {reason}");
            return;
        }
    };
    if guard.password.is_none() {
        tracing::info!("security prompt watcher off: no guest password provisioned");
        return;
    }
    tracing::info!(
        classes = %config.classes.join(","),
        password_source = guard.password_source,
        "security prompt watcher on"
    );
    let options = Options {
        classes: config.classes.clone(),
        dry_run: false,
        wait: Duration::ZERO,
    };
    struct Seen {
        first: Instant,
        attempts: u32,
    }
    let mut seen: HashMap<String, Seen> = HashMap::new();
    let mut warned_untrusted = false;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(config.interval);
        let found = match find_dialogs() {
            Ok(f) => f,
            Err(e) => {
                if !warned_untrusted {
                    tracing::warn!("security prompt watcher cannot scan: {e}");
                    warned_untrusted = true;
                }
                continue;
            }
        };
        let live: Vec<String> = found.iter().map(Found::signature).collect();
        seen.retain(|sig, _| live.contains(sig));
        for f in found {
            if !options.enabled(f.plan.kind) || f.plan.press.is_none() {
                continue;
            }
            let entry = seen.entry(f.signature()).or_insert(Seen {
                first: Instant::now(),
                attempts: 0,
            });
            if entry.first.elapsed() < config.debounce || entry.attempts >= config.max_attempts {
                continue;
            }
            entry.attempts += 1;
            let r = answer(&f, &guard, &options);
            match r.action {
                Action::Answered => {}
                _ => tracing::warn!(
                    kind = r.kind.as_str(),
                    action = ?r.action,
                    detail = %r.detail,
                    "security prompt not answered"
                ),
            }
        }
    }
}
