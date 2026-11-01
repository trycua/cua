// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Tips: one at a time, each with a real trigger, one line and at most one
//! action. They replace the sidebar's "Get started" checklist and the agent
//! banner. A tip shows on the page it belongs to (the Spaces page or a
//! Space's page); a tip that belongs to no page shows in the sidebar, only
//! while no page shows one.
//!
//! | Tip           | Where                     | Trigger                                   | Action          | Done by                                   |
//! |---------------|---------------------------|-------------------------------------------|-----------------|-------------------------------------------|
//! | `try-agent`   | Spaces page, Space page   | the newest running Space (or this page's) | Copy (request)  | Next tip, ×                               |
//! | `power`       | Spaces page               | a stopped Space                           | none            | powering from a tile, Next tip, ×         |
//! | `bulk-select` | Spaces page               | 3+ deletable stopped and a nearly full disk | Select stopped | a bulk delete, Next tip, ×                |
//! | `share`       | Space page                | a running Space's page                    | Share…          | sharing any Space, Next tip, ×            |
//! | `on-auto`     | sidebar                   | a second computer is online               | Copy (command)  | ×                                         |
//!
//! The `try-agent` request names your own agent: the first connected of
//! Hermes, Claude Code, Codex and Cursor (in that order, never Claude
//! first), else the first installed, else "your agent".
//!
//! "Don't show tips" turns them all off; Settings, General turns them back
//! on (the ones not done come back) and Reset tips starts them over. The
//! state lives in the app settings ([`crate::settings::AppSettings::tips`]),
//! so every shell agrees. Tips send no usage events: the telemetry schema
//! has no words for them.

use serde::{Deserialize, Serialize};

/// Every tip, in the order they are offered.
pub const TIP_IDS: [&str; 5] = ["try-agent", "power", "bulk-select", "share", "on-auto"];

/// The agents a tip names, in order, with their names (setup ids, as
/// `cua-agent-setup`'s registry has them).
pub const AGENT_ORDER: [(&str, &str); 4] = [
    ("hermes", "Hermes"),
    ("claude-code", "Claude Code"),
    ("codex", "Codex"),
    ("cursor", "Cursor"),
];

/// The command the `on-auto` tip copies.
pub const ON_AUTO_COMMAND: &str = "cua create --on auto";

/// "Next tip".
pub const NEXT: &str = "Next tip";
/// "Don't show tips".
pub const DONT_SHOW: &str = "Don't show tips";
/// The × (accessibility label).
pub const CLOSE: &str = "Close";

/// Tips as remembered (in the app settings).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TipsState {
    /// "Don't show tips" was chosen (Settings' Show tips is off).
    pub off: bool,
    /// Tips done, skipped or closed: they never come back (until Reset).
    pub done: Vec<String>,
}

/// Where a tip shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TipPlace {
    /// Above the Spaces page's grid.
    #[default]
    Spaces,
    /// On a Space's page.
    Space,
    /// At the foot of the sidebar.
    Sidebar,
}

/// What the tips read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TipFacts {
    /// The newest running Space's name (the Spaces page).
    pub newest_running: Option<String>,
    /// On a running Space's page: its name.
    pub space_page: Option<String>,
    /// Agents with Cua set up (setup ids).
    pub agents_connected: Vec<String>,
    /// Agents found on this computer (setup ids).
    pub agents_installed: Vec<String>,
    /// A Space is stopped.
    pub has_stopped: bool,
    /// How many stopped Spaces you may delete.
    pub deletable_stopped: u32,
    /// This computer's disk is nearly full.
    pub disk_nearly_full: bool,
    /// A second computer of yours is online.
    pub second_computer_online: bool,
    /// You already shared a Space.
    pub shared_any: bool,
    /// A page shows a tip now (the sidebar's waits).
    pub page_tip_shown: bool,
}

