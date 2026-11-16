// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! @mentioning an agent in a Space's composer: who the picker offers, what
//! sending does for each @mention, and what the thread says when an agent
//! can't be woken.
//!
//! - **The picker** ([`candidates`]): the agents already in the Space, then
//!   each agent that can start in it (`agent_add`) under a free handle.
//! - **Sending** ([`plan`]): the message is always posted. Each @mention of
//!   an agent in the Space wakes it (the thread does that); an @mention of an
//!   agent that isn't in the Space yet adds it first, asleep, so the posted
//!   message wakes it. An agent whose app has no sign-in it can use in a
//!   Space is not added: the step says why ([`CantWake`]), and the thread
//!   shows that under the message with Connect.
//! - **Ask first** ([`first_message`]): the job is posted as `@<handle> <job>`,
//!   the same summon as a message typed in the Space.
//!
//! The @mention grammar is the thread's (`cua_spaces_ext::thread::parse_mentions`).

use serde::{Deserialize, Serialize};

use crate::connectors::{self, ConnectorsInput};

/// An agent that can start in a Space (`agent_add`'s harnesses), with the
/// handle it gets and its name.
struct Startable {
    harness: &'static str,
    handle: &'static str,
    label: &'static str,
    /// The setup id its sign-in is checked under (Settings, Connections).
    app: &'static str,
}

const STARTABLE: &[Startable] = &[
    Startable {
        harness: "claude-code",
        handle: "claude",
        label: "Claude Code",
        app: "claude-code",
    },
    Startable {
        harness: "openai-codex",
        handle: "codex",
        label: "Codex",
        app: "codex",
    },
    Startable {
        harness: "hermes",
        handle: "hermes",
        label: "Hermes",
        app: "hermes",
    },
    Startable {
        harness: "openclaw",
        handle: "openclaw",
        label: "OpenClaw",
        app: "openclaw",
    },
];

/// The harness an agent id runs as in a Space: Ask first's ids are the
/// harness ids (`openai-codex`); `codex` is the app's setup id for it.
fn harness_of(id: &str) -> &str {
    match id {
        "codex" => "openai-codex",
        other => other,
    }
}

fn startable(harness: &str) -> Option<&'static Startable> {
    STARTABLE.iter().find(|s| s.harness == harness_of(harness))
}

/// `@name` tokens in `text` (lowercased, deduplicated, in order), as the
/// thread reads them: `a-z`, `0-9`, `.`, `_`, `-`; a trailing `.` or `-`
/// ends the sentence, not the name; an `@` inside a word (an email) is not
/// a mention.
pub fn parse_mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')) {
            let mut j = i + 1;
            let mut name = String::new();
            while j < chars.len() {
                let c = chars[j].to_ascii_lowercase();
                if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                    name.push(c);
                    j += 1;
                } else {
                    break;
                }
            }
            let name = name.trim_end_matches(['.', '-']).to_string();
            let valid = !name.is_empty()
                && name.len() <= 64
                && name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
            if valid && !out.contains(&name) {
                out.push(name);
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// An agent already in the Space.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SummonMember {
    /// What the Space calls it: the `@handle` the thread matches.
    pub handle: String,
    /// Its name on the host (`claude-8` in `cua agent ls`), when that differs
    /// from the handle. Never what the page types; an @mention of it still
    /// reaches this member (the thread accepts it as an alias).
    pub key: String,
    /// It runs on this device's own sign-ins (a Home's agent, added from here):
    /// waking it is checked against them, so "not connected" is said up front
    /// instead of a message nobody answers. Other members run on their owner's
    /// keys, which this device cannot see.
    pub mine: bool,
    /// `claude-code`, `codex`, ... (empty when the thread doesn't say).
    pub harness: String,
    pub label: String,
}

