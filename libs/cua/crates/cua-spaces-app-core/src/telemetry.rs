// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which anonymous usage events a UI transition means, for both shells.
//!
//! The apps drive the core's reducers; these functions look at one step of
//! a reducer (the state before and the action) and answer with
//! [`TelemetrySignal`]s: fixed words, flags and durations, never names,
//! paths, URLs, emails or anything typed. Both shells send the signals the
//! same way ([`record`], feature `telemetry`: the Tauri app on its own
//! client, the SwiftUI app through the FFI on the SDK's), so the same flow
//! yields the same events in either app; the `telemetry-funnel` parity flow
//! checks that.
//!
//! What each signal becomes is `cua-telemetry`'s schema (the docs page
//! "Telemetry and privacy" lists it); a word outside its vocabulary is
//! dropped or coarsened there.

use serde::{Deserialize, Serialize};

use crate::drive_settings::{StorageAction, StorageInput, StorageRequest, StorageState};
use crate::model::{SpaceKind, SpaceOs};
use crate::onboarding::{
    OnboardingAction, OnboardingMode, OnboardingState, OnboardingStep, StorageChoice,
};
use crate::share::{ShareInput, ShareRequest, ShareSheetAction, ShareSheetState};
use crate::spaces::creating::{CreateAction, CreatesState, PendingCreate};

/// One usage event a shell should record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum TelemetrySignal {
    /// A Spaces app feature was used (`cua_spaces_feature_used`).
    Feature {
        /// A fixed feature name (`space_create_local`, `teleport_drop`, ...).
        feature: String,
    },
    /// An install or activation funnel step (`cua_onboarding_step`;
    /// `first_*` steps count once per install).
    Step {
        /// A fixed step name (`app_launched`, `signed_in`, ...).
        step: String,
        /// It worked.
        ok: bool,
    },
    /// A first-run page was shown or left (`cua_onboarding_page`).
    OnboardingPage {
        /// `welcome`, `signin`, `agents`, `presentation`, `volume`,
        /// `this_machine` or `done`.
        page: String,
        /// `shown`, `completed`, `skipped` or `back`.
        action: String,
        /// The page's answer as a fixed word, or `none`.
        choice: String,
    },
    /// The New Space panel (`cua_space_wizard`).
    SpaceWizard {
        /// `opened`, `cancelled` or `submitted`.
        action: String,
    },
    /// A Space create reached ready or failed (`cua_space_create`).
    SpaceCreate {
        /// `local`, `cloud`, `direct` or `relay`.
        location: String,
        /// `linux`, `windows` or `macos`.
        guest_os: String,
        /// `container`, `vm` or `unknown`.
        kind: String,
        /// `ok` or `error`.
        outcome: String,
        /// The phase it failed in; `none` when it succeeded.
        failed_phase: String,
        /// It failed because a phase stopped moving.
        stalled: bool,
        /// Milliseconds from Create to ready, the failure, or the cancel
        /// finishing (bucketed before it is sent).
        elapsed_ms: u64,
        /// GPU acceleration was turned on.
        #[serde(default)]
        gpu: bool,
    },
    /// A Space create started (`cua_space_create_started`).
    SpaceCreateStarted {
        /// `local`, `cloud`, `direct` or `relay`.
        location: String,
        /// `linux`, `windows` or `macos`.
        guest_os: String,
        /// `container`, `vm` or `unknown`.
        kind: String,
        /// GPU acceleration was turned on.
        gpu: bool,
    },
    /// A Cua Volume setting was saved (`cua_volume_setup`).
    VolumeSetup {
        /// `onboarding` or `settings`.
        surface: String,
        /// `this_mac`, `s3` or `later`.
        storage: String,
        /// Added to Finder (mounted on Linux).
        add_to_finder: bool,
        /// `fskit`, `nfs`, `fuse`, `none` or `unknown`.
        mount_method: String,
        /// `ok` or `error`.
        outcome: String,
    },
    /// A Space was shared, unshared or its role changed (`cua_share`).
    Share {
        /// `share`, `unshare` or `change_role`.
        action: String,
        /// `viewer`, `editor` or `none`.
        role: String,
        /// `ok` or `error`.
        outcome: String,
    },
    /// An updater step (`cua_app_update`).
    AppUpdate {
        /// `checked`, `found`, `not_found`, `installed` or `failed`.
        action: String,
        /// `stable` or `beta`.
        channel: String,
        /// `user` or `background`.
        trigger: String,
    },
    /// This device enrolled with the relay, or failed to
    /// (`cua_device_enroll`).
    DeviceEnroll {
        /// `sign_in`, `approval` or `rekey`.
        method: String,
        /// `ok` or `error`.
        outcome: String,
    },
    /// An experiment was turned on or off in Settings, Experiments
    /// (`cua_experiment`).
    Experiment {
        /// `experiment_on` or `experiment_off`.
        action: String,
        /// `cua_volume`, `your_cloud` or `sharing`.
        experiment: String,
    },
    /// Which experiments are on now (no event of its own: the daily
    /// `cua_app_active` carries them as `experiments_on`).
    ExperimentsOn {
        /// Their ids (`cua_volume`, ...); empty: none.
        experiments: Vec<String>,
    },
}

