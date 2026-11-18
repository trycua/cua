// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Starting a task from a phone: the Home box with an agent pick, a place
//! pick and "Add a repo", and the request that becomes a Space (or a message
//! to an agent already in one).
//!
//! It is the same flow the web Home runs (`use-start-task.ts`): the job is
//! the first message `@<handle> <job>` ([`crate::summon::first_message`]), a
//! repository is a `Repo:` line the machine adds to it, and the machine's daemon makes the
//! Space or finds the agent's home. A phone is not a host, so the choices it
//! offers come from the account's machines (`task_options`), and what cannot
//! start says why instead of being hidden.
//!
//! Example jobs for an empty box are the prompt library
//! ([`crate::first_prompts`], read through `firstPrompts.choose` by the shell);
//! nothing here keeps a sentence of them.

use serde::{Deserialize, Serialize};

use crate::summon::{self, SummonCandidate, SummonInput, SummonMember};

/// A machine the job could run on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MachineOption {
    pub id: String,
    pub name: String,
    pub online: bool,
    /// The machine has room to make a Space now.
    pub can_create: bool,
    /// Why not, in the daemon's words ("no room").
    pub why: Option<String>,
    /// `linux`, `macos`, `windows`.
    pub os: Vec<String>,
    /// The agents (harness ids) that are connected on this machine. None: the machine did not say, and any agent is tried.
    pub ready_agents: Option<Vec<String>>,
}

/// A coding agent the job could go to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentOption {
    /// `claude-code`, `openai-codex`, `hermes`, `openclaw`.
    pub harness: String,
    pub label: String,
    pub ready: bool,
    pub why: Option<String>,
}

/// A Space that already exists, to message instead of making a new one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpaceOption {
    pub id: String,
    pub name: String,
    pub machine_id: String,
    pub online: bool,
    /// The agents already in it (their handles decide the first message).
    pub agents: Vec<SummonMember>,
}

/// Where the job runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WhereChoice {
    /// The daemon places it (local first, room, overflow to a linked cloud).
    Auto,
    Machine {
        machine: String,
    },
    Space {
        space: String,
    },
}

/// What the person has picked so far.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Choice {
    pub agent: Option<String>,
    #[serde(rename = "where")]
    pub place: Option<WhereChoice>,
    /// What was typed in "Add a repo": `owner/name` or a GitHub link.
    pub repo: Option<String>,
}

/// What the screen reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StartTaskInput {
    pub machines: Vec<MachineOption>,
    pub agents: Vec<AgentOption>,
    pub spaces: Vec<SpaceOption>,
    pub choice: Choice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    pub harness: String,
    pub label: String,
    pub selected: bool,
    pub enabled: bool,
    /// Why it cannot start, or none.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhereRow {
    /// `auto`, `machine:<id>` or `space:<id>`.
    pub id: String,
    /// `auto`, `machine`, `space`.
    pub kind: String,
    pub label: String,
    pub detail: Option<String>,
    pub selected: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoChip {
    /// "Add a repo", or `owner/name` once set.
    pub label: String,
    pub set: bool,
    pub placeholder: String,
    pub hint: String,
    pub clear_label: String,
    /// What was typed cannot be read as a repository.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartingLabels {
    pub title: String,
    pub detail: String,
    pub failed_title: String,
    pub retry: String,
    /// The button that leaves the screen while the task is still being made (it keeps going).
    pub leave: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTaskView {
    pub title: String,
    pub placeholder: String,
    pub agent_title: String,
    pub where_title: String,
    pub agents: Vec<AgentRow>,
    pub wheres: Vec<WhereRow>,
    pub repo: RepoChip,
    pub start_label: String,
    pub cancel_label: String,
    /// What the screen says while the machine makes the Space (it can take minutes), and if that fails.
    pub starting: StartingLabels,
    /// The pick that will be used (null: nothing can start).
    pub agent: Option<String>,
    #[serde(rename = "where")]
    pub place: WhereChoice,
    /// Why Start cannot be pressed besides an empty box.
    pub blocker: Option<String>,
}

const AUTO: &str = "Let Cua decide";

fn agent_label(harness: &str) -> String {
    summon::candidates(&SummonInput {
        can_add: true,
        ..Default::default()
    })
    .into_iter()
    .find(|c: &SummonCandidate| c.harness == harness)
    .map(|c| c.label)
    .unwrap_or_else(|| harness.to_string())
}

/// `owner/name` and the https link of a GitHub repository typed in the box.
/// Accepts `owner/name`, `github.com/owner/name`, `https://github.com/owner/name`
/// with an optional `.git`, a trailing slash or a deeper page.
pub fn parse_repo(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let t = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    let t = t.strip_prefix("www.").unwrap_or(t);
    let t = t.strip_prefix("github.com/").unwrap_or(t);
    let mut parts = t.split('/').filter(|p| !p.is_empty());
    let owner = parts.next()?;
    let name = parts.next()?.trim_end_matches(".git");
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && s != "."
            && s != ".."
    };
    // `owner` has no dot in GitHub; a dotted first part is a host that is not GitHub.
    if !ok(owner) || !ok(name) || owner.contains('.') {
        return None;
    }
    Some((
        format!("{owner}/{name}"),
        format!("https://github.com/{owner}/{name}"),
    ))
}

