// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The CI Runners page: GitHub Actions jobs on your own computers. Every
//! app draws it from [`view`], so the words, the numbers' formats, the tabs,
//! the setup checklist and the connect steps are the core's.
//!
//! The data is `cua ci overview` (`cua-ci`'s `overview` module, snake_case
//! JSON): this module mirrors only the fields it reads, so it builds for
//! wasm and a newer overview with more fields still reads. What changes on
//! the computer (connecting GitHub, choosing repositories, the goal) is done
//! by the host running `cua ci app <op>`; this core only decides what the
//! page asks for and says.
//!
//! One page, one screen of state ([`State`]): the tab, the expanded job, the
//! repository picker with its search, the open help topic and the connect
//! sheet. [`reduce`] moves it, [`view`] draws it.

use serde::{Deserialize, Serialize};

/// The page's title.
pub const TITLE: &str = "CI Runners";

/// The line under the title.
pub const INTRO: &str = "Run your GitHub Actions jobs on your own computers. Each job gets a fresh Space that is deleted afterwards.";

/// The workflow line that sends a job here.
pub const RUNS_ON: &str = "runs-on: cua";

const DAY_MS: u64 = 86_400_000;

// ---------------------------------------------------------------------
// The overview, as `cua ci overview` prints it (only what is read)
// ---------------------------------------------------------------------

/// One step of the setup checklist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SetupStep {
    /// `connect_github`, `repositories`, `runner_label`, `first_job_passed`.
    pub id: String,
    /// `done`, `todo` or `unknown`.
    pub state: String,
    /// One line of detail.
    pub detail: String,
}

/// The GitHub connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Github {
    /// A connection exists.
    pub connected: bool,
    /// `hosted`, `own` or `none`.
    pub mode: String,
    /// The organization or repository served.
    pub login: String,
    /// Where repositories are chosen on GitHub.
    pub choose_url: String,
    /// A reading of the service failed.
    pub note: String,
}

/// A repository that ran jobs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoUse {
    /// `owner/name`.
    pub name: String,
    /// Jobs in the last 7 days.
    pub jobs_7d: u32,
}

/// A repository the connection can see.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoChoice {
    /// `owner/name`.
    pub name: String,
    /// Jobs of it are served.
    pub allowed: bool,
}

/// The repositories.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Repositories {
    /// Repositories that ran jobs lately.
    pub used: Vec<RepoUse>,
    /// Every repository the connection can see (hosted), when read.
    pub all: Option<Vec<RepoChoice>>,
}

/// The overview line's numbers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Jobs started in the last 24 hours.
    pub starts_24h: u32,
    /// Jobs finished in the last 7 days.
    pub jobs_7d: u32,
    /// Share that passed, 0 to 1.
    pub pass_rate_7d: Option<f64>,
    /// Micro-dollars over 7 days.
    pub spend_7d_micros: u64,
}

/// One job.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Job {
    /// `<run>/<job>`.
    pub job_key: String,
    /// The job's name.
    pub name: String,
    /// `owner/name`.
    pub repo: String,
    /// `queued`, `running`, `passed`, `failed`.
    pub state: String,
    /// Unix ms queued.
    pub queued_ms: u64,
    /// Waited for a computer, ms.
    pub wait_ms: Option<u64>,
    /// Ran, ms.
    pub run_ms: Option<u64>,
    /// Micro-dollars.
    pub cost_micros: Option<u64>,
    /// Micro-dollars with idle time.
    pub all_in_micros: Option<u64>,
    /// `warm_pool`, `fork`, `cold_create`.
    pub start_mode: String,
    /// The computer's name.
    pub machine: String,
    /// The computer type.
    pub machine_type: String,
    /// The cloud.
    pub cloud: String,
    /// Spot.
    pub spot: bool,
    /// The pool whose events hold the job's log.
    pub log_pool: String,
    /// The slot of the job.
    pub log_slot: String,
}

/// The autopilot's plan for a kind of job.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PilotClass {
    /// `macos` or `linux`.
    pub class: String,
    /// `learning` or `steady`.
    pub phase: String,
    /// Jobs learned from.
    pub jobs_seen: usize,
    /// Computers kept ready now.
    pub warm_now: u32,
    /// Seconds an idle computer is kept.
    pub idle_ttl_secs: u64,
    /// Spot.
    pub spot: bool,
    /// The decisions, in words.
    pub reasons: Vec<String>,
}

/// The autopilot of a target.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pilot {
    /// The plans.
    pub classes: Vec<PilotClass>,
}

/// A target and its goal.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Target {
    /// `aws`, `hetzner`, ...
    pub target: String,
    /// `cheapest`, `balanced`, `fastest`.
    pub goal: String,
    /// A fixed reserve.
    pub warm: Option<u32>,
    /// The autopilot.
    pub autopilot: Option<Pilot>,
}

/// The budget.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    /// Micro-dollars committed this month.
    pub month_committed_micros: u64,
    /// The monthly budget, micro-dollars.
    pub monthly_micros: u64,
    /// `ok`, `warn`, `exhausted`.
    pub state: String,
}

/// `cua ci overview`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overview {
    /// Unix ms the overview was built.
    pub generated_ms: u64,
    /// The checklist.
    pub setup: Vec<SetupStep>,
    /// The GitHub connection.
    pub github: Github,
    /// The repositories.
    pub repositories: Repositories,
    /// The overview line.
    pub usage: Usage,
    /// The jobs.
    pub jobs: Vec<Job>,
    /// The targets.
    pub targets: Vec<Target>,
    /// The budget.
    pub budget: Budget,
}

// ---------------------------------------------------------------------
// The page's state
// ---------------------------------------------------------------------

/// The jobs table's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tab {
    /// Every job.
    #[default]
    All,
    /// Queued and running.
    Running,
    /// Failed.
    Failed,
}

/// Where the connect sheet is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Not open.
    #[default]
    Closed,
    /// The two ways to connect.
    Choose,
    /// Waiting for the person to finish on GitHub.
    Waiting,
    /// The token form.
    Token,
}

/// The connect sheet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Connect {
    /// Where it is.
    pub phase: Phase,
    /// Something is running (the sheet's button spins).
    pub busy: bool,
    /// The GitHub page to finish on, once there is one.
    pub url: Option<String>,
    /// The organization typed in the token form.
    pub org: String,
    /// Why the last try did not work.
    pub error: Option<String>,
}

/// What the page remembers while it is open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    /// The jobs table's tab.
    pub tab: Tab,
    /// The job whose detail is open.
    pub expanded: Option<String>,
    /// The repository picker is open.
    pub picker: bool,
    /// The picker's search.
    pub query: String,
    /// The open help topic.
    pub help: Option<String>,
    /// The connect sheet.
    pub connect: Connect,
}

