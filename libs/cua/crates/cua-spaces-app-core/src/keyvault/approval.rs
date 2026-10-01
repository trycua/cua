// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The approval sheet for a waiting request.
//!
//! Nothing is selected by default: Approve stays disabled until the user
//! ticks at least one item, and approves only those. A request that would
//! import new items (sites the vault does not hold yet) is all or nothing,
//! because the broker imports them on approval and their ids are not known
//! before: the user ticks every row or denies. The daemon then asks for
//! Touch ID or the login password; no shell runs its own prompt.

use super::view::{SigningBadge, claims, pending_summary, short_caller, signing_badge, wants};
use super::wire::{KeyvaultOverview, KvCommand, KvSelector};
use serde::{Deserialize, Serialize};

/// The sheet's state: which rows are ticked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalState {
    /// The request.
    pub request_id: String,
    /// Ticked row keys (item ids, or `import:<n>` for new items).
    pub selected: Vec<String>,
}

/// Opens the sheet with nothing selected.
pub fn open(request_id: &str) -> ApprovalState {
    ApprovalState {
        request_id: request_id.into(),
        selected: vec![],
    }
}

/// An input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ApprovalAction {
    /// Tick or untick a row.
    Toggle {
        /// Row key.
        key: String,
    },
    /// Tick every row.
    SelectAll,
    /// Untick every row.
    Clear,
}

/// One row of the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRow {
    /// Row key.
    pub key: String,
    /// Site or app.
    pub title: String,
    /// Account, or "Not imported yet".
    pub account: String,
    /// Ticked.
    pub selected: bool,
    /// A new item the approval imports.
    pub is_import: bool,
}

/// The sheet as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    /// The request.
    pub request_id: String,
    /// "Allow com.example.koalabot?"
    pub title: String,
    /// Verified caller (short).
    pub caller: String,
    /// Signing badge.
    pub badge: SigningBadge,
    /// Where to ("dev-1").
    pub targets: String,
    /// For how long.
    pub wants: String,
    /// One-line summary.
    pub summary: String,
    /// Unverified claims.
    pub claims: Vec<String>,
    /// Rows.
    pub rows: Vec<ApprovalRow>,
    /// Approve is enabled.
    pub can_approve: bool,
    /// Why not, in one line (none when it can, or nothing is ticked yet).
    pub blocked_reason: Option<String>,
    /// "Approve 2 items".
    pub approve_label: String,
    /// The request is gone (answered elsewhere).
    pub gone: bool,
}

fn rows(o: &KeyvaultOverview, state: &ApprovalState) -> Vec<ApprovalRow> {
    let Some(p) = o.pending.iter().find(|p| p.id == state.request_id) else {
        return vec![];
    };
    let mut out: Vec<ApprovalRow> = p
        .items
        .iter()
        .map(|i| ApprovalRow {
            key: i.id.clone(),
            title: i.site.clone().unwrap_or_else(|| i.app_display.clone()),
            account: super::view::account_of(i),
            selected: state.selected.contains(&i.id),
            is_import: false,
        })
        .collect();
    out.extend(p.needs_import.iter().enumerate().map(|(n, s)| {
        let key = format!("import:{n}");
        let title = match s {
            KvSelector::Site { site, .. } => site.clone(),
            KvSelector::App { app } => app.clone(),
            KvSelector::Item { id } => id.clone(),
            KvSelector::Login { site } => site.clone(),
        };
        ApprovalRow {
            selected: state.selected.contains(&key),
            key,
            title,
            account: "Not imported yet".into(),
            is_import: true,
        }
    }));
    out
}

/// Advances the sheet (unknown keys are ignored).
pub fn reduce(
    o: &KeyvaultOverview,
    state: &ApprovalState,
    action: &ApprovalAction,
) -> ApprovalState {
    let all: Vec<String> = rows(o, state).into_iter().map(|r| r.key).collect();
    let mut s = state.clone();
    match action {
        ApprovalAction::Toggle { key } => {
            if !all.contains(key) {
                return s;
            }
            if let Some(i) = s.selected.iter().position(|k| k == key) {
                s.selected.remove(i);
            } else {
                s.selected.push(key.clone());
            }
            // Keep the request's order, so the command is stable.
            s.selected = all
                .iter()
                .filter(|k| s.selected.contains(k))
                .cloned()
                .collect();
        }
        ApprovalAction::SelectAll => s.selected = all,
        ApprovalAction::Clear => s.selected.clear(),
    }
    s
}

/// The sheet as drawn.
pub fn view(o: &KeyvaultOverview, state: &ApprovalState) -> ApprovalView {
    let rows = rows(o, state);
    let pending = o.pending.iter().find(|p| p.id == state.request_id);
    let disabled = o.status.as_ref().is_some_and(|s| s.disabled);
    let ticked = rows.iter().filter(|r| r.selected).count();
    let imports_unticked = rows.iter().any(|r| r.is_import && !r.selected);
    let blocked_reason = if pending.is_none() {
        Some("This request was already answered.".to_string())
    } else if disabled {
        Some("Keyvault is off. Turn it on to approve.".to_string())
    } else if ticked > 0 && imports_unticked {
        Some("This request imports new items; select all or deny.".to_string())
    } else {
        None
    };
    let can_approve = pending.is_some() && !disabled && ticked > 0 && !imports_unticked;
    let (caller, badge, targets, wants_s, summary, claims_v) = match pending {
        Some(p) => (
            short_caller(&p.caller_display),
            signing_badge(&p.caller),
            p.request.targets.join(", "),
            wants(p),
            pending_summary(p),
            claims(p),
        ),
        None => (
            String::new(),
            SigningBadge {
                text: String::new(),
                tone: super::view::Tone::Warn,
            },
            String::new(),
            String::new(),
            String::new(),
            vec![],
        ),
    };
    ApprovalView {
        request_id: state.request_id.clone(),
        title: format!("Allow {caller}?"),
        caller,
        badge,
        targets,
        wants: wants_s,
        summary,
        claims: claims_v,
        approve_label: match ticked {
            0 => "Approve".into(),
            1 => "Approve 1 item".into(),
            n => format!("Approve {n} items"),
        },
        rows,
        can_approve,
        blocked_reason,
        gone: pending.is_none(),
    }
}

/// The broker request Approve sends, when it can: exactly the ticked
/// items, or everything when every row (including imports) is ticked.
pub fn approve_command(o: &KeyvaultOverview, state: &ApprovalState) -> Option<KvCommand> {
    let v = view(o, state);
    if !v.can_approve {
        return None;
    }
    let all_ticked = v.rows.iter().all(|r| r.selected);
    let has_imports = v.rows.iter().any(|r| r.is_import);
    Some(KvCommand::Approve {
        request_id: state.request_id.clone(),
        items: if all_ticked && has_imports {
            None
        } else {
            Some(
                v.rows
                    .iter()
                    .filter(|r| r.selected && !r.is_import)
                    .map(|r| r.key.clone())
                    .collect(),
            )
        },
    })
}

/// The broker request Deny sends.
pub fn deny_command(state: &ApprovalState) -> KvCommand {
    KvCommand::Deny {
        request_id: state.request_id.clone(),
    }
}
