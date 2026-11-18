// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Approvals on a phone: which buttons a card has, "1 of 3" when several wait,
//! and the one line an answered card collapses to.
//!
//! An approval is asked only at the edge of a Space (a push to a repository,
//! a stored login, reaching past the network policy): inside the Space the
//! agent runs auto-approved, because the Space is the sandbox. So a card has
//! at most three answers:
//!
//! * **Allow once**: this approval only;
//! * **Allow for this Space**: the daemon remembers this kind of action for
//!   this Space and answers it itself from then on (offered only when the
//!   daemon says a Space grant makes sense: `scope_choices` has `space`);
//! * **Decline**.
//!
//! The words are made here from structured data (the approval's kind, action
//! and scope choices); an agent's own text is never turned into a button.

use serde::{Deserialize, Serialize};

/// One open approval.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pending {
    /// The approval entry's seq in the thread.
    pub seq: u64,
    /// What the host says it is (never the agent's words): `push`, `request` (an agent's own), `ability`, `join`, or empty.
    pub kind: String,
    /// What would happen, as the entry says.
    pub action: String,
    /// `once`, `space`. Empty: once only.
    pub scope_choices: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewInput {
    pub pending: Vec<Pending>,
    /// The seq being shown (the oldest open one when none).
    pub current: Option<u64>,
    /// The card's own words for the two plain answers (the push card says "Push and open PR" and "Not now").
    pub allow_label: Option<String>,
    pub deny_label: Option<String>,
}

/// One button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Button {
    /// `allow-once`, `allow-space`, `decline`.
    pub id: String,
    pub label: String,
    pub approve: bool,
    /// `once` or `space` for an allow; none for a decline.
    pub scope: Option<String>,
    /// `primary`, `secondary`, `quiet`.
    pub role: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// "1 of 3", only when more than one waits.
    pub counter: Option<String>,
    pub index: usize,
    pub total: usize,
    pub current: Option<u64>,
    /// Next and previous seqs to step to, when several wait.
    pub next: Option<u64>,
    pub previous: Option<u64>,
    pub next_label: Option<String>,
    pub previous_label: Option<String>,
    pub buttons: Vec<Button>,
    /// What "Allow for this Space" will do, in a line.
    pub scope_note: Option<String>,
}

fn kind_phrase(kind: &str) -> &'static str {
    match kind {
        "push" => "pushes like this",
        "request" => "requests like this",
        _ => "this kind of request",
    }
}

/// The buttons and the counter for the approval being shown.
pub fn view(i: &ViewInput) -> View {
    let total = i.pending.len();
    let index = i
        .current
        .and_then(|c| i.pending.iter().position(|p| p.seq == c))
        .unwrap_or(0);
    let cur = i.pending.get(index);
    let space = cur.is_some_and(|p| p.scope_choices.iter().any(|c| c == "space"));
    let mut buttons = vec![
        Button {
            id: "decline".into(),
            label: i.deny_label.clone().unwrap_or_else(|| "Decline".into()),
            approve: false,
            scope: None,
            role: "quiet".into(),
            hint: None,
        },
        Button {
            id: "allow-once".into(),
            label: i.allow_label.clone().unwrap_or_else(|| "Allow once".into()),
            approve: true,
            scope: Some("once".into()),
            role: "primary".into(),
            hint: None,
        },
    ];
    let scope_note = if space {
        let note = format!(
            "Allow for this Space: Cua won't ask again for {} here.",
            kind_phrase(cur.map(|p| p.kind.as_str()).unwrap_or(""))
        );
        buttons.push(Button {
            id: "allow-space".into(),
            label: "Allow for this Space".into(),
            approve: true,
            scope: Some("space".into()),
            role: "secondary".into(),
            hint: Some(note.clone()),
        });
        Some(note)
    } else {
        None
    };
    View {
        counter: (total > 1).then(|| format!("{} of {}", index + 1, total)),
        index,
        total,
        current: cur.map(|p| p.seq),
        next: (index + 1 < total).then(|| i.pending[index + 1].seq),
        previous: (index > 0).then(|| i.pending[index - 1].seq),
        next_label: (index + 1 < total).then(|| "Next approval".into()),
        previous_label: (index > 0).then(|| "Previous approval".into()),
        buttons,
        scope_note,
    }
}

