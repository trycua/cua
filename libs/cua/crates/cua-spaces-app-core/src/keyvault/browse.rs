// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault browser: a sidebar of categories (All, Waiting, Access,
//! Recent) and one row per app, and the list for the chosen one. The vault
//! list itself (items grouped by app and site, with their locks) is
//! [`super::vault`]; this picks what the page shows.

use super::view::{
    AccessRow, Decision, PendingRow, access_rows, ago, pending_rows, recent_decisions,
};
use super::wire::KeyvaultOverview;
use crate::util::collate;
use serde::{Deserialize, Serialize};

/// How many decisions Recent shows.
pub const RECENT_LIMIT: usize = 12;

/// A sidebar category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KvCategory {
    /// Every item, grouped by app.
    All,
    /// Requests waiting for approval.
    Waiting,
    /// Live grants, rules and copies.
    Access,
    /// Recent decisions.
    Recent,
}

impl KvCategory {
    /// Title.
    pub fn title(self) -> &'static str {
        match self {
            KvCategory::All => "All Items",
            KvCategory::Waiting => "Waiting",
            KvCategory::Access => "Access",
            KvCategory::Recent => "Recent",
        }
    }

    /// SF Symbol.
    pub fn symbol(self) -> &'static str {
        match self {
            KvCategory::All => "key",
            KvCategory::Waiting => "clock",
            KvCategory::Access => "checkmark.shield",
            KvCategory::Recent => "list.bullet.rectangle",
        }
    }
}

/// A category row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryRow {
    /// Category.
    pub category: KvCategory,
    /// Title.
    pub title: String,
    /// SF Symbol.
    pub symbol: String,
    /// Count (none for Recent).
    pub count: Option<u32>,
    /// The sidebar badge: Waiting's count while something waits.
    pub badge: Option<u32>,
}

/// An app row: everything saved from one app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRow {
    /// The provider id (`chrome`): the key, and what the icon is looked up by.
    pub key: String,
    /// "Google Chrome".
    pub title: String,
    /// Items.
    pub items: u32,
    /// Something waits for this app.
    pub waiting: bool,
}

/// The Keyvault sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvSidebar {
    /// All Items, Waiting, Access, Recent.
    pub categories: Vec<CategoryRow>,
    /// One row per app, by name.
    pub apps: Vec<AppRow>,
}

/// The sidebar.
pub fn sidebar(o: &KeyvaultOverview, now: i64) -> KvSidebar {
    let row = |c: KvCategory, count: Option<u32>| CategoryRow {
        category: c,
        title: c.title().into(),
        symbol: c.symbol().into(),
        count,
        badge: (c == KvCategory::Waiting)
            .then_some(count)
            .flatten()
            .filter(|n| *n > 0),
    };
    let mut apps: Vec<AppRow> = Vec::new();
    for i in &o.items {
        match apps.iter_mut().find(|a| a.key == i.provider_id) {
            Some(a) => a.items += 1,
            None => apps.push(AppRow {
                key: i.provider_id.clone(),
                title: i.app_display.clone(),
                items: 1,
                waiting: o
                    .pending
                    .iter()
                    .any(|p| p.items.iter().any(|x| x.provider_id == i.provider_id)),
            }),
        }
    }
    apps.sort_by(|a, b| collate(&a.title, &b.title));
    KvSidebar {
        categories: vec![
            row(KvCategory::All, Some(o.items.len() as u32)),
            row(KvCategory::Waiting, Some(o.pending.len() as u32)),
            row(KvCategory::Access, Some(access_rows(o, now).len() as u32)),
            row(KvCategory::Recent, None),
        ],
        apps,
    }
}

/// What the sidebar selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum KvSelection {
    /// A category.
    Category {
        /// Which.
        category: KvCategory,
    },
    /// One app's items.
    App {
        /// Its provider id.
        key: String,
    },
}

/// A Recent row with its age.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentRow {
    /// The decision.
    pub decision: Decision,
    /// "now", "5 min ago".
    pub age: String,
}

/// The pane for a selection other than the vault list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvListView {
    /// Title.
    pub title: String,
    /// The vault list shows (All Items, or one app).
    pub vault: bool,
    /// Waiting.
    pub pending: Vec<PendingRow>,
    /// Access.
    pub access: Vec<AccessRow>,
    /// Recent.
    pub recent: Vec<RecentRow>,
    /// "Nothing is waiting." and friends.
    pub empty_text: Option<String>,
}

/// The pane for `selection`. For All Items and an app, `vault` is true and
/// the list is [`super::vault::view`] (narrowed to the app by
/// [`super::vault::VaultState::app`]).
pub fn list(o: &KeyvaultOverview, selection: &KvSelection, now: i64) -> KvListView {
    let mut v = KvListView {
        title: String::new(),
        vault: false,
        pending: vec![],
        access: vec![],
        recent: vec![],
        empty_text: None,
    };
    match selection {
        KvSelection::Category { category } => {
            v.title = category.title().into();
            match category {
                KvCategory::All => v.vault = true,
                KvCategory::Waiting => {
                    v.pending = pending_rows(o);
                    if v.pending.is_empty() {
                        v.empty_text = Some("Nothing is waiting.".into());
                    }
                }
                KvCategory::Access => {
                    v.access = access_rows(o, now);
                    if v.access.is_empty() {
                        v.empty_text = Some("No live access.".into());
                    }
                }
                KvCategory::Recent => {
                    v.recent = recent_decisions(o, RECENT_LIMIT)
                        .into_iter()
                        .map(|d| RecentRow {
                            age: ago(now - d.entry.ts_ms as i64),
                            decision: d,
                        })
                        .collect();
                    if v.recent.is_empty() {
                        v.empty_text = Some("Nothing yet.".into());
                    }
                }
            }
        }
        KvSelection::App { key } => match o.items.iter().find(|i| &i.provider_id == key) {
            Some(i) => {
                v.title = i.app_display.clone();
                v.vault = true;
            }
            None => v.empty_text = Some("Nothing is saved from this app.".into()),
        },
    }
    v
}
