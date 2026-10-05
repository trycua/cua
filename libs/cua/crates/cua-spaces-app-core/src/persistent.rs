// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Agents": the persistent agents of this machine, and one agent's memory,
//! routines and access to the user's computers.
//!
//! Plain data in (what the `persistent_agent_list`, `volume_ls`/`volume_read`/
//! `volume_history` (as the user), `routine_list` and `computer_access_list`
//! tools return) and plain data out (one line per agent, the detail's three
//! tabs, and the command to run). Allowing an agent to use a computer
//! widens what it can reach, so the shell runs that command through the
//! daemon, which asks for presence first; everything else here narrows or
//! only reads.

use serde::{Deserialize, Serialize};

/// A persistent agent, as `persistent_agent_list` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PersistentAgentInput {
    pub name: String,
    /// Harness id (`claude-code`, `hermes`, ...).
    pub harness: String,
    /// The Space it works in.
    pub space: String,
    pub paused: bool,
    /// `running`, `suspended` or `released`.
    pub space_state: String,
    /// Its current run, if any.
    pub run_id: Option<String>,
    /// Unix ms of the last home save (0: never).
    pub saved_ms: u64,
    pub last_error: Option<String>,
}

/// A drive entry (`volume_ls`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DriveEntryInput {
    pub path: String,
    pub name: String,
    pub folder: bool,
    pub size: u64,
}

/// One version of a file (`volume_history`, newest first).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileVersionInput {
    pub version: String,
    pub modified_ms: u64,
    pub deleted: bool,
    pub latest: bool,
}

/// An opened memory file: its text and its history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenFileInput {
    pub path: String,
    /// The text (empty for a binary file).
    pub text: String,
    pub binary: bool,
    pub versions: Vec<FileVersionInput>,
}

/// A routine (`routine_list`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoutineInput {
    pub id: String,
    pub title: String,
    /// `Every day at 8:00 AM`.
    pub label: String,
    pub enabled: bool,
}

/// A computer grant (`computer_access_list`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ComputerGrantInput {
    pub agent: String,
    pub machine: String,
    pub revoked: bool,
}

/// A computer-access audit line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessAuditInput {
    pub ts_ms: u64,
    /// `grant`, `revoke`, `use`, `denied`.
    pub action: String,
    /// `agent:ada` or `user`.
    pub principal: String,
    /// The machine.
    pub path: String,
    pub detail: String,
}

/// Everything the Agents page reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentsInput {
    pub agents: Vec<PersistentAgentInput>,
    /// The selected agent's home, `agents/<name>/`, recursively (files only).
    pub home: Vec<DriveEntryInput>,
    /// The file the shell loaded for the open path.
    pub file: Option<OpenFileInput>,
    /// The selected agent's routines.
    pub routines: Vec<RoutineInput>,
    /// Every computer grant.
    pub grants: Vec<ComputerGrantInput>,
    /// Recent computer-access audit lines, newest first.
    pub audit: Vec<AccessAuditInput>,
    /// This machine's Space id when it is set up for access (`relay:<id>`).
    pub this_machine: Option<String>,
}

/// A detail tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentTab {
    #[default]
    Memory,
    Routines,
    Access,
}

/// When a new routine fires.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoutineScheduleKind {
    Every,
    #[default]
    Daily,
    Weekly,
}

/// The new-routine form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoutineForm {
    pub title: String,
    pub prompt: String,
    pub schedule: RoutineScheduleKind,
    /// Minutes for `every`.
    pub minutes: u32,
    /// `HH:MM` for daily and weekly.
    pub time: String,
    /// `mon` ... `sun` for weekly.
    pub weekday: String,
}

impl Default for RoutineForm {
    fn default() -> Self {
        Self {
            title: String::new(),
            prompt: String::new(),
            schedule: RoutineScheduleKind::Daily,
            minutes: 60,
            time: "08:00".into(),
            weekday: "mon".into(),
        }
    }
}