fn feature(f: &str) -> TelemetrySignal {
    TelemetrySignal::Feature { feature: f.into() }
}

fn step(s: &str, ok: bool) -> TelemetrySignal {
    TelemetrySignal::Step { step: s.into(), ok }
}

fn page(p: &str, action: &str, choice: &str) -> TelemetrySignal {
    TelemetrySignal::OnboardingPage {
        page: p.into(),
        action: action.into(),
        choice: choice.into(),
    }
}

fn outcome(ok: bool) -> String {
    if ok { "ok" } else { "error" }.into()
}

// ---- The app ---------------------------------------------------------------

/// The app started (`app_launched`, every launch).
pub fn launched() -> Vec<TelemetrySignal> {
    vec![step("app_launched", true)]
}

/// A Spaces app feature (a fixed name; see the docs). Shells use this for
/// the features no reducer sees (open a viewer, drop a file, ...).
pub fn feature_used(name: &str) -> Vec<TelemetrySignal> {
    vec![feature(name)]
}

// ---- First run ---------------------------------------------------------------

/// A page's telemetry name.
pub fn page_name(s: OnboardingStep) -> &'static str {
    match s {
        OnboardingStep::Welcome => "welcome",
        OnboardingStep::Signin => "signin",
        OnboardingStep::Agents => "agents",
        OnboardingStep::Presentation => "presentation",
        OnboardingStep::Drive => "volume",
        OnboardingStep::Mode => "this_machine",
        OnboardingStep::Done => "done",
    }
}

fn storage_word(c: StorageChoice) -> &'static str {
    match c {
        StorageChoice::Local => "this_mac",
        StorageChoice::S3 => "s3",
        StorageChoice::Later => "later",
    }
}

fn mount_method(m: Option<&crate::drive_settings::DriveMountInput>) -> String {
    m.map(|m| m.method.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// "Start using Cua Spaces" on Done (nothing while usage data is off).
pub fn onboarding_finished(s: &OnboardingState) -> Vec<TelemetrySignal> {
    if !s.shares_usage() {
        return vec![];
    }
    vec![
        page("done", "completed", "none"),
        step("onboarding_completed", true),
    ]
}

/// How `s` leaves its page going forward: `(action, choice)`.
fn leave(s: &OnboardingState, a: &OnboardingAction) -> (&'static str, &'static str) {
    match s.step {
        OnboardingStep::Welcome | OnboardingStep::Done => ("completed", "none"),
        OnboardingStep::Signin => {
            if s.identity.as_deref().is_some_and(|i| !i.is_empty()) {
                ("completed", "signed_in")
            } else {
                ("skipped", "not_signed_in")
            }
        }
        OnboardingStep::Agents => match a {
            OnboardingAction::AgentsDone { configured } if !configured.is_empty() => {
                ("completed", "agents_set_up")
            }
            _ => ("skipped", "no_agents"),
        },
        OnboardingStep::Presentation => (
            "completed",
            if s.menu_bar {
                "menu_bar_only"
            } else {
                "notch_and_menu_bar"
            },
        ),
        OnboardingStep::Drive => {
            if s.storage_choice == StorageChoice::Later && !s.drive_mount {
                ("skipped", "later")
            } else {
                ("completed", storage_word(s.storage_choice))
            }
        }
        OnboardingStep::Mode => match a {
            OnboardingAction::ModeChosen {
                mode: OnboardingMode::Host,
            } => ("completed", "host"),
            _ => ("completed", "access_others"),
        },
    }
}

/// The events one first-run step means: the page left (completed, skipped
/// or back, with its answer), the page shown, the Cua Volume choice when
/// the Volume page is left, and `signed_in`.
///
/// Nothing is derived on Welcome (its notice and usage-data switch come
/// first): leaving it starts the run (`onboarding_shown`, Welcome shown and
/// completed). Nothing at all while the switch is off.
pub fn onboarding(before: &OnboardingState, action: &OnboardingAction) -> Vec<TelemetrySignal> {
    let after = crate::onboarding::reduce(before, action);
    if !after.shares_usage() {
        return vec![];
    }
    if before.step == OnboardingStep::Welcome && after.step == OnboardingStep::Welcome {
        return vec![];
    }
    let mut out = vec![];
    if before.step == OnboardingStep::Welcome {
        out.push(step("onboarding_shown", true));
        out.push(page("welcome", "shown", "none"));
        // Signed in before the run started (an account the app already had).
        if after.identity.as_deref().is_some_and(|i| !i.is_empty()) {
            out.push(step("signed_in", true));
        }
    }
    if let OnboardingAction::SignedIn { identity } = action
        && !identity.is_empty()
        && before.identity.as_deref() != Some(identity.as_str())
    {
        out.push(step("signed_in", true));
    }
    if after.step == before.step {
        return out;
    }
    match action {
        OnboardingAction::Back => out.push(page(page_name(before.step), "back", "none")),
        // The machine cannot mount, or the Cua Volume experiment was turned
        // off: the Volume page goes by itself.
        OnboardingAction::DriveChecked { .. } | OnboardingAction::ExperimentsLoaded { .. } => {
            out.push(page(page_name(before.step), "skipped", "none"))
        }
        _ => {
            let (act, choice) = leave(&after_answers(before, &after), action);
            out.push(page(page_name(before.step), act, choice));
            if before.step == OnboardingStep::Drive {
                let usable = after.drive_status.as_ref().is_some_and(|m| m.supported());
                out.push(TelemetrySignal::VolumeSetup {
                    surface: "onboarding".into(),
                    storage: storage_word(after.storage_choice).into(),
                    add_to_finder: usable && after.drive_mount,
                    mount_method: mount_method(after.drive_status.as_ref()),
                    outcome: "ok".into(),
                });
            }
        }
    }
    out.push(page(page_name(after.step), "shown", "none"));
    out
}

/// The answers a page is left with: the state after the step, on the page
/// being left (so the choice made by the leaving action counts).
fn after_answers(before: &OnboardingState, after: &OnboardingState) -> OnboardingState {
    OnboardingState {
        step: before.step,
        ..after.clone()
    }
}

// ---- Space creates -------------------------------------------------------------

fn os_word(os: SpaceOs) -> &'static str {
    match os {
        SpaceOs::Macos => "macos",
        SpaceOs::Windows => "windows",
        SpaceOs::Linux => "linux",
    }
}

fn kind_word(k: Option<SpaceKind>) -> &'static str {
    match k {
        Some(SpaceKind::Container) => "container",
        Some(SpaceKind::Vm) => "vm",
        None => "unknown",
    }
}

