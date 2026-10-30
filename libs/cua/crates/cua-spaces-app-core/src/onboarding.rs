// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! First run: Welcome, Command line (only while the app's bundled `cua`
//! needs installing), Sign in, AI agents, where Cua Spaces shows up, Cua
//! Volume (only with its experiment on, Settings, Experiments; hidden
//! where the drive cannot mount, Windows), This machine, Done. One step per
//! page with page dots; every write asks first (the shells run the writes,
//! this only orders the pages and holds the answers).

use serde::{Deserialize, Serialize};

use crate::drive_settings::{
    DriveCheckInput, DriveMountInput, DriveStorageInput, DriveStorageUpdate, StorageAction,
    StorageInput, StorageRequest, StorageState, form_ready, mount_label, not_available,
    storage_reduce, storage_section,
};
use crate::experiments::Experiments;
use crate::installer::{CliInstallPlan, InstallMethod};
use crate::model::SpaceOs;
use crate::settings::{SettingsOption, SettingsRow, TelemetryInput};

/// A page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnboardingStep {
    /// Welcome.
    Welcome,
    /// Install the bundled `cua`, while its plan needs it.
    Cli,
    /// Sign in to Cua.
    Signin,
    /// Coding agents: cua skills and the cua MCP server.
    Agents,
    /// Where Cua Spaces shows up: the notch and the menu bar, or the menu
    /// bar only (the "Spaces tab in the notch" setting).
    Presentation,
    /// The Cua Volume as a Finder volume (a mount on Linux), off unless
    /// ticked. Skipped where it cannot mount (Windows).
    Drive,
    /// Access other machines, or set this one up for unattended access.
    Mode,
    /// Done.
    Done,
}

/// Page order.
pub const STEPS: [OnboardingStep; 8] = [
    OnboardingStep::Welcome,
    OnboardingStep::Cli,
    OnboardingStep::Signin,
    OnboardingStep::Agents,
    OnboardingStep::Presentation,
    OnboardingStep::Drive,
    OnboardingStep::Mode,
    OnboardingStep::Done,
];

impl OnboardingStep {
    /// Page dot label.
    pub fn label(self) -> &'static str {
        match self {
            OnboardingStep::Welcome => "Welcome",
            OnboardingStep::Cli => "Command line",
            OnboardingStep::Presentation => "Menu bar",
            OnboardingStep::Signin => "Sign in",
            OnboardingStep::Agents => "AI agents",
            OnboardingStep::Drive => "Cua Volume",
            OnboardingStep::Mode => "This machine",
            OnboardingStep::Done => "Done",
        }
    }
}

/// The pages this run shows, in order: [`STEPS`] without Command line
/// unless the bundled `cua` needs installing, and without Cua Volume while
/// its experiment is off, or where it cannot mount (Windows).
pub fn steps(s: &OnboardingState) -> Vec<OnboardingStep> {
    STEPS
        .iter()
        .copied()
        .filter(|st| *st != OnboardingStep::Cli || cli_shown(s))
        .filter(|st| *st != OnboardingStep::Drive || drive_shown(s))
        .collect()
}

fn cli_shown(s: &OnboardingState) -> bool {
    s.cli_plan
        .as_ref()
        .is_some_and(|p| p.source.is_some() && !p.up_to_date)
}

/// The Cua Volume page shows: the Cua Volume experiment is on, everywhere
/// but a Windows machine the daemon cannot mount on.
fn drive_shown(s: &OnboardingState) -> bool {
    s.experiments.cua_volume
        && (s.drive_os != Some(SpaceOs::Windows)
            || s.drive_status.as_ref().is_some_and(|m| m.supported()))
}

/// The page after `step` in this run.
fn next(s: &OnboardingState, step: OnboardingStep) -> OnboardingStep {
    let all = steps(s);
    let i = all.iter().position(|x| *x == step).unwrap_or(0);
    all.get(i + 1).copied().unwrap_or(step)
}

/// The Cua Volume page's command for the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriveStepRequest {
    /// `volume_mount`; answer with `drive-mounted`.
    Mount,
    /// `volume_unmount`; answer with `drive-mounted`.
    Unmount,
}

/// Where the volume's files live, as picked on its page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageChoice {
    /// This machine (the `fs` backend, no setup).
    #[default]
    Local,
    /// The user's own S3-compatible bucket.
    S3,
    /// Decide later, in Settings, Storage.
    Later,
}

/// What this machine is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnboardingMode {
    /// Access other machines (nothing installed).
    Client,
    /// Set this machine up for unattended access.
    Host,
}

/// The flow's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingState {
    /// Page.
    pub step: OnboardingStep,
    /// Where `cua` was installed, when it was.
    pub cli_target: Option<String>,
    /// The bundled `cua`'s install plan, as the shell read it.
    #[serde(default)]
    pub cli_plan: Option<CliInstallPlan>,
    /// Continue on Command line: the shell is installing.
    #[serde(default)]
    pub cli_installing: bool,
    /// Why the install failed.
    #[serde(default)]
    pub cli_error: Option<String>,
    /// Menu bar only (no notch). The same setting as Settings' "Spaces tab
    /// in the notch".
    #[serde(default)]
    pub menu_bar: bool,
    /// Signed-in identity.
    pub identity: Option<String>,
    /// Agents configured.
    pub agents: Vec<String>,
    /// The choice.
    pub mode: Option<OnboardingMode>,
    /// The installer's preselection (`--mode host`, MDM file).
    pub installer_mode: Option<OnboardingMode>,
    /// This machine's system, once the drive was checked.
    #[serde(default)]
    pub drive_os: Option<SpaceOs>,
    /// `volume_mount_status`, when the daemon answered.
    #[serde(default)]
    pub drive_status: Option<DriveMountInput>,
    /// The drive was checked (answered or not).
    #[serde(default)]
    pub drive_checked: bool,
    /// The Cua Volume checkbox (off by default).
    #[serde(default)]
    pub drive_mount: bool,
    /// What the shell is running for the page.
    #[serde(default)]
    pub drive_request: Option<DriveStepRequest>,
    /// Why mounting failed.
    #[serde(default)]
    pub drive_error: Option<String>,
    /// `volume_storage`, when the daemon answered.
    #[serde(default)]
    pub drive_storage: Option<DriveStorageInput>,
    /// Where the files live, as picked.
    #[serde(default)]
    pub storage_choice: StorageChoice,
    /// The home folder (paths show as `~/...`).
    #[serde(default)]
    pub drive_home: Option<String>,
    /// The user picked (the saved backend no longer decides).
    #[serde(default)]
    pub storage_picked: bool,
    /// The bucket form and its test or save
    /// ([`crate::drive_settings`]'s state; its `request` is the shell's).
    #[serde(default)]
    pub storage: StorageState,
    /// Done's "Launch at login" checkbox (ticked unless unticked). The
    /// shell applies it when the first run finishes.
    #[serde(default = "ticked")]
    pub launch_at_login: bool,
    /// The usage-data switch on Welcome: the machine's telemetry setting
    /// (Settings' "Share usage data to help improve Cua", `cua telemetry off`), none
    /// until the shell read it (then it shows on).
    #[serde(default)]
    pub telemetry: Option<TelemetryInput>,
    /// Settings, Experiments: the Cua Volume page (and its Done line) only
    /// while Cua Volume is on. Off until the shell says otherwise.
    #[serde(default)]
    pub experiments: Experiments,
    /// The run's start was counted (`onboarding_shown`, Welcome shown):
    /// when Welcome showed on a machine whose usage notice was already
    /// shown, else when Welcome was left. Once per run, however often
    /// Welcome shows again (Back).
    #[serde(default)]
    pub run_counted: bool,
    /// A page opened from Done's to-do list: leaving it (Continue, Skip or
    /// Back) comes back to Done.
    #[serde(default)]
    pub revisit: bool,
}

fn ticked() -> bool {
    true
}

impl OnboardingState {
    /// Usage data is shared: on until the user (or the environment) turns
    /// it off.
    pub fn shares_usage(&self) -> bool {
        self.telemetry.as_ref().is_none_or(|t| t.enabled)
    }
}

/// An input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum OnboardingAction {
    /// "Get started".
    Start,
    /// The app installed its bundled `cua` on first launch (any page).
    CliInstalled {
        /// Where.
        target: Option<String>,
    },
    /// The bundled `cua`'s install plan, read or installed (any page).
    CliPlanned {
        /// The plan.
        plan: CliInstallPlan,
    },
    /// Continue (`install`) or Skip from Command line.
    CliDone {
        /// Install.
        install: bool,
    },
    /// The Command line page's install failed.
    CliInstallFailed {
        /// Why.
        error: String,
    },
    /// A presentation card was picked (it stays on the page).
    PresentationPicked {
        /// Menu bar only.
        #[serde(rename = "menuBar")]
        menu_bar: bool,
    },
    /// Continue from the presentation page.
    PresentationDone,
    /// Signed in.
    SignedIn {
        /// Identity.
        identity: String,
    },
    /// Continue (or Skip) from Sign in.
    SigninDone,
    /// The agents step finished.
    AgentsDone {
        /// Configured agents.
        configured: Vec<String>,
    },
    /// The machine choice was made.
    ModeChosen {
        /// Mode.
        mode: OnboardingMode,
    },
    /// What the drive mount can do here (`volume_mount_status`; none when
    /// the daemon could not answer). Any page.
    DriveChecked {
        /// This machine's system.
        os: SpaceOs,
        /// The daemon's answer.
        status: Option<DriveMountInput>,
    },
    /// The Cua Volume checkbox.
    DriveToggled {
        /// Ticked.
        on: bool,
    },
    /// Continue from Cua Volume: mounts or unmounts first when the checkbox
    /// and the daemon disagree.
    DriveContinue,
    /// `volume_mount` / `volume_unmount` answered.
    DriveMounted {
        /// The status after it.
        status: DriveMountInput,
    },
    /// `volume_mount` / `volume_unmount` failed.
    DriveFailed {
        /// Why.
        error: String,
    },
    /// `volume_storage` answered. Any page.
    DriveStorageLoaded {
        /// The daemon's answer.
        storage: DriveStorageInput,
        /// The home folder, to show paths as `~/...`.
        #[serde(default)]
        home: Option<String>,
    },
    /// Where the files live was picked.
    StorageChosen {
        /// The choice.
        choice: StorageChoice,
    },
    /// An edit of the bucket form, Test connection, or a test's answer
    /// (`set-field`, `set-path-style`, `test`, `checked`, `failed`).
    DriveStorage {
        /// The Storage section's action.
        action: StorageAction,
    },
    /// Continue's `volume_storage_set` answered.
    DriveStorageSaved {
        /// The daemon's check.
        check: DriveCheckInput,
    },
    /// Done's "Launch at login" checkbox.
    LaunchAtLoginToggled {
        /// Ticked.
        on: bool,
    },
    /// Back one page (not from Welcome or Done).
    Back,
    /// The machine's telemetry setting, as the shell read it. Any page.
    TelemetryLoaded {
        /// The setting.
        telemetry: TelemetryInput,
    },
    /// Welcome is on screen (each time it shows; after `telemetry-loaded`).
    /// On a machine that already showed the usage notice, with usage data
    /// on, the run's start counts now rather than when Welcome is left.
    WelcomeShown,
    /// Welcome's "Share usage data to help improve Cua" switch (not while the
    /// environment decides). The shell writes the setting.
    UsageDataToggled {
        /// On.
        on: bool,
    },
    /// Settings, Experiments, as the shell read them (at the start, and
    /// whenever a switch changes). Any page; turning Cua Volume off on its
    /// page moves on.
    ExperimentsLoaded {
        /// The switches.
        experiments: Experiments,
    },
    /// A to-do on Done was clicked: its page opens, and leaving it comes
    /// back to Done. Cua Volume needs its experiment on first (the shell
    /// turns it on and sends `experiments-loaded`).
    TodoChosen {
        /// Which.
        id: OnboardingTodoId,
    },
    /// The coding agents that already use cua (set up earlier, from the
    /// CLI or Settings), as the shell read them. Any page.
    AgentsDetected {
        /// Their names.
        configured: Vec<String>,
    },
}