/// What a tip's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TipActionKind {
    /// No button.
    None,
    /// Copies `value` (the request).
    Copy,
    /// Opens the list for this computer, biggest first, with the stopped
    /// Spaces picked.
    SelectStopped,
    /// Opens the Share sheet for this page's Space.
    Share,
    /// Copies `value` (a command).
    CopyCommand,
}

/// A tip's one action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TipAction {
    /// What it does.
    pub kind: TipActionKind,
    /// Its label ("Copy", "Select stopped", "Share…"; empty: no button).
    pub label: String,
    /// What a copy puts on the clipboard.
    pub value: Option<String>,
    /// What the button says for a moment after a copy ("Copied").
    pub done_label: Option<String>,
}

/// A link that opens the request in the agent itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TipLink {
    /// "Open in Cursor".
    pub label: String,
    /// The agent's deep link with the request in it.
    pub url: String,
}

/// A tip as drawn: a soft amber lightbulb, one line, at most one action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tip {
    /// One of [`TIP_IDS`].
    pub id: String,
    /// Where it shows.
    pub place: TipPlace,
    /// The line ("Try cua-builder in Claude Code").
    pub text: String,
    /// Read-only text under it, with Copy (the request, the command).
    pub code: Option<String>,
    /// The agent the line names (setup id; none: "your agent").
    pub agent: Option<String>,
    /// Its action.
    pub action: TipAction,
    /// "Next tip" (none: the sidebar's tip has only ×).
    pub next: Option<String>,
    /// "Don't show tips" (none in the sidebar).
    pub dont_show: Option<String>,
    /// The × (accessibility label).
    pub close: String,
    /// A second action next to Copy: opens the request in the named agent,
    /// only for an agent with a prompt link (Cursor).
    pub open_in: Option<TipLink>,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum TipsEvent {
    /// Next tip on `id`: it is skipped for good.
    Next {
        /// The tip.
        id: String,
    },
    /// × on `id`: closed for good.
    Close {
        /// The tip.
        id: String,
    },
    /// "Don't show tips".
    DontShow,
    /// A Space was turned on or off from a tile (the `power` tip is done).
    Powered,
    /// Spaces were deleted together (the `bulk-select` tip is done).
    BulkDeleted,
    /// A Space was shared (the `share` tip is done).
    Shared,
    /// Settings' Show tips switch.
    ShowTips {
        /// On.
        on: bool,
    },
    /// Settings' Reset tips: every tip comes back.
    Reset,
}

/// The agent a tip names: the first connected in [`AGENT_ORDER`], else the
/// first installed, else none.
pub fn agent(facts: &TipFacts) -> Option<(&'static str, &'static str)> {
    let first = |ids: &[String]| {
        AGENT_ORDER
            .iter()
            .find(|(id, _)| ids.iter().any(|a| a == id))
            .copied()
    };
    first(&facts.agents_connected).or_else(|| first(&facts.agents_installed))
}

/// Cursor's prompt deep link: opens Cursor with `text` in the chat.
pub const CURSOR_PROMPT_LINK: &str = "cursor://anysphere.cursor-deeplink/prompt?text=";

/// `text` for a URL's query: everything but RFC 3986's unreserved
/// characters percent-encoded (UTF-8).
pub fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The link that opens `request` in the agent `id`, for an agent that has
/// one (Cursor).
pub fn open_in_url(id: &str, request: &str) -> Option<String> {
    (id == "cursor").then(|| format!("{CURSOR_PROMPT_LINK}{}", url_encode(request)))
}

/// The request a `try-agent` tip copies.
pub fn request(space: &str) -> String {
    format!("Open the Cua Space \"{space}\" and tell me what's on its screen.")
}

fn action(kind: TipActionKind, label: &str, value: Option<String>) -> TipAction {
    let copies = matches!(kind, TipActionKind::Copy | TipActionKind::CopyCommand);
    TipAction {
        kind,
        label: label.into(),
        value,
        done_label: copies.then(|| "Copied".into()),
    }
}