fn create_event(p: &PendingCreate, outcome: &str, stalled: bool, now_ms: i64) -> TelemetrySignal {
    TelemetrySignal::SpaceCreate {
        location: p.provider.as_str().into(),
        guest_os: os_word(p.os).into(),
        kind: kind_word(p.kind).into(),
        outcome: outcome.into(),
        failed_phase: if outcome == "ok" {
            "none".into()
        } else {
            p.phase.clone()
        },
        stalled: outcome == "error" && stalled,
        elapsed_ms: (now_ms - p.started_at).max(0) as u64,
        gpu: p.gpu,
    }
}

fn stalled(p: &PendingCreate) -> bool {
    p.error
        .as_deref()
        .is_some_and(|e| e.ends_with(crate::spaces::creating::STALL_HINT))
}

/// The events one step of the Spaces being created means, at `now_ms`:
/// a create started (`first_space_created`), reached ready
/// (`cua_space_create` ok, `first_space_ready`) or failed (`error`, with
/// the phase). A create that stalls is counted when it fails for good or
/// is dismissed while stalled, not while it may still recover.
///
/// Cancel (pressed while it is being created) counts once, as `cancelled`,
/// when the cancel finishes: by `cancel-done`, or by the create ending
/// while it was being cancelled. A cancel that fails counts nothing.
pub fn creates(before: &CreatesState, action: &CreateAction, now_ms: i64) -> Vec<TelemetrySignal> {
    let find = |id: &str| before.pending.iter().find(|p| p.id == id);
    match action {
        CreateAction::Start { id, provider, .. } if find(id).is_none() => {
            let after = crate::spaces::creating::reduce(before, action);
            let started = after.pending.iter().find(|p| &p.id == id).map(|p| {
                TelemetrySignal::SpaceCreateStarted {
                    location: p.provider.as_str().into(),
                    guest_os: os_word(p.os).into(),
                    kind: kind_word(p.kind).into(),
                    gpu: p.gpu,
                }
            });
            [feature(if provider.as_str() == "cloud" {
                "space_create_cloud"
            } else {
                "space_create_local"
            })]
            .into_iter()
            .chain(started)
            .chain([step("first_space_created", true)])
            .collect()
        }
        CreateAction::Finish { id, .. } => match find(id) {
            Some(p) if p.space_id.is_none() => vec![
                create_event(p, "ok", false, now_ms),
                step("first_space_ready", true),
            ],
            _ => vec![],
        },
        CreateAction::Fail { id, .. } => match find(id) {
            Some(p) if p.space_id.is_none() && p.cancelling => {
                vec![create_event(p, "cancelled", false, now_ms)]
            }
            Some(p) if p.space_id.is_none() && (p.error.is_none() || stalled(p)) => {
                vec![create_event(p, "error", stalled(p), now_ms)]
            }
            _ => vec![],
        },
        CreateAction::CancelDone { id } => match find(id) {
            Some(p) if p.space_id.is_none() && p.cancelling => {
                vec![create_event(p, "cancelled", false, now_ms)]
            }
            _ => vec![],
        },
        CreateAction::Dismiss { id } => match find(id) {
            Some(p) if p.space_id.is_none() && stalled(p) => {
                vec![create_event(p, "error", true, now_ms)]
            }
            _ => vec![],
        },
        _ => vec![],
    }
}

// ---- Settings, Storage ---------------------------------------------------------