/// The first state.
pub fn initial(
    installer_mode: Option<OnboardingMode>,
    identity: Option<String>,
) -> OnboardingState {
    OnboardingState {
        step: OnboardingStep::Welcome,
        cli_target: None,
        cli_plan: None,
        cli_installing: false,
        cli_error: None,
        menu_bar: false,
        identity,
        agents: vec![],
        mode: None,
        installer_mode,
        drive_os: None,
        drive_status: None,
        drive_checked: false,
        drive_mount: false,
        drive_request: None,
        drive_error: None,
        drive_storage: None,
        storage_choice: StorageChoice::Local,
        storage_picked: false,
        storage: StorageState::default(),
        drive_home: None,
        launch_at_login: true,
        telemetry: None,
        experiments: Experiments::default(),
        run_counted: false,
        revisit: false,
    }
}

/// The save Continue needs first: a bucket not saved yet (or edited), or
/// back to this machine from a saved bucket.
fn storage_save(s: &OnboardingState) -> Option<DriveStorageUpdate> {
    let saved = s.drive_storage.as_ref()?;
    match s.storage_choice {
        StorageChoice::S3 if saved.backend != "s3" || s.storage.dirty => {
            let mut form = s.storage.form.clone();
            form.backend = "s3".into();
            Some(form.update(false))
        }
        StorageChoice::Local if saved.backend == "s3" => Some(DriveStorageUpdate {
            backend: "fs".into(),
            ..Default::default()
        }),
        _ => None,
    }
}

/// Continue can run: a bucket needs its name and keys (saved ones count).
fn storage_ready(s: &OnboardingState) -> bool {
    if s.storage_choice != StorageChoice::S3 || s.drive_storage.is_none() {
        return true;
    }
    if s.storage.check.as_ref().is_some_and(|c| !c.ok) {
        return false;
    }
    let saved = s
        .drive_storage
        .as_ref()
        .is_some_and(|d| d.backend == "s3" && d.has_keys);
    let mut form = s.storage.form.clone();
    form.backend = "s3".into();
    form_ready(&form, saved)
}

/// After the storage is settled: mount or unmount when the checkbox and the
/// daemon disagree, else on to the next page.
fn continue_mount(n: &mut OnboardingState) {
    let enabled = n.drive_status.as_ref().is_some_and(|m| m.enabled);
    if !drive_usable(n) || n.drive_mount == enabled {
        n.step = next(n, OnboardingStep::Drive);
    } else {
        n.drive_request = Some(if n.drive_mount {
            DriveStepRequest::Mount
        } else {
            DriveStepRequest::Unmount
        });
    }
}

fn drive_usable(s: &OnboardingState) -> bool {
    s.drive_status.as_ref().is_some_and(|m| m.supported())
}

/// Advances the flow. Out-of-order actions are ignored.
pub fn reduce(s: &OnboardingState, a: &OnboardingAction) -> OnboardingState {
    let mut n = reduce_page(s, a);
    // A page opened from Done: whatever leaves it (Back too, even from a
    // page this run otherwise skips) lands on Done again.
    let back = matches!(a, OnboardingAction::Back) && !s.cli_installing;
    if s.revisit && s.step != OnboardingStep::Done && (n.step != s.step || back) {
        n.step = OnboardingStep::Done;
        n.revisit = false;
    }
    n
}

fn reduce_page(s: &OnboardingState, a: &OnboardingAction) -> OnboardingState {
    use OnboardingStep::*;
    let mut n = s.clone();
    match (s.step, a) {
        (Welcome, OnboardingAction::Start) => {
            n.step = next(s, Welcome);
            n.run_counted = true;
        }
        (Welcome, OnboardingAction::WelcomeShown) => {
            if s.shares_usage() && s.telemetry.as_ref().is_some_and(|t| t.notice_shown) {
                n.run_counted = true;
            }
        }
        (_, OnboardingAction::CliInstalled { target }) => n.cli_target = target.clone(),
        (_, OnboardingAction::CliPlanned { plan }) => {
            n.cli_plan = Some(plan.clone());
            // A working `cua` anywhere it is usually installed (install.sh's
            // copy included), not only the app's own.
            if let Some(found) = plan.found.clone() {
                n.cli_target = Some(found);
            } else if plan.up_to_date {
                n.cli_target = Some(plan.target.clone());
            }
            if s.step == Cli && !cli_shown(&n) {
                n.cli_installing = false;
                n.step = next(s, Cli);
            }
            if !matches!(s.step, Welcome | Done) && !cli_shown(s) && cli_shown(&n) {
                n.step = Cli;
            }
        }
        (Cli, OnboardingAction::CliDone { install }) if !s.cli_installing => {
            n.cli_error = None;
            if *install {
                n.cli_installing = true;
            } else {
                n.step = next(s, Cli);
            }
        }
        (Cli, OnboardingAction::CliInstallFailed { error }) if s.cli_installing => {
            n.cli_installing = false;
            n.cli_error = Some(error.clone());
        }
        (Presentation, OnboardingAction::PresentationPicked { menu_bar }) => n.menu_bar = *menu_bar,
        (Presentation, OnboardingAction::PresentationDone) => n.step = next(s, Presentation),
        (_, OnboardingAction::SignedIn { identity }) => n.identity = Some(identity.clone()),
        (Signin, OnboardingAction::SigninDone) => n.step = Agents,
        (Agents, OnboardingAction::AgentsDone { configured }) => {
            // Added to the agents already found (a Skip keeps them).
            for name in configured {
                if !n.agents.contains(name) {
                    n.agents.push(name.clone());
                }
            }
            n.step = Presentation;
        }
        (Mode, OnboardingAction::ModeChosen { mode }) => {
            n.mode = Some(*mode);
            n.step = Done;
        }
        (_, OnboardingAction::DriveChecked { os, status }) => {
            n.drive_os = Some(*os);
            n.drive_checked = true;
            n.drive_status = status.clone();
            n.drive_mount = status.as_ref().is_some_and(|m| m.enabled && m.supported());
            if n.step == Drive && !drive_shown(&n) {
                n.step = next(&n, Presentation);
            }
        }
        (Drive, OnboardingAction::DriveToggled { on })
            if s.drive_request.is_none() && drive_usable(s) =>
        {
            n.drive_mount = *on;
            n.drive_error = None;
        }
        (Drive, OnboardingAction::DriveContinue)
            if s.drive_request.is_none() && !s.storage.busy && storage_ready(s) =>
        {
            n.drive_error = None;
            match storage_save(s) {
                Some(update) => {
                    n.storage.busy = true;
                    n.storage.error = None;
                    n.storage.check = None;
                    n.storage.request = Some(StorageRequest::Save { update });
                }
                None => continue_mount(&mut n),
            }
        }
        (_, OnboardingAction::DriveStorageLoaded { storage, home }) => {
            n.drive_home = home.clone();
            n.storage = storage_reduce(
                &s.storage,
                &StorageAction::Loaded {
                    storage: storage.clone(),
                },
            );
            if !s.storage_picked {
                n.storage_choice = if storage.backend == "s3" {
                    StorageChoice::S3
                } else {
                    StorageChoice::Local
                };
                n.storage.form.backend = storage.backend.clone();
            }
            n.drive_storage = Some(storage.clone());
        }
        (Drive, OnboardingAction::StorageChosen { choice }) if !s.storage.busy => {
            n.storage_choice = *choice;
            n.storage_picked = true;
            n.storage.check = None;
            n.storage.error = None;
            if *choice == StorageChoice::S3 {
                n.storage.form.backend = "s3".into();
            }
        }
        (Drive, OnboardingAction::DriveStorage { action })
            if matches!(
                action,
                StorageAction::SetField { .. }
                    | StorageAction::SetPathStyle { .. }
                    | StorageAction::Test
                    | StorageAction::Checked { .. }
                    | StorageAction::Failed { .. }
                    | StorageAction::ShowManual { .. }
                    | StorageAction::Adopted { .. }
            ) && s.storage_choice == StorageChoice::S3 =>
        {
            n.storage.form.backend = "s3".into();
            n.storage = storage_reduce(&n.storage, action);
        }
        (Drive, OnboardingAction::DriveStorageSaved { check }) => {
            let sent = match &s.storage.request {
                Some(StorageRequest::Save { update }) => Some(update.clone()),
                _ => None,
            };
            n.storage = storage_reduce(
                &s.storage,
                &StorageAction::Saved {
                    check: check.clone(),
                },
            );
            if check.ok && check.applied {
                if let (Some(u), Some(d)) = (sent, n.drive_storage.as_mut()) {
                    d.backend = u.backend.clone();
                    d.has_keys |= u.access_key_id.is_some();
                    if let Some(s3) = u.s3 {
                        d.s3 = Some(s3);
                    }
                }
                n.storage.dirty = false;
                continue_mount(&mut n);
            }
        }
        (Drive, OnboardingAction::DriveMounted { status }) => {
            n.drive_request = None;
            n.drive_status = Some(status.clone());
            let settled = status.enabled == s.drive_mount
                && !matches!(status.state.as_str(), "needs_approval" | "error");
            if settled {
                n.step = next(s, Drive);
            }
        }
        (Drive, OnboardingAction::DriveFailed { error }) => {
            n.drive_request = None;
            n.drive_error = Some(error.clone());
        }
        (Done, OnboardingAction::LaunchAtLoginToggled { on }) => n.launch_at_login = *on,
        (Done, OnboardingAction::TodoChosen { id }) => {
            let page = match id {
                OnboardingTodoId::Cli => Some(Cli),
                OnboardingTodoId::Signin => Some(Signin),
                OnboardingTodoId::Agents => Some(Agents),
                OnboardingTodoId::Volume => drive_shown(s).then_some(Drive),
            };
            if let Some(page) = page {
                n.step = page;
                n.revisit = true;
                n.cli_error = None;
            }
        }
        (_, OnboardingAction::AgentsDetected { configured }) => {
            for name in configured {
                if !n.agents.contains(name) {
                    n.agents.push(name.clone());
                }
            }
        }
        (_, OnboardingAction::TelemetryLoaded { telemetry }) => {
            n.telemetry = Some(telemetry.clone())
        }
        (step, OnboardingAction::ExperimentsLoaded { experiments }) => {
            n.experiments = *experiments;
            // Turned off on its own page: the page goes (whatever it set
            // up stays, as Settings, Storage left it).
            if step == Drive && !drive_shown(&n) {
                n.step = next(&n, Presentation);
                n.drive_request = None;
            }
        }
        (Welcome, OnboardingAction::UsageDataToggled { on })
            if s.telemetry
                .as_ref()
                .is_none_or(|t| t.locked_by.as_deref().is_none_or(str::is_empty)) =>
        {
            let mut t = s.telemetry.clone().unwrap_or_default();
            t.enabled = *on;
            n.telemetry = Some(t);
        }
        (step, OnboardingAction::Back) if step != Welcome && !s.cli_installing => {
            let all = steps(s);
            if let Some(i) = all.iter().position(|x| *x == step).filter(|i| *i > 0) {
                n.step = all[i - 1];
            }
        }
        _ => {}
    }
    n
}