/// The command the shell runs (through the daemon's tools).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AgentsRequest {
    /// `agent_pause`.
    Pause { name: String },
    /// `agent_resume`.
    Resume { name: String },
    /// Load the selected agent's home, routines and grants.
    Load { name: String },
    /// `volume_read` + `volume_history` of a home file.
    ReadFile { path: String },
    /// `volume_restore`.
    Restore { path: String, version: String },
    /// `routine_add` (one of the schedule fields is set).
    AddRoutine {
        agent: String,
        title: String,
        prompt: String,
        every_minutes: Option<u32>,
        daily_at: Option<String>,
        weekly_on: Option<String>,
    },
    /// `routine_set_enabled`.
    SetRoutine { id: String, enabled: bool },
    /// `routine_remove`.
    RemoveRoutine { id: String },
    /// `computer_access_grant` (the daemon asks for presence).
    Allow { agent: String, machine: String },
    /// `computer_access_revoke`.
    Revoke { agent: String, machine: String },
}

impl AgentsRequest {
    /// One line naming the command (logs, tests, parity transcripts).
    pub fn text(&self) -> String {
        match self {
            AgentsRequest::Pause { name } => format!("pause {name}"),
            AgentsRequest::Resume { name } => format!("resume {name}"),
            AgentsRequest::Load { name } => format!("load {name}"),
            AgentsRequest::ReadFile { path } => format!("read {path}"),
            AgentsRequest::Restore { path, version } => format!("restore {path} {version}"),
            AgentsRequest::AddRoutine {
                agent,
                title,
                every_minutes,
                daily_at,
                weekly_on,
                ..
            } => {
                let when = match (every_minutes, daily_at, weekly_on) {
                    (Some(m), _, _) => format!("every {m}"),
                    (_, Some(t), _) => format!("daily {t}"),
                    (_, _, Some(w)) => format!("weekly {w}"),
                    _ => String::new(),
                };
                format!("add routine {agent} {title:?} {when}")
            }
            AgentsRequest::SetRoutine { id, enabled } => {
                format!("routine {id} {}", if *enabled { "on" } else { "off" })
            }
            AgentsRequest::RemoveRoutine { id } => format!("remove routine {id}"),
            AgentsRequest::Allow { agent, machine } => format!("allow {agent} on {machine}"),
            AgentsRequest::Revoke { agent, machine } => format!("revoke {agent} on {machine}"),
        }
    }
}

/// The page's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentsState {
    pub selected: Option<String>,
    pub tab: AgentTab,
    /// The memory file open in the detail.
    pub open_path: Option<String>,
    pub form: RoutineForm,
    pub busy: bool,
    pub error: Option<String>,
    pub request: Option<AgentsRequest>,
}

/// An input to the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentsAction {
    Select { name: String },
    SetTab { tab: AgentTab },
    OpenFile { path: String },
    CloseFile,
    Pause { name: String },
    Resume { name: String },
    Restore { version: String },
    SetTitle { title: String },
    SetPrompt { prompt: String },
    SetSchedule { schedule: RoutineScheduleKind },
    SetMinutes { minutes: u32 },
    SetTime { time: String },
    SetWeekday { weekday: String },
    AddRoutine,
    ToggleRoutine { id: String },
    RemoveRoutine { id: String },
    AllowComputer { machine: String },
    RevokeComputer { machine: String },
    Done,
    Failed { error: String },
}

/// One agent, one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRowView {
    pub name: String,
    /// `Hermes in local:dev`.
    pub detail: String,
    /// `Running`, `Idle`, `Paused`.
    pub state: String,
    /// `Pause` or `Resume`.
    pub action_label: String,
    pub selected: bool,
}

/// One line with an id and an optional action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    pub id: String,
    pub text: String,
    /// Right-aligned secondary text.
    pub trailing: String,
    pub action_label: Option<String>,
    /// A second action (Deny next to Approve).
    pub secondary_label: Option<String>,
    pub on: Option<bool>,
}

/// A tab in the detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabView {
    pub tab: AgentTab,
    pub label: String,
    pub selected: bool,
}

/// The open file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub path: String,
    pub text: String,
    pub versions: Vec<LineView>,
    pub close_label: String,
}