fn agent_rows(input: &StartTaskInput) -> (Vec<AgentRow>, Option<String>) {
    let mut options = input.agents.clone();
    if options.is_empty() {
        // No list from the daemon yet: the agents that can start, assumed ready.
        options = summon::candidates(&SummonInput {
            can_add: true,
            ..Default::default()
        })
        .into_iter()
        .map(|c| AgentOption {
            harness: c.harness,
            label: c.label,
            ready: true,
            why: None,
        })
        .collect();
    }
    for o in &mut options {
        if o.label.is_empty() {
            o.label = agent_label(&o.harness);
        }
    }
    let wanted = input.choice.agent.as_deref();
    let chosen = wanted
        .and_then(|w| options.iter().find(|o| o.harness == w && o.ready))
        .or_else(|| options.iter().find(|o| o.ready))
        .map(|o| o.harness.clone());
    let rows = options
        .iter()
        .map(|o| AgentRow {
            selected: chosen.as_deref() == Some(o.harness.as_str()),
            enabled: o.ready,
            detail: if o.ready {
                None
            } else {
                Some(o.why.clone().unwrap_or_else(|| "Not connected yet".into()))
            },
            harness: o.harness.clone(),
            label: o.label.clone(),
        })
        .collect();
    (rows, chosen)
}