/// A page dot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepDot {
    /// Page.
    pub step: OnboardingStep,
    /// Label.
    pub label: String,
    /// Current.
    pub current: bool,
}

/// The flow as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingView {
    /// Page.
    pub step: OnboardingStep,
    /// Page dots.
    pub dots: Vec<StepDot>,
    /// Heading.
    pub title: String,
    /// One line under it.
    pub lede: String,
    /// The primary button.
    pub primary_label: String,
    /// A Skip button shows.
    pub can_skip: bool,
    /// A Back button shows.
    pub can_back: bool,
    /// The stacked Cua mark shows (Welcome).
    pub show_mark: bool,
    /// The machine choice to highlight (the installer's preselection).
    pub preselected_mode: OnboardingMode,
    /// Done's facts.
    pub summary: Vec<crate::spaces::sidebar::Fact>,
    /// This machine's two answers (the installer's preselection first
    /// highlighted), on its page.
    pub choices: Vec<ModeChoice>,
    /// The presentation page's two cards, side by side.
    pub presentations: Vec<PresentationCard>,
    /// Done's example prompts ([`PROMPTS`]), one line each: shown under
    /// both columns under "Try asking your agent:", to suggest what to ask a
    /// coding agent. Each copies to the clipboard on click.
    pub prompts: Vec<String>,
    /// The first-run telemetry notice (the Welcome page, so the first run
    /// itself can be measured): nothing is sent before it has been shown
    /// once.
    pub notice: Option<String>,
    /// Its link: label.
    pub notice_link_label: Option<String>,
    /// Its link: URL.
    pub notice_link_url: Option<String>,
    /// Welcome's usage-data switch, next to the notice: nothing is sent
    /// before the user leaves Welcome, and nothing at all while it is off.
    #[serde(default)]
    pub usage: Option<UsageToggle>,
    /// The Cua Volume page's card.
    pub drive: Option<DriveCard>,
    /// Done's "Launch at login" checkbox.
    pub launch_at_login: Option<OnboardingCheckbox>,
    /// Done's to-do list: the Cua CLI, the account, an AI agent and Cua
    /// Volume, each set up or with its fix. The title says "Almost done"
    /// until every one that is not optional is.
    #[serde(default)]
    pub todos: Vec<OnboardingTodo>,
}

/// A checkbox with one muted line under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingCheckbox {
    /// "Launch at login".
    pub label: String,
    /// Ticked.
    pub checked: bool,
    /// The line under it.
    pub note: String,
}

/// A line of Done's to-do list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnboardingTodoId {
    /// The Cua CLI.
    Cli,
    /// The Cua account.
    Signin,
    /// A coding agent that uses cua.
    Agents,
    /// Cua Volume (optional).
    Volume,
}

/// One line of Done's to-do list: what is set up, and a one-click fix for
/// what is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingTodo {
    /// Which.
    pub id: OnboardingTodoId,
    /// "Cua CLI".
    pub title: String,
    /// One plain line under it.
    pub note: String,
    /// Set up.
    pub done: bool,
    /// Nice to have: "You're all set" does not wait for it.
    pub optional: bool,
    /// The fix's button ("Install the Cua CLI"), while not done.
    pub action: Option<String>,
}

/// The Cua Volume page's card: an animated miniature
/// ([`crate::drive_mount_preview`]) over one checkbox line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveCard {
    /// "Add Cua Volume to Finder" (Linux: "Mount Cua Volume").
    pub label: String,
    /// The miniature's spoken description.
    pub image_label: String,
    /// Ticked.
    pub checked: bool,
    /// The checkbox can be used (the daemon can mount here).
    pub enabled: bool,
    /// Mounting or unmounting.
    pub busy: bool,
    /// One muted line under it (checking, why not, what to approve).
    pub note: Option<String>,
    /// Why mounting failed.
    pub error: Option<String>,
    /// "Open System Settings" while macOS waits for the extension's
    /// approval.
    pub settings_label: Option<String>,
    /// Where it goes.
    pub settings_url: Option<String>,
    /// "Where your files live" (none until the daemon answered).
    pub storage_title: Option<String>,
    /// This Mac, Your S3 bucket, Set up later.
    pub storage_options: Vec<SettingsOption>,
    /// The bucket's fields, Test connection and its answer (Your S3
    /// bucket), drawn like Settings' rows.
    pub storage_rows: Vec<SettingsRow>,
    /// One muted line for Set up later.
    pub storage_note: Option<String>,
    /// This Mac: "Stored in ~/.cua/volume/data" (the daemon's path).
    pub stored_in: Option<String>,
    /// The mounted volume: "In Finder at ~/Cua Volume".
    pub mounted_at: Option<String>,
    /// Full paths for tooltips and Show in Finder: stored, mounted.
    pub stored_path: Option<String>,
    pub mounted_path: Option<String>,
    /// Continue can be pressed.
    pub can_continue: bool,
}

/// The Cua Volume page's card for `s`.
pub fn drive_card(s: &OnboardingState) -> DriveCard {
    let os = s.drive_os.unwrap_or(SpaceOs::Macos);
    let status = s.drive_status.as_ref();
    let usable = drive_usable(s);
    let approval = status.filter(|m| m.state == "needs_approval");
    let note = if !s.drive_checked {
        Some("Checking\u{2026}".to_string())
    } else if !usable {
        Some(
            status
                .and_then(|m| m.detail.clone())
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| not_available(os)),
        )
    } else {
        approval.map(|m| {
            m.detail
                .clone()
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| "Allow the Cua Volume extension in System Settings.".into())
        })
    };
    let error = s.drive_error.clone().or_else(|| {
        status.filter(|m| m.state == "error").map(|m| {
            m.detail
                .clone()
                .unwrap_or_else(|| "The volume could not be mounted.".into())
        })
    });
    let storage = s.drive_storage.as_ref();
    let storage_rows = match (storage, s.storage_choice) {
        (Some(d), StorageChoice::S3) => {
            let mut state = s.storage.clone();
            state.form.backend = "s3".into();
            let input = StorageInput {
                os,
                storage: Some(d.clone()),
                ..Default::default()
            };
            let mut rows: Vec<SettingsRow> = storage_section(&input, &state)
                .rows
                .into_iter()
                .filter(|r| r.id.starts_with("s3-") || r.id == "storage-error")
                .collect();
            for r in &mut rows {
                if r.id == "s3-endpoint" {
                    r.placeholder = Some("AWS, R2 or MinIO URL".into());
                }
            }
            rows
        }
        _ => vec![],
    };
    let busy = s.drive_request.is_some() || s.storage.busy;
    DriveCard {
        storage_title: storage.map(|_| "Where your files live".into()),
        storage_options: if storage.is_some() {
            [
                (
                    StorageChoice::Local,
                    "local",
                    crate::drive_settings::this_machine(os),
                ),
                (StorageChoice::S3, "s3", "Your S3 bucket"),
                (StorageChoice::Later, "later", "Set up later"),
            ]
            .into_iter()
            .map(|(c, id, label)| SettingsOption {
                id: id.into(),
                label: label.into(),
                active: s.storage_choice == c,
            })
            .collect()
        } else {
            vec![]
        },
        storage_rows,
        storage_note: (storage.is_some() && s.storage_choice == StorageChoice::Later)
            .then(|| "Settings, Storage, any time.".into()),
        stored_in: storage
            .filter(|d| s.storage_choice == StorageChoice::Local && !d.fs_path.is_empty())
            .map(|d| {
                format!(
                    "Stored in {}",
                    crate::paths::display_path(&d.fs_path, s.drive_home.as_deref())
                )
            }),
        stored_path: storage
            .filter(|_| s.storage_choice == StorageChoice::Local)
            .map(|d| d.fs_path.clone())
            .filter(|p| !p.is_empty()),
        mounted_at: status.and_then(|m| {
            m.mounted_path().map(|p| {
                format!(
                    "{} {}",
                    if m.in_finder() {
                        "In Finder at"
                    } else {
                        "Mounted at"
                    },
                    crate::paths::display_path(p, s.drive_home.as_deref())
                )
            })
        }),
        mounted_path: status.and_then(|m| m.mounted_path().map(str::to_string)),
        can_continue: !busy && storage_ready(s),
        label: mount_label(os).into(),
        image_label: if os == SpaceOs::Linux {
            "Files from a Space arriving in the mounted Cua Volume"
        } else {
            "Files from a Space arriving in Cua Volume in Finder"
        }
        .into(),
        checked: s.drive_mount,
        enabled: usable && !busy,
        busy,
        note,
        error,
        settings_label: approval
            .and_then(|m| m.settings_url.as_ref())
            .map(|_| "Open System Settings".into()),
        settings_url: approval.and_then(|m| m.settings_url.clone()),
    }
}