/// What summoning reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SummonInput {
    pub members: Vec<SummonMember>,
    /// People's handles, so a new agent never takes one.
    pub people: Vec<String>,
    /// Agents can be added to this Space (`thread.agentAdd` there, and you may post).
    pub can_add: bool,
    /// The text is what the person typed in this Space's thread: an @mention
    /// of nobody here is reported (never a silent no-op). Off for Ask first's
    /// job, which may @mention computers and Spaces as places.
    pub report_unknown: bool,
    /// Coding apps and saved sign-ins (Settings, Connections).
    pub connectors: ConnectorsInput,
}

/// A row in the composer's @ picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummonCandidate {
    pub handle: String,
    pub label: String,
    pub harness: String,
    /// Already in the Space.
    pub member: bool,
    /// "In this Space", "Starts in this Space", "Not connected yet".
    pub detail: String,
}

/// A free handle: `base`, else `base-2`, ...
fn free_handle(base: &str, taken: &[String]) -> String {
    let taken: Vec<String> = taken.iter().map(|t| t.to_ascii_lowercase()).collect();
    if !taken.iter().any(|t| t == base) {
        return base.into();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|h| !taken.contains(h))
        .unwrap_or_else(|| base.into())
}

fn taken(input: &SummonInput) -> Vec<String> {
    input
        .members
        .iter()
        .flat_map(|m| [m.handle.clone(), m.key.clone()])
        .filter(|h| !h.is_empty())
        .chain(input.people.iter().cloned())
        .collect()
}

/// The member an `@token` names: its handle, else (as an alias) its host
/// name. The same rule the thread applies when it reads the posted text.
pub fn member_named<'a>(input: &'a SummonInput, token: &str) -> Option<&'a SummonMember> {
    input
        .members
        .iter()
        .find(|m| m.handle.eq_ignore_ascii_case(token))
        .or_else(|| {
            input
                .members
                .iter()
                .find(|m| !m.key.is_empty() && m.key.eq_ignore_ascii_case(token))
        })
}

/// The handle a new `harness` agent gets in this Space.
fn new_handle(input: &SummonInput, s: &Startable) -> String {
    free_handle(s.handle, &taken(input))
}

/// The agents the picker offers: those in the Space first, then each one
/// that can start in it (one per kind not already there).
pub fn candidates(input: &SummonInput) -> Vec<SummonCandidate> {
    let mut out: Vec<SummonCandidate> = input
        .members
        .iter()
        .map(|m| SummonCandidate {
            handle: m.handle.clone(),
            label: m.label.clone(),
            harness: m.harness.clone(),
            member: true,
            detail: "In this Space".into(),
        })
        .collect();
    if input.can_add {
        for s in STARTABLE {
            if input.members.iter().any(|m| m.harness == s.harness) {
                continue;
            }
            let ready = connectors::signed_in(&input.connectors, s.app);
            out.push(SummonCandidate {
                handle: new_handle(input, s),
                label: s.label.into(),
                harness: s.harness.into(),
                member: false,
                // The Connections words: a sign-in that stopped working needs a reconnect.
                detail: if ready {
                    "Starts in this Space".into()
                } else if connectors::needs_reconnect(&input.connectors, s.app) {
                    "Needs reconnect".into()
                } else {
                    "Not connected".into()
                },
            });
        }
    }
    out
}

/// Why an agent can't be woken, and what the thread offers under the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CantWake {
    /// `not-connected` (no sign-in it can use), `cant-add` (this Space can't
    /// add agents), `failed` (adding or waking it failed), `no-such-member`
    /// (an @mention of nobody in this Space).
    pub reason: String,
    pub handle: String,
    pub harness: String,
    /// The agent's name ("Claude Code").
    pub name: String,
    /// The setup id Connect runs for (Settings, Connections).
    pub app: String,
    /// "Claude Code isn't connected yet".
    pub text: String,
    pub detail: String,
    /// "Connect Claude Code" (absent when Connect can't help).
    pub connect_label: Option<String>,
    /// While Connect runs.
    pub connecting_label: String,
    /// The app's page in Settings, Connections ("Claude Code settings").
    pub open_label: Option<String>,
    /// Once connected: the message goes again.
    pub retry_label: String,
    /// After the retry.
    pub retried_text: String,
    /// Only you see it: it is about this device's sign-ins, never posted to the thread.
    pub only_you: bool,
    pub only_you_label: String,
    pub dismiss_label: String,
}