fn where_rows(
    input: &StartTaskInput,
    agent: Option<&str>,
    agent_label: &str,
) -> (Vec<WhereRow>, WhereChoice, Option<String>) {
    let has_agent = |m: &MachineOption| match (&m.ready_agents, agent) {
        (Some(ready), Some(a)) => ready.iter().any(|r| r == a),
        _ => true,
    };
    let any_online = input
        .machines
        .iter()
        .any(|m| m.online && m.can_create && has_agent(m));
    let mut place = input.choice.place.clone().unwrap_or(WhereChoice::Auto);
    // A pick that is gone falls back to Auto.
    let known = match &place {
        WhereChoice::Auto => true,
        WhereChoice::Machine { machine } => input.machines.iter().any(|m| &m.id == machine),
        WhereChoice::Space { space } => input.spaces.iter().any(|s| &s.id == space),
    };
    if !known {
        place = WhereChoice::Auto;
    }
    let mut rows = vec![WhereRow {
        id: "auto".into(),
        kind: "auto".into(),
        label: AUTO.into(),
        detail: Some(if any_online {
            "Picks the computer with the most room".into()
        } else {
            "None of your computers can start it now".into()
        }),
        selected: place == WhereChoice::Auto,
        enabled: any_online,
    }];
    for m in &input.machines {
        let (detail, enabled) = if !m.online {
            (Some("Offline".to_string()), false)
        } else if !m.can_create {
            (
                Some(
                    m.why
                        .clone()
                        .unwrap_or_else(|| "No room for a new Space".into()),
                ),
                false,
            )
        } else if !has_agent(m) {
            (Some(format!("{agent_label} isn't connected here")), false)
        } else {
            (Some("New Space".to_string()), true)
        };
        rows.push(WhereRow {
            id: format!("machine:{}", m.id),
            kind: "machine".into(),
            label: m.name.clone(),
            detail,
            selected: place
                == WhereChoice::Machine {
                    machine: m.id.clone(),
                },
            enabled,
        });
    }
    for s in &input.spaces {
        rows.push(WhereRow {
            id: format!("space:{}", s.id),
            kind: "space".into(),
            label: format!("In {}", s.name),
            detail: Some(if s.online {
                "Message the agent there".to_string()
            } else {
                "Offline".to_string()
            }),
            selected: place
                == WhereChoice::Space {
                    space: s.id.clone(),
                },
            enabled: s.online,
        });
    }
    // A selected row that cannot start is the blocker.
    let blocker =
        rows.iter()
            .find(|r| r.selected && !r.enabled)
            .map(|r| match r.kind.as_str() {
                "auto" => "None of your computers can start a task now. Check that one is online."
                    .to_string(),
                "machine" => match r.detail.as_deref() {
                    Some(d) if d.ends_with("isn't connected here") => format!("{}: {d}.", r.label),
                    _ => format!("{} can't start a task now.", r.label),
                },
                _ => format!("{} is offline.", r.label.trim_start_matches("In ")),
            });
    (rows, place, blocker)
}

/// The Start a task screen.
pub fn view(input: &StartTaskInput) -> StartTaskView {
    let (agents, agent) = agent_rows(input);
    let agent_name = agents
        .iter()
        .find(|a| a.selected)
        .map(|a| a.label.clone())
        .unwrap_or_default();
    let (wheres, place, mut blocker) = where_rows(input, agent.as_deref(), &agent_name);
    if agent.is_none() {
        blocker = Some(
            "No coding agent is connected yet. Connect one in Cua Spaces on a Mac or PC.".into(),
        );
    }
    let typed = input
        .choice
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    let parsed = typed.and_then(parse_repo);
    let repo = RepoChip {
        label: parsed
            .as_ref()
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "Add a repo".into()),
        set: parsed.is_some(),
        placeholder: "owner/name or a GitHub link".into(),
        hint: "The agent clones it into the Space.".into(),
        clear_label: "Remove repo".into(),
        problem: if typed.is_some() && parsed.is_none() {
            Some("That doesn't look like a repository. Use owner/name or a GitHub link.".into())
        } else {
            None
        },
    };
    StartTaskView {
        title: "What do you want to do?".into(),
        placeholder: "Describe the job".into(),
        agent_title: "Agent".into(),
        where_title: "Where".into(),
        agents,
        wheres,
        repo,
        start_label: "Start".into(),
        cancel_label: "Cancel".into(),
        starting: StartingLabels {
            title: "Starting your task".into(),
            detail:
                "A computer is making a Space and handing it the job. You can leave: it keeps going."
                    .into(),
            failed_title: "The task did not start".into(),
            retry: "Try again".into(),
            leave: "Close".into(),
        },
        agent,
        place,
        blocker,
    }
}

/// The repository of a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    pub url: String,
    pub name: String,
}

/// The `task_start` arguments (the wire the daemon takes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskRequest {
    /// The idempotency key: the same id twice is one task.
    pub client_id: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The job as typed.
    pub prompt: String,
    /// The first message the thread gets (`@handle job`, with the repo line).
    pub message: String,
    #[serde(rename = "where")]
    pub place: WhereChoice,
    pub os: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoUrl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoUrl {
    pub url: String,
}