fn backend_word(backend: &str) -> &'static str {
    match backend {
        "s3" => "s3",
        _ => "this_mac",
    }
}

/// The events one step of Settings, Storage means: a storage choice saved,
/// the Finder volume turned on or off, Open in Finder.
pub fn storage(
    input: &StorageInput,
    before: &StorageState,
    action: &StorageAction,
) -> Vec<TelemetrySignal> {
    let mount = input.mount.as_ref();
    let in_finder = mount.is_some_and(|m| m.enabled);
    let backend = input
        .storage
        .as_ref()
        .map(|s| backend_word(&s.backend))
        .unwrap_or("this_mac");
    let volume = |storage: &str, add_to_finder: bool, ok: bool| TelemetrySignal::VolumeSetup {
        surface: "settings".into(),
        storage: storage.into(),
        add_to_finder,
        mount_method: mount_method(mount),
        outcome: outcome(ok),
    };
    match (&before.request, action) {
        (Some(StorageRequest::Save { update }), StorageAction::Saved { check }) => vec![volume(
            backend_word(&update.backend),
            in_finder,
            check.ok && check.applied,
        )],
        (Some(StorageRequest::Save { update }), StorageAction::Failed { .. }) => {
            vec![volume(backend_word(&update.backend), in_finder, false)]
        }
        (Some(StorageRequest::Mount), StorageAction::Done) => vec![volume(backend, true, true)],
        (Some(StorageRequest::Mount), StorageAction::Failed { .. }) => {
            vec![volume(backend, true, false)]
        }
        (Some(StorageRequest::Unmount), StorageAction::Done) => {
            vec![volume(backend, false, true)]
        }
        (Some(StorageRequest::Reveal { .. }), StorageAction::Done) => {
            vec![feature("volume_open_finder")]
        }
        _ => vec![],
    }
}

// ---- Sharing -----------------------------------------------------------------

/// The events one step of the Share sheet means: a share, unshare or role
/// change finished or failed. Never who.
pub fn share(
    input: &ShareInput,
    before: &ShareSheetState,
    action: &ShareSheetAction,
) -> Vec<TelemetrySignal> {
    let ok = match action {
        ShareSheetAction::Done => true,
        ShareSheetAction::Failed { .. } => false,
        _ => return vec![],
    };
    let (act, role) = match &before.request {
        Some(ShareRequest::Share { who, role, .. }) => (
            if input.shares.iter().any(|s| &s.who == who) {
                "change_role"
            } else {
                "share"
            },
            if role == "editor" { "editor" } else { "viewer" },
        ),
        Some(ShareRequest::Unshare { .. }) => ("unshare", "none"),
        None => return vec![],
    };
    vec![TelemetrySignal::Share {
        action: act.into(),
        role: role.into(),
        outcome: outcome(ok),
    }]
}

// ---- Enrollment ----------------------------------------------------------------

/// The events one step of the enroll sheet means: enrolled by a fresh
/// sign-in or by an approval, or failed.
pub fn enroll(
    before: &crate::devices::EnrollState,
    action: &crate::devices::EnrollAction,
) -> Vec<TelemetrySignal> {
    use crate::devices::{EnrollAction as A, EnrollMethod as M, EnrollPhase as P};
    let method = match before.method {
        Some(M::Approve) => "approval",
        _ => "sign_in",
    };
    let signal = |ok: bool| TelemetrySignal::DeviceEnroll {
        method: method.into(),
        outcome: outcome(ok),
    };
    match action {
        A::Registered { enrolled: true, .. } if before.phase != P::Enrolled => vec![signal(true)],
        A::Approved if before.phase == P::Waiting => vec![signal(true)],
        A::Failed { .. } if before.phase != P::Failed && before.method.is_some() => {
            vec![signal(false)]
        }
        _ => vec![],
    }
}

// ---- Experiments ---------------------------------------------------------------

/// Which experiments are on (at launch, so the day's `cua_app_active`
/// carries them).
pub fn experiments_on(experiments: &crate::experiments::Experiments) -> Vec<TelemetrySignal> {
    vec![TelemetrySignal::ExperimentsOn {
        experiments: experiments
            .on_ids()
            .into_iter()
            .map(str::to_string)
            .collect(),
    }]
}

/// The events of a change in Settings, Experiments: one `experiment_on` or
/// `experiment_off` per switch that changed, then which are on now.
/// Nothing when nothing changed.
pub fn experiments_changed(
    before: &crate::experiments::Experiments,
    after: &crate::experiments::Experiments,
) -> Vec<TelemetrySignal> {
    let mut out: Vec<TelemetrySignal> = crate::experiments::ALL
        .into_iter()
        .filter(|e| before.is_on(*e) != after.is_on(*e))
        .map(|e| TelemetrySignal::Experiment {
            action: if after.is_on(e) {
                "experiment_on"
            } else {
                "experiment_off"
            }
            .into(),
            experiment: e.id().into(),
        })
        .collect();
    if !out.is_empty() {
        out.extend(experiments_on(after));
    }
    out
}

// ---- Recording ---------------------------------------------------------------