fn cant_wake(
    input: &SummonInput,
    s: &Startable,
    handle: &str,
    reason: &str,
    error: Option<&str>,
) -> CantWake {
    let name = s.label;
    let plan = connectors::connect_plan(&input.connectors, s.app);
    let providers: Vec<&str> = connectors::app_providers(s.app)
        .iter()
        .filter_map(|p| connectors::provider_label(p))
        .collect();
    let (text, detail, connect) = match reason {
        "not-connected" => (
            format!("{name} isn't connected yet"),
            if providers.is_empty() {
                format!("Connect {name} so it can work in your Spaces.")
            } else if providers.len() > 3 {
                format!(
                    "It needs a connected provider to work in your Spaces. Your message is saved: it goes again once {name} is connected."
                )
            } else {
                format!(
                    "It needs a sign-in to {} to work in your Spaces. Your message is saved: it goes again once {name} is connected.",
                    connectors::or_list(&providers)
                )
            },
            true,
        ),
        "cant-add" => (
            format!("{name} can't join this Space"),
            "This Space can't add agents. Ask its owner, or use another Space.".to_string(),
            false,
        ),
        _ => (
            format!("{name} couldn't start"),
            error
                .map(str::to_string)
                .unwrap_or_else(|| "Something went wrong. Send it again to try once more.".into()),
            false,
        ),
    };
    let in_settings = input
        .connectors
        .apps
        .iter()
        .any(|a| a.agent == s.app && a.installed);
    CantWake {
        reason: reason.into(),
        handle: handle.into(),
        harness: s.harness.into(),
        name: name.into(),
        app: s.app.into(),
        text,
        detail,
        connect_label: connect.then(|| format!("Connect {name}")),
        connecting_label: if plan.app_token {
            "Reading your Claude Code sign-in…".into()
        } else if plan.device {
            "Enter the code in your browser…".into()
        } else {
            "Connecting…".into()
        },
        open_label: in_settings.then(|| format!("{name} settings")),
        retry_label: "Send again".into(),
        retried_text: format!("{name} is connected. Sent again."),
        only_you: true,
        only_you_label: ONLY_YOU.into(),
        dismiss_label: "Dismiss".into(),
    }
}

/// The label on everything in a thread that only this device shows.
pub const ONLY_YOU: &str = "Only visible to you";

/// A line in the thread that only you see (it is never posted): a message
/// waiting for its Space, or a note about this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalNote {
    pub text: String,
    pub only_you: bool,
    pub only_you_label: String,
    /// Absent where dismissing makes no sense (a queued message goes once the Space is ready).
    pub dismiss_label: Option<String>,
}

/// The line under a message written while its Space is being made (`creating`),
/// or while it is being sent just after.
pub fn queued_note(creating: bool) -> LocalNote {
    LocalNote {
        text: if creating {
            "Sends when the Space is ready"
        } else {
            "Sending…"
        }
        .into(),
        only_you: true,
        only_you_label: ONLY_YOU.into(),
        dismiss_label: None,
    }
}

/// What sending does for one @mention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummonStep {
    pub handle: String,
    pub harness: String,
    /// `wake` (in the Space: the posted message wakes it), `add` (add it
    /// asleep under `handle` first, then post), `blocked` (post only; see
    /// `cant_wake`), `unknown` (nobody by that name is in the Space: post
    /// only; `cant_wake` says so).
    pub action: String,
    pub cant_wake: Option<CantWake>,
}