/// What `cua ci app connect` answered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectAnswer {
    /// `waiting`, `connected`, `sign_in`, `failed`.
    pub state: String,
    /// The GitHub page to open.
    pub url: Option<String>,
    /// A sentence, when it failed.
    pub message: Option<String>,
}

/// Something the person or the host did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    /// A tab was chosen.
    Tab {
        /// Which.
        tab: Tab,
    },
    /// A job's row was pressed (opens it, or closes it when open).
    Expand {
        /// Its `job_key`.
        key: String,
    },
    /// The repository picker opened or closed.
    Picker {
        /// Open.
        open: bool,
    },
    /// The search changed.
    Query {
        /// The text.
        value: String,
    },
    /// A help topic was pressed (opens it, or closes it when open).
    Help {
        /// Its id.
        id: String,
    },
    /// "Connect GitHub" was pressed.
    ConnectOpen,
    /// The sheet was closed or cancelled.
    ConnectClose,
    /// "Continue on GitHub" was pressed: the host asks `cua ci app connect`.
    ConnectStart,
    /// `cua ci app connect` answered.
    ConnectAnswer {
        /// The answer.
        answer: ConnectAnswer,
    },
    /// "Use a token instead".
    UseToken,
    /// Back from the token form.
    TokenBack,
    /// The organization field changed.
    TokenOrg {
        /// The text.
        value: String,
    },
    /// "Connect" in the token form was pressed.
    TokenSubmit,
    /// The token connect finished.
    TokenDone {
        /// It worked.
        ok: bool,
        /// A sentence, when it did not.
        message: Option<String>,
    },
}

/// Moves the state.
pub fn reduce(state: &State, action: &Action) -> State {
    let mut s = state.clone();
    match action {
        Action::Tab { tab } => {
            s.tab = *tab;
            s.expanded = None;
        }
        Action::Expand { key } => {
            s.expanded = if s.expanded.as_deref() == Some(key) {
                None
            } else {
                Some(key.clone())
            };
        }
        Action::Picker { open } => {
            s.picker = *open;
            if !open {
                s.query.clear();
            }
        }
        Action::Query { value } => s.query = value.clone(),
        Action::Help { id } => {
            s.help = if s.help.as_deref() == Some(id) {
                None
            } else {
                Some(id.clone())
            };
        }
        Action::ConnectOpen => {
            s.connect = Connect {
                phase: Phase::Choose,
                org: s.connect.org.clone(),
                ..Connect::default()
            };
        }
        Action::ConnectClose => {
            s.connect = Connect {
                org: s.connect.org.clone(),
                ..Connect::default()
            };
        }
        Action::ConnectStart => {
            s.connect.busy = true;
            s.connect.error = None;
        }
        Action::ConnectAnswer { answer } => match answer.state.as_str() {
            "waiting" => {
                s.connect.phase = Phase::Waiting;
                s.connect.busy = false;
                s.connect.error = None;
                s.connect.url = answer.url.clone().or(s.connect.url.clone());
            }
            "connected" => s.connect = Connect::default(),
            "sign_in" => {
                s.connect.phase = Phase::Choose;
                s.connect.busy = false;
                s.connect.error = Some("Sign in to Cua first, then connect GitHub.".into());
            }
            _ => {
                s.connect.phase = Phase::Choose;
                s.connect.busy = false;
                s.connect.url = None;
                s.connect.error = Some(
                    answer
                        .message
                        .clone()
                        .unwrap_or_else(|| "GitHub could not be connected.".into()),
                );
            }
        },
        Action::UseToken => {
            s.connect.phase = Phase::Token;
            s.connect.busy = false;
            s.connect.error = None;
        }
        Action::TokenBack => {
            s.connect.phase = Phase::Choose;
            s.connect.busy = false;
            s.connect.error = None;
        }
        Action::TokenOrg { value } => s.connect.org = value.clone(),
        Action::TokenSubmit => {
            s.connect.busy = true;
            s.connect.error = None;
        }
        Action::TokenDone { ok, message } => {
            if *ok {
                s.connect = Connect::default();
            } else {
                s.connect.busy = false;
                s.connect.error = Some(
                    message
                        .clone()
                        .unwrap_or_else(|| "GitHub did not accept the token.".into()),
                );
            }
        }
    }
    s
}

// ---------------------------------------------------------------------
// Formats
// ---------------------------------------------------------------------

/// A duration for the Wait and Run columns: `6 s`, `1 min 12 s`, `12 min`,
/// `1 h 5 min`; a dash when unknown.
pub fn duration(ms: Option<u64>) -> String {
    let Some(ms) = ms else {
        return "-".into();
    };
    let s = (ms + 500) / 1000;
    if ms < 1000 {
        return "<1 s".into();
    }
    if s < 60 {
        return format!("{s} s");
    }
    let m = s / 60;
    if m < 10 {
        let r = s % 60;
        return if r == 0 {
            format!("{m} min")
        } else {
            format!("{m} min {r} s")
        };
    }
    if m < 60 {
        return format!("{m} min");
    }
    let h = m / 60;
    let r = m % 60;
    if r == 0 {
        format!("{h} h")
    } else {
        format!("{h} h {r} min")
    }
}

/// Micro-dollars as dollars: `$0.04`, `<$0.01`, `$12.40`.
pub fn dollars(micros: u64) -> String {
    if micros == 0 {
        return "$0.00".into();
    }
    if micros < 10_000 {
        return "<$0.01".into();
    }
    let cents = (micros + 5_000) / 10_000;
    let whole = cents / 100;
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("${grouped}.{:02}", cents % 100)
}