/// Done's line for the drive.
fn drive_fact(s: &OnboardingState) -> String {
    let os = s.drive_os.unwrap_or(SpaceOs::Macos);
    match s
        .drive_status
        .as_ref()
        .filter(|m| m.enabled && s.drive_mount)
    {
        None => "Off".into(),
        Some(m) => match m.state.as_str() {
            "mounted" if m.in_finder() || os == SpaceOs::Macos => "In Finder".into(),
            "mounted" => m.path.clone().unwrap_or_else(|| "Mounted".into()),
            "needs_approval" => "Waiting for approval".into(),
            "mounting" => "Mounting".into(),
            _ => "Off".into(),
        },
    }
}

/// One answer on the This machine page.
/// Welcome's "Share usage data to help improve Cua" switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageToggle {
    /// "Share usage data to help improve Cua".
    pub label: String,
    /// On.
    pub on: bool,
    /// It can be changed (the environment does not decide).
    pub enabled: bool,
    /// Why it cannot ("Set by env DO_NOT_TRACK").
    pub help: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeChoice {
    /// What it means.
    pub mode: OnboardingMode,
    /// Label.
    pub label: String,
    /// Highlighted (the installer's preselection, else the first).
    pub preselected: bool,
}

/// A card on the presentation page: an animated miniature of the app in
/// that mode ([`crate::onboarding_preview`]), and its one-line title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationCard {
    /// Menu bar only.
    pub menu_bar: bool,
    /// "Notch and menu bar".
    pub title: String,
    /// The card's id (`onboarding-notch`, `onboarding-menu-bar`): its
    /// accessibility identifier and list key.
    pub id: String,
    /// The miniature's spoken description.
    pub image_label: String,
    /// Picked.
    pub selected: bool,
}

/// Done's example prompts: what to ask a coding agent once it has the cua
/// skills and MCP server. Each works with the shipped tools:
/// - a Linux test run: `create_space`, `upload`, `space_bash`;
/// - finish a web task in a Space's browser: `teleport_browser_session`, then
///   `call_tool` on the Space's cua-driver;
/// - sharing: `share_space` (role viewer).
pub const PROMPTS: [&str; 3] = [
    "Open a Linux Space and run my test suite in it",
    "Use my Space's browser to finish the checkout test and show me",
    "Share this Space with a teammate as a viewer",
];

/// The presentation cards for a choice (notch first, the default).
pub fn presentation_cards(menu_bar: bool) -> Vec<PresentationCard> {
    [
        (
            false,
            "Notch and menu bar",
            "onboarding-notch",
            "The Spaces tiles open from the notch, and the menu bar menu",
        ),
        (
            true,
            "Menu bar only",
            "onboarding-menu-bar",
            "The menu bar menu alone",
        ),
    ]
    .into_iter()
    .map(|(mb, title, id, label)| PresentationCard {
        menu_bar: mb,
        title: title.into(),
        id: id.into(),
        image_label: label.into(),
        selected: mb == menu_bar,
    })
    .collect()
}

/// The first run's fixed words (buttons and states inside the steps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingCopy {
    /// "Back".
    pub back: String,
    /// "Skip".
    pub skip: String,
    /// "Continue".
    pub continue_label: String,
    /// "Try again".
    pub try_again: String,
    /// "Checking…" (the install plan loads).
    pub checking: String,
    /// The install script when the app bundles no `cua`.
    pub install_script: String,
    /// "Install".
    pub install: String,
    /// "Installing…".
    pub installing: String,
    /// The PATH switch.
    pub add_to_path: String,
    /// When another `cua` comes first on PATH.
    pub shadowed: String,
    /// "Sign in".
    pub sign_in: String,
    /// While the browser is open.
    pub sign_in_waiting: String,
    /// While agents are detected.
    pub agents_looking: String,
    /// No agent found.
    pub agents_none: String,
    /// The skills switch.
    pub agents_skills: String,
    /// The MCP switch.
    pub agents_mcp: String,
    /// "Set up".
    pub agents_set_up: String,
    /// "Setting up…".
    pub agents_setting_up: String,
    /// After setup: heading.
    pub agents_done_title: String,
    /// After setup: one line.
    pub agents_done_lede: String,
    /// The background computer-use card's one line (its checkbox).
    pub agents_driver: String,
    /// The card's miniature, for accessibility.
    pub agents_driver_image: String,
    /// Over the permission rows (Done, after host setup).
    pub permissions_title: String,
    /// The permission rows' button.
    pub open_settings: String,
    /// The Sign in page's Teams line ("Teams · manage them at app.cua.ai").
    pub teams: String,
    /// Its link ("Open the account app").
    pub teams_link: String,
    /// The link's URL: the account app, which administers teams.
    pub teams_url: String,
}

/// The first run's fixed words.
pub fn copy() -> OnboardingCopy {
    OnboardingCopy {
        back: "Back".into(),
        skip: "Skip".into(),
        continue_label: "Continue".into(),
        try_again: "Try again".into(),
        checking: "Checking\u{2026}".into(),
        install_script: "curl -fsSL https://cua.ai/install.sh | sh".into(),
        install: "Install".into(),
        installing: "Installing\u{2026}".into(),
        add_to_path: "Add to PATH".into(),
        shadowed: "Another cua comes first on PATH.".into(),
        sign_in: "Sign in".into(),
        sign_in_waiting: "Waiting for the browser\u{2026}".into(),
        agents_looking: "Looking for agents\u{2026}".into(),
        agents_none: "No agents found.".into(),
        agents_skills: "cua skills".into(),
        agents_mcp: "cua MCP server".into(),
        agents_set_up: "Set up".into(),
        agents_setting_up: "Setting up\u{2026}".into(),
        agents_done_title: "Your AI agents are set up".into(),
        agents_done_lede: "Your agents can now use Spaces.".into(),
        agents_driver: "cua-driver skill for background computer-use".into(),
        agents_driver_image: "An agent clicking in a background window while you keep working"
            .into(),
        permissions_title: crate::host::PERMISSIONS_TITLE.into(),
        open_settings: crate::host::OPEN_SETTINGS_LABEL.into(),
        teams: format!(
            "{} \u{b7} {}",
            crate::settings::TEAMS_LABEL,
            crate::settings::TEAMS_VALUE.to_lowercase()
        ),
        teams_link: crate::settings::TEAMS_BUTTON.into(),
        teams_url: crate::settings::TEAMS_URL.into(),
    }
}

/// The Sign in page's line once signed in.
pub fn signed_in_text(identity: Option<&str>) -> String {
    match identity.filter(|i| !i.is_empty()) {
        Some(id) => format!("Signed in as {id}."),
        None => "Signed in.".into(),
    }
}

/// The Sign in page's line while the browser is open.
pub fn sign_in_code_text(user_code: Option<&str>) -> String {
    match user_code.filter(|c| !c.is_empty()) {
        Some(code) => format!("Confirm {code} in your browser."),
        None => "Finish in your browser.".into(),
    }
}

/// The install target's tooltip when a `cua` is already there.
pub fn replaces_text(installed_version: Option<&str>) -> String {
    match installed_version.filter(|v| !v.is_empty()) {
        Some(v) => format!("Replaces the cua already there ({v})"),
        None => "Replaces the cua already there".into(),
    }
}

/// The Command line page's line once `cua` is current.
pub fn installed_at_text(target: &str) -> String {
    format!("Installed at {target}")
}

fn cli_lede(plan: &CliInstallPlan) -> String {
    let verb = match plan.method {
        Some(InstallMethod::Symlink) => "Links",
        _ => "Copies",
    };
    if plan.on_path {
        format!("{verb} cua to {}.", plan.target)
    } else {
        format!(
            "{verb} cua to {} and adds {} to PATH in {}.",
            plan.target,
            plan.bin_dir,
            plan.path_profile.as_deref().unwrap_or("your shell profile")
        )
    }
}

/// How the copy names the computer the app runs on.
fn this_computer(os: SpaceOs) -> &'static str {
    match os {
        SpaceOs::Macos => "this Mac",
        SpaceOs::Windows => "this PC",
        _ => "this machine",
    }
}

/// The flow as drawn, in a Mac's words.
pub fn view(s: &OnboardingState) -> OnboardingView {
    view_on(s, SpaceOs::Macos)
}

/// Where the app stays once the window closes: the menu bar on a Mac, the
/// tray elsewhere.
pub fn done_lede(os: SpaceOs) -> &'static str {
    match os {
        SpaceOs::Macos => "Cua Spaces is in your menu bar.",
        _ => "Cua Spaces is in your system tray.",
    }
}

/// [`view`] in the words of the system the app runs on (`os`).
pub fn view_on(s: &OnboardingState, os: SpaceOs) -> OnboardingView {
    view_for(s, os, true)
}

