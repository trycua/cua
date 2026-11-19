// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The approval sheet for a waiting request.
//!
//! Nothing is selected by default: Approve stays disabled until the user
//! ticks at least one site, and approves only the items of those. A request
//! that would save new items (sites the vault does not hold yet) is all or
//! nothing,
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
    /// Ticked row keys (`<app>|<site>`, or `import:<n>` for new items).
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

/// One row of the sheet: a site (or an app's files) with how many items
/// it carries, or an app the approval would save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRow {
    /// Row key: `<app>|<site>`, `<app>|files`, or `import:<n>`.
    pub key: String,
    /// The site, or the app for files and imports.
    pub title: String,
    /// "3 cookies, 1 password", or "Not saved yet".
    pub account: String,
    /// The app's provider id (its icon), when known.
    pub provider_id: String,
    /// Items the row stands for.
    pub items: u32,
    /// The items' ids.
    pub item_ids: Vec<String>,
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
    use super::vault::{KvKind, site_of};
    let Some(p) = o.pending.iter().find(|p| p.id == state.request_id) else {
        return vec![];
    };
    // Group the request's items by app and site (files apart).
    let mut groups: Vec<(String, String, String, Vec<&super::wire::KvItem>)> = Vec::new();
    for i in &p.items {
        let is_file = KvKind::from_wire(&i.kind) == KvKind::File;
        let (key, title) = if is_file {
            (
                format!("{}|files", i.provider_id),
                format!("{} files", i.app_display),
            )
        } else {
            let site = i.domain.as_deref().map(site_of).filter(|s| !s.is_empty());
            (
                format!("{}|{}", i.provider_id, site.clone().unwrap_or_default()),
                site.unwrap_or_else(|| i.app_display.clone()),
            )
        };
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.3.push(i),
            None => groups.push((key, title, i.provider_id.clone(), vec![i])),
        }
    }
    let mut out: Vec<ApprovalRow> = groups
        .into_iter()
        .map(|(key, title, provider_id, its)| {
            let mut kinds: Vec<(KvKind, u32)> = Vec::new();
            for i in &its {
                let k = KvKind::from_wire(&i.kind);
                match kinds.iter_mut().find(|(x, _)| *x == k) {
                    Some((_, n)) => *n += 1,
                    None => kinds.push((k, 1)),
                }
            }
            ApprovalRow {
                selected: state.selected.contains(&key),
                account: kinds
                    .iter()
                    .map(|(k, n)| k.plural(*n))
                    .collect::<Vec<_>>()
                    .join(", "),
                items: its.len() as u32,
                item_ids: its.iter().map(|i| i.id.clone()).collect(),
                key,
                title,
                provider_id,
                is_import: false,
            }
        })
        .collect();
    out.extend(p.needs_import.iter().enumerate().map(|(n, s)| {
        let key = format!("import:{n}");
        let (title, provider_id) = match s {
            KvSelector::Site { site, app } => (site.clone(), app.clone()),
            KvSelector::App { app } => (app.clone(), app.clone()),
            KvSelector::Item { id } => (id.clone(), String::new()),
            KvSelector::Login { site } => (site.clone(), String::new()),
        };
        ApprovalRow {
            selected: state.selected.contains(&key),
            key,
            title,
            account: "Not saved yet".into(),
            provider_id,
            items: 0,
            item_ids: vec![],
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
        Some("This request saves new items; select all or deny.".to_string())
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
        approve_label: match rows
            .iter()
            .filter(|r| r.selected)
            .map(|r| r.items)
            .sum::<u32>()
        {
            0 if ticked == 0 => "Approve".into(),
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
                    .flat_map(|r| r.item_ids.iter().cloned())
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