/// The selected agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetailView {
    pub name: String,
    /// `Home: agents/ada/ · saved 5m ago` (one line).
    pub subtitle: String,
    pub tabs: Vec<TabView>,
    pub memory: Vec<LineView>,
    pub memory_empty: String,
    pub file: Option<FileView>,
    pub routines: Vec<LineView>,
    pub routines_empty: String,
    pub form: RoutineForm,
    pub schedules: Vec<LineView>,
    pub can_add_routine: bool,
    pub add_routine_label: String,
    pub access: Vec<LineView>,
    pub access_empty: String,
    /// `Allow on this Mac` when this machine is set up and not granted.
    pub allow_this_machine_label: Option<String>,
    pub audit: Vec<LineView>,
    pub error: Option<String>,
}

/// The page as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsView {
    pub title: String,
    pub rows: Vec<AgentRowView>,
    pub empty_text: String,
    pub detail: Option<AgentDetailView>,
    pub busy: bool,
    pub error: Option<String>,
    pub request: Option<AgentsRequest>,
    /// [`AgentsRequest::text`] of `request`.
    pub request_text: Option<String>,
}

fn harness_name(id: &str) -> String {
    match id {
        "claude-code" => "Claude Code".into(),
        "openai-codex" => "Codex".into(),
        "hermes" => "Hermes".into(),
        "openclaw" => "OpenClaw".into(),
        "gemini-cli" => "Gemini CLI".into(),
        "google-antigravity" => "Antigravity".into(),
        "goose" => "Goose".into(),
        "opencode" => "OpenCode".into(),
        "pi" => "Pi".into(),
        other => other.into(),
    }
}