/// What pressing Start does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTaskPlan {
    pub ok: bool,
    /// Why not, in words.
    pub error: Option<String>,
    pub request: Option<TaskRequest>,
    /// The line under the message while its Space is made.
    pub queued_note: String,
    /// The name the new thread opens under until the Space has one.
    pub title: String,
}

fn fail(error: &str) -> StartTaskPlan {
    StartTaskPlan {
        ok: false,
        error: Some(error.into()),
        request: None,
        queued_note: summon::queued_note(true).text,
        title: String::new(),
    }
}

/// A short title for a thread from the job: the first line, cut.
fn title_of(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(40).collect();
    if line.chars().count() > 40 {
        out.truncate(out.trim_end().len());
        out.push('\u{2026}');
    }
    out
}

/// The request for `prompt` with the picks in `input`. `client_id` is the
/// caller's idempotency key; `model` is the agent's model when it picked one.
pub fn plan(
    input: &StartTaskInput,
    prompt: &str,
    client_id: &str,
    model: Option<&str>,
) -> StartTaskPlan {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return fail("Write what you want done.");
    }
    let id = client_id.trim();
    if id.len() < 8
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return fail("This task has no id yet.");
    }
    let v = view(input);
    if let Some(b) = &v.blocker {
        return fail(b);
    }
    let Some(agent) = v.agent.clone() else {
        return fail("No coding agent is connected yet.");
    };
    let repo = match input
        .choice
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        None => None,
        Some(r) => match parse_repo(r) {
            Some((_, url)) => Some(url),
            None => {
                return fail(
                    "That doesn't look like a repository. Use owner/name or a GitHub link.",
                );
            }
        },
    };
    // Who is already in the Space decides the handle in the first message.
    let members = match &v.place {
        WhereChoice::Space { space } => input
            .spaces
            .iter()
            .find(|s| &s.id == space)
            .map(|s| s.agents.clone())
            .unwrap_or_default(),
        _ => vec![],
    };
    let first = summon::first_message(
        &SummonInput {
            members,
            people: vec![],
            can_add: true,
            ..Default::default()
        },
        &agent,
        prompt,
    );
    // The machine adds the `Repo:` line to the job it posts (the one place it is made), so the message is the job alone.
    let message = first.text.clone();
    // A task starts in a Linux Space (the machine's rule for now; a Mac or Windows Space is picked by naming one that exists).
    let os = "linux".to_string();
    StartTaskPlan {
        ok: true,
        error: None,
        title: title_of(prompt),
        queued_note: summon::queued_note(true).text,
        request: Some(TaskRequest {
            client_id: id.to_string(),
            agent,
            model: model.map(str::to_string).filter(|m| !m.is_empty()),
            prompt: prompt.to_string(),
            message,
            place: v.place,
            os,
            repo: repo.map(|url| RepoUrl { url }),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> StartTaskInput {
        StartTaskInput {
            machines: vec![
                MachineOption {
                    id: "m1".into(),
                    name: "Studio Mac".into(),
                    online: true,
                    can_create: true,
                    os: vec!["macos".into(), "linux".into()],
                    ..Default::default()
                },
                MachineOption {
                    id: "m2".into(),
                    name: "Desk PC".into(),
                    online: false,
                    can_create: false,
                    ..Default::default()
                },
                MachineOption {
                    id: "m3".into(),
                    name: "Old Mac".into(),
                    online: true,
                    can_create: false,
                    why: Some("No room".into()),
                    ..Default::default()
                },
            ],
            agents: vec![
                AgentOption {
                    harness: "claude-code".into(),
                    label: "Claude Code".into(),
                    ready: true,
                    why: None,
                },
                AgentOption {
                    harness: "openai-codex".into(),
                    label: "Codex".into(),
                    ready: false,
                    why: Some("Sign in to Codex in Settings".into()),
                },
            ],
            spaces: vec![SpaceOption {
                id: "relay:s1".into(),
                name: "checkout-api".into(),
                machine_id: "m1".into(),
                online: true,
                agents: vec![SummonMember {
                    handle: "claude".into(),
                    harness: "claude-code".into(),
                    label: "Claude Code".into(),
                    ..Default::default()
                }],
            }],
            choice: Choice::default(),
        }
    }

    #[test]
    fn repos_are_read_in_the_usual_spellings() {
        for t in [
            "acme/app",
            "github.com/acme/app",
            "https://github.com/acme/app",
            "https://github.com/acme/app.git",
            "http://www.github.com/acme/app/",
            "https://github.com/acme/app/pull/3",
        ] {
            assert_eq!(
                parse_repo(t),
                Some(("acme/app".into(), "https://github.com/acme/app".into())),
                "{t}"
            );
        }
        for t in [
            "",
            "acme",
            "https://gitlab.example.com/acme/app",
            "acme/..",
            "ac me/app",
            "../x",
        ] {
            assert_eq!(parse_repo(t), None, "{t}");
        }
    }

    #[test]
    fn the_view_picks_the_first_ready_agent_and_auto() {
        let v = view(&input());
        assert_eq!(v.agent.as_deref(), Some("claude-code"));
        assert_eq!(v.place, WhereChoice::Auto);
        assert!(v.agents[0].selected && v.agents[0].enabled);
        assert!(!v.agents[1].enabled);
        assert_eq!(
            v.agents[1].detail.as_deref(),
            Some("Sign in to Codex in Settings")
        );
        assert_eq!(v.wheres[0].label, "Let Cua decide");
        assert!(v.wheres[0].selected);
        assert_eq!(v.wheres[2].detail.as_deref(), Some("Offline"));
        assert!(!v.wheres[2].enabled);
        assert_eq!(v.wheres[3].detail.as_deref(), Some("No room"));
        assert_eq!(v.wheres[4].label, "In checkout-api");
        assert_eq!(v.repo.label, "Add a repo");
        assert_eq!(v.blocker, None);
        assert_eq!(v.title, "What do you want to do?");
    }

    #[test]
    fn a_gone_pick_falls_back_and_no_agent_blocks() {
        let mut i = input();
        i.choice.place = Some(WhereChoice::Machine {
            machine: "gone".into(),
        });
        i.choice.agent = Some("openai-codex".into());
        let v = view(&i);
        assert_eq!(v.place, WhereChoice::Auto);
        assert_eq!(
            v.agent.as_deref(),
            Some("claude-code"),
            "an agent that is not ready is not picked"
        );
        i.agents.iter_mut().for_each(|a| a.ready = false);
        let v = view(&i);
        assert_eq!(v.agent, None);
        assert!(v.blocker.unwrap().contains("No coding agent"));
    }

    #[test]
    fn an_offline_pick_is_the_blocker() {
        let mut i = input();
        i.choice.place = Some(WhereChoice::Machine {
            machine: "m2".into(),
        });
        assert_eq!(
            view(&i).blocker.as_deref(),
            Some("Desk PC can't start a task now.")
        );
        i.machines.iter_mut().for_each(|m| m.online = false);
        i.choice.place = None;
        assert!(view(&i).blocker.unwrap().contains("None of your computers"));
    }

    #[test]
    fn the_plan_is_the_first_message_a_summon_would_post() {
        let mut i = input();
        i.choice.repo = Some("acme/app".into());
        let p = plan(&i, "  fix the flaky test  ", "task-0001", Some("sonnet"));
        assert!(p.ok, "{p:?}");
        let r = p.request.unwrap();
        assert_eq!(
            r.message, "@claude fix the flaky test",
            "the machine adds the Repo line"
        );
        assert_eq!(r.prompt, "fix the flaky test");
        assert_eq!(r.client_id, "task-0001");
        assert_eq!(r.agent, "claude-code");
        assert_eq!(r.model.as_deref(), Some("sonnet"));
        assert_eq!(r.os, "linux");
        assert_eq!(
            r.repo,
            Some(RepoUrl {
                url: "https://github.com/acme/app".into()
            })
        );
        assert_eq!(p.queued_note, "Sends when the Space is ready");
        assert_eq!(p.title, "fix the flaky test");
        // The wire is snake_case with `where`.
        let wire = serde_json::to_value(&r).unwrap();
        assert_eq!(wire["where"], serde_json::json!({"kind": "auto"}));
        assert_eq!(wire["client_id"], "task-0001");
    }

    #[test]
    fn a_task_for_a_space_uses_the_handle_of_the_agent_already_there() {
        let mut i = input();
        i.choice.place = Some(WhereChoice::Space {
            space: "relay:s1".into(),
        });
        let r = plan(&i, "status?", "task-0002", None).request.unwrap();
        assert_eq!(r.message, "@claude status?");
        assert_eq!(
            r.place,
            WhereChoice::Space {
                space: "relay:s1".into()
            }
        );
        i.choice.place = Some(WhereChoice::Machine {
            machine: "m1".into(),
        });
        let r = plan(&i, "status?", "task-0002", None).request.unwrap();
        assert_eq!(r.os, "linux", "a task starts in a Linux Space");
    }

    #[test]
    fn what_cannot_start_says_why() {
        let i = input();
        assert_eq!(
            plan(&i, "  ", "task-0003", None).error.as_deref(),
            Some("Write what you want done.")
        );
        let mut bad = input();
        bad.choice.repo = Some("not a repo".into());
        assert!(
            plan(&bad, "x", "task-0004", None)
                .error
                .unwrap()
                .contains("repository")
        );
        assert!(view(&bad).repo.problem.is_some());
        assert!(!plan(&i, "x", " ", None).ok);
    }

    #[test]
    fn a_machine_where_the_agent_is_not_connected_says_so_and_is_not_offered() {
        let mut i = input();
        i.machines[0].ready_agents = Some(vec!["hermes".into()]);
        let v = view(&i);
        assert_eq!(
            v.wheres[1].detail.as_deref(),
            Some("Claude Code isn't connected here")
        );
        assert!(!v.wheres[1].enabled);
        // Let Cua decide has no machine left for Claude Code now.
        assert!(!v.wheres[0].enabled);
        assert!(v.blocker.unwrap().contains("None of your computers"));
        // Another agent that is connected there starts fine.
        i.choice.agent = Some("hermes".into());
        i.agents.push(AgentOption {
            harness: "hermes".into(),
            label: "Hermes".into(),
            ready: true,
            why: None,
        });
        let v = view(&i);
        assert!(v.wheres[1].enabled && v.wheres[0].enabled);
        assert_eq!(v.blocker, None);
        // A picked machine without the agent is the blocker, by name.
        i.choice.agent = Some("claude-code".into());
        i.choice.place = Some(WhereChoice::Machine {
            machine: "m1".into(),
        });
        assert_eq!(
            view(&i).blocker.as_deref(),
            Some("Studio Mac: Claude Code isn't connected here.")
        );
    }

    #[test]
    fn a_task_id_is_checked_like_the_machine_checks_it() {
        let i = input();
        let long = "x".repeat(200);
        for bad in ["short", "has a space in it", long.as_str()] {
            assert!(!plan(&i, "x", bad, None).ok, "{bad}");
        }
        assert!(plan(&i, "x", "0f8e6c1a-1b2c-4d3e-8f90-123456789abc", None).ok);
    }

    #[test]
    fn an_empty_agent_list_offers_every_agent_that_can_start() {
        let mut i = input();
        i.agents.clear();
        let v = view(&i);
        let labels: Vec<&str> = v.agents.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Claude Code", "Codex", "Hermes", "OpenClaw"]);
        assert_eq!(v.agent.as_deref(), Some("claude-code"));
    }
}