/// A whole-dollar budget: `$50`.
fn dollars_whole(micros: u64) -> String {
    if micros.is_multiple_of(1_000_000) {
        format!("${}", micros / 1_000_000)
    } else {
        dollars(micros)
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn civil(days: u64) -> (u64, u64) {
    // Days since 1970-01-01 to (month 1..=12, day), the civil-from-days way.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let _ = era;
    (m as u64, d as u64)
}

/// When a job was queued, against `now`: `Just now`, `5 min ago`,
/// `3 hours ago`, `Yesterday`, `4 days ago`, `Oct 24`.
pub fn when(queued_ms: u64, now_ms: u64) -> String {
    if queued_ms == 0 {
        return "-".into();
    }
    let d = now_ms.saturating_sub(queued_ms);
    if d < 45_000 {
        return "Just now".into();
    }
    let min = d / 60_000;
    if min < 60 {
        return format!("{} min ago", min.max(1));
    }
    let h = d / 3_600_000;
    if h < 24 {
        return format!("{h} hour{} ago", if h == 1 { "" } else { "s" });
    }
    let days = d / DAY_MS;
    if days < 2 {
        return "Yesterday".into();
    }
    if days < 7 {
        return format!("{days} days ago");
    }
    let (m, day) = civil(queued_ms / DAY_MS);
    format!("{} {day}", MONTHS[(m - 1) as usize])
}

fn cloud_name(c: &str) -> String {
    match c {
        "aws" => "AWS".into(),
        "gcp" => "Google Cloud".into(),
        "azure" => "Azure".into(),
        "hetzner" => "Hetzner".into(),
        "modal" => "Modal".into(),
        "macstadium" => "MacStadium".into(),
        "" => String::new(),
        other => {
            let mut cs = other.chars();
            cs.next()
                .map(|f| f.to_uppercase().collect::<String>() + cs.as_str())
                .unwrap_or_default()
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

// ---------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------

/// A button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Button {
    /// What it does: `connect`, `choose-repositories`, `copy-label`.
    pub id: String,
    /// Its label.
    pub label: String,
}

/// One row of the setup checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRow {
    /// The step.
    pub id: String,
    /// Its title.
    pub title: String,
    /// One line under it.
    pub detail: String,
    /// `done`, `todo` or `pending` (the service has not said yet).
    pub state: String,
    /// The button that does it.
    pub action: Option<Button>,
}

/// The setup checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupView {
    /// Its heading.
    pub title: String,
    /// The rows.
    pub rows: Vec<SetupRow>,
    /// "2 of 4".
    pub progress: String,
}

/// One number of the overview line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    /// What it counts.
    pub label: String,
    /// The number.
    pub value: String,
    /// `warn` or `bad` for a budget near or at its limit.
    pub tone: Option<String>,
}

/// A tab of the jobs table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabView {
    /// The tab.
    pub id: Tab,
    /// Its label.
    pub label: String,
    /// How many jobs.
    pub count: usize,
    /// It is the open one.
    pub selected: bool,
}

/// One fact of a job's detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    /// What.
    pub label: String,
    /// Its value.
    pub value: String,
}

/// The bar of a job's timeline: how long it waited and how long it ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    /// Percent of the bar that is the wait (0 to 100).
    pub wait_pct: u32,
    /// Percent that is the run.
    pub run_pct: u32,
    /// "Waited for a computer 6 s".
    pub wait_label: String,
    /// "Ran 1 min 12 s".
    pub run_label: String,
}

/// Where to find a job's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRef {
    /// `run/job`.
    pub job_key: String,
    /// The pool.
    pub pool: String,
    /// The slot.
    pub slot: String,
}

/// An open job's detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDetail {
    /// The timeline, once the job has a wait or a run.
    pub timeline: Option<Timeline>,
    /// Cost, computer, cloud, how it started.
    pub facts: Vec<Fact>,
    /// "Open on GitHub".
    pub open_label: String,
    /// The job's page at GitHub, when its repository is known.
    pub open_url: Option<String>,
    /// "Show log".
    pub log_label: String,
    /// What the log is of.
    pub log: LogRef,
}

/// A row of the jobs table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRowView {
    /// `run/job`.
    pub key: String,
    /// The job's name.
    pub title: String,
    /// Its repository, second line.
    pub repo: String,
    /// `queued`, `running`, `passed`, `failed`.
    pub state: String,
    /// `Queued`, `Running`, `Passed`, `Failed`.
    pub state_label: String,
    /// The Wait column.
    pub wait: String,
    /// The Run column.
    pub run: String,
    /// The Cost column.
    pub cost: String,
    /// The When column.
    pub when: String,
    /// Its detail is open.
    pub expanded: bool,
    /// The detail, when open.
    pub detail: Option<JobDetail>,
}

/// The jobs table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsView {
    /// Its heading.
    pub title: String,
    /// The tabs.
    pub tabs: Vec<TabView>,
    /// Column headings: job, wait, run, cost, when.
    pub columns: Vec<String>,
    /// The rows of the open tab.
    pub rows: Vec<JobRowView>,
    /// What to say with no rows.
    pub empty: String,
}

/// One repository in the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRowView {
    /// `owner/name`.
    pub name: String,
    /// `Ready` or `Not allowed`.
    pub status: String,
    /// Jobs line: `12 jobs this week`.
    pub jobs: String,
    /// It may run jobs.
    pub allowed: bool,
    /// The switch can change it (the hosted app's list).
    pub can_toggle: bool,
}

/// The repository picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerView {
    /// Its heading.
    pub title: String,
    /// "3 of 12 repositories can run jobs".
    pub summary: String,
    /// The search field's placeholder.
    pub search_label: String,
    /// The rows that match.
    pub rows: Vec<RepoRowView>,
    /// What to say with no rows.
    pub empty: String,
    /// "Choose on GitHub".
    pub choose_label: String,
    /// Where.
    pub choose_url: Option<String>,
    /// "Connect GitHub account" (also when connected: another account).
    pub connect_label: String,
}

/// One goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalOption {
    /// `cheapest`, `balanced`, `fastest`.
    pub id: String,
    /// Its label.
    pub label: String,
    /// Selected.
    pub selected: bool,
}

/// One kind of job's plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PilotLine {
    /// `macOS` or `Linux`.
    pub label: String,
    /// What it decided.
    pub line: String,
}

/// A cloud's autopilot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutopilotView {
    /// The target (`aws`).
    pub target: String,
    /// `AWS`.
    pub title: String,
    /// The goals.
    pub goals: Vec<GoalOption>,
    /// The words under the goals.
    pub goal_note: String,
    /// The budget used: `$12.40 of $50 this month`.
    pub budget: String,
    /// `warn` or `bad`.
    pub budget_tone: Option<String>,
    /// What it decided per kind of job.
    pub plans: Vec<PilotLine>,
    /// Why, in sentences.
    pub reasons: Vec<String>,
}

/// One help topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelpTopic {
    /// Its id.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Its text (open only).
    pub body: Option<String>,
    /// It is the open one.
    pub open: bool,
}

/// The connect sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectView {
    /// `choose`, `waiting` or `token`.
    pub phase: String,
    /// Its title.
    pub title: String,
    /// What it says.
    pub body: String,
    /// The main button.
    pub primary: String,
    /// Something is running.
    pub busy: bool,
    /// The second way, a text button.
    pub secondary: Option<String>,
    /// Cancel or Back.
    pub close: String,
    /// The GitHub page to open (waiting).
    pub url: Option<String>,
    /// Why the last try did not work.
    pub error: Option<String>,
    /// The token form.
    pub token: Option<TokenForm>,
}

/// The token form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenForm {
    /// The organization field's label.
    pub org_label: String,
    /// Its placeholder.
    pub org_placeholder: String,
    /// What was typed.
    pub org: String,
    /// The token field's label.
    pub token_label: String,
    /// What the token needs.
    pub token_help: String,
    /// "Create a token on GitHub".
    pub create_label: String,
    /// The GitHub page that makes it.
    pub create_url: String,
    /// The primary button can be pressed (an organization is typed).
    pub can_submit: bool,
}