/// `5m ago`, `3h ago`, `2d ago`, `never`.
pub fn ago(ms: u64, now_ms: u64) -> String {
    if ms == 0 {
        return "never".into();
    }
    let s = now_ms.saturating_sub(ms) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn valid_time(t: &str) -> bool {
    t.split_once(':').is_some_and(|(h, m)| {
        h.len() <= 2
            && m.len() == 2
            && h.parse::<u32>().is_ok_and(|h| h < 24)
            && m.parse::<u32>().is_ok_and(|m| m < 60)
    })
}

const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn form_ok(f: &RoutineForm) -> bool {
    !f.title.trim().is_empty()
        && !f.prompt.trim().is_empty()
        && match f.schedule {
            RoutineScheduleKind::Every => f.minutes >= 1,
            RoutineScheduleKind::Daily => valid_time(&f.time),
            RoutineScheduleKind::Weekly => {
                valid_time(&f.time) && WEEKDAYS.contains(&f.weekday.as_str())
            }
        }
}

/// A new page.
pub fn agents_initial() -> AgentsState {
    AgentsState::default()
}

fn start(s: &mut AgentsState, r: AgentsRequest) {
    s.busy = true;
    s.error = None;
    s.request = Some(r);
}

/// Advances the page.
pub fn agents_reduce(
    input: &AgentsInput,
    state: &AgentsState,
    action: &AgentsAction,
) -> AgentsState {
    let mut s = state.clone();
    let agent = s.selected.clone();
    match action {
        AgentsAction::Select { name } if input.agents.iter().any(|a| &a.name == name) => {
            s.selected = Some(name.clone());
            s.open_path = None;
            s.error = None;
            if !s.busy {
                start(&mut s, AgentsRequest::Load { name: name.clone() });
            }
        }
        AgentsAction::SetTab { tab } => s.tab = *tab,
        AgentsAction::OpenFile { path } if !s.busy => {
            s.open_path = Some(path.clone());
            start(&mut s, AgentsRequest::ReadFile { path: path.clone() });
        }
        AgentsAction::CloseFile => s.open_path = None,
        AgentsAction::Pause { name } if !s.busy => {
            start(&mut s, AgentsRequest::Pause { name: name.clone() })
        }
        AgentsAction::Resume { name } if !s.busy => {
            start(&mut s, AgentsRequest::Resume { name: name.clone() })
        }
        AgentsAction::Restore { version } if !s.busy => {
            if let Some(path) = s.open_path.clone() {
                start(
                    &mut s,
                    AgentsRequest::Restore {
                        path,
                        version: version.clone(),
                    },
                );
            }
        }
        AgentsAction::SetTitle { title } => s.form.title = title.clone(),
        AgentsAction::SetPrompt { prompt } => s.form.prompt = prompt.clone(),
        AgentsAction::SetSchedule { schedule } => s.form.schedule = *schedule,
        AgentsAction::SetMinutes { minutes } => s.form.minutes = *minutes,
        AgentsAction::SetTime { time } => s.form.time = time.trim().to_string(),
        AgentsAction::SetWeekday { weekday } => s.form.weekday = weekday.to_ascii_lowercase(),
        AgentsAction::AddRoutine if !s.busy && form_ok(&s.form) => {
            if let Some(agent) = agent {
                let f = s.form.clone();
                start(
                    &mut s,
                    AgentsRequest::AddRoutine {
                        agent,
                        title: f.title.trim().into(),
                        prompt: f.prompt.trim().into(),
                        every_minutes: (f.schedule == RoutineScheduleKind::Every)
                            .then_some(f.minutes),
                        daily_at: (f.schedule == RoutineScheduleKind::Daily)
                            .then(|| f.time.clone()),
                        weekly_on: (f.schedule == RoutineScheduleKind::Weekly)
                            .then(|| format!("{} {}", f.weekday, f.time)),
                    },
                );
            }
        }
        AgentsAction::ToggleRoutine { id } if !s.busy => {
            if let Some(r) = input.routines.iter().find(|r| &r.id == id) {
                start(
                    &mut s,
                    AgentsRequest::SetRoutine {
                        id: id.clone(),
                        enabled: !r.enabled,
                    },
                );
            }
        }
        AgentsAction::RemoveRoutine { id } if !s.busy => {
            start(&mut s, AgentsRequest::RemoveRoutine { id: id.clone() })
        }
        AgentsAction::AllowComputer { machine } if !s.busy => {
            if let Some(agent) = agent {
                start(
                    &mut s,
                    AgentsRequest::Allow {
                        agent,
                        machine: machine.clone(),
                    },
                );
            }
        }
        AgentsAction::RevokeComputer { machine } if !s.busy => {
            if let Some(agent) = agent {
                start(
                    &mut s,
                    AgentsRequest::Revoke {
                        agent,
                        machine: machine.clone(),
                    },
                );
            }
        }
        AgentsAction::Done => {
            if matches!(s.request, Some(AgentsRequest::AddRoutine { .. })) {
                s.form = RoutineForm::default();
            }
            if matches!(s.request, Some(AgentsRequest::Restore { .. })) {
                s.open_path = None;
            }
            s.busy = false;
            s.request = None;
        }
        AgentsAction::Failed { error } => {
            s.busy = false;
            s.request = None;
            s.error = Some(error.clone());
        }
        _ => {}
    }
    s
}

fn line(
    id: &str,
    text: String,
    trailing: String,
    action: Option<&str>,
    on: Option<bool>,
) -> LineView {
    LineView {
        id: id.into(),
        text,
        trailing,
        action_label: action.map(str::to_string),
        secondary_label: None,
        on,
    }
}

/// The page as drawn at `now_ms`.
pub fn agents_view(input: &AgentsInput, state: &AgentsState, now_ms: u64) -> AgentsView {
    let mut agents: Vec<&PersistentAgentInput> = input.agents.iter().collect();
    agents.sort_by_key(|a| a.name.clone());
    let rows = agents
        .iter()
        .map(|a| AgentRowView {
            name: a.name.clone(),
            detail: format!("{} in {}", harness_name(&a.harness), a.space),
            state: if a.paused {
                "Paused"
            } else if a.run_id.is_some() {
                "Running"
            } else {
                "Idle"
            }
            .into(),
            action_label: if a.paused { "Resume" } else { "Pause" }.into(),
            selected: state.selected.as_deref() == Some(a.name.as_str()),
        })
        .collect();
    let detail = state
        .selected
        .as_deref()
        .and_then(|n| input.agents.iter().find(|a| a.name == n))
        .map(|a| detail_view(input, state, a, now_ms));
    AgentsView {
        title: "Agents".into(),
        rows,
        empty_text: "No persistent agents. Create one with cua agent create, or ask your agent to."
            .into(),
        detail,
        busy: state.busy,
        error: state.error.clone(),
        request: state.request.clone(),
        request_text: state.request.as_ref().map(AgentsRequest::text),
    }
}

fn detail_view(
    input: &AgentsInput,
    state: &AgentsState,
    a: &PersistentAgentInput,
    now_ms: u64,
) -> AgentDetailView {
    let prefix = format!("agents/{}/", a.name);
    let tabs = [
        (AgentTab::Memory, "Memory"),
        (AgentTab::Routines, "Routines"),
        (AgentTab::Access, "Access to this computer"),
    ]
    .into_iter()
    .map(|(tab, label)| TabView {
        tab,
        label: label.into(),
        selected: state.tab == tab,
    })
    .collect();
    let memory = input
        .home
        .iter()
        .filter(|e| !e.folder)
        .map(|e| {
            let rel = e.path.strip_prefix(&prefix).unwrap_or(&e.path).to_string();
            line(&e.path, rel, size(e.size), None, None)
        })
        .collect();
    let file = state.open_path.as_ref().and_then(|p| {
        input
            .file
            .as_ref()
            .filter(|f| &f.path == p)
            .map(|f| FileView {
                path: f.path.strip_prefix(&prefix).unwrap_or(&f.path).to_string(),
                text: if f.binary {
                    "Not a text file.".into()
                } else {
                    f.text.clone()
                },
                versions: f
                    .versions
                    .iter()
                    .filter(|v| !v.deleted)
                    .map(|v| {
                        line(
                            &v.version,
                            if v.latest {
                                "Current".into()
                            } else {
                                ago(v.modified_ms, now_ms)
                            },
                            String::new(),
                            (!v.latest).then_some("Restore"),
                            None,
                        )
                    })
                    .collect(),
                close_label: "Close".into(),
            })
    });
    let routines = input
        .routines
        .iter()
        .map(|r| {
            line(
                &r.id,
                r.title.clone(),
                r.label.clone(),
                Some("Remove"),
                Some(r.enabled),
            )
        })
        .collect();
    let live: Vec<&ComputerGrantInput> = input
        .grants
        .iter()
        .filter(|g| g.agent == a.name && !g.revoked)
        .collect();
    let access = live
        .iter()
        .map(|g| {
            let this = input.this_machine.as_deref() == Some(g.machine.as_str());
            line(
                &g.machine,
                if this {
                    "This computer".into()
                } else {
                    g.machine.clone()
                },
                String::new(),
                Some("Revoke"),
                None,
            )
        })
        .collect();
    let allow_this_machine_label = input
        .this_machine
        .as_ref()
        .filter(|m| !live.iter().any(|g| &&g.machine == m))
        .map(|_| "Allow on this computer".to_string());
    let principal = format!("agent:{}", a.name);
    let audit = input
        .audit
        .iter()
        .filter(|e| e.principal == principal || e.detail.contains(&principal))
        .take(10)
        .map(|e| {
            let verb = match e.action.as_str() {
                "grant" => "Allowed",
                "revoke" => "Revoked",
                "use" => "Used",
                "denied" => "Refused",
                other => other,
            };
            line(
                &format!("{}-{}", e.ts_ms, e.action),
                format!("{verb} {}", e.path),
                ago(e.ts_ms, now_ms),
                None,
                None,
            )
        })
        .collect();
    AgentDetailView {
        name: a.name.clone(),
        subtitle: format!("Home {prefix}, saved {}", ago(a.saved_ms, now_ms)),
        tabs,
        memory,
        memory_empty: "Nothing saved yet.".into(),
        file,
        routines,
        routines_empty: "No routines.".into(),
        form: state.form.clone(),
        schedules: [
            (RoutineScheduleKind::Every, "Every N minutes"),
            (RoutineScheduleKind::Daily, "Every day"),
            (RoutineScheduleKind::Weekly, "Every week"),
        ]
        .into_iter()
        .map(|(k, label)| {
            let id = match k {
                RoutineScheduleKind::Every => "every",
                RoutineScheduleKind::Daily => "daily",
                RoutineScheduleKind::Weekly => "weekly",
            };
            line(
                id,
                label.into(),
                String::new(),
                None,
                Some(state.form.schedule == k),
            )
        })
        .collect(),
        can_add_routine: form_ok(&state.form) && !state.busy,
        add_routine_label: "Add routine".into(),
        access,
        access_empty: "This agent cannot use your computers.".into(),
        allow_this_machine_label,
        audit,
        error: a.last_error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> AgentsInput {
        AgentsInput {
            agents: vec![
                PersistentAgentInput {
                    name: "ada".into(),
                    harness: "hermes".into(),
                    space: "local:dev".into(),
                    run_id: Some("run-1".into()),
                    saved_ms: 1_000,
                    ..Default::default()
                },
                PersistentAgentInput {
                    name: "bob".into(),
                    harness: "claude-code".into(),
                    space: "cloud:bob".into(),
                    paused: true,
                    space_state: "released".into(),
                    ..Default::default()
                },
            ],
            this_machine: Some("relay:0123".into()),
            routines: vec![RoutineInput {
                id: "R1".into(),
                title: "Morning".into(),
                label: "Every day at 8:00 AM".into(),
                enabled: true,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn rows_state_and_pause_resume() {
        let i = input();
        let v = agents_view(&i, &agents_initial(), 61_000);
        assert_eq!(v.rows[0].detail, "Hermes in local:dev");
        assert_eq!(
            (v.rows[0].state.as_str(), v.rows[0].action_label.as_str()),
            ("Running", "Pause")
        );
        assert_eq!(
            (v.rows[1].state.as_str(), v.rows[1].action_label.as_str()),
            ("Paused", "Resume")
        );
        let s = agents_reduce(
            &i,
            &agents_initial(),
            &AgentsAction::Pause { name: "ada".into() },
        );
        assert_eq!(s.request, Some(AgentsRequest::Pause { name: "ada".into() }));
        let s = agents_reduce(&i, &s, &AgentsAction::Resume { name: "bob".into() });
        assert_eq!(
            s.request,
            Some(AgentsRequest::Pause { name: "ada".into() }),
            "busy"
        );
    }

    #[test]
    fn routines_form_and_access() {
        let i = input();
        let s = agents_reduce(
            &i,
            &agents_initial(),
            &AgentsAction::Select { name: "ada".into() },
        );
        let s = agents_reduce(&i, &s, &AgentsAction::Done);
        let s = agents_reduce(
            &i,
            &s,
            &AgentsAction::SetTitle {
                title: "Sweep".into(),
            },
        );
        let v = agents_view(&i, &s, 0);
        assert!(!v.detail.unwrap().can_add_routine, "no prompt");
        let s = agents_reduce(
            &i,
            &s,
            &AgentsAction::SetPrompt {
                prompt: "Triage".into(),
            },
        );
        let s = agents_reduce(
            &i,
            &s,
            &AgentsAction::SetSchedule {
                schedule: RoutineScheduleKind::Weekly,
            },
        );
        let s = agents_reduce(
            &i,
            &s,
            &AgentsAction::SetTime {
                time: "9:30".into(),
            },
        );
        let s = agents_reduce(&i, &s, &AgentsAction::AddRoutine);
        assert_eq!(
            s.request,
            Some(AgentsRequest::AddRoutine {
                agent: "ada".into(),
                title: "Sweep".into(),
                prompt: "Triage".into(),
                every_minutes: None,
                daily_at: None,
                weekly_on: Some("mon 9:30".into()),
            })
        );
        let s = agents_reduce(&i, &s, &AgentsAction::Done);
        assert_eq!(s.form, RoutineForm::default());
        let d = agents_view(&i, &s, 0).detail.unwrap();
        assert_eq!(
            d.allow_this_machine_label.as_deref(),
            Some("Allow on this computer")
        );
        let s = agents_reduce(
            &i,
            &s,
            &AgentsAction::AllowComputer {
                machine: "relay:0123".into(),
            },
        );
        assert_eq!(
            s.request,
            Some(AgentsRequest::Allow {
                agent: "ada".into(),
                machine: "relay:0123".into()
            })
        );
    }
}