/// [`view_on`] for a shell with (`cli_page`) or without a Command line
/// page: without one, Done neither lists the Cua CLI nor waits for it.
pub fn view_for(s: &OnboardingState, os: SpaceOs, cli_page: bool) -> OnboardingView {
    use crate::spaces::sidebar::Fact;
    use OnboardingStep::*;
    let cli = s.cli_plan.as_ref().map(cli_lede).unwrap_or_default();
    let here = this_computer(os);
    let mode_title = format!("How do you want to use {here}?");
    let (title, lede, primary) = match s.step {
        Welcome => (
            "Welcome to Cua Spaces",
            "Computers for you and your AI agents. Setup takes about a minute.",
            "Get started",
        ),
        Cli => (
            if s.cli_plan.as_ref().is_some_and(|p| p.on_path) {
                "Install the Cua CLI"
            } else {
                "Install the Cua CLI and add it to your PATH"
            },
            cli.as_str(),
            if s.cli_installing {
                "Installing\u{2026}"
            } else {
                "Continue"
            },
        ),
        Presentation => (
            "Where should Cua Spaces show up?",
            "You can change this in Settings.",
            "Continue",
        ),
        Signin => (
            "Sign in",
            "Sign in to reach your machines and Spaces from any of your machines. You can skip this and sign in later.",
            "Continue",
        ),
        Agents => (
            "AI agents",
            "Give your coding agents the cua skills and MCP server.",
            "Continue",
        ),
        Drive => (
            "Cua Volume",
            "Memory and files your agents share, across every Space.",
            if matches!(s.storage.request, Some(StorageRequest::Save { .. })) {
                "Saving\u{2026}"
            } else if s.drive_request.is_some() {
                "Mounting\u{2026}"
            } else {
                "Continue"
            },
        ),
        Mode => (
            mode_title.as_str(),
            "Use it only to open your other machines, or let your other machines reach it too. You can change this later.",
            "Continue",
        ),
        Done => ("", "", "Start using Cua Spaces"),
    };
    let todos = if s.step == Done {
        done_todos(s)
            .into_iter()
            .filter(|t| cli_page || t.id != OnboardingTodoId::Cli)
            .collect()
    } else {
        vec![]
    };
    let left = todos.iter().filter(|t| !t.done && !t.optional).count();
    let done_words = (s.step == Done).then(|| done_heading(left, os));
    let (title, lede) = match &done_words {
        Some((t, l)) => (t.as_str(), l.as_str()),
        None => (title, lede),
    };
    let summary = if s.step == Done {
        vec![
            Fact {
                label: "Shows in".into(),
                value: if s.menu_bar {
                    "Menu bar only"
                } else {
                    "Notch and menu bar"
                }
                .into(),
                copy: None,
                help: None,
                warning: None,
            },
            Fact {
                label: "This machine".into(),
                value: if s.mode == Some(OnboardingMode::Host) {
                    "Your other machines can reach it"
                } else {
                    "Opens your other machines"
                }
                .into(),
                copy: None,
                help: None,
                warning: None,
            },
        ]
    } else {
        vec![]
    };
    OnboardingView {
        step: s.step,
        dots: steps(s)
            .iter()
            .map(|st| StepDot {
                step: *st,
                label: st.label().into(),
                current: *st == s.step,
            })
            .collect(),
        title: title.into(),
        lede: lede.into(),
        primary_label: primary.into(),
        can_skip: (s.step == Cli && !s.cli_installing)
            || s.step == Agents
            || (s.step == Signin && s.identity.is_none()),
        can_back: s.step != Welcome && !s.cli_installing,
        show_mark: s.step == Welcome,
        preselected_mode: s.installer_mode.unwrap_or(OnboardingMode::Client),
        summary,
        choices: if s.step == Mode {
            let pre = s.installer_mode.unwrap_or(OnboardingMode::Client);
            [
                (
                    OnboardingMode::Client,
                    format!("Just use {here} to open my other machines"),
                ),
                (
                    OnboardingMode::Host,
                    format!("Let my other machines reach {here}"),
                ),
            ]
            .into_iter()
            .map(|(mode, label)| ModeChoice {
                mode,
                label,
                preselected: mode == pre,
            })
            .collect()
        } else {
            vec![]
        },
        presentations: if s.step == Presentation {
            presentation_cards(s.menu_bar)
        } else {
            vec![]
        },
        prompts: if s.step == Done {
            PROMPTS.iter().map(|p| (*p).to_string()).collect()
        } else {
            vec![]
        },
        notice: (s.step == Welcome).then(|| {
            "Cua collects usage data, like which features you use and when something goes wrong. When you're signed in, it's linked to your account through a random ID. Never your files, what you type or your screen. Change this here or in Settings, Privacy."
                .into()
        }),
        notice_link_label: (s.step == Welcome).then(|| "What is collected".into()),
        notice_link_url: (s.step == Welcome).then(|| crate::settings::TELEMETRY_DOCS_URL.into()),
        usage: (s.step == Welcome).then(|| {
            let locked = s
                .telemetry
                .as_ref()
                .and_then(|t| t.locked_by.clone())
                .filter(|b| !b.is_empty());
            UsageToggle {
                label: "Share usage data to help improve Cua".into(),
                on: s.shares_usage(),
                enabled: locked.is_none(),
                help: locked.map(|by| format!("Set by {by}")),
            }
        }),
        drive: (s.step == Drive).then(|| drive_card(s)),
        launch_at_login: (s.step == Done).then(|| OnboardingCheckbox {
            label: crate::login_item::DONE_LABEL.into(),
            checked: s.launch_at_login,
            note: crate::login_item::note(s.experiments.cua_volume).into(),
        }),
        todos,
    }
}

/// Done's heading and the line under it: "You're all set" only when it is
/// true, else "Almost done" and how many things are left.
pub fn done_heading(left: usize, os: SpaceOs) -> (String, String) {
    let place = match os {
        SpaceOs::Macos => "the menu bar",
        _ => "the system tray",
    };
    match left {
        0 => ("You're all set".into(), done_lede(os).into()),
        1 => (
            "Almost done".into(),
            format!("One thing is left. You can finish it now, or later from {place}."),
        ),
        n => (
            "Almost done".into(),
            format!("{n} things are left. You can finish them now, or later from {place}."),
        ),
    }
}

/// The Cua CLI's one plain line (next to its name everywhere it shows).
pub const CLI_HINT: &str = "Command-line tools for you and your agents.";
/// Cua Volume's one plain line.
pub const VOLUME_HINT: &str = "Shared folder: files your agents can share across your machines.";

