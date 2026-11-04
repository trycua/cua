// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Two small prompts about This Mac, the user's real desktop, as every app
//! shows them. Each has one main button and one way out, and neither ever
//! stops an agent.
//!
//! * **A second agent** ([`second_agent`]). An agent starts using This
//!   Mac's screen, mouse or keyboard while another one is already using
//!   them: "They'll share your mouse. Add a Space so they each get their
//!   own?" Detected from this Mac's access log, which the host already
//!   reads for the app (`host.status` → `recentAccess`): `cua-spacesd`
//!   ends the line of an agent's call with the agent session that made it
//!   (` · agent Claude Code (agent:claude-code-4242)`, one per `cua mcp`
//!   process). Two sessions on the desktop services within
//!   [`ACTIVE_MS`], the newer one first seen in that window, is the
//!   moment. Shown at most once a day per computer, never after "Don't
//!   ask again" ([`NudgeMemory`]).
//! * **A risky action** ([`risky_action`]). `cua mcp` asks before it runs
//!   something risky on This Mac (`cua config set nudges.risky_actions
//!   on`; off by default): "Run this in a separate Space instead?" The app
//!   draws the question; no answer in time means "Run it here".
//!
//! What this can't see: an agent that drives the desktop with its own Cua
//! Driver (not through Cua Spaces) writes no access line, and the agent's
//! name and session are its own claim (the account before the tag is
//! verified).

use serde::{Deserialize, Serialize};

use crate::host::HostAccess;

/// How recent a desktop access must be for its agent to count as working
/// now. The host writes at most one line per caller and service a minute
/// while it keeps calling, so three minutes covers a working session.
pub const ACTIVE_MS: i64 = 3 * 60_000;

/// The tag `cua-spacesd` puts before the agent session on an access line
/// (`cua-spacesd-server`'s `auth::AGENT_SESSION_TAG`; keep in sync).
pub const AGENT_SESSION_TAG: &str = " \u{b7} agent ";

/// The services that move the mouse, type or read the screen: two agents
/// on these share one pointer. A shell, files or watching a stream do not.
const DESKTOP_SERVICES: &[&str] = &[
    "ComputerService",
    "DriverService",
    "DesktopService",
    "AccessibilityService",
    "WindowsService",
    "MCP",
];

/// The caller before the tag and the agent session after it (`Claude
/// Code`, `agent:claude-code-4242`), when the line names one.
pub fn agent_session(who: &str) -> (&str, Option<(&str, &str)>) {
    let Some((caller, rest)) = who.rsplit_once(AGENT_SESSION_TAG) else {
        return (who, None);
    };
    let session = rest
        .strip_suffix(')')
        .and_then(|r| r.rsplit_once(" ("))
        .filter(|(name, id)| !name.is_empty() && id.starts_with("agent:"));
    match session {
        Some(s) => (caller, Some(s)),
        None => (who, None),
    }
}

/// Whether an access line used the desktop (the screen, mouse or keyboard).
pub fn uses_desktop(who: &str, what: &str) -> bool {
    if crate::host::access_kind(who, what) != crate::host::HostAccessKind::Connection {
        return false;
    }
    let service = what.split(" (").next().unwrap_or(what);
    let service = service.split('/').next().unwrap_or(service);
    DESKTOP_SERVICES.contains(&service)
}

/// One agent session on This Mac's desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAgent {
    /// `agent:claude-code-4242`.
    pub session: String,
    /// "Claude Code".
    pub name: String,
    /// Its first desktop access in the log (epoch ms).
    pub first_ms: i64,
    /// Its latest one.
    pub last_ms: i64,
}

/// The agent sessions that used This Mac's desktop and are still working
/// at `now_ms` (a desktop access in the last [`ACTIVE_MS`]), the one that
/// started last first.
pub fn desktop_agents(recent: &[HostAccess], now_ms: i64) -> Vec<DesktopAgent> {
    let mut agents: Vec<DesktopAgent> = vec![];
    for a in recent {
        if !uses_desktop(&a.who, &a.what) {
            continue;
        }
        let (_, Some((name, session))) = agent_session(&a.who) else {
            continue;
        };
        match agents.iter_mut().find(|x| x.session == session) {
            Some(x) => {
                x.first_ms = x.first_ms.min(a.at_ms);
                x.last_ms = x.last_ms.max(a.at_ms);
            }
            None => agents.push(DesktopAgent {
                session: session.into(),
                name: name.into(),
                first_ms: a.at_ms,
                last_ms: a.at_ms,
            }),
        }
    }
    agents.retain(|x| x.last_ms >= now_ms - ACTIVE_MS);
    agents.sort_by(|a, b| b.first_ms.cmp(&a.first_ms).then(a.session.cmp(&b.session)));
    agents
}