/// What sending `text` does: the steps before and around posting it, one
/// per @mentioned agent. People and unknown handles have no step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummonPlan {
    pub steps: Vec<SummonStep>,
}

/// Resolves an @handle to an agent kind not yet in the Space: its own handle
/// (`@claude`), or the free handle the picker offered (`@claude-2`).
fn startable_for(input: &SummonInput, handle: &str) -> Option<&'static Startable> {
    STARTABLE.iter().find(|s| {
        !input.members.iter().any(|m| m.harness == s.harness)
            && (handle == s.handle || handle == new_handle(input, s))
    })
}

/// The line under a message that @mentions nobody who is in the Space.
fn unknown_mention(input: &SummonInput, handle: &str) -> CantWake {
    let here: Vec<String> = input
        .members
        .iter()
        .map(|m| format!("@{}", m.handle))
        .collect();
    CantWake {
        reason: "no-such-member".into(),
        handle: handle.into(),
        harness: String::new(),
        name: String::new(),
        app: String::new(),
        text: format!("Nobody called @{handle} is in this Space"),
        detail: if here.is_empty() {
            "Your message was posted, but no agent is in this Space to read it. Add one first."
                .into()
        } else {
            format!(
                "Your message was posted, but it did not reach anyone. Agents here: {}.",
                here.join(", ")
            )
        },
        connect_label: None,
        connecting_label: String::new(),
        open_label: None,
        retry_label: String::new(),
        retried_text: String::new(),
        only_you: true,
        only_you_label: ONLY_YOU.into(),
        dismiss_label: "Dismiss".into(),
    }
}

pub fn plan(input: &SummonInput, text: &str) -> SummonPlan {
    let steps = parse_mentions(text)
        .into_iter()
        .filter_map(|h| {
            if let Some(m) = member_named(input, &h) {
                // In the Space already: the posted message wakes it. It runs on
                // its owner's keys, which this device can't see, so nothing is
                // checked here; the thread says if it can't start. One of this
                // device's own (a Home's agent) is: it runs on these sign-ins.
                let blocked = startable(&m.harness)
                    .filter(|s| m.mine && !connectors::signed_in(&input.connectors, s.app))
                    .map(|s| cant_wake(input, s, &m.handle, "not-connected", None));
                return Some(SummonStep {
                    handle: m.handle.clone(),
                    harness: m.harness.clone(),
                    action: if blocked.is_some() { "blocked" } else { "wake" }.into(),
                    cant_wake: blocked,
                });
            }
            let Some(s) = startable_for(input, &h) else {
                // A person, `@everyone`, or nobody: only the last is a mistake worth a line.
                let person = input.people.iter().any(|p| p.eq_ignore_ascii_case(&h));
                if !input.report_unknown || person || h == "everyone" {
                    return None;
                }
                return Some(SummonStep {
                    handle: h.clone(),
                    harness: String::new(),
                    action: "unknown".into(),
                    cant_wake: Some(unknown_mention(input, &h)),
                });
            };
            let blocked = if !input.can_add {
                Some(cant_wake(input, s, &h, "cant-add", None))
            } else if !connectors::signed_in(&input.connectors, s.app) {
                Some(cant_wake(input, s, &h, "not-connected", None))
            } else {
                None
            };
            Some(SummonStep {
                handle: h.clone(),
                harness: s.harness.into(),
                action: if blocked.is_some() { "blocked" } else { "add" }.into(),
                cant_wake: blocked,
            })
        })
        .collect();
    SummonPlan { steps }
}

/// The note for an agent whose add or wake failed with `error` (the host's words).
pub fn failed(input: &SummonInput, harness: &str, handle: &str, error: &str) -> Option<CantWake> {
    let s = startable(harness)?;
    // `ANTHROPIC_API_KEY is not set` reads as "api key".
    let e = error.to_ascii_lowercase().replace('_', " ");
    let reason = if [
        "api key",
        "sign in",
        "signed in",
        "credential",
        "unauthorized",
        "401",
        "login",
        "auth",
    ]
    .iter()
    .any(|w| e.contains(w))
    {
        "not-connected"
    } else {
        "failed"
    };
    Some(cant_wake(input, s, handle, reason, Some(error)))
}