/// The whole page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CiView {
    /// The title.
    pub title: String,
    /// The line under it.
    pub intro: String,
    /// "Connect GitHub", when nothing is connected and no checklist shows.
    pub connect_button: Option<Button>,
    /// The checklist, until setup is complete.
    pub setup: Option<SetupView>,
    /// The overview line.
    pub stats: Vec<Stat>,
    /// The jobs.
    pub jobs: JobsView,
    /// The repository picker's entry or its body.
    pub repositories: PickerView,
    /// The repository picker is open.
    pub picker_open: bool,
    /// The heading of the sizing section.
    pub sizing_title: String,
    /// The label of the row that names the cloud.
    pub cloud_label: String,
    /// The clouds' sizing: goal, budget and reasons.
    pub sizing: Vec<AutopilotView>,
    /// Said when no cloud is added.
    pub no_cloud: String,
    /// The help.
    pub help_title: String,
    /// The topics.
    pub help: Vec<HelpTopic>,
    /// The connect sheet, when open.
    pub connect: Option<ConnectView>,
    /// A quiet note (the service could not be read).
    pub note: Option<String>,
    /// The line to copy into a workflow.
    pub runs_on: String,
}

/// The GitHub page that makes a token with the scopes Cua CI needs.
pub const TOKEN_URL: &str =
    "https://github.com/settings/tokens/new?scopes=repo,admin:org&description=Cua%20CI";

/// The help topics.
const HELP: &[(&str, &str, &str)] = &[
    (
        "queued",
        "A job stays queued",
        "Check that the workflow says runs-on: cua, that its repository is ready here, and that a computer is online. A job waits at GitHub until a computer takes it, for up to 24 hours. A Mac runs at most two macOS jobs at a time, so a third waits for one to end.",
    ),
    (
        "public",
        "Public repositories",
        "Anyone can open a pull request from a fork of a public repository, and it can change the workflow it runs. Cua serves a fork's pull request without secrets, with read-only caches and off your private network. In the repository, require approval for outside contributors under Settings, Actions, General.",
    ),
    (
        "env",
        "A variable or secret is missing",
        "A job gets the variables its workflow sets and the secrets GitHub sends it. GitHub sends a fork's pull request no secrets. Nothing from this computer's own environment is passed into the job's Space.",
    ),
    (
        "slow",
        "A job starts slowly",
        "A Space takes a few seconds to start on Linux and longer for a Mac, unless one is kept ready. The Wait column shows the time each job waited.",
    ),
    (
        "tool",
        "A tool is missing",
        "Each job starts from a fresh copy of an image. Install what the job needs in a step, or save an image with the tool in it and use that for your jobs.",
    ),
    (
        "debug",
        "A job fails and you want to see why",
        "Open the job here for what Cua saw: the Space starting, the runner and the end of the job. Open on GitHub shows the output of each step.",
    ),
];

fn state_label(state: &str) -> &'static str {
    match state {
        "queued" => "Queued",
        "running" => "Running",
        "passed" => "Passed",
        "failed" => "Failed",
        _ => "",
    }
}

fn job_title(j: &Job) -> String {
    if j.name.is_empty() {
        format!("Job {}", j.job_key)
    } else {
        j.name.clone()
    }
}

fn open_url(j: &Job) -> Option<String> {
    let (run, job) = j.job_key.split_once('/')?;
    if j.repo.is_empty() || run.is_empty() || job.is_empty() {
        return None;
    }
    if !j.repo.contains('/') {
        return None;
    }
    Some(format!(
        "https://github.com/{}/actions/runs/{run}/job/{job}",
        j.repo
    ))
}

fn detail(j: &Job) -> JobDetail {
    let timeline = match (j.wait_ms, j.run_ms) {
        (None, None) => None,
        (w, r) => {
            let w = w.unwrap_or(0);
            let r = r.unwrap_or(0);
            let total = (w + r).max(1);
            let wait_pct = ((w * 100 + total / 2) / total) as u32;
            let run_pct = if r == 0 { 0 } else { 100 - wait_pct };
            Some(Timeline {
                wait_pct: if r == 0 { 100 } else { wait_pct },
                run_pct,
                wait_label: format!("Waited for a computer {}", duration(j.wait_ms)),
                run_label: if j.run_ms.is_some() {
                    format!("Ran {}", duration(j.run_ms))
                } else {
                    "Still running".into()
                },
            })
        }
    };
    let mut facts = Vec::new();
    let finished = matches!(j.state.as_str(), "passed" | "failed");
    let cost = if finished && j.cloud.is_empty() {
        "Free, it ran on your own computer".to_string()
    } else if let Some(c) = j.cost_micros {
        let mut v = dollars(c);
        if let Some(a) = j.all_in_micros.filter(|a| *a > c) {
            v.push_str(&format!(", {} with idle time", dollars(a)));
        }
        v
    } else {
        "Not known yet".into()
    };
    facts.push(Fact {
        label: "Cost".into(),
        value: cost,
    });
    let machine = if j.machine.is_empty() {
        "Your own computer".to_string()
    } else if j.machine_type.is_empty() {
        j.machine.clone()
    } else {
        format!("{}, {}", j.machine, j.machine_type)
    };
    facts.push(Fact {
        label: "Computer".into(),
        value: machine,
    });
    if !j.cloud.is_empty() {
        let mut c = cloud_name(&j.cloud);
        if j.spot {
            c.push_str(", spot");
        }
        facts.push(Fact {
            label: "Cloud".into(),
            value: c,
        });
    }
    let start = match j.start_mode.as_str() {
        "warm_pool" => Some("A Space that was already running"),
        "fork" => Some("A copy of a saved Space"),
        "cold_create" => Some("A new Space"),
        _ => None,
    };
    if let Some(s) = start {
        facts.push(Fact {
            label: "Started from".into(),
            value: s.into(),
        });
    }
    JobDetail {
        timeline,
        facts,
        open_label: "Open on GitHub".into(),
        open_url: open_url(j),
        log_label: "Show log".into(),
        log: LogRef {
            job_key: j.job_key.clone(),
            pool: if j.log_pool.is_empty() {
                "hosted".into()
            } else {
                j.log_pool.clone()
            },
            slot: j.log_slot.clone(),
        },
    }
}

fn in_tab(j: &Job, tab: Tab) -> bool {
    match tab {
        Tab::All => true,
        Tab::Running => matches!(j.state.as_str(), "queued" | "running"),
        Tab::Failed => j.state == "failed",
    }
}