/// What an app remembers about the second-agent prompt, for one computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NudgeMemory {
    /// The local day (`2026-10-18`) it was last shown on this computer.
    pub shown_day: Option<String>,
    /// "Don't ask again".
    pub off: bool,
}

/// Whether the prompt may show `today` (the local day, `2026-10-18`):
/// not turned off, and not shown today on this computer.
pub fn allows(memory: &NudgeMemory, today: &str) -> bool {
    !memory.off && memory.shown_day.as_deref() != Some(today)
}

/// A prompt as drawn: a line saying what happened, a short question, one
/// main button and one way out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Nudge {
    /// `second-agent` or `risky-action`.
    pub kind: String,
    /// What it is about: the two sessions, or the request id. The same
    /// moment keeps the same key.
    pub key: String,
    /// The small line on top: "Codex just started on this Mac".
    pub eyebrow: String,
    /// "Claude Code is already working here."
    pub title: String,
    pub body: String,
    /// A quieter line under the body, if any.
    pub note: Option<String>,
    /// The main button.
    pub primary: String,
    /// The way out.
    pub secondary: String,
    /// "Don't ask again", when the prompt has one.
    pub never: Option<String>,
}

/// The second-agent prompt's moment, if this is one: two agent sessions
/// are using This Mac's desktop and the newer one started in the last
/// [`ACTIVE_MS`]. `None` too when [`allows`] says no.
pub fn second_agent(
    recent: &[HostAccess],
    now_ms: i64,
    today: &str,
    memory: &NudgeMemory,
) -> Option<Nudge> {
    if !allows(memory, today) {
        return None;
    }
    let agents = desktop_agents(recent, now_ms);
    let [newer, other, ..] = agents.as_slice() else {
        return None;
    };
    if newer.first_ms < now_ms - ACTIVE_MS || newer.first_ms <= other.first_ms {
        return None;
    }
    let same = newer.name == other.name;
    Some(Nudge {
        kind: "second-agent".into(),
        key: format!("{}+{}", other.session, newer.session),
        eyebrow: if same {
            format!("Another {} just started on this Mac", newer.name)
        } else {
            format!("{} just started on this Mac", newer.name)
        },
        title: format!("{} is already working here.", other.name),
        body: "They'll share your mouse. Add a Space so they each get their own?".into(),
        note: Some("It's made in the background. You can keep working.".into()),
        primary: "Add a Space".into(),
        secondary: "Keep sharing".into(),
        never: Some("Don't ask again".into()),
    })
}

/// A risky action `cua mcp` is holding for an answer (its request file).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RiskyRequest {
    pub id: String,
    /// "Claude Code".
    pub agent: String,
    /// What it wants to do, in plain words: "empty the Trash".
    pub action: String,
    /// When it asked (epoch ms).
    pub at_ms: i64,
    /// When it goes ahead without an answer (epoch ms).
    pub expires_ms: i64,
}