/// The line an answered approval collapses to (the shell adds who and when).
pub fn result_line(approve: bool, scope: Option<&str>, title: &str) -> String {
    let word = match (approve, scope) {
        (false, _) => "Declined",
        (true, Some("space")) => "Allowed for this Space",
        (true, _) => "Allowed",
    };
    if title.trim().is_empty() {
        word.into()
    } else {
        format!("{word}: {}", title.trim())
    }
}

/// The words posted for an answer in the thread when the daemon wants one
/// (`Approved`/`Denied` is the wire's own note, not what a person reads).
pub fn answer_note(approve: bool, scope: Option<&str>) -> &'static str {
    match (approve, scope) {
        (false, _) => "Declined",
        (true, Some("space")) => "Allowed for this Space",
        (true, _) => "Allowed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(seq: u64, kind: &str, choices: &[&str]) -> Pending {
        Pending {
            seq,
            kind: kind.into(),
            action: "x".into(),
            scope_choices: choices.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn one_approval_has_no_counter_and_the_card_words() {
        let v = view(&ViewInput {
            pending: vec![p(7, "push", &["once"])],
            allow_label: Some("Push and open PR".into()),
            deny_label: Some("Not now".into()),
            ..Default::default()
        });
        assert_eq!(v.counter, None);
        let ids: Vec<&str> = v.buttons.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["decline", "allow-once"]);
        assert_eq!(v.buttons[0].label, "Not now");
        assert_eq!(v.buttons[1].label, "Push and open PR");
        assert_eq!(v.scope_note, None);
    }

    #[test]
    fn a_space_grant_is_offered_only_when_the_daemon_says_so() {
        let v = view(&ViewInput {
            pending: vec![p(3, "request", &["once", "space"])],
            ..Default::default()
        });
        let labels: Vec<&str> = v.buttons.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Decline", "Allow once", "Allow for this Space"]);
        assert_eq!(v.buttons[2].scope.as_deref(), Some("space"));
        assert!(v.buttons.iter().all(|b| b.approve || b.scope.is_none()));
        assert_eq!(
            v.scope_note.as_deref(),
            Some("Allow for this Space: Cua won't ask again for requests like this here.")
        );
    }

    #[test]
    fn several_approvals_say_n_of_m_and_step() {
        let pending = vec![
            p(2, "push", &[]),
            p(5, "request", &["space"]),
            p(9, "", &[]),
        ];
        let first = view(&ViewInput {
            pending: pending.clone(),
            ..Default::default()
        });
        assert_eq!(first.counter.as_deref(), Some("1 of 3"));
        assert_eq!((first.previous, first.next), (None, Some(5)));
        let mid = view(&ViewInput {
            pending: pending.clone(),
            current: Some(5),
            ..Default::default()
        });
        assert_eq!(mid.counter.as_deref(), Some("2 of 3"));
        assert_eq!((mid.previous, mid.next), (Some(2), Some(9)));
        assert_eq!(mid.buttons.len(), 3);
        let last = view(&ViewInput {
            pending,
            current: Some(9),
            ..Default::default()
        });
        assert_eq!(last.counter.as_deref(), Some("3 of 3"));
        assert_eq!(last.next, None);
        assert_eq!(last.previous_label.as_deref(), Some("Previous approval"));
        // A current seq that is gone shows the first.
        let gone = view(&ViewInput {
            pending: vec![p(2, "", &[]), p(3, "", &[])],
            current: Some(99),
            ..Default::default()
        });
        assert_eq!(gone.counter.as_deref(), Some("1 of 2"));
    }

    #[test]
    fn no_approval_is_an_empty_view() {
        let v = view(&ViewInput::default());
        assert_eq!((v.total, v.current), (0, None));
        assert_eq!(v.counter, None);
    }

    #[test]
    fn answered_cards_say_what_was_decided() {
        assert_eq!(
            result_line(true, Some("once"), "Push fix/retry"),
            "Allowed: Push fix/retry"
        );
        assert_eq!(
            result_line(true, Some("space"), "Push fix/retry"),
            "Allowed for this Space: Push fix/retry"
        );
        assert_eq!(
            result_line(false, None, "Push fix/retry"),
            "Declined: Push fix/retry"
        );
        assert_eq!(result_line(false, Some("space"), " "), "Declined");
        assert_eq!(answer_note(true, Some("space")), "Allowed for this Space");
    }
}