fn setup_view(o: &Overview) -> Option<SetupView> {
    if o.setup.is_empty() || o.setup.iter().all(|s| s.state == "done") {
        return None;
    }
    let connected = o.github.connected;
    let title_of = |id: &str| match id {
        "connect_github" => "Connect GitHub",
        "repositories" => "Give access to repositories",
        "runner_label" => "Use the runner label in a workflow",
        "first_job_passed" => "A job passed",
        _ => "",
    };
    let mut rows = Vec::new();
    for s in &o.setup {
        let state = match s.state.as_str() {
            "done" => "done",
            "unknown" => "pending",
            _ => "todo",
        };
        let detail = match (s.id.as_str(), state) {
            ("connect_github", "done") if !o.github.login.is_empty() => {
                format!("Connected to {}", o.github.login)
            }
            ("connect_github", "todo") => {
                "Sign in to GitHub and choose where Cua may run jobs".into()
            }
            ("runner_label", "todo") => format!("Add {RUNS_ON} to a job in a workflow"),
            ("first_job_passed", "todo") => {
                "A job of yours has not passed on a Cua computer yet".into()
            }
            _ => s.detail.clone(),
        };
        let action = match (s.id.as_str(), state) {
            ("connect_github", "todo") => Some(Button {
                id: "connect".into(),
                label: "Connect GitHub".into(),
            }),
            ("repositories", "todo" | "pending") if connected => Some(Button {
                id: "choose-repositories".into(),
                label: "Choose repositories".into(),
            }),
            ("runner_label", "todo") if connected => Some(Button {
                id: "copy-label".into(),
                label: "Copy the line".into(),
            }),
            _ => None,
        };
        rows.push(SetupRow {
            id: s.id.clone(),
            title: title_of(&s.id).to_string(),
            detail,
            state: state.into(),
            action,
        });
    }
    let done = rows.iter().filter(|r| r.state == "done").count();
    Some(SetupView {
        title: "Set up".into(),
        progress: format!("{done} of {}", rows.len()),
        rows,
    })
}

fn stats(o: &Overview) -> Vec<Stat> {
    let mut v = vec![
        Stat {
            label: "Starts in 24 hours".into(),
            value: o.usage.starts_24h.to_string(),
            tone: None,
        },
        Stat {
            label: "Jobs in 7 days".into(),
            value: o.usage.jobs_7d.to_string(),
            tone: None,
        },
        Stat {
            label: "Passed".into(),
            value: o
                .usage
                .pass_rate_7d
                .map(|r| format!("{}%", (r * 100.0).round() as u32))
                .unwrap_or_else(|| "-".into()),
            tone: None,
        },
        Stat {
            label: "Spent in 7 days".into(),
            value: dollars(o.usage.spend_7d_micros),
            tone: None,
        },
    ];
    if o.budget.monthly_micros > 0 {
        v.push(Stat {
            label: "Budget this month".into(),
            value: format!(
                "{} of {}",
                dollars(o.budget.month_committed_micros),
                dollars_whole(o.budget.monthly_micros)
            ),
            tone: match o.budget.state.as_str() {
                "warn" => Some("warn".into()),
                "exhausted" => Some("bad".into()),
                _ => None,
            },
        });
    }
    v
}

fn picker(o: &Overview, query: &str) -> PickerView {
    let q = query.trim().to_lowercase();
    let used = |name: &str| -> u32 {
        o.repositories
            .used
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
            .map_or(0, |r| r.jobs_7d)
    };
    let jobs_line = |n: u32| match n {
        0 => "No jobs this week".to_string(),
        1 => "1 job this week".to_string(),
        n => format!("{n} jobs this week"),
    };
    let mut rows: Vec<RepoRowView> = match &o.repositories.all {
        Some(all) => all
            .iter()
            .map(|r| RepoRowView {
                name: r.name.clone(),
                status: if r.allowed { "Ready" } else { "Not allowed" }.into(),
                jobs: jobs_line(used(&r.name)),
                allowed: r.allowed,
                can_toggle: true,
            })
            .collect(),
        None => o
            .repositories
            .used
            .iter()
            .map(|r| RepoRowView {
                name: r.name.clone(),
                status: "Ready".into(),
                jobs: jobs_line(r.jobs_7d),
                allowed: true,
                can_toggle: false,
            })
            .collect(),
    };
    rows.retain(|r| q.is_empty() || r.name.to_lowercase().contains(&q));
    let summary = match &o.repositories.all {
        Some(all) => {
            let n = all.iter().filter(|r| r.allowed).count();
            format!(
                "{} of {} can run jobs",
                n,
                plural(all.len(), "repository", "repositories")
            )
        }
        None if o.repositories.used.is_empty() => "No repository has run a job yet".into(),
        None => format!(
            "{} ran jobs this week",
            plural(o.repositories.used.len(), "repository", "repositories")
        ),
    };
    PickerView {
        title: "Repositories".into(),
        summary,
        search_label: "Search repositories".into(),
        empty: if q.is_empty() {
            "No repositories to show yet".into()
        } else {
            "No repository matches".into()
        },
        rows,
        choose_label: "Choose on GitHub".into(),
        choose_url: Some(o.github.choose_url.clone()).filter(|u| !u.is_empty()),
        connect_label: "Connect GitHub account".into(),
    }
}

fn goal_note(goal: &str) -> &'static str {
    match goal {
        "cheapest" => {
            "Keeps the bill at about what the jobs cost. A job may wait for a computer to start."
        }
        "fastest" => "Keeps more computers ready for the shortest starts, inside the budget.",
        _ => "Keeps computers ready when they pay for themselves, so most jobs start at once.",
    }
}

fn autopilot_views(o: &Overview) -> Vec<AutopilotView> {
    let budget = if o.budget.monthly_micros > 0 {
        format!(
            "{} of {} used this month",
            dollars(o.budget.month_committed_micros),
            dollars_whole(o.budget.monthly_micros)
        )
    } else {
        "No monthly budget set".into()
    };
    let tone = match o.budget.state.as_str() {
        "warn" => Some("warn".to_string()),
        "exhausted" => Some("bad".to_string()),
        _ => None,
    };
    o.targets
        .iter()
        .map(|t| {
            let goal = if t.goal.is_empty() {
                "balanced"
            } else {
                &t.goal
            };
            let classes = t
                .autopilot
                .as_ref()
                .map(|a| a.classes.as_slice())
                .unwrap_or(&[]);
            let mut plans = Vec::new();
            let mut reasons: Vec<String> = Vec::new();
            for c in classes {
                let label = match c.class.as_str() {
                    "macos" => "macOS",
                    "linux" => "Linux",
                    other => other,
                }
                .to_string();
                let line = if c.phase == "learning" {
                    format!(
                        "Learning from your jobs, {} of 20 seen. Nothing is kept ready yet.",
                        c.jobs_seen.min(20)
                    )
                } else {
                    let ready = match c.warm_now {
                        0 => "No computer kept ready now".to_string(),
                        1 => "1 computer kept ready now".to_string(),
                        n => format!("{n} computers kept ready now"),
                    };
                    format!(
                        "{ready}, idle computers kept {} min, {}.",
                        c.idle_ttl_secs.div_ceil(60),
                        if c.spot {
                            "spot computers"
                        } else {
                            "on-demand computers"
                        }
                    )
                };
                plans.push(PilotLine { label, line });
                for r in &c.reasons {
                    if !reasons.contains(r) {
                        reasons.push(r.clone());
                    }
                }
            }
            if let Some(w) = t.warm {
                reasons.insert(
                    0,
                    format!(
                        "You fixed the ready reserve at {} with the command line.",
                        plural(w as usize, "computer", "computers")
                    ),
                );
            }
            AutopilotView {
                target: t.target.clone(),
                title: cloud_name(&t.target),
                goals: [
                    ("cheapest", "Cheapest"),
                    ("balanced", "Balanced"),
                    ("fastest", "Fastest"),
                ]
                .iter()
                .map(|(id, label)| GoalOption {
                    id: (*id).into(),
                    label: (*label).into(),
                    selected: *id == goal,
                })
                .collect(),
                goal_note: goal_note(goal).into(),
                budget: budget.clone(),
                budget_tone: tone.clone(),
                plans,
                reasons,
            }
        })
        .collect()
}