fn tip(id: &str, place: TipPlace, text: String, action: TipAction) -> Tip {
    let page = place != TipPlace::Sidebar;
    Tip {
        id: id.into(),
        place,
        text,
        code: None,
        agent: None,
        action,
        next: page.then(|| NEXT.into()),
        dont_show: page.then(|| DONT_SHOW.into()),
        close: CLOSE.into(),
        open_in: None,
    }
}

fn try_agent(place: TipPlace, space: &str, facts: &TipFacts) -> Tip {
    let named = agent(facts);
    let text = format!(
        "Try {space} in {}",
        named.map(|(_, n)| n).unwrap_or("your agent")
    );
    let req = request(space);
    let mut t = tip(
        "try-agent",
        place,
        text,
        action(TipActionKind::Copy, "Copy", Some(req.clone())),
    );
    t.code = Some(req.clone());
    t.agent = named.map(|(id, _)| id.to_owned());
    t.open_in = named.and_then(|(id, name)| {
        open_in_url(id, &req).map(|url| TipLink {
            label: format!("Open in {name}"),
            url,
        })
    });
    t
}

/// The tip for `place` now, if any: the first that applies and is not done.
pub fn view(place: TipPlace, facts: &TipFacts, state: &TipsState) -> Option<Tip> {
    if state.off {
        return None;
    }
    let open = |id: &str| !state.done.iter().any(|d| d == id);
    match place {
        TipPlace::Spaces => {
            if let Some(space) = facts
                .newest_running
                .as_deref()
                .filter(|_| open("try-agent"))
            {
                return Some(try_agent(place, space, facts));
            }
            if facts.has_stopped && open("power") {
                return Some(tip(
                    "power",
                    place,
                    "Point at a Space to turn it on or off without opening it.".into(),
                    action(TipActionKind::None, "", None),
                ));
            }
            if facts.deletable_stopped >= 3 && facts.disk_nearly_full && open("bulk-select") {
                return Some(tip(
                    "bulk-select",
                    place,
                    "Select several Spaces to delete them together.".into(),
                    action(TipActionKind::SelectStopped, "Select stopped", None),
                ));
            }
            None
        }
        TipPlace::Space => {
            let space = facts.space_page.as_deref()?;
            if open("try-agent") {
                return Some(try_agent(place, space, facts));
            }
            if !facts.shared_any && open("share") {
                return Some(tip(
                    "share",
                    place,
                    format!("Share {space} with a teammate."),
                    action(TipActionKind::Share, "Share\u{2026}", None),
                ));
            }
            None
        }
        TipPlace::Sidebar => {
            if facts.page_tip_shown || !facts.second_computer_online || !open("on-auto") {
                return None;
            }
            let mut t = tip(
                "on-auto",
                place,
                "New Spaces can go to whichever machine has room.".into(),
                action(
                    TipActionKind::CopyCommand,
                    "Copy",
                    Some(ON_AUTO_COMMAND.into()),
                ),
            );
            t.code = Some(ON_AUTO_COMMAND.into());
            Some(t)
        }
    }
}

fn mark(state: &mut TipsState, id: &str) {
    if TIP_IDS.contains(&id) && !state.done.iter().any(|d| d == id) {
        state.done.push(id.into());
    }
}