/// The risky-action prompt for `request`, or `None` once it has gone ahead
/// on its own (`now_ms` past its deadline).
pub fn risky_action(request: &RiskyRequest, now_ms: i64) -> Option<Nudge> {
    if request.id.is_empty() || now_ms >= request.expires_ms {
        return None;
    }
    let agent = if request.agent.trim().is_empty() {
        "An agent"
    } else {
        request.agent.trim()
    };
    let action = request.action.trim().trim_end_matches('.');
    let secs = ((request.expires_ms - now_ms) as f64 / 1000.0).ceil() as i64;
    Some(Nudge {
        kind: "risky-action".into(),
        key: request.id.clone(),
        eyebrow: format!("{agent} wants to {action} on your desktop"),
        title: "Run this in a separate Space instead?".into(),
        body: "A fresh computer inside this Mac. If something goes wrong there, your files and settings stay safe.".into(),
        note: Some(format!(
            "If you don't choose, it runs here in {secs} second{}.",
            if secs == 1 { "" } else { "s" }
        )),
        primary: "Use a separate Space".into(),
        secondary: "Run it here".into(),
        never: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000;

    fn line(at_ms: i64, who: &str, what: &str) -> HostAccess {
        HostAccess {
            at_ms,
            via: "relay".into(),
            who: who.into(),
            what: what.into(),
        }
    }

    fn agent(name: &str, session: &str) -> String {
        format!("ada@example.com (u1){AGENT_SESSION_TAG}{name} ({session})")
    }

    #[test]
    fn an_access_line_names_its_agent_session() {
        assert_eq!(
            agent_session(&agent("Claude Code", "agent:claude-code-4242")),
            (
                "ada@example.com (u1)",
                Some(("Claude Code", "agent:claude-code-4242"))
            )
        );
        assert_eq!(
            agent_session("ada@example.com (u1)"),
            ("ada@example.com (u1)", None)
        );
        // A tag without a real session is just text.
        let odd = format!("x{AGENT_SESSION_TAG}nobody");
        assert_eq!(agent_session(&odd), (odd.as_str(), None));
    }

    #[test]
    fn only_screen_mouse_and_keyboard_count() {
        let who = agent("Codex", "agent:codex-1");
        assert!(uses_desktop(&who, "DriverService"));
        assert!(uses_desktop(&who, "ComputerService"));
        assert!(!uses_desktop(&who, "ProcessService"));
        assert!(!uses_desktop(&who, "FilesystemService"));
        assert!(!uses_desktop(
            &who,
            "ComputerService/Screenshot (thumbnail)"
        ));
        assert!(!uses_desktop(&who, "DriverService/ListTools (background)"));
        assert!(!uses_desktop(
            &who,
            "refused DriverService (view-only share)"
        ));
    }

    #[test]
    fn a_second_agent_on_the_desktop_is_the_moment() {
        let log = vec![
            // Newest first, as the host gives it.
            line(
                NOW - 10_000,
                &agent("Codex", "agent:codex-77"),
                "DriverService",
            ),
            line(
                NOW - 20_000,
                &agent("Claude Code", "agent:claude-code-4242"),
                "ComputerService",
            ),
            line(
                NOW - 30 * 60_000,
                &agent("Claude Code", "agent:claude-code-4242"),
                "DriverService",
            ),
        ];
        let n = second_agent(&log, NOW, "2026-10-18", &NudgeMemory::default()).unwrap();
        assert_eq!(n.kind, "second-agent");
        assert_eq!(n.eyebrow, "Codex just started on this Mac");
        assert_eq!(n.title, "Claude Code is already working here.");
        assert_eq!(
            n.body,
            "They'll share your mouse. Add a Space so they each get their own?"
        );
        assert_eq!(
            (n.primary.as_str(), n.secondary.as_str()),
            ("Add a Space", "Keep sharing")
        );
        assert_eq!(n.never.as_deref(), Some("Don't ask again"));
        assert_eq!(n.key, "agent:claude-code-4242+agent:codex-77");
    }

    #[test]
    fn two_sessions_of_one_agent_count_too() {
        let log = vec![
            line(
                NOW - 5_000,
                &agent("Claude Code", "agent:claude-code-2"),
                "DriverService",
            ),
            line(
                NOW - 50_000,
                &agent("Claude Code", "agent:claude-code-1"),
                "DriverService",
            ),
            line(
                NOW - 9 * 60_000,
                &agent("Claude Code", "agent:claude-code-1"),
                "DriverService",
            ),
        ];
        let n = second_agent(&log, NOW, "d", &NudgeMemory::default()).unwrap();
        assert_eq!(n.eyebrow, "Another Claude Code just started on this Mac");
    }

    #[test]
    fn no_moment_without_two_working_agents_or_a_new_one() {
        let mem = NudgeMemory::default();
        // One agent.
        let one = vec![line(
            NOW - 1_000,
            &agent("Codex", "agent:codex-1"),
            "DriverService",
        )];
        assert!(second_agent(&one, NOW, "d", &mem).is_none());
        // The other one stopped a while ago.
        let stopped = vec![
            line(
                NOW - 1_000,
                &agent("Codex", "agent:codex-1"),
                "DriverService",
            ),
            line(
                NOW - ACTIVE_MS - 1,
                &agent("Claude Code", "agent:cc-1"),
                "DriverService",
            ),
        ];
        assert!(second_agent(&stopped, NOW, "d", &mem).is_none());
        // Both have been at it for a long time: not a start.
        let long = vec![
            line(
                NOW - 1_000,
                &agent("Codex", "agent:codex-1"),
                "DriverService",
            ),
            line(
                NOW - 2_000,
                &agent("Claude Code", "agent:cc-1"),
                "DriverService",
            ),
            line(
                NOW - 20 * 60_000,
                &agent("Codex", "agent:codex-1"),
                "DriverService",
            ),
            line(
                NOW - 30 * 60_000,
                &agent("Claude Code", "agent:cc-1"),
                "DriverService",
            ),
        ];
        assert!(second_agent(&long, NOW, "d", &mem).is_none());
        // The second one only runs commands: it doesn't share the mouse.
        let shell = vec![
            line(
                NOW - 1_000,
                &agent("Codex", "agent:codex-1"),
                "ProcessService",
            ),
            line(
                NOW - 2_000,
                &agent("Claude Code", "agent:cc-1"),
                "DriverService",
            ),
        ];
        assert!(second_agent(&shell, NOW, "d", &mem).is_none());
        // Lines from before the session tag existed name no agent.
        let old = vec![
            line(NOW - 1_000, "ada@example.com (u1)", "DriverService"),
            line(NOW - 2_000, "token (claims Codex)", "DriverService"),
        ];
        assert!(second_agent(&old, NOW, "d", &mem).is_none());
    }

    #[test]
    fn once_a_day_per_computer_and_never_after_dont_ask_again() {
        let log = vec![
            line(
                NOW - 1_000,
                &agent("Codex", "agent:codex-1"),
                "DriverService",
            ),
            line(
                NOW - 2_000,
                &agent("Claude Code", "agent:cc-1"),
                "DriverService",
            ),
            line(
                NOW - 60_000,
                &agent("Claude Code", "agent:cc-1"),
                "DriverService",
            ),
        ];
        let shown = NudgeMemory {
            shown_day: Some("2026-10-18".into()),
            off: false,
        };
        assert!(second_agent(&log, NOW, "2026-10-18", &shown).is_none());
        assert!(second_agent(&log, NOW, "2026-10-19", &shown).is_some());
        let off = NudgeMemory {
            shown_day: None,
            off: true,
        };
        assert!(second_agent(&log, NOW, "2026-10-19", &off).is_none());
    }

    #[test]
    fn recent_access_names_the_agent_not_its_session() {
        assert_eq!(
            crate::host::access_who("relay", &agent("Codex", "agent:codex-1")),
            "Codex for ada@example.com (u1)"
        );
        assert_eq!(
            crate::host::access_who(
                "token",
                &format!("token (claims Codex){AGENT_SESSION_TAG}Codex (agent:codex-1)")
            ),
            "Codex (access token)"
        );
    }

    #[test]
    fn the_prompts_answer_through_dispatch() {
        use serde_json::json;
        let log = json!([
            {"atMs": NOW - 1_000, "via": "relay", "who": agent("Codex", "agent:codex-1"), "what": "DriverService"},
            {"atMs": NOW - 2_000, "via": "relay", "who": agent("Claude Code", "agent:cc-1"), "what": "DriverService"},
            {"atMs": NOW - 60_000, "via": "relay", "who": agent("Claude Code", "agent:cc-1"), "what": "DriverService"},
        ]);
        let n = crate::dispatch::call_value(
            "nudge.secondAgent",
            json!({"recentAccess": log, "nowMs": NOW, "today": "2026-10-18"}),
        )
        .unwrap();
        assert_eq!(n["primary"], "Add a Space");
        let off = crate::dispatch::call_value(
            "nudge.secondAgent",
            json!({"recentAccess": log, "nowMs": NOW, "today": "2026-10-18", "memory": {"off": true}}),
        )
        .unwrap();
        assert!(off.is_null());
        let r = crate::dispatch::call_value(
            "nudge.riskyAction",
            json!({"request": {"id": "r", "agent": "Codex", "action": "uninstall an app", "atMs": NOW, "expiresMs": NOW + 9_000}, "nowMs": NOW}),
        )
        .unwrap();
        assert_eq!(
            r["eyebrow"],
            "Codex wants to uninstall an app on your desktop"
        );
    }

    #[test]
    fn the_risky_prompt_says_what_and_runs_here_by_default() {
        let r = RiskyRequest {
            id: "r1".into(),
            agent: "Claude Code".into(),
            action: "empty the Trash".into(),
            at_ms: NOW,
            expires_ms: NOW + 30_000,
        };
        let n = risky_action(&r, NOW + 500).unwrap();
        assert_eq!(
            n.eyebrow,
            "Claude Code wants to empty the Trash on your desktop"
        );
        assert_eq!(n.title, "Run this in a separate Space instead?");
        assert_eq!(
            (n.primary.as_str(), n.secondary.as_str()),
            ("Use a separate Space", "Run it here")
        );
        assert_eq!(
            n.note.as_deref(),
            Some("If you don't choose, it runs here in 30 seconds.")
        );
        assert!(n.never.is_none());
        assert!(risky_action(&r, NOW + 30_000).is_none());
        let nameless = RiskyRequest {
            agent: " ".into(),
            ..r
        };
        assert!(
            risky_action(&nameless, NOW)
                .unwrap()
                .eyebrow
                .starts_with("An agent wants to")
        );
    }
}