fn connect_view(o: &Overview, c: &Connect) -> Option<ConnectView> {
    let (phase, title, body, primary, secondary, close) = match c.phase {
        Phase::Closed => return None,
        Phase::Choose => (
            "choose",
            if o.github.connected {
                "Connect another GitHub account"
            } else {
                "Connect GitHub"
            },
            "GitHub opens in your browser. Pick the organization and the repositories Cua may run jobs for. Nothing else is needed.",
            "Continue on GitHub",
            Some("Use a token instead".to_string()),
            "Cancel",
        ),
        Phase::Waiting => (
            "waiting",
            "Finish on GitHub",
            "Pick the organization and the repositories in the page that opened. This page carries on by itself when you are done.",
            "Open GitHub again",
            None,
            "Cancel",
        ),
        Phase::Token => (
            "token",
            "Connect with a token",
            "Use this when you cannot install the Cua app on GitHub. The token stays on this computer and is never sent to Cua.",
            "Connect",
            None,
            "Back",
        ),
    };
    let token = (c.phase == Phase::Token).then(|| TokenForm {
        org_label: "GitHub organization".into(),
        org_placeholder: "acme".into(),
        org: c.org.clone(),
        token_label: "Token".into(),
        token_help: "Needs the repo and admin:org scopes. If you saved a GitHub token in Settings, Connections, leave this empty and it is used.".into(),
        create_label: "Create a token on GitHub".into(),
        create_url: TOKEN_URL.into(),
        can_submit: !c.org.trim().is_empty(),
    });
    Some(ConnectView {
        phase: phase.into(),
        title: title.into(),
        body: body.into(),
        primary: primary.into(),
        busy: c.busy,
        secondary,
        close: close.into(),
        url: c.url.clone(),
        error: c.error.clone(),
        token,
    })
}