/// The tips after `event`.
pub fn reduce(state: &TipsState, event: &TipsEvent) -> TipsState {
    let mut s = state.clone();
    match event {
        TipsEvent::Next { id } | TipsEvent::Close { id } => mark(&mut s, id),
        TipsEvent::DontShow => s.off = true,
        TipsEvent::Powered => mark(&mut s, "power"),
        TipsEvent::BulkDeleted => mark(&mut s, "bulk-select"),
        TipsEvent::Shared => mark(&mut s, "share"),
        TipsEvent::ShowTips { on } => s.off = !on,
        TipsEvent::Reset => s = TipsState::default(),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> TipFacts {
        TipFacts {
            newest_running: Some("cua-builder".into()),
            agents_connected: vec!["claude-code".into()],
            ..Default::default()
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_new_space_is_tried_in_your_agent() {
        let t = view(TipPlace::Spaces, &facts(), &TipsState::default()).unwrap();
        assert_eq!(t.id, "try-agent");
        assert_eq!(t.text, "Try cua-builder in Claude Code");
        assert_eq!(
            t.code.as_deref(),
            Some("Open the Cua Space \"cua-builder\" and tell me what's on its screen.")
        );
        assert_eq!(t.action.kind, TipActionKind::Copy);
        assert_eq!(t.action.label, "Copy");
        assert_eq!(t.action.value, t.code);
        assert_eq!(t.action.done_label.as_deref(), Some("Copied"));
        assert_eq!(t.agent.as_deref(), Some("claude-code"));
        assert_eq!(t.next.as_deref(), Some("Next tip"));
        assert_eq!(t.dont_show.as_deref(), Some("Don't show tips"));
        assert_eq!(t.close, "Close");
    }

    #[test]
    fn cursor_also_opens_the_request_in_cursor() {
        let f = TipFacts {
            agents_connected: ids(&["cursor"]),
            ..facts()
        };
        let t = view(TipPlace::Spaces, &f, &TipsState::default()).unwrap();
        assert_eq!(t.text, "Try cua-builder in Cursor");
        assert_eq!(t.action.kind, TipActionKind::Copy, "Copy stays");
        let link = t.open_in.unwrap();
        assert_eq!(link.label, "Open in Cursor");
        assert_eq!(
            link.url,
            "cursor://anysphere.cursor-deeplink/prompt?text=Open%20the%20Cua%20Space%20%22cua-builder%22%20and%20tell%20me%20what%27s%20on%20its%20screen."
        );
        // Agents without a prompt link, and no agent: no Open in.
        for agents in [vec!["claude-code"], vec!["hermes", "cursor"], vec![]] {
            let f = TipFacts {
                agents_connected: ids(&agents),
                agents_installed: vec![],
                ..facts()
            };
            let t = view(TipPlace::Spaces, &f, &TipsState::default()).unwrap();
            assert_eq!(t.open_in, None, "{agents:?}");
        }
        // Other tips never have one.
        let s = TipsState {
            off: false,
            done: ids(&["try-agent"]),
        };
        let f = TipFacts {
            has_stopped: true,
            ..facts()
        };
        assert_eq!(view(TipPlace::Spaces, &f, &s).unwrap().open_in, None);
    }

    #[test]
    fn url_encoding_keeps_only_unreserved_characters() {
        assert_eq!(url_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(url_encode("a b&c=d/\"é\""), "a%20b%26c%3Dd%2F%22%C3%A9%22");
    }

    #[test]
    fn hermes_comes_before_claude_code() {
        let f = TipFacts {
            agents_connected: ids(&["cursor", "claude-code", "hermes", "codex"]),
            ..facts()
        };
        assert_eq!(agent(&f), Some(("hermes", "Hermes")));
        let t = view(TipPlace::Spaces, &f, &TipsState::default()).unwrap();
        assert_eq!(t.text, "Try cua-builder in Hermes");
        let f = TipFacts {
            agents_connected: ids(&["cursor", "codex"]),
            ..facts()
        };
        assert_eq!(agent(&f), Some(("codex", "Codex")));
    }

    #[test]
    fn a_connected_agent_beats_an_installed_one() {
        let f = TipFacts {
            agents_connected: ids(&["cursor"]),
            agents_installed: ids(&["hermes", "cursor"]),
            ..facts()
        };
        assert_eq!(agent(&f), Some(("cursor", "Cursor")));
        // None connected: the first installed.
        let f = TipFacts {
            agents_connected: vec![],
            agents_installed: ids(&["codex", "claude-code"]),
            ..facts()
        };
        assert_eq!(agent(&f), Some(("claude-code", "Claude Code")));
        // An agent the tips do not name counts as none.
        let f = TipFacts {
            agents_connected: ids(&["openclaw"]),
            agents_installed: vec![],
            ..facts()
        };
        let t = view(TipPlace::Spaces, &f, &TipsState::default()).unwrap();
        assert_eq!(t.text, "Try cua-builder in your agent");
        assert_eq!(t.agent, None);
    }

    #[test]
    fn next_tip_moves_on_to_the_one_that_applies() {
        let f = TipFacts {
            has_stopped: true,
            ..facts()
        };
        let s = reduce(
            &TipsState::default(),
            &TipsEvent::Next {
                id: "try-agent".into(),
            },
        );
        let t = view(TipPlace::Spaces, &f, &s).unwrap();
        assert_eq!(t.id, "power");
        assert_eq!(
            t.text,
            "Point at a Space to turn it on or off without opening it."
        );
        assert_eq!(t.action.kind, TipActionKind::None);
        assert_eq!(t.action.label, "");
        // Powering from a tile finishes it; nothing else applies.
        let s = reduce(&s, &TipsEvent::Powered);
        assert_eq!(s.done, ids(&["try-agent", "power"]));
        assert_eq!(view(TipPlace::Spaces, &f, &s), None);
    }

    #[test]
    fn several_stopped_and_a_full_disk_offer_select_stopped() {
        let done = TipsState {
            off: false,
            done: ids(&["try-agent", "power"]),
        };
        let f = TipFacts {
            has_stopped: true,
            deletable_stopped: 3,
            disk_nearly_full: true,
            ..facts()
        };
        let t = view(TipPlace::Spaces, &f, &done).unwrap();
        assert_eq!(t.id, "bulk-select");
        assert_eq!(t.text, "Select several Spaces to delete them together.");
        assert_eq!(t.action.kind, TipActionKind::SelectStopped);
        assert_eq!(t.action.label, "Select stopped");
        assert_eq!(t.action.done_label, None);
        // Two stopped, or room on the disk: not yet.
        let two = TipFacts {
            deletable_stopped: 2,
            ..f.clone()
        };
        assert_eq!(view(TipPlace::Spaces, &two, &done), None);
        let room = TipFacts {
            disk_nearly_full: false,
            ..f.clone()
        };
        assert_eq!(view(TipPlace::Spaces, &room, &done), None);
        // A bulk delete finishes it.
        let s = reduce(&done, &TipsEvent::BulkDeleted);
        assert_eq!(view(TipPlace::Spaces, &f, &s), None);
    }

    #[test]
    fn a_running_spaces_page_tries_it_then_offers_share() {
        let f = TipFacts {
            space_page: Some("cua-builder".into()),
            newest_running: None,
            ..facts()
        };
        let t = view(TipPlace::Space, &f, &TipsState::default()).unwrap();
        assert_eq!(t.id, "try-agent");
        assert_eq!(t.place, TipPlace::Space);
        let s = reduce(
            &TipsState::default(),
            &TipsEvent::Close {
                id: "try-agent".into(),
            },
        );
        let t = view(TipPlace::Space, &f, &s).unwrap();
        assert_eq!(t.id, "share");
        assert_eq!(t.text, "Share cua-builder with a teammate.");
        assert_eq!(t.action.kind, TipActionKind::Share);
        assert_eq!(t.action.label, "Share\u{2026}");
        // Sharing any Space finishes it, and so does having shared one.
        assert_eq!(
            view(TipPlace::Space, &f, &reduce(&s, &TipsEvent::Shared)),
            None
        );
        let shared = TipFacts {
            shared_any: true,
            ..f.clone()
        };
        assert_eq!(view(TipPlace::Space, &shared, &s), None);
        // A stopped Space's page (no page fact): no tip.
        let stopped = TipFacts {
            space_page: None,
            ..f
        };
        assert_eq!(view(TipPlace::Space, &stopped, &TipsState::default()), None);
    }

    #[test]
    fn the_sidebar_waits_for_page_tips() {
        let f = TipFacts {
            second_computer_online: true,
            ..Default::default()
        };
        let t = view(TipPlace::Sidebar, &f, &TipsState::default()).unwrap();
        assert_eq!(t.id, "on-auto");
        assert_eq!(t.text, "New Spaces can go to whichever machine has room.");
        assert_eq!(t.code.as_deref(), Some("cua create --on auto"));
        assert_eq!(t.action.kind, TipActionKind::CopyCommand);
        assert_eq!(t.action.value.as_deref(), Some("cua create --on auto"));
        assert_eq!(t.next, None);
        assert_eq!(t.dont_show, None);
        let busy = TipFacts {
            page_tip_shown: true,
            ..f.clone()
        };
        assert_eq!(view(TipPlace::Sidebar, &busy, &TipsState::default()), None);
        let one = TipFacts {
            second_computer_online: false,
            ..f.clone()
        };
        assert_eq!(view(TipPlace::Sidebar, &one, &TipsState::default()), None);
        let closed = reduce(
            &TipsState::default(),
            &TipsEvent::Close {
                id: "on-auto".into(),
            },
        );
        assert_eq!(view(TipPlace::Sidebar, &f, &closed), None);
    }

    #[test]
    fn dont_show_turns_them_all_off_until_settings() {
        let off = reduce(&TipsState::default(), &TipsEvent::DontShow);
        assert!(off.off);
        for place in [TipPlace::Spaces, TipPlace::Space, TipPlace::Sidebar] {
            let f = TipFacts {
                space_page: Some("x".into()),
                second_computer_online: true,
                ..facts()
            };
            assert_eq!(view(place, &f, &off), None);
        }
        // Back on: the ones not done come back.
        let partly = TipsState {
            off: true,
            done: ids(&["try-agent"]),
        };
        let on = reduce(&partly, &TipsEvent::ShowTips { on: true });
        assert_eq!(on.done, ids(&["try-agent"]));
        let f = TipFacts {
            has_stopped: true,
            ..facts()
        };
        assert_eq!(view(TipPlace::Spaces, &f, &on).unwrap().id, "power");
        // Reset starts them over.
        assert_eq!(reduce(&partly, &TipsEvent::Reset), TipsState::default());
    }

    #[test]
    fn done_never_repeats_and_ignores_unknown_ids() {
        let s = reduce(
            &TipsState::default(),
            &TipsEvent::Next {
                id: "try-agent".into(),
            },
        );
        let s = reduce(
            &s,
            &TipsEvent::Close {
                id: "try-agent".into(),
            },
        );
        let s = reduce(
            &s,
            &TipsEvent::Close {
                id: "mention".into(),
            },
        );
        assert_eq!(s.done, ids(&["try-agent"]));
    }

    #[test]
    fn events_and_tips_on_the_wire() {
        let e: TipsEvent =
            serde_json::from_value(serde_json::json!({"type": "show-tips", "on": false})).unwrap();
        assert_eq!(e, TipsEvent::ShowTips { on: false });
        let e: TipsEvent =
            serde_json::from_value(serde_json::json!({"type": "bulk-deleted"})).unwrap();
        assert_eq!(e, TipsEvent::BulkDeleted);
        let v =
            serde_json::to_value(view(TipPlace::Spaces, &facts(), &TipsState::default())).unwrap();
        assert_eq!(v["action"]["kind"], "copy");
        assert_eq!(v["dontShow"], "Don't show tips");
        let f: TipFacts = serde_json::from_value(serde_json::json!({
            "newestRunning": "a", "deletableStopped": 4, "diskNearlyFull": true
        }))
        .unwrap();
        assert_eq!(f.deletable_stopped, 4);
        let s: TipsState = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(s, TipsState::default());
    }

    #[test]
    fn the_named_agents_are_the_setup_registrys() {
        // The ids are the registry's; the names are the short ones a tip
        // reads ("Codex", where the registry says "OpenAI Codex").
        for (id, name) in AGENT_ORDER {
            let spec = cua_agent_setup::registry::find(id)
                .unwrap_or_else(|| panic!("{id} is not in cua-agent-setup's registry"));
            assert_eq!(spec.id, id);
            assert!(spec.name.ends_with(name), "{id}: {} vs {name}", spec.name);
        }
    }
}