/// Records `signals` on `t` and marks the day active. Returns how many
/// events were queued (none while telemetry is off, before the first-run
/// notice, or for a word outside the schema).
#[cfg(feature = "telemetry")]
pub fn record(t: &cua_telemetry::Telemetry, signals: &[TelemetrySignal]) -> usize {
    use cua_telemetry::Captured;
    use cua_telemetry::events::{self, Outcome};
    use std::time::Duration;

    let mut queued = 0;
    for s in signals {
        let r = match s {
            TelemetrySignal::Feature { feature } => {
                events::spaces_feature_used(feature).map(|e| t.capture(e))
            }
            TelemetrySignal::Step { step, ok } => {
                Some(t.capture_step(step, if *ok { Outcome::Ok } else { Outcome::Error }))
            }
            TelemetrySignal::OnboardingPage {
                page,
                action,
                choice,
            } => events::onboarding_page(page, action, choice).map(|e| t.capture(e)),
            TelemetrySignal::SpaceWizard { action } => {
                events::space_wizard(action).map(|e| t.capture(e))
            }
            TelemetrySignal::SpaceCreate {
                location,
                guest_os,
                kind,
                outcome,
                failed_phase,
                stalled,
                elapsed_ms,
                gpu,
            } => Some(t.capture(events::space_create(
                &events::SpaceCreate {
                    on: location,
                    guest_os,
                    kind,
                    last_phase: failed_phase,
                    stalled: *stalled,
                    gpu: *gpu,
                },
                Outcome::from_word(outcome),
                Duration::from_millis(*elapsed_ms),
            ))),
            TelemetrySignal::SpaceCreateStarted {
                location,
                guest_os,
                kind,
                gpu,
            } => Some(t.capture(events::space_create_started(location, guest_os, kind, *gpu))),
            TelemetrySignal::VolumeSetup {
                surface,
                storage,
                add_to_finder,
                mount_method,
                outcome,
            } => Some(t.capture(events::volume_setup(
                &events::VolumeSetup {
                    surface,
                    storage,
                    add_to_finder: *add_to_finder,
                    mount_method,
                },
                Outcome::from_word(outcome),
            ))),
            TelemetrySignal::Share {
                action,
                role,
                outcome,
            } => events::share(action, role, Outcome::from_word(outcome)).map(|e| t.capture(e)),
            TelemetrySignal::AppUpdate {
                action,
                channel,
                trigger,
            } => events::app_update(action, channel, trigger).map(|e| t.capture(e)),
            TelemetrySignal::DeviceEnroll { method, outcome } => {
                events::device_enroll(method, Outcome::from_word(outcome)).map(|e| t.capture(e))
            }
            TelemetrySignal::Experiment { action, experiment } => {
                events::experiment(action, experiment).map(|e| t.capture(e))
            }
            // Not an event: the day's `cua_app_active` carries them.
            TelemetrySignal::ExperimentsOn { experiments } => {
                let ids: Vec<&str> = experiments.iter().map(String::as_str).collect();
                t.set_experiments_on(&ids);
                None
            }
        };
        if r == Some(Captured::Queued) {
            queued += 1;
        }
    }
    if !signals.is_empty() {
        t.capture_active_day();
    }
    queued
}

/// A launch before the first-run notice was ever shown waits for it.
#[cfg(feature = "telemetry")]
static LAUNCH_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app started: records `app_launched` now, or, on a machine that has
/// not shown the first-run notice yet (nothing may be sent before it),
/// when [`acknowledge_notice`] runs. Returns whether it was recorded now.
#[cfg(feature = "telemetry")]
pub fn start(t: &cua_telemetry::Telemetry) -> bool {
    if t.notice_shown() {
        record(t, &launched());
        true
    } else {
        LAUNCH_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
        false
    }
}

/// The first run showed the notice: records that, then the launch that
/// waited for it.
#[cfg(feature = "telemetry")]
pub fn acknowledge_notice(t: &cua_telemetry::Telemetry) {
    t.acknowledge_notice();
    if LAUNCH_PENDING.swap(false, std::sync::atomic::Ordering::SeqCst) {
        record(t, &launched());
    }
}