/// Ask first's job, as the Space's first message: `@<handle> <job>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstMessage {
    /// What is posted.
    pub text: String,
    /// The agent's handle in the Space (null: this agent can't join a Space; the job is posted as typed).
    pub handle: Option<String>,
    pub harness: String,
}

/// `agent`'s first message for `prompt` (`agent` is Ask first's id).
pub fn first_message(input: &SummonInput, agent: &str, prompt: &str) -> FirstMessage {
    let prompt = prompt.trim();
    let harness = harness_of(agent).to_string();
    let handle = input
        .members
        .iter()
        .find(|m| m.harness == harness)
        .map(|m| m.handle.clone())
        .or_else(|| startable(&harness).map(|s| new_handle(input, s)));
    FirstMessage {
        text: match &handle {
            Some(h) => format!("@{h} {prompt}"),
            None => prompt.to_string(),
        },
        handle,
        harness,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_keys::AgentKeyInput;
    use crate::connectors::ConnectorAppInput;

    fn input(keys: &[&str]) -> SummonInput {
        SummonInput {
            members: vec![SummonMember {
                handle: "hermes".into(),
                harness: "hermes".into(),
                label: "Hermes".into(),
                ..Default::default()
            }],
            people: vec!["grace".into()],
            can_add: true,
            report_unknown: true,
            connectors: ConnectorsInput {
                apps: vec![ConnectorAppInput {
                    agent: "claude-code".into(),
                    name: "Claude Code".into(),
                    installed: true,
                    ..Default::default()
                }],
                keys: crate::agent_keys::AgentKeysInput {
                    keys: keys
                        .iter()
                        .map(|e| AgentKeyInput {
                            provider: "other".into(),
                            env: (*e).into(),
                            last4: "abcd".into(),
                            added_ms: 1,
                            expired: false,
                            label: None,
                            region: None,
                            fields: Default::default(),
                        })
                        .collect(),
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }

    #[test]
    fn mentions_read_as_the_thread_reads_them() {
        assert_eq!(
            parse_mentions("@claude run it, then @Codex. cc a@b.com and @x"),
            vec!["claude", "codex", "x"]
        );
        assert_eq!(parse_mentions("@re.view-1."), vec!["re.view-1"]);
        assert!(parse_mentions("@ @-").is_empty());
        assert_eq!(parse_mentions("@claude @claude"), vec!["claude"]);
    }

    #[test]
    fn picker_lists_members_then_agents_that_can_start() {
        let c = candidates(&input(&[]));
        assert_eq!(c[0].handle, "hermes");
        assert!(c[0].member);
        let rest: Vec<_> = c[1..].iter().map(|c| c.handle.as_str()).collect();
        assert_eq!(rest, ["claude", "codex", "openclaw"]);
        assert_eq!(c[1].label, "Claude Code");
        assert_eq!(c[1].detail, "Not connected");
        let c = candidates(&input(&["CLAUDE_CODE_OAUTH_TOKEN"]));
        assert_eq!(c[1].detail, "Starts in this Space");
        // A Space that can't add agents offers only its members.
        let mut i = input(&[]);
        i.can_add = false;
        assert_eq!(candidates(&i).len(), 1);
    }

    #[test]
    fn a_new_agent_never_takes_a_used_handle() {
        let mut i = input(&[]);
        i.people.push("claude".into());
        let c = candidates(&i);
        assert_eq!(
            c.iter()
                .find(|c| c.harness == "claude-code")
                .unwrap()
                .handle,
            "claude-2"
        );
        let p = plan(&i, "@claude-2 hi");
        assert_eq!(p.steps[0].harness, "claude-code");
    }

    #[test]
    fn the_hosts_name_for_a_member_is_an_alias_never_what_the_page_types() {
        // `cua agent ls` says claude-8; the Space calls the member @claude.
        let mut i = input(&[]);
        i.members.push(SummonMember {
            handle: "claude".into(),
            key: "claude-8".into(),
            harness: "claude-code".into(),
            label: "Claude Code".into(),
            ..Default::default()
        });
        // Ask first's message names the handle, whichever name it was given.
        assert_eq!(first_message(&i, "claude-code", "hi").text, "@claude hi");
        // The alias still wakes the member (no add, no unknown note).
        let s = &plan(&i, "@claude-8 hi").steps;
        assert_eq!(s.len(), 1);
        assert_eq!(
            (s[0].handle.as_str(), s[0].action.as_str()),
            ("claude", "wake")
        );
        // A new agent never takes the host name of a member either.
        i.members.clear();
        i.members.push(SummonMember {
            handle: "hermes".into(),
            key: "claude-2".into(),
            harness: "hermes".into(),
            label: "Hermes".into(),
            ..Default::default()
        });
        assert_eq!(
            candidates(&i)
                .iter()
                .find(|c| c.harness == "claude-code")
                .unwrap()
                .handle,
            "claude"
        );
    }

    #[test]
    fn a_homes_agent_without_a_sign_in_says_so_instead_of_waking_nothing() {
        let mut i = input(&[]);
        i.members.push(SummonMember {
            handle: "claude".into(),
            key: "claude-8".into(),
            harness: "claude-code".into(),
            label: "Claude Code".into(),
            mine: true,
        });
        let s = &plan(&i, "@claude hi").steps[0];
        assert_eq!(s.action, "blocked");
        let c = s.cant_wake.as_ref().unwrap();
        assert_eq!(c.reason, "not-connected");
        assert_eq!(c.text, "Claude Code isn't connected yet");
        assert_eq!(c.handle, "claude");
        // Signed in: woken. Not this device's: never checked against its sign-ins.
        let mut ok = input(&["CLAUDE_CODE_OAUTH_TOKEN"]);
        ok.members = i.members.clone();
        assert_eq!(plan(&ok, "@claude hi").steps[0].action, "wake");
        i.members[1].mine = false;
        assert_eq!(plan(&i, "@claude hi").steps[0].action, "wake");
    }

    #[test]
    fn an_at_of_nobody_here_is_said_plainly() {
        let p = plan(&input(&["CLAUDE_CODE_OAUTH_TOKEN"]), "@ghost are you there");
        let c = p.steps[0].cant_wake.clone().unwrap();
        assert_eq!(p.steps[0].action, "unknown");
        assert_eq!(c.reason, "no-such-member");
        assert_eq!(c.text, "Nobody called @ghost is in this Space");
        assert!(c.detail.contains("Agents here: @hermes"), "{}", c.detail);
        assert_eq!(c.connect_label, None);
        // A person, or Ask first's job (places are @mentioned there), is not reported.
        assert!(plan(&input(&[]), "@grace hi").steps.is_empty());
        let mut i = input(&[]);
        i.report_unknown = false;
        assert!(plan(&i, "@my-mac run it").steps.is_empty());
    }

    #[test]
    fn sending_adds_wakes_or_says_why_not() {
        // Signed in: Claude Code is added, then woken by the message; Hermes,
        // in the Space, is woken; people and unknown names have no step.
        let i = input(&["CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY"]);
        let p = plan(
            &i,
            "@claude fix the build, @hermes review, cc @grace @nobody",
        );
        let s: Vec<_> = p
            .steps
            .iter()
            .map(|s| (s.handle.as_str(), s.action.as_str()))
            .collect();
        // `@nobody` is nobody in this Space: it says so instead of doing nothing.
        assert_eq!(
            s,
            [("claude", "add"), ("hermes", "wake"), ("nobody", "unknown")]
        );

        // No sign-in: blocked, with Connect.
        let p = plan(&input(&[]), "@claude fix the build");
        let st = &p.steps[0];
        assert_eq!(st.action, "blocked");
        let c = st.cant_wake.as_ref().unwrap();
        assert_eq!(c.reason, "not-connected");
        assert_eq!(c.text, "Claude Code isn't connected yet");
        assert_eq!(c.connect_label.as_deref(), Some("Connect Claude Code"));
        assert_eq!(c.open_label.as_deref(), Some("Claude Code settings"));
        assert_eq!(c.retry_label, "Send again");
        assert_eq!(c.connecting_label, "Reading your Claude Code sign-in…");
        assert!(c.detail.contains("sign-in to Anthropic"));

        // A member runs on its owner's keys: woken, never checked against this device's.
        let p = plan(&input(&[]), "@hermes go");
        assert_eq!(p.steps[0].action, "wake");
        // A new Hermes needs a connected provider.
        let mut i = input(&[]);
        i.members.clear();
        let c = plan(&i, "@hermes go").steps[0].cant_wake.clone().unwrap();
        assert!(c.detail.contains("a connected provider"));

        // Codex isn't on This Mac: no settings page to open; Connect still asks for a key.
        let c = plan(&input(&[]), "@codex go").steps[0]
            .cant_wake
            .clone()
            .unwrap();
        assert_eq!(c.open_label, None);
        assert_eq!(c.connecting_label, "Connecting…");

        // A Space that can't add agents says so, without Connect.
        let mut i = input(&["CLAUDE_CODE_OAUTH_TOKEN"]);
        i.can_add = false;
        let c = plan(&i, "@claude go").steps[0].cant_wake.clone().unwrap();
        assert_eq!(c.reason, "cant-add");
        assert_eq!(c.connect_label, None);
    }

    #[test]
    fn device_notes_say_only_you_see_them() {
        let c = plan(&input(&[]), "@claude go").steps[0]
            .cant_wake
            .clone()
            .unwrap();
        assert!(c.only_you);
        assert_eq!(c.only_you_label, "Only visible to you");
        assert_eq!(c.dismiss_label, "Dismiss");
        let q = queued_note(true);
        assert!(q.only_you);
        assert_eq!(q.text, "Sends when the Space is ready");
        assert_eq!(q.dismiss_label, None);
        assert_eq!(queued_note(false).text, "Sending…");
    }

    #[test]
    fn a_failed_add_reads_as_its_reason() {
        let i = input(&[]);
        let c = failed(&i, "claude-code", "claude", "ANTHROPIC_API_KEY is not set").unwrap();
        assert_eq!(c.reason, "not-connected");
        let c = failed(&i, "claude-code", "claude", "disk full").unwrap();
        assert_eq!(c.reason, "failed");
        assert_eq!(c.text, "Claude Code couldn't start");
        assert_eq!(c.detail, "disk full");
        assert!(failed(&i, "nope", "x", "e").is_none());
    }

    #[test]
    fn ask_first_posts_the_job_with_a_mention() {
        let i = input(&[]);
        let m = first_message(&i, "claude-code", "  fix the build ");
        assert_eq!(m.text, "@claude fix the build");
        assert_eq!(m.handle.as_deref(), Some("claude"));
        let m = first_message(&i, "openai-codex", "go");
        assert_eq!(
            (m.text.as_str(), m.harness.as_str()),
            ("@codex go", "openai-codex")
        );
        // An agent already in the Space keeps its handle.
        assert_eq!(first_message(&i, "hermes", "go").text, "@hermes go");
        // An agent that can't join a Space: the job as typed.
        let m = first_message(&i, "goose", "go");
        assert_eq!((m.text.as_str(), m.handle), ("go", None));
    }
}