/// Draws the page.
pub fn view(o: &Overview, state: &State) -> CiView {
    let now = o.generated_ms;
    let count = |t: Tab| o.jobs.iter().filter(|j| in_tab(j, t)).count();
    let tabs = [
        (Tab::All, "All"),
        (Tab::Running, "Running"),
        (Tab::Failed, "Failed"),
    ]
    .iter()
    .map(|(id, label)| TabView {
        id: *id,
        label: (*label).into(),
        count: count(*id),
        selected: *id == state.tab,
    })
    .collect();
    let rows: Vec<JobRowView> = o
        .jobs
        .iter()
        .filter(|j| in_tab(j, state.tab))
        .map(|j| {
            let expanded = state.expanded.as_deref() == Some(j.job_key.as_str());
            JobRowView {
                key: j.job_key.clone(),
                title: job_title(j),
                repo: if j.repo.is_empty() {
                    "Repository not recorded".into()
                } else {
                    j.repo.clone()
                },
                state: j.state.clone(),
                state_label: state_label(&j.state).into(),
                wait: duration(j.wait_ms),
                run: if j.state == "running" && j.run_ms.is_none() {
                    "Running".into()
                } else {
                    duration(j.run_ms)
                },
                cost: if matches!(j.state.as_str(), "passed" | "failed") && j.cloud.is_empty() {
                    "Free".into()
                } else {
                    j.cost_micros.map_or_else(|| "-".into(), dollars)
                },
                when: when(j.queued_ms, now),
                expanded,
                detail: expanded.then(|| detail(j)),
            }
        })
        .collect();
    let empty = match (state.tab, o.jobs.is_empty()) {
        (_, true) if !o.github.connected => {
            "Connect GitHub and run a workflow with runs-on: cua. Its jobs show here.".to_string()
        }
        (_, true) => "No jobs yet. Add runs-on: cua to a job in a workflow and push.".to_string(),
        (Tab::Running, false) => "Nothing is running".to_string(),
        (Tab::Failed, false) => "No job failed".to_string(),
        (Tab::All, false) => "No jobs".to_string(),
    };
    let setup = setup_view(o);
    let connect_button = (setup.is_none() && !o.github.connected).then(|| Button {
        id: "connect".into(),
        label: "Connect GitHub".into(),
    });
    CiView {
        title: TITLE.into(),
        intro: INTRO.into(),
        connect_button,
        setup,
        stats: stats(o),
        jobs: JobsView {
            title: "Jobs".into(),
            tabs,
            columns: ["Job", "Wait", "Run", "Cost", "When"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            rows,
            empty,
        },
        repositories: picker(o, &state.query),
        picker_open: state.picker,
        sizing_title: "Automatic sizing".into(),
        cloud_label: "Cloud".into(),
        sizing: autopilot_views(o),
        no_cloud: "Jobs run on your own computers, one fresh Space each.".into(),
        help_title: "Help".into(),
        help: HELP
            .iter()
            .map(|(id, title, body)| {
                let open = state.help.as_deref() == Some(*id);
                HelpTopic {
                    id: (*id).into(),
                    title: (*title).into(),
                    body: open.then(|| (*body).to_string()),
                    open,
                }
            })
            .collect(),
        connect: connect_view(o, &state.connect),
        note: Some(o.github.note.clone()).filter(|n| !n.is_empty()),
        runs_on: RUNS_ON.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_a_person_says_them() {
        assert_eq!(duration(None), "-");
        assert_eq!(duration(Some(400)), "<1 s");
        assert_eq!(duration(Some(6_000)), "6 s");
        assert_eq!(duration(Some(72_000)), "1 min 12 s");
        assert_eq!(duration(Some(120_000)), "2 min");
        assert_eq!(duration(Some(12 * 60_000)), "12 min");
        assert_eq!(duration(Some(65 * 60_000)), "1 h 5 min");
        assert_eq!(duration(Some(2 * 3_600_000)), "2 h");
    }

    #[test]
    fn money_is_dollars_and_cents() {
        assert_eq!(dollars(0), "$0.00");
        assert_eq!(dollars(4_000), "<$0.01");
        assert_eq!(dollars(40_000), "$0.04");
        assert_eq!(dollars(12_400_000), "$12.40");
        assert_eq!(dollars(1_234_560_000), "$1,234.56");
        assert_eq!(dollars_whole(50_000_000), "$50");
    }

    #[test]
    fn when_is_relative_then_a_date() {
        let now = 1_800_000_000_000u64;
        assert_eq!(when(0, now), "-");
        assert_eq!(when(now - 10_000, now), "Just now");
        assert_eq!(when(now - 5 * 60_000, now), "5 min ago");
        assert_eq!(when(now - 3_600_000, now), "1 hour ago");
        assert_eq!(when(now - 3 * 3_600_000, now), "3 hours ago");
        assert_eq!(when(now - 30 * 3_600_000, now), "Yesterday");
        assert_eq!(when(now - 4 * DAY_MS, now), "4 days ago");
        // 1_800_000_000_000 ms is 2027-01-15; nine days earlier is Jan 6.
        assert_eq!(when(now - 9 * DAY_MS, now), "Jan 6");
    }

    fn job(key: &str, state: &str, wait: u64, run: u64) -> Job {
        Job {
            job_key: key.into(),
            name: "build".into(),
            repo: "acme/app".into(),
            state: state.into(),
            queued_ms: 1_000,
            wait_ms: Some(wait),
            run_ms: Some(run),
            ..Job::default()
        }
    }

    #[test]
    fn the_tabs_count_and_filter_and_a_row_opens_with_its_timeline() {
        let o = Overview {
            generated_ms: 61_000,
            jobs: vec![
                job("1/1", "passed", 6_000, 72_000),
                job("1/2", "failed", 2_000, 30_000),
                Job {
                    run_ms: None,
                    ..job("1/3", "running", 1_000, 0)
                },
            ],
            ..Overview::default()
        };
        let v = view(&o, &State::default());
        assert_eq!(
            v.jobs.tabs.iter().map(|t| t.count).collect::<Vec<_>>(),
            [3, 1, 1]
        );
        let s = reduce(
            &reduce(&State::default(), &Action::Tab { tab: Tab::Failed }),
            &Action::Expand { key: "1/2".into() },
        );
        let v = view(&o, &s);
        assert_eq!(v.jobs.rows.len(), 1);
        let d = v.jobs.rows[0].detail.as_ref().unwrap();
        let t = d.timeline.as_ref().unwrap();
        assert_eq!((t.wait_pct, t.run_pct), (6, 94));
        assert_eq!(t.wait_label, "Waited for a computer 2 s");
        assert_eq!(
            d.open_url.as_deref(),
            Some("https://github.com/acme/app/actions/runs/1/job/2")
        );
        // Pressing the open row closes it; changing tab closes it too.
        assert_eq!(
            reduce(&s, &Action::Expand { key: "1/2".into() }).expanded,
            None
        );
        assert_eq!(reduce(&s, &Action::Tab { tab: Tab::All }).expanded, None);
    }

    #[test]
    fn a_job_on_your_own_machine_is_free() {
        let o = Overview {
            generated_ms: 100_000,
            jobs: vec![job("2/1", "passed", 1_000, 5_000)],
            ..Overview::default()
        };
        let v = view(
            &o,
            &State {
                expanded: Some("2/1".into()),
                ..State::default()
            },
        );
        assert_eq!(v.jobs.rows[0].cost, "Free");
        let facts = &v.jobs.rows[0].detail.as_ref().unwrap().facts;
        assert_eq!(facts[0].value, "Free, it ran on your own computer");
        assert_eq!(facts[1].value, "Your own computer");
    }

    #[test]
    fn setup_shows_until_it_is_complete_and_offers_the_next_step() {
        let step = |id: &str, state: &str| SetupStep {
            id: id.into(),
            state: state.into(),
            detail: String::new(),
        };
        let mut o = Overview {
            setup: vec![
                step("connect_github", "todo"),
                step("repositories", "todo"),
                step("runner_label", "todo"),
                step("first_job_passed", "todo"),
            ],
            ..Overview::default()
        };
        let v = view(&o, &State::default());
        let s = v.setup.unwrap();
        assert_eq!(s.progress, "0 of 4");
        assert_eq!(s.rows[0].action.as_ref().unwrap().id, "connect");
        assert!(s.rows[1].action.is_none(), "repositories wait for GitHub");
        o.github = Github {
            connected: true,
            mode: "hosted".into(),
            login: "acme".into(),
            ..Github::default()
        };
        o.setup[0].state = "done".into();
        o.setup[1].state = "unknown".into();
        let s = view(&o, &State::default()).setup.unwrap();
        assert_eq!(s.rows[0].detail, "Connected to acme");
        assert_eq!(s.rows[1].state, "pending");
        assert_eq!(s.rows[1].action.as_ref().unwrap().id, "choose-repositories");
        assert_eq!(s.rows[2].action.as_ref().unwrap().id, "copy-label");
        for x in &mut o.setup {
            x.state = "done".into();
        }
        assert!(view(&o, &State::default()).setup.is_none());
    }

    #[test]
    fn the_picker_filters_and_says_what_is_ready() {
        let o = Overview {
            github: Github {
                connected: true,
                choose_url: "https://github.com/organizations/acme/settings/installations".into(),
                ..Github::default()
            },
            repositories: Repositories {
                used: vec![RepoUse {
                    name: "acme/app".into(),
                    jobs_7d: 12,
                }],
                all: Some(vec![
                    RepoChoice {
                        name: "acme/app".into(),
                        allowed: true,
                    },
                    RepoChoice {
                        name: "acme/api".into(),
                        allowed: false,
                    },
                    RepoChoice {
                        name: "acme/docs".into(),
                        allowed: true,
                    },
                ]),
            },
            ..Overview::default()
        };
        let v = view(&o, &State::default()).repositories;
        assert_eq!(v.summary, "2 of 3 repositories can run jobs");
        assert_eq!(v.rows[0].jobs, "12 jobs this week");
        assert_eq!(v.rows[1].status, "Not allowed");
        assert!(v.rows[1].can_toggle);
        let s = reduce(
            &State::default(),
            &Action::Query {
                value: "API".into(),
            },
        );
        let v = view(&o, &s).repositories;
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.rows[0].name, "acme/api");
        let s = reduce(
            &s,
            &Action::Query {
                value: "zzz".into(),
            },
        );
        assert_eq!(view(&o, &s).repositories.empty, "No repository matches");
        // Without the service's list, the repositories that ran jobs show, fixed.
        let mut offline = o.clone();
        offline.repositories.all = None;
        let v = view(&offline, &State::default()).repositories;
        assert_eq!(v.summary, "1 repository ran jobs this week");
        assert!(!v.rows[0].can_toggle);
    }

    #[test]
    fn connecting_goes_choose_waiting_connected_and_a_failure_comes_back() {
        let s = reduce(&State::default(), &Action::ConnectOpen);
        assert_eq!(s.connect.phase, Phase::Choose);
        let s = reduce(&s, &Action::ConnectStart);
        assert!(s.connect.busy);
        let s = reduce(
            &s,
            &Action::ConnectAnswer {
                answer: ConnectAnswer {
                    state: "waiting".into(),
                    url: Some("https://github.com/apps/cua-ci/installations/new".into()),
                    message: None,
                },
            },
        );
        assert_eq!((s.connect.phase, s.connect.busy), (Phase::Waiting, false));
        let v = connect_view(&Overview::default(), &s.connect).unwrap();
        assert_eq!(v.title, "Finish on GitHub");
        assert!(v.url.is_some());
        let done = reduce(
            &s,
            &Action::ConnectAnswer {
                answer: ConnectAnswer {
                    state: "connected".into(),
                    ..ConnectAnswer::default()
                },
            },
        );
        assert_eq!(done.connect, Connect::default());
        let failed = reduce(
            &s,
            &Action::ConnectAnswer {
                answer: ConnectAnswer {
                    state: "failed".into(),
                    message: Some("The GitHub link expired. Connect again.".into()),
                    ..ConnectAnswer::default()
                },
            },
        );
        assert_eq!(failed.connect.phase, Phase::Choose);
        assert_eq!(
            failed.connect.error.as_deref(),
            Some("The GitHub link expired. Connect again.")
        );
        let signed_out = reduce(
            &reduce(&State::default(), &Action::ConnectOpen),
            &Action::ConnectAnswer {
                answer: ConnectAnswer {
                    state: "sign_in".into(),
                    ..ConnectAnswer::default()
                },
            },
        );
        assert!(
            signed_out
                .connect
                .error
                .unwrap()
                .starts_with("Sign in to Cua")
        );
    }

    #[test]
    fn the_token_form_needs_an_organization_and_keeps_it_between_tries() {
        let s = reduce(&State::default(), &Action::ConnectOpen);
        let s = reduce(&s, &Action::UseToken);
        let v = connect_view(&Overview::default(), &s.connect).unwrap();
        assert!(!v.token.as_ref().unwrap().can_submit);
        let s = reduce(
            &s,
            &Action::TokenOrg {
                value: "acme".into(),
            },
        );
        assert!(
            connect_view(&Overview::default(), &s.connect)
                .unwrap()
                .token
                .unwrap()
                .can_submit
        );
        let s = reduce(&s, &Action::TokenSubmit);
        assert!(s.connect.busy);
        let s = reduce(
            &s,
            &Action::TokenDone {
                ok: false,
                message: Some("GitHub refused the token (401)".into()),
            },
        );
        assert!(!s.connect.busy);
        assert_eq!(s.connect.org, "acme");
        assert_eq!(s.connect.phase, Phase::Token);
        let s = reduce(
            &s,
            &Action::TokenDone {
                ok: true,
                message: None,
            },
        );
        assert_eq!(s.connect.phase, Phase::Closed);
        // The org survives closing the sheet.
        let s = reduce(
            &reduce(
                &reduce(&State::default(), &Action::ConnectOpen),
                &Action::TokenOrg {
                    value: "acme".into(),
                },
            ),
            &Action::ConnectClose,
        );
        assert_eq!(s.connect.org, "acme");
    }

    #[test]
    fn an_autopilot_shows_its_goal_budget_and_reasons() {
        let o = Overview {
            budget: Budget {
                month_committed_micros: 12_400_000,
                monthly_micros: 50_000_000,
                state: "warn".into(),
            },
            targets: vec![Target {
                target: "aws".into(),
                goal: "fastest".into(),
                warm: None,
                autopilot: Some(Pilot {
                    classes: vec![
                        PilotClass {
                            class: "linux".into(),
                            phase: "steady".into(),
                            jobs_seen: 80,
                            warm_now: 2,
                            idle_ttl_secs: 600,
                            spot: true,
                            reasons: vec!["Jobs arrive on weekdays from 9 to 18.".into()],
                        },
                        PilotClass {
                            class: "macos".into(),
                            phase: "learning".into(),
                            jobs_seen: 7,
                            ..PilotClass::default()
                        },
                    ],
                }),
            }],
            ..Overview::default()
        };
        let v = view(&o, &State::default());
        assert_eq!(
            (v.sizing_title.as_str(), v.cloud_label.as_str()),
            ("Automatic sizing", "Cloud")
        );
        let a = &v.sizing[0];
        assert_eq!(a.title, "AWS");
        assert_eq!(a.goals.iter().find(|g| g.selected).unwrap().id, "fastest");
        assert_eq!(a.budget, "$12.40 of $50 used this month");
        assert_eq!(a.budget_tone.as_deref(), Some("warn"));
        assert_eq!(
            a.plans[0].line,
            "2 computers kept ready now, idle computers kept 10 min, spot computers."
        );
        assert_eq!(
            a.plans[1].line,
            "Learning from your jobs, 7 of 20 seen. Nothing is kept ready yet."
        );
        assert_eq!(a.reasons, ["Jobs arrive on weekdays from 9 to 18."]);
    }

    #[test]
    fn help_opens_one_topic_at_a_time() {
        let s = reduce(
            &State::default(),
            &Action::Help {
                id: "queued".into(),
            },
        );
        let s = reduce(&s, &Action::Help { id: "slow".into() });
        let v = view(&Overview::default(), &s);
        assert_eq!(v.help.iter().filter(|h| h.open).count(), 1);
        assert!(
            v.help
                .iter()
                .find(|h| h.id == "slow")
                .unwrap()
                .body
                .is_some()
        );
        assert!(
            reduce(&s, &Action::Help { id: "slow".into() })
                .help
                .is_none()
        );
    }

    #[test]
    fn the_words_follow_the_house_rules() {
        // Every string the page can say: computers, never devices or
        // computers; no middle dot chains.
        let o = Overview {
            jobs: vec![job("1/1", "passed", 1_000, 2_000)],
            ..Default::default()
        };
        let mut s = State::default();
        s.connect.phase = Phase::Token;
        s.expanded = Some("1/1".into());
        // The words people read (the string values), not the field names.
        fn words(v: &serde_json::Value, out: &mut String) {
            match v {
                serde_json::Value::String(s) => {
                    out.push_str(s);
                    out.push('\n');
                }
                serde_json::Value::Array(a) => a.iter().for_each(|x| words(x, out)),
                serde_json::Value::Object(m) => m.values().for_each(|x| words(x, out)),
                _ => {}
            }
        }
        let mut text = String::new();
        words(&serde_json::to_value(view(&o, &s)).unwrap(), &mut text);
        let text = text.to_lowercase();
        assert!(!text.contains('\u{00b7}'), "a middle dot");
        for bad in ["device", "machine", "autopilot"] {
            assert!(!text.contains(bad), "{bad}");
        }
    }
}