/// The first run left Welcome with its usage-data switch at `on`: writes
/// the machine's setting when it changed (`[telemetry] enabled` in
/// `$CUA_HOME/config.toml`, the same as Settings and `cua telemetry off`),
/// then records that the notice was shown. Nothing was queued or sent
/// before this; with the switch off nothing is after it either (the held
/// `app_launched` included).
#[cfg(feature = "telemetry")]
pub fn welcome_left(t: &cua_telemetry::Telemetry, on: bool) -> std::io::Result<()> {
    t.refresh();
    if t.is_enabled() != on {
        t.set_enabled(on)?;
    }
    acknowledge_notice(t);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive_settings::DriveMountInput;
    use crate::model::SpaceProvider;
    use crate::onboarding::{initial, reduce};

    fn pages(v: &[TelemetrySignal]) -> Vec<String> {
        v.iter()
            .filter_map(|s| match s {
                TelemetrySignal::OnboardingPage {
                    page,
                    action,
                    choice,
                } => Some(format!("{page} {action} {choice}")),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn signing_in_and_skipping_are_told_apart() {
        let s = reduce(&initial(None, None), &OnboardingAction::Start);
        let skip = onboarding(&s, &OnboardingAction::SigninDone);
        assert_eq!(
            pages(&skip),
            ["signin skipped not_signed_in", "agents shown none"]
        );
        let signed = onboarding(
            &s,
            &OnboardingAction::SignedIn {
                identity: "alice@example.com".into(),
            },
        );
        assert_eq!(signed, vec![step("signed_in", true)]);
        let s = reduce(
            &s,
            &OnboardingAction::SignedIn {
                identity: "alice@example.com".into(),
            },
        );
        assert_eq!(
            pages(&onboarding(&s, &OnboardingAction::SigninDone)),
            ["signin completed signed_in", "agents shown none"]
        );
        // Signed in again as the same identity: not a new step.
        assert!(
            onboarding(
                &s,
                &OnboardingAction::SignedIn {
                    identity: "alice@example.com".into()
                }
            )
            .is_empty()
        );
        // Back is its own action, and never carries an answer.
        let agents = reduce(&s, &OnboardingAction::SigninDone);
        assert_eq!(
            pages(&onboarding(&agents, &OnboardingAction::Back)),
            ["agents back none", "signin shown none"]
        );
    }

    #[test]
    fn no_signal_carries_caller_text() {
        let s = reduce(&initial(None, None), &OnboardingAction::Start);
        let s = reduce(
            &s,
            &OnboardingAction::SignedIn {
                identity: "alice@example.com".into(),
            },
        );
        let all = [
            onboarding(&s, &OnboardingAction::SigninDone),
            onboarding(
                &reduce(&s, &OnboardingAction::SigninDone),
                &OnboardingAction::AgentsDone {
                    configured: vec!["/Users/alice/secret-agent".into()],
                },
            ),
        ];
        let text = serde_json::to_string(&all).unwrap();
        assert!(!text.contains("alice"), "{text}");
        assert!(!text.contains("secret"), "{text}");
    }

    fn pending(id: &str, phase: &str, error: Option<&str>) -> PendingCreate {
        let mut st = crate::spaces::creating::reduce(
            &CreatesState::default(),
            &CreateAction::Start {
                id: id.into(),
                name: "my-private-space".into(),
                os: SpaceOs::Linux,
                provider: SpaceProvider::Local,
                now: 1_000,
                image: Some("linux:24.04".into()),
                kind: Some(SpaceKind::Container),
                host_arch: None,
                gpu: false,
            },
        );
        let p = &mut st.pending[0];
        p.phase = phase.into();
        p.error = error.map(str::to_string);
        p.clone()
    }

    #[test]
    fn a_create_counts_once_when_ready_or_failed_for_good() {
        let st = CreatesState {
            pending: vec![pending("pending:a", "booting", None)],
            deleting: vec![],
            powering: vec![],
        };
        let ready = creates(
            &st,
            &CreateAction::Finish {
                id: "pending:a".into(),
                space_id: "local:x".into(),
            },
            48_000,
        );
        assert_eq!(
            ready[0],
            TelemetrySignal::SpaceCreate {
                location: "local".into(),
                guest_os: "linux".into(),
                kind: "container".into(),
                outcome: "ok".into(),
                failed_phase: "none".into(),
                stalled: false,
                elapsed_ms: 47_000,
                gpu: false,
            }
        );
        assert_eq!(ready[1], step("first_space_ready", true));
        let failed = creates(
            &st,
            &CreateAction::Fail {
                id: "pending:a".into(),
                error: "boom at /Users/alice".into(),
            },
            5_000,
        );
        assert!(matches!(
            &failed[0],
            TelemetrySignal::SpaceCreate { failed_phase, stalled: false, outcome, .. }
                if failed_phase == "booting" && outcome == "error"
        ));
        // A stalled row is counted when dismissed, not when it stalls.
        let stall = crate::spaces::creating::stall_error("pulling", 900.0);
        let st = CreatesState {
            pending: vec![pending("pending:b", "pulling", Some(&stall))],
            deleting: vec![],
            powering: vec![],
        };
        assert!(creates(&st, &CreateAction::Tick { now: 9_000 }, 9_000).is_empty());
        let gone = creates(
            &st,
            &CreateAction::Dismiss {
                id: "pending:b".into(),
            },
            9_000,
        );
        assert!(matches!(
            &gone[0],
            TelemetrySignal::SpaceCreate { stalled: true, failed_phase, .. } if failed_phase == "pulling"
        ));
        // A failed (not stalled) row dismissed was already counted.
        let st = CreatesState {
            pending: vec![pending("pending:c", "pulling", Some("no network"))],
            deleting: vec![],
            powering: vec![],
        };
        assert!(
            creates(
                &st,
                &CreateAction::Dismiss {
                    id: "pending:c".into()
                },
                9_000
            )
            .is_empty()
        );
    }

    #[test]
    fn settings_storage_reports_the_choice_and_the_finder_volume() {
        let input = StorageInput {
            mount: Some(DriveMountInput {
                enabled: true,
                state: "mounted".into(),
                method: "fskit".into(),
                path: Some("/Volumes/Cua".into()),
                volume_name: "Cua".into(),
                detail: None,
                settings_url: None,
            }),
            ..Default::default()
        };
        let before = StorageState {
            request: Some(StorageRequest::Mount),
            busy: true,
            ..Default::default()
        };
        assert_eq!(
            storage(&input, &before, &StorageAction::Done),
            vec![TelemetrySignal::VolumeSetup {
                surface: "settings".into(),
                storage: "this_mac".into(),
                add_to_finder: true,
                mount_method: "fskit".into(),
                outcome: "ok".into(),
            }]
        );
        let before = StorageState {
            request: Some(StorageRequest::Reveal {
                path: "/Users/alice/Cua".into(),
            }),
            ..Default::default()
        };
        assert_eq!(
            storage(&input, &before, &StorageAction::Done),
            vec![feature("volume_open_finder")]
        );
    }

    /// A switch in Settings, Experiments: one `experiment_on` or
    /// `experiment_off` with the experiment's id, then the set that is on.
    #[test]
    fn an_experiment_toggle_is_one_event_and_the_set_that_is_on() {
        use crate::experiments::Experiments;
        let off = Experiments::default();
        let on = Experiments {
            sharing: true,
            ..Default::default()
        };
        let signals = experiments_changed(&off, &on);
        assert_eq!(
            signals,
            vec![
                TelemetrySignal::Experiment {
                    action: "experiment_on".into(),
                    experiment: "sharing".into(),
                },
                TelemetrySignal::ExperimentsOn {
                    experiments: vec!["sharing".into()],
                },
            ]
        );
        assert_eq!(
            serde_json::to_value(&signals).unwrap(),
            serde_json::json!([
                {"type": "experiment", "action": "experiment_on", "experiment": "sharing"},
                {"type": "experiments-on", "experiments": ["sharing"]},
            ])
        );
        assert_eq!(
            experiments_changed(&on, &off),
            vec![
                TelemetrySignal::Experiment {
                    action: "experiment_off".into(),
                    experiment: "sharing".into(),
                },
                TelemetrySignal::ExperimentsOn {
                    experiments: vec![]
                },
            ]
        );
        assert!(experiments_changed(&on, &on).is_empty(), "nothing changed");
        assert_eq!(
            experiments_on(&Experiments::all_on()),
            vec![TelemetrySignal::ExperimentsOn {
                experiments: vec!["cua_volume".into(), "your_cloud".into(), "sharing".into()]
            }]
        );
        // Loading the switches on Welcome derives nothing.
        let s = crate::onboarding::initial(None, None);
        assert!(
            onboarding(
                &s,
                &OnboardingAction::ExperimentsLoaded {
                    experiments: Experiments::all_on()
                }
            )
            .is_empty()
        );
    }

    /// The toggle is a valid `cua_experiment`, and the day's
    /// `cua_app_active` says which experiments are on.
    #[cfg(feature = "telemetry")]
    #[test]
    fn a_toggle_records_cua_experiment_and_the_active_day_carries_the_set() {
        use crate::experiments::Experiments;
        use std::sync::Arc;
        let home = tempfile::tempdir().unwrap();
        let sink = Arc::new(cua_telemetry::sink::MemorySink::new());
        let t = cua_telemetry::Telemetry::builder()
            .env(|k| (k == "CUA_TELEMETRY_FORBID_NETWORK").then(|| "1".into()))
            .home(home.path())
            .sink(sink.clone())
            .product("spaces_app", "0.2.0")
            .foreground()
            .build();
        t.acknowledge_notice();
        let on = Experiments {
            cua_volume: true,
            sharing: true,
            ..Default::default()
        };
        let signals = experiments_changed(&Experiments::default(), &on);
        assert_eq!(
            record(&t, &signals),
            2,
            "two toggles; the set is not an event"
        );
        t.flush(std::time::Duration::from_secs(2));
        let events = sink.events();
        let toggles: Vec<(String, String)> = events
            .iter()
            .filter(|e| e["event"] == "cua_experiment")
            .map(|e| {
                (
                    e["properties"]["action"].as_str().unwrap().to_string(),
                    e["properties"]["experiment"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            toggles,
            [
                ("experiment_on".to_string(), "cua_volume".to_string()),
                ("experiment_on".to_string(), "sharing".to_string())
            ]
        );
        let active = events
            .iter()
            .find(|e| e["event"] == "cua_app_active")
            .expect("the day is active");
        assert_eq!(active["properties"]["experiments_on"], "cua_volume+sharing");
    }

    /// Every signal the parity flow derives is a valid event: each one is
    /// queued on a client (none dropped by the schema), and no name, path,
    /// email or bucket from the flow's inputs is in any payload.
    #[cfg(feature = "telemetry")]
    #[test]
    fn every_funnel_signal_records_as_a_valid_event() {
        use std::sync::Arc;
        let golden: serde_json::Value =
            serde_json::from_str(include_str!("../parity/golden/telemetry-funnel.json")).unwrap();
        let signals: Vec<TelemetrySignal> = golden["frames"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|f| f["signals"].as_array().unwrap().clone())
            .map(|s| serde_json::from_value(s).unwrap())
            .collect();
        assert!(signals.len() > 40);
        let home = tempfile::tempdir().unwrap();
        let sink = Arc::new(cua_telemetry::sink::MemorySink::new());
        let t = cua_telemetry::Telemetry::builder()
            .env(|k| match k {
                "CUA_TELEMETRY_FORBID_NETWORK" => Some("1".into()),
                _ => None,
            })
            .home(home.path())
            .sink(sink.clone())
            .product("spaces_app", "0.2.0")
            .foreground()
            .build();
        t.acknowledge_notice();
        // Once-per-install steps repeat in the flow; everything else queues.
        let once = signals
            .iter()
            .filter(
                |s| matches!(s, TelemetrySignal::Step { step, .. } if step.starts_with("first_")),
            )
            .count();
        let queued = record(&t, &signals);
        t.flush(std::time::Duration::from_secs(2));
        let firsts: std::collections::BTreeSet<_> = signals
            .iter()
            .filter_map(|s| match s {
                TelemetrySignal::Step { step, .. } if step.starts_with("first_") => Some(step),
                _ => None,
            })
            .collect();
        assert_eq!(queued, signals.len() - once + firsts.len());
        let text = serde_json::to_string(&sink.events())
            .unwrap()
            .to_lowercase();
        for f in [
            "maya",
            "example",
            "secret",
            "/users",
            "private-bucket",
            "k7q",
            "eve@",
        ] {
            assert!(!text.contains(f), "{f} leaked");
        }
        let names: std::collections::BTreeSet<String> = sink
            .events()
            .iter()
            .map(|e| e["event"].as_str().unwrap().to_string())
            .collect();
        for e in [
            "cua_app_active",
            "cua_onboarding_page",
            "cua_onboarding_step",
            "cua_space_create",
            "cua_space_create_started",
            "cua_volume_setup",
            "cua_share",
            "cua_device_enroll",
            "cua_spaces_feature_used",
        ] {
            assert!(names.contains(e), "no {e}");
        }
    }

    /// Welcome's usage-data switch: off sends nothing across a whole first
    /// run (not the held launch, not a page, not an event recorded
    /// directly), and writes the machine's setting; on sends the funnel,
    /// starting with the launch that waited for the notice.
    #[cfg(feature = "telemetry")]
    #[test]
    fn the_welcome_switch_decides_before_anything_is_sent() {
        use std::sync::Arc;
        let flow: serde_json::Value =
            serde_json::from_str(include_str!("../parity/telemetry-funnel.json")).unwrap();
        let client = |home: &std::path::Path| {
            let sink = Arc::new(cua_telemetry::sink::MemorySink::new());
            let t = cua_telemetry::Telemetry::builder()
                .env(|k| (k == "CUA_TELEMETRY_FORBID_NETWORK").then(|| "1".into()))
                .home(home)
                .sink(sink.clone())
                .product("spaces_app", "0.2.0")
                .notice_mode(cua_telemetry::NoticeMode::External)
                .foreground()
                .build();
            (t, sink)
        };
        let run = |t: &cua_telemetry::Telemetry, name: &str, on: bool| {
            assert!(!start(t), "a first run waits for the notice");
            let mut st = crate::onboarding::initial(None, None);
            for a in flow[name].as_array().unwrap() {
                let a: OnboardingAction = serde_json::from_value(a.clone()).unwrap();
                if st.step == OnboardingStep::Welcome && matches!(a, OnboardingAction::Start) {
                    // Nothing is queued while Welcome shows.
                    assert_eq!(t.queued(), 0);
                    welcome_left(t, on).unwrap();
                }
                record(t, &onboarding(&st, &a));
                st = crate::onboarding::reduce(&st, &a);
            }
            record(t, &onboarding_finished(&st));
            // Something recorded directly (an SDK event, a feature).
            record(t, &feature_used("space_open_viewer"));
            t.flush(std::time::Duration::from_secs(2));
        };

        let off = tempfile::tempdir().unwrap();
        let (t, sink) = client(off.path());
        run(&t, "no-usage-data", false);
        assert!(sink.events().is_empty(), "{:?}", sink.events());
        assert!(t.show_last(100).is_empty());
        let config = std::fs::read_to_string(off.path().join("config.toml")).unwrap();
        assert!(config.contains("enabled = \"off\""), "{config}");

        let on = tempfile::tempdir().unwrap();
        let (t, sink) = client(on.path());
        run(&t, "onboarding", true);
        let steps: Vec<String> = sink
            .events()
            .iter()
            .filter(|e| e["event"] == "cua_onboarding_step")
            .map(|e| e["properties"]["step"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            steps,
            [
                "app_launched",
                "onboarding_shown",
                "signed_in",
                "onboarding_completed"
            ]
        );
        assert!(
            !on.path().join("config.toml").exists(),
            "on is the default: nothing written"
        );
    }
}