/// Done's to-do list, in page order. Cua Volume shows wherever it can
/// mount (its experiment need not be on: "Turn on" turns it on).
fn done_todos(s: &OnboardingState) -> Vec<OnboardingTodo> {
    let todo = |id, title: &str, note: String, done: bool, optional, action: &str| OnboardingTodo {
        id,
        title: title.into(),
        note,
        done,
        optional,
        action: (!done).then(|| action.to_string()),
    };
    let signed_in = s.identity.as_deref().filter(|i| !i.is_empty());
    let mut out = vec![
        todo(
            OnboardingTodoId::Cli,
            "Cua CLI",
            CLI_HINT.into(),
            s.cli_target.is_some(),
            false,
            "Install the Cua CLI",
        ),
        todo(
            OnboardingTodoId::Signin,
            "Account",
            match signed_in {
                Some(id) => format!("Signed in as {id}."),
                None => "Reach your machines from any machine.".into(),
            },
            signed_in.is_some(),
            false,
            "Sign in",
        ),
        todo(
            OnboardingTodoId::Agents,
            "AI agents",
            if s.agents.is_empty() {
                "Let Claude Code, Codex and others use Cua.".into()
            } else {
                format!("{} can use Cua.", s.agents.join(", "))
            },
            !s.agents.is_empty(),
            false,
            "Connect an AI agent",
        ),
    ];
    let can_mount = s.drive_os != Some(SpaceOs::Windows)
        || s.drive_status.as_ref().is_some_and(|m| m.supported());
    if can_mount {
        let on = drive_fact(s);
        let mounted = s.experiments.cua_volume
            && s.drive_mount
            && s.drive_status
                .as_ref()
                .is_some_and(|m| m.enabled && m.state == "mounted");
        out.push(todo(
            OnboardingTodoId::Volume,
            "Cua Volume",
            if s.experiments.cua_volume && on != "Off" {
                format!("{VOLUME_HINT} {on}.")
            } else {
                VOLUME_HINT.into()
            },
            mounted,
            true,
            "Turn on",
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    /// A Windows machine whose drive check answers while its Volume page
    /// shows moves on to This machine (the page does not apply there).
    /// The first state with the Cua Volume experiment on.
    fn volume_on() -> OnboardingState {
        reduce(
            &initial(None, None),
            &OnboardingAction::ExperimentsLoaded {
                experiments: Experiments {
                    cua_volume: true,
                    ..Default::default()
                },
            },
        )
    }

    /// Done is honest: "Almost done" while the CLI, the account or an
    /// agent is missing, with a fix for each; a fix opens its page and
    /// leaving it comes back to Done.
    #[test]
    fn done_lists_what_is_missing_and_each_fix_comes_back() {
        let done = OnboardingState {
            step: OnboardingStep::Done,
            mode: Some(OnboardingMode::Client),
            ..initial(None, None)
        };
        let v = view(&done);
        assert_eq!(v.title, "Almost done");
        assert_eq!(
            v.lede,
            "3 things are left. You can finish them now, or later from the menu bar."
        );
        let fixes: Vec<_> = v.todos.iter().filter_map(|t| t.action.clone()).collect();
        assert_eq!(
            fixes,
            [
                "Install the Cua CLI",
                "Sign in",
                "Connect an AI agent",
                "Turn on"
            ]
        );
        assert!(
            v.summary
                .iter()
                .any(|f| f.label == "This machine" && f.value == "Opens your other machines")
        );
        // Sign in from Done, then back to Done.
        let at = reduce(
            &done,
            &OnboardingAction::TodoChosen {
                id: OnboardingTodoId::Signin,
            },
        );
        assert_eq!(at.step, OnboardingStep::Signin);
        let signed = reduce(
            &at,
            &OnboardingAction::SignedIn {
                identity: "ada@example.com".into(),
            },
        );
        let back = reduce(&signed, &OnboardingAction::SigninDone);
        assert_eq!(back.step, OnboardingStep::Done);
        assert!(!back.revisit);
        assert_eq!(
            view(&back).lede.split('.').next(),
            Some("2 things are left")
        );
        // Back from a page this run skipped (no Command line page) too.
        let cli = reduce(
            &back,
            &OnboardingAction::TodoChosen {
                id: OnboardingTodoId::Cli,
            },
        );
        assert_eq!(cli.step, OnboardingStep::Cli);
        assert_eq!(
            reduce(&cli, &OnboardingAction::Back).step,
            OnboardingStep::Done
        );
        // Cua Volume needs its experiment first: nothing until it is on.
        let vol = OnboardingTodoId::Volume;
        assert_eq!(
            reduce(&back, &OnboardingAction::TodoChosen { id: vol }).step,
            OnboardingStep::Done
        );
        // The CLI from install.sh and agents set up earlier count.
        let found = reduce(
            &back,
            &OnboardingAction::CliPlanned {
                plan: CliInstallPlan {
                    found: Some("/h/.local/bin/cua".into()),
                    ..plan(false, false)
                },
            },
        );
        assert_eq!(
            found.step,
            OnboardingStep::Done,
            "a late plan never leaves Done"
        );
        let all = reduce(
            &found,
            &OnboardingAction::AgentsDetected {
                configured: vec!["Codex".into()],
            },
        );
        assert_eq!(view(&all).title, "You're all set");
    }

    /// The last page says where the app stays: the menu bar on a Mac, the
    /// tray on Windows and Linux.
    #[test]
    fn done_names_the_menu_bar_only_on_a_mac() {
        let done = OnboardingState {
            step: OnboardingStep::Done,
            cli_target: Some("/h/.local/bin/cua".into()),
            agents: vec!["Claude Code".into()],
            ..initial(None, Some("ada@example.com".into()))
        };
        assert_eq!(view(&done).title, "You're all set");
        assert_eq!(view(&done).lede, "Cua Spaces is in your menu bar.");
        assert_eq!(view_on(&done, SpaceOs::Macos), view(&done));
        for os in [SpaceOs::Windows, SpaceOs::Linux] {
            assert_eq!(
                view_on(&done, os).lede,
                "Cua Spaces is in your system tray."
            );
        }
        let one_left = OnboardingState {
            agents: vec![],
            ..done.clone()
        };
        assert_eq!(
            view_on(&one_left, SpaceOs::Linux).lede,
            "One thing is left. You can finish it now, or later from the system tray."
        );
    }

    #[test]
    fn the_mode_step_names_the_computer_in_everyday_words() {
        let mut s = reduce(&initial(None, None), &OnboardingAction::Start);
        s.step = OnboardingStep::Mode;
        let mac = view_on(&s, SpaceOs::Macos);
        assert_eq!(mac.title, "How do you want to use this Mac?");
        let labels: Vec<_> = mac.choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Just use this Mac to open my other machines",
                "Let my other machines reach this Mac"
            ]
        );
        assert_eq!(
            view_on(&s, SpaceOs::Windows).title,
            "How do you want to use this PC?"
        );
        assert_eq!(
            view_on(&s, SpaceOs::Linux).title,
            "How do you want to use this machine?"
        );
    }

    #[test]
    fn signin_explains_account_access_and_allows_skipping() {
        let s = reduce(&initial(None, None), &OnboardingAction::Start);
        for os in [SpaceOs::Macos, SpaceOs::Windows, SpaceOs::Linux] {
            let v = view_on(&s, os);
            assert_eq!(v.title, "Sign in");
            assert_eq!(
                v.lede,
                "Sign in to reach your machines and Spaces from any of your machines. You can skip this and sign in later."
            );
            assert!(v.can_skip);
        }
        assert_eq!(
            reduce(&s, &OnboardingAction::SigninDone).step,
            OnboardingStep::Agents
        );
    }

    /// Cua Volume off (the default): no Volume page, dot, Done line or
    /// promise in Done's launch-at-login line; turning it off on its page
    /// moves on.
    #[test]
    fn the_volume_page_shows_only_with_its_experiment() {
        let mut s = initial(None, None);
        for a in [
            OnboardingAction::Start,
            OnboardingAction::SigninDone,
            OnboardingAction::AgentsDone { configured: vec![] },
            OnboardingAction::DriveChecked {
                os: SpaceOs::Macos,
                status: Some(mount("off", false)),
            },
        ] {
            s = reduce(&s, &a);
        }
        assert_eq!(s.step, OnboardingStep::Presentation);
        assert!(view(&s).dots.iter().all(|d| d.label != "Cua Volume"));
        s = reduce(&s, &OnboardingAction::PresentationDone);
        assert_eq!(s.step, OnboardingStep::Mode, "no Volume page");
        assert_eq!(
            reduce(&s, &OnboardingAction::Back).step,
            OnboardingStep::Presentation
        );
        s = reduce(
            &s,
            &OnboardingAction::ModeChosen {
                mode: OnboardingMode::Client,
            },
        );
        let done = view(&s);
        let volume = done
            .todos
            .iter()
            .find(|t| t.id == OnboardingTodoId::Volume)
            .unwrap();
        assert!(volume.optional && !volume.done, "offered, off");
        assert_eq!(volume.action.as_deref(), Some("Turn on"));
        assert_eq!(volume.note, VOLUME_HINT);
        assert_eq!(
            done.launch_at_login.unwrap().note,
            crate::login_item::NOTE_WITHOUT_VOLUME
        );
        // On: the page, its dot and its Done line are back.
        let on = reduce(
            &s,
            &OnboardingAction::ExperimentsLoaded {
                experiments: Experiments {
                    cua_volume: true,
                    ..Default::default()
                },
            },
        );
        let v = view(&on);
        assert!(v.dots.iter().any(|d| d.label == "Cua Volume"));
        assert!(v.todos.iter().any(|t| t.title == "Cua Volume"));
        assert_eq!(v.launch_at_login.unwrap().note, crate::login_item::NOTE);
        // Turned off while its page shows: on to the next page.
        let at = OnboardingState {
            step: OnboardingStep::Drive,
            ..volume_on()
        };
        let off = reduce(
            &at,
            &OnboardingAction::ExperimentsLoaded {
                experiments: Experiments::default(),
            },
        );
        assert_eq!(off.step, OnboardingStep::Mode);
    }

    #[test]
    fn a_late_windows_drive_check_leaves_the_volume_page() {
        let mut s = volume_on();
        for a in [
            OnboardingAction::Start,
            OnboardingAction::SigninDone,
            OnboardingAction::AgentsDone { configured: vec![] },
            OnboardingAction::PresentationDone,
        ] {
            s = reduce(&s, &a);
        }
        assert_eq!(s.step, OnboardingStep::Drive);
        let s = reduce(
            &s,
            &OnboardingAction::DriveChecked {
                os: SpaceOs::Windows,
                status: None,
            },
        );
        assert_eq!(s.step, OnboardingStep::Mode);
    }

    use super::*;

    fn plan(up_to_date: bool, on_path: bool) -> CliInstallPlan {
        CliInstallPlan {
            found: up_to_date.then(|| "/h/.local/bin/cua".into()),
            source: Some("/Applications/Cua Spaces.app/Contents/MacOS/cua".into()),
            target: "/h/.local/bin/cua".into(),
            bin_dir: "/h/.local/bin".into(),
            up_to_date,
            on_path,
            path_profile: Some("/h/.zshrc".into()),
            ..Default::default()
        }
    }

    #[test]
    fn the_command_line_page_asks_before_installing() {
        let planned = reduce(
            &initial(None, None),
            &OnboardingAction::CliPlanned {
                plan: plan(false, false),
            },
        );
        assert_eq!(planned.cli_target, None, "nothing installed yet");
        let s = reduce(&planned, &OnboardingAction::Start);
        assert_eq!(s.step, OnboardingStep::Cli);
        let v = view(&s);
        assert_eq!(v.title, "Install the Cua CLI and add it to your PATH");
        assert_eq!(
            v.lede,
            "Copies cua to /h/.local/bin/cua and adds /h/.local/bin to PATH in /h/.zshrc."
        );
        assert_eq!(v.primary_label, "Continue");
        assert!(v.can_skip && v.can_back);
        assert!(
            v.dots
                .iter()
                .any(|d| d.label == "Command line" && d.current)
        );
        let skipped = reduce(&s, &OnboardingAction::CliDone { install: false });
        assert_eq!(skipped.step, OnboardingStep::Signin);
        assert!(!skipped.cli_installing, "Skip installs nothing");
        assert_eq!(
            reduce(&skipped, &OnboardingAction::Back).step,
            OnboardingStep::Cli
        );
        let went_on = reduce(
            &reduce(&s, &OnboardingAction::CliDone { install: true }),
            &OnboardingAction::CliPlanned {
                plan: plan(true, true),
            },
        );
        assert_eq!(went_on.step, OnboardingStep::Signin);
        assert_eq!(went_on.cli_target.as_deref(), Some("/h/.local/bin/cua"));
        assert!(
            view(&went_on)
                .dots
                .iter()
                .all(|d| d.label != "Command line")
        );
        let on_path = reduce(
            &s,
            &OnboardingAction::CliPlanned {
                plan: plan(false, true),
            },
        );
        assert_eq!(view(&on_path).title, "Install the Cua CLI");
        for p in [
            plan(true, true),
            CliInstallPlan {
                found: None,
                source: None,
                ..plan(false, false)
            },
        ] {
            let s = reduce(
                &reduce(
                    &initial(None, None),
                    &OnboardingAction::CliPlanned { plan: p },
                ),
                &OnboardingAction::Start,
            );
            assert_eq!(s.step, OnboardingStep::Signin, "current or not bundled");
        }
        let current = reduce(
            &initial(None, None),
            &OnboardingAction::CliPlanned {
                plan: plan(true, true),
            },
        );
        assert_eq!(current.cli_target.as_deref(), Some("/h/.local/bin/cua"));
        let linked = reduce(
            &s,
            &OnboardingAction::CliPlanned {
                plan: CliInstallPlan {
                    found: None,
                    method: Some(InstallMethod::Symlink),
                    ..plan(false, true)
                },
            },
        );
        assert_eq!(view(&linked).lede, "Links cua to /h/.local/bin/cua.");
    }

    #[test]
    fn continue_waits_on_the_command_line_page_until_the_install_finishes() {
        let s = reduce(
            &reduce(
                &initial(None, None),
                &OnboardingAction::CliPlanned {
                    plan: plan(false, false),
                },
            ),
            &OnboardingAction::Start,
        );
        let installing = reduce(&s, &OnboardingAction::CliDone { install: true });
        assert_eq!(installing.step, OnboardingStep::Cli);
        assert!(installing.cli_installing);
        let v = view(&installing);
        assert_eq!(v.primary_label, "Installing\u{2026}");
        assert!(!v.can_back && !v.can_skip);
        for a in [
            OnboardingAction::CliDone { install: true },
            OnboardingAction::CliDone { install: false },
            OnboardingAction::Back,
        ] {
            assert_eq!(
                reduce(&installing, &a),
                installing,
                "{a:?} while installing"
            );
        }
        let failed = reduce(
            &installing,
            &OnboardingAction::CliInstallFailed {
                error: "Permission denied".into(),
            },
        );
        assert_eq!(failed.step, OnboardingStep::Cli);
        assert!(!failed.cli_installing);
        assert_eq!(failed.cli_error.as_deref(), Some("Permission denied"));
        assert_eq!(failed.cli_target, None);
        assert!(view(&failed).can_skip && view(&failed).can_back);
        let again = reduce(&failed, &OnboardingAction::CliDone { install: true });
        assert!(again.cli_installing);
        assert_eq!(again.cli_error, None);
        let installed = reduce(
            &again,
            &OnboardingAction::CliPlanned {
                plan: plan(true, true),
            },
        );
        assert_eq!(installed.step, OnboardingStep::Signin);
        assert!(!installed.cli_installing);
        assert_eq!(installed.cli_target.as_deref(), Some("/h/.local/bin/cua"));
    }

    #[test]
    fn a_plan_that_arrives_after_get_started_still_shows_the_command_line_page() {
        let started = reduce(&initial(None, None), &OnboardingAction::Start);
        assert_eq!(started.step, OnboardingStep::Signin);
        let s = reduce(
            &started,
            &OnboardingAction::CliPlanned {
                plan: plan(false, false),
            },
        );
        assert_eq!(s.step, OnboardingStep::Cli);
        assert_eq!(
            reduce(&s, &OnboardingAction::CliDone { install: false }).step,
            OnboardingStep::Signin
        );
        let again = reduce(
            &s,
            &OnboardingAction::CliPlanned {
                plan: plan(false, false),
            },
        );
        assert_eq!(again.step, OnboardingStep::Cli);
        let current = reduce(
            &started,
            &OnboardingAction::CliPlanned {
                plan: plan(true, true),
            },
        );
        assert_eq!(current.step, OnboardingStep::Signin);
    }

    #[test]
    fn no_command_line_page_without_a_plan_and_the_presentation_choice_before_this_machine() {
        let mut s = volume_on();
        assert!(view(&s).dots.iter().all(|d| d.label != "Command line"));
        s = reduce(
            &s,
            &OnboardingAction::CliInstalled {
                target: Some("/h/.local/bin/cua".into()),
            },
        );
        assert_eq!(
            s.cli_target.as_deref(),
            Some("/h/.local/bin/cua"),
            "any page"
        );
        for a in [
            OnboardingAction::Start,
            OnboardingAction::SigninDone,
            OnboardingAction::AgentsDone { configured: vec![] },
        ] {
            s = reduce(&s, &a);
        }
        assert_eq!(s.step, OnboardingStep::Presentation);
        let v = view(&s);
        let cards: Vec<_> = v
            .presentations
            .iter()
            .map(|c| (c.title.as_str(), c.selected))
            .collect();
        assert_eq!(
            cards,
            [("Notch and menu bar", true), ("Menu bar only", false)],
            "notch by default"
        );
        s = reduce(&s, &OnboardingAction::PresentationPicked { menu_bar: true });
        assert!(view(&s).presentations[1].selected);
        s = reduce(&s, &OnboardingAction::PresentationDone);
        assert_eq!(s.step, OnboardingStep::Drive);
        s = reduce(&s, &OnboardingAction::DriveContinue);
        assert_eq!(s.step, OnboardingStep::Mode);
        s = reduce(
            &s,
            &OnboardingAction::ModeChosen {
                mode: OnboardingMode::Client,
            },
        );
        let done = view(&s);
        assert_eq!(done.prompts, PROMPTS, "Done shows the example prompts");
        assert!(
            done.prompts.len() == 3
                && done
                    .prompts
                    .iter()
                    .all(|p| !p.contains('\n') && p.chars().count() <= 72),
            "short single lines"
        );
        s.step = OnboardingStep::Mode;
        assert!(view(&s).prompts.is_empty(), "only on Done");
        s.step = OnboardingStep::Done;
        assert!(view(&s).can_back, "Back on every page after Welcome");
        let facts = view(&s).summary;
        assert!(
            facts
                .iter()
                .any(|f| f.label == "Shows in" && f.value == "Menu bar only")
        );
        let cli = view(&s)
            .todos
            .into_iter()
            .find(|t| t.id == OnboardingTodoId::Cli)
            .unwrap();
        assert!(cli.done && cli.action.is_none(), "{cli:?}");
        assert_eq!(cli.title, "Cua CLI");
        assert_eq!(cli.note, CLI_HINT);
    }

    fn mount(state: &str, enabled: bool) -> DriveMountInput {
        DriveMountInput {
            enabled,
            state: state.into(),
            method: "fskit".into(),
            path: (state == "mounted").then(|| "/Volumes/Cua Volume".into()),
            volume_name: "Cua Volume".into(),
            detail: (state == "needs_approval")
                .then(|| "Turn on Cua Volume in File System Extensions.".into()),
            settings_url: (state == "needs_approval")
                .then(|| "x-apple.systempreferences:ext".into()),
        }
    }

    #[test]
    fn launch_at_login_is_ticked_whatever_mode_is_chosen() {
        for installer_mode in [
            None,
            Some(OnboardingMode::Client),
            Some(OnboardingMode::Host),
        ] {
            for mode in [OnboardingMode::Client, OnboardingMode::Host] {
                let mut s = initial(installer_mode, None);
                for action in [
                    OnboardingAction::Start,
                    OnboardingAction::SigninDone,
                    OnboardingAction::AgentsDone { configured: vec![] },
                    OnboardingAction::PresentationDone,
                    OnboardingAction::ModeChosen { mode },
                ] {
                    s = reduce(&s, &action);
                }
                assert_eq!(s.step, OnboardingStep::Done);
                // Ticked for both modes: an unattended host needs it, and a
                // client keeps its Spaces and agents after a restart.
                let expected = true;
                assert_eq!(s.launch_at_login, expected);
                let checkbox = view(&s).launch_at_login.unwrap();
                assert_eq!(checkbox.checked, expected);
                assert_eq!(checkbox.note, crate::login_item::NOTE_WITHOUT_VOLUME);
                s = reduce(
                    &s,
                    &OnboardingAction::LaunchAtLoginToggled { on: !expected },
                );
                assert_eq!(view(&s).launch_at_login.unwrap().checked, !expected);
                s = reduce(&s, &OnboardingAction::Back);
                s = reduce(&s, &OnboardingAction::ModeChosen { mode });
                // Choosing the mode again keeps the user's untick.
                assert_eq!(s.launch_at_login, !expected);
            }
        }
    }

    #[test]
    fn done_has_launch_at_login_ticked_by_default() {
        let mut s = initial(None, None);
        assert!(view(&s).launch_at_login.is_none(), "only on Done");
        s = reduce(&s, &OnboardingAction::LaunchAtLoginToggled { on: false });
        assert!(s.launch_at_login, "ignored before Done");
        s.step = OnboardingStep::Done;
        let cb = view(&s).launch_at_login.unwrap();
        assert_eq!(
            (cb.label.as_str(), cb.checked),
            (crate::login_item::DONE_LABEL, true)
        );
        s = reduce(&s, &OnboardingAction::LaunchAtLoginToggled { on: false });
        assert!(!view(&s).launch_at_login.unwrap().checked);
        // A state saved before the setting existed reads as ticked.
        let mut old = serde_json::to_value(initial(None, None)).unwrap();
        old.as_object_mut().unwrap().remove("launchAtLogin");
        let old: OnboardingState = serde_json::from_value(old).unwrap();
        assert!(old.launch_at_login);
    }

    fn at_drive() -> OnboardingState {
        OnboardingState {
            step: OnboardingStep::Drive,
            ..volume_on()
        }
    }

    #[test]
    fn the_drive_page_is_off_by_default_and_mounts_on_continue() {
        let s = at_drive();
        let card = view(&s).drive.unwrap();
        assert_eq!(card.label, "Add Cua Volume to Finder");
        assert!(!card.checked && !card.enabled);
        assert_eq!(card.note.as_deref(), Some("Checking\u{2026}"));
        let s = reduce(
            &s,
            &OnboardingAction::DriveChecked {
                os: SpaceOs::Macos,
                status: Some(mount("off", false)),
            },
        );
        let card = view(&s).drive.unwrap();
        assert!(
            !card.checked && card.enabled && card.note.is_none(),
            "off by default"
        );
        let s = reduce(&s, &OnboardingAction::DriveToggled { on: true });
        let s = reduce(&s, &OnboardingAction::DriveContinue);
        assert_eq!(s.drive_request, Some(DriveStepRequest::Mount));
        assert_eq!(s.step, OnboardingStep::Drive);
        assert_eq!(view(&s).primary_label, "Mounting\u{2026}");
        // macOS asks for the extension first: the page says so and stays.
        let s = reduce(
            &s,
            &OnboardingAction::DriveMounted {
                status: mount("needs_approval", true),
            },
        );
        assert_eq!(s.step, OnboardingStep::Drive);
        let card = view(&s).drive.unwrap();
        assert_eq!(card.settings_label.as_deref(), Some("Open System Settings"));
        assert!(card.note.unwrap().contains("File System Extensions"));
        // Continue again: the daemon finishes on its own once approved.
        let s = reduce(&s, &OnboardingAction::DriveContinue);
        assert_eq!(s.step, OnboardingStep::Mode);
        let s = reduce(
            &s,
            &OnboardingAction::ModeChosen {
                mode: OnboardingMode::Client,
            },
        );
        let volume = view(&s)
            .todos
            .into_iter()
            .find(|t| t.id == OnboardingTodoId::Volume)
            .unwrap();
        assert!(!volume.done);
        assert!(volume.note.ends_with("Waiting for approval."), "{volume:?}");
    }

    #[test]
    fn linux_mounts_windows_skips_and_no_daemon_is_honest() {
        let s = reduce(
            &at_drive(),
            &OnboardingAction::DriveChecked {
                os: SpaceOs::Linux,
                status: Some(DriveMountInput {
                    method: "fuse".into(),
                    state: "off".into(),
                    ..Default::default()
                }),
            },
        );
        assert_eq!(view(&s).drive.unwrap().label, "Mount Cua Volume");
        let s = reduce(
            &at_drive(),
            &OnboardingAction::DriveChecked {
                os: SpaceOs::Macos,
                status: None,
            },
        );
        let card = view(&s).drive.unwrap();
        assert!(!card.enabled);
        assert_eq!(card.note.as_deref(), Some("Not available on this Mac yet"));
        // Nothing to mount: Continue just moves on.
        assert_eq!(
            reduce(&s, &OnboardingAction::DriveContinue).step,
            OnboardingStep::Mode
        );
        // Windows: no page, no dot, no fact; Back skips it too.
        let mut w = initial(None, None);
        w.step = OnboardingStep::Presentation;
        let w = reduce(
            &w,
            &OnboardingAction::DriveChecked {
                os: SpaceOs::Windows,
                status: None,
            },
        );
        assert!(view(&w).dots.iter().all(|d| d.label != "Cua Volume"));
        let w = reduce(&w, &OnboardingAction::PresentationDone);
        assert_eq!(w.step, OnboardingStep::Mode);
        assert_eq!(
            reduce(&w, &OnboardingAction::Back).step,
            OnboardingStep::Presentation
        );
        // Unticking after an earlier mount unmounts before moving on.
        let s = reduce(
            &at_drive(),
            &OnboardingAction::DriveChecked {
                os: SpaceOs::Macos,
                status: Some(mount("mounted", true)),
            },
        );
        assert!(s.drive_mount, "follows the saved setting");
        let s = reduce(&s, &OnboardingAction::DriveToggled { on: false });
        let s = reduce(&s, &OnboardingAction::DriveContinue);
        assert_eq!(s.drive_request, Some(DriveStepRequest::Unmount));
        let s = reduce(
            &s,
            &OnboardingAction::DriveMounted {
                status: mount("off", false),
            },
        );
        assert_eq!(s.step, OnboardingStep::Mode);
    }

    fn fs_storage() -> DriveStorageInput {
        DriveStorageInput {
            backend: "fs".into(),
            fs_path: "/Users/maya/.cua/volume/data".into(),
            ..Default::default()
        }
    }

    fn ready_drive() -> OnboardingState {
        let s = reduce(
            &at_drive(),
            &OnboardingAction::DriveChecked {
                os: SpaceOs::Macos,
                status: Some(mount("off", false)),
            },
        );
        reduce(
            &s,
            &OnboardingAction::DriveStorageLoaded {
                storage: fs_storage(),
                home: Some("/Users/maya".into()),
            },
        )
    }

    #[test]
    fn this_mac_is_the_default_and_needs_no_setup() {
        let s = ready_drive();
        let card = view(&s).drive.unwrap();
        let active: Vec<_> = card
            .storage_options
            .iter()
            .map(|o| (o.label.as_str(), o.active))
            .collect();
        assert_eq!(
            active,
            [
                ("This Mac", true),
                ("Your S3 bucket", false),
                ("Set up later", false)
            ]
        );
        assert!(card.storage_rows.is_empty() && card.can_continue);
        assert_eq!(
            card.stored_in.as_deref(),
            Some("Stored in ~/.cua/volume/data")
        );
        assert!(card.mounted_at.is_none(), "not mounted");
        let s = reduce(&s, &OnboardingAction::DriveContinue);
        assert!(s.storage.request.is_none(), "nothing to save");
        assert_eq!(s.step, OnboardingStep::Mode);
    }

    #[test]
    fn a_bucket_is_filled_in_tested_and_saved_before_mounting() {
        use crate::drive_settings::StorageField;
        let mut s = reduce(
            &ready_drive(),
            &OnboardingAction::StorageChosen {
                choice: StorageChoice::S3,
            },
        );
        let card = view(&s).drive.unwrap();
        assert!(!card.can_continue, "needs a bucket and keys");
        assert!(card.storage_rows.iter().any(|r| r.id == "s3-prompt"));
        s = reduce(
            &s,
            &OnboardingAction::DriveStorage {
                action: StorageAction::ShowManual { on: true },
            },
        );
        let card = view(&s).drive.unwrap();
        assert!(card.storage_rows.iter().any(|r| r.id == "s3-secret"));
        for (field, value) in [
            (StorageField::Endpoint, "http://127.0.0.1:9000"),
            (StorageField::Bucket, "cua-volume"),
            (StorageField::AccessKeyId, "maya-drive"),
            (StorageField::SecretAccessKey, "fixture-secret"),
        ] {
            s = reduce(
                &s,
                &OnboardingAction::DriveStorage {
                    action: StorageAction::SetField {
                        field,
                        value: value.into(),
                    },
                },
            );
        }
        s = reduce(
            &s,
            &OnboardingAction::DriveStorage {
                action: StorageAction::Test,
            },
        );
        assert!(matches!(
            s.storage.request,
            Some(StorageRequest::Test { .. })
        ));
        let ok = DriveCheckInput {
            ok: true,
            reachable: true,
            authorized: true,
            versioning: true,
            ..Default::default()
        };
        s = reduce(
            &s,
            &OnboardingAction::DriveStorage {
                action: StorageAction::Checked { check: ok.clone() },
            },
        );
        s = reduce(&s, &OnboardingAction::DriveToggled { on: true });
        s = reduce(&s, &OnboardingAction::DriveContinue);
        let Some(StorageRequest::Save { update }) = &s.storage.request else {
            panic!("{:?}", s.storage.request)
        };
        assert_eq!(update.backend, "s3");
        assert_eq!(update.secret_access_key.as_deref(), Some("fixture-secret"));
        assert_eq!(view(&s).primary_label, "Saving\u{2026}");
        s = reduce(
            &s,
            &OnboardingAction::DriveStorageSaved {
                check: DriveCheckInput {
                    applied: true,
                    ..ok
                },
            },
        );
        assert!(
            s.storage.form.secret_access_key.is_empty(),
            "the form forgets the keys"
        );
        assert_eq!(
            s.drive_request,
            Some(DriveStepRequest::Mount),
            "then the mount"
        );
        assert_eq!(s.drive_storage.as_ref().unwrap().backend, "s3");
    }

    #[test]
    fn a_failed_save_stays_and_later_skips_storage() {
        let mut s = reduce(
            &ready_drive(),
            &OnboardingAction::StorageChosen {
                choice: StorageChoice::Later,
            },
        );
        let card = view(&s).drive.unwrap();
        assert!(card.storage_note.is_some() && card.can_continue);
        s = reduce(&s, &OnboardingAction::DriveContinue);
        assert_eq!(s.step, OnboardingStep::Mode);
        // A saved bucket shows as chosen; a failed save keeps the page.
        let s3 = DriveStorageInput {
            backend: "s3".into(),
            has_keys: true,
            s3: Some(crate::drive_settings::DriveS3Input {
                bucket: "cua-volume".into(),
                ..Default::default()
            }),
            ..fs_storage()
        };
        let s = reduce(
            &ready_drive(),
            &OnboardingAction::DriveStorageLoaded {
                storage: s3,
                home: None,
            },
        );
        assert_eq!(s.storage_choice, StorageChoice::S3);
        assert!(view(&s).drive.unwrap().can_continue, "saved keys count");
        let s = reduce(
            &s,
            &OnboardingAction::StorageChosen {
                choice: StorageChoice::Local,
            },
        );
        let s = reduce(&s, &OnboardingAction::DriveContinue);
        let Some(StorageRequest::Save { update }) = &s.storage.request else {
            panic!()
        };
        assert_eq!(update.backend, "fs");
        let s = reduce(
            &s,
            &OnboardingAction::DriveStorageSaved {
                check: DriveCheckInput {
                    detail: Some("The daemon could not switch.".into()),
                    ..Default::default()
                },
            },
        );
        assert_eq!(s.step, OnboardingStep::Drive);
    }

    #[test]
    fn the_agent_prompt_connects_on_its_own() {
        let mut s = reduce(
            &ready_drive(),
            &OnboardingAction::StorageChosen {
                choice: StorageChoice::S3,
            },
        );
        assert!(!view(&s).drive.unwrap().can_continue);
        let s3 = DriveStorageInput {
            backend: "s3".into(),
            has_keys: true,
            s3: Some(crate::drive_settings::DriveS3Input {
                bucket: "maya-cua-volume".into(),
                ..Default::default()
            }),
            ..fs_storage()
        };
        s = reduce(
            &s,
            &OnboardingAction::DriveStorageLoaded {
                storage: s3,
                home: None,
            },
        );
        assert!(matches!(
            s.storage.request,
            Some(StorageRequest::Adopt { .. })
        ));
        assert!(!view(&s).drive.unwrap().can_continue, "connecting");
        s = reduce(
            &s,
            &OnboardingAction::DriveStorage {
                action: StorageAction::Adopted {
                    check: DriveCheckInput {
                        ok: true,
                        reachable: true,
                        authorized: true,
                        versioning: true,
                        applied: true,
                        detail: None,
                    },
                },
            },
        );
        let card = view(&s).drive.unwrap();
        assert!(card.can_continue);
        assert!(
            card.storage_rows
                .iter()
                .any(|r| r.value.as_deref() == Some("Connected, versioning on"))
        );
        s = reduce(&s, &OnboardingAction::DriveContinue);
        assert!(s.storage.request.is_none(), "nothing left to save");
        assert_eq!(s.step, OnboardingStep::Mode);
    }
}
