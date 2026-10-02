// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault browser: a sidebar of
//! categories (All, Waiting, Access, Recent) and one row per site, a list
//! for the chosen category, and a detail pane per site with each account's
//! unattended switch.

use super::view::{
    AccessRow, Decision, PendingRow, SiteGroup, Tri, access_rows, ago, group_items, pending_rows,
    recent_decisions,
};
use super::wire::KeyvaultOverview;
use serde::{Deserialize, Serialize};

/// How many decisions Recent shows.
pub const RECENT_LIMIT: usize = 12;

/// A sidebar category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KvCategory {
    /// Every site.
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
            KvCategory::All => "All",
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

/// A site row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteRow {
    /// Group key.
    pub key: String,
    /// "github.com" or "Slack".
    pub title: String,
    /// Accounts.
    pub accounts: u32,
    /// Something waits for this site.
    pub waiting: bool,
}

/// The Keyvault sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvSidebar {
    /// All, Waiting, Access, Recent.
    pub categories: Vec<CategoryRow>,
    /// One row per site or app.
    pub sites: Vec<SiteRow>,
}

/// The sidebar.
pub fn sidebar(o: &KeyvaultOverview, now: i64) -> KvSidebar {
    let groups = group_items(o, now, "");
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
    KvSidebar {
        categories: vec![
            row(KvCategory::All, Some(o.items.len() as u32)),
            row(KvCategory::Waiting, Some(o.pending.len() as u32)),
            row(KvCategory::Access, Some(access_rows(o, now).len() as u32)),
            row(KvCategory::Recent, None),
        ],
        sites: groups
            .iter()
            .map(|g| SiteRow {
                key: g.key.clone(),
                title: g.title.clone(),
                accounts: g.rows.len() as u32,
                waiting: g.rows.iter().any(|r| {
                    r.consent
                        .iter()
                        .any(|c| c.kind == super::view::ConsentChipKind::Pending)
                }),
            })
            .collect(),
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
    /// A site.
    Site {
        /// Group key.
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

/// The list pane for a selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvListView {
    /// Title.
    pub title: String,
    /// Sites (All, or the one site).
    pub sites: Vec<SiteGroup>,
    /// Waiting.
    pub pending: Vec<PendingRow>,
    /// Access.
    pub access: Vec<AccessRow>,
    /// Recent.
    pub recent: Vec<RecentRow>,
    /// "No items yet." and friends.
    pub empty_text: Option<String>,
}

/// The list for `selection`, filtered by `query` (sites only).
pub fn list(o: &KeyvaultOverview, selection: &KvSelection, now: i64, query: &str) -> KvListView {
    let mut v = KvListView {
        title: String::new(),
        sites: vec![],
        pending: vec![],
        access: vec![],
        recent: vec![],
        empty_text: None,
    };
    match selection {
        KvSelection::Category { category } => {
            v.title = category.title().into();
            match category {
                KvCategory::All => {
                    v.sites = group_items(o, now, query);
                    if v.sites.is_empty() {
                        v.empty_text = Some(if o.items.is_empty() {
                            "No items yet.".into()
                        } else {
                            "No matches".into()
                        });
                    }
                }
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
        KvSelection::Site { key } => {
            let groups = group_items(o, now, "");
            match groups.into_iter().find(|g| &g.key == key) {
                Some(g) => {
                    v.title = g.title.clone();
                    v.sites = vec![g];
                }
                None => v.empty_text = Some("This site is gone.".into()),
            }
        }
    }
    v
}

/// The detail pane of one site: the site switch and each account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteDetail {
    /// The group.
    pub group: SiteGroup,
    /// The site switch shows (more than one account).
    pub site_switch: bool,
    /// The site switch's state.
    pub site_state: Tri,
    /// The site switch can be flipped.
    pub site_switch_enabled: bool,
    /// Its tooltip.
    pub site_switch_help: String,
}

/// The detail of the site `key`.
pub fn site_detail(o: &KeyvaultOverview, key: &str, now: i64) -> Option<SiteDetail> {
    let group = group_items(o, now, "").into_iter().find(|g| g.key == key)?;
    let disabled = o.status.as_ref().is_some_and(|s| s.disabled);
    Some(SiteDetail {
        site_switch: group.rows.len() > 1,
        site_state: group.unattended,
        site_switch_enabled: !group.locked && !disabled,
        site_switch_help: if group.locked {
            "Identity providers always ask".into()
        } else {
            "Every account on this site".into()
        },
        group,
    })
}
