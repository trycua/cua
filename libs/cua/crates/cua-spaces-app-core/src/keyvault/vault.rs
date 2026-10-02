// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The vault list: a password manager's list, grouped by the app each secret
//! came from.
//!
//! The vault holds one item per secret (a cookie, a localStorage value, a
//! password, a file). The list groups them by app, and inside an app by
//! site (the domain), with files together. Each item shows a lock: locked
//! needs the user's approval for every use; unlocked allows unattended
//! access. Items, a whole site or a whole app can be selected and locked,
//! unlocked or deleted together. Search matches a domain, a key, an app or
//! a type.
//!
//! Pure shaping of what the broker returned, plus a small reducer for the
//! list's own state (the query, the selection, the open groups). Nothing here
//! decides access, and no value is ever here: the broker has none to give.

use super::view::{Tri, ago};
use super::wire::{KeyvaultOverview, KvCommand, KvItem};
use crate::util::collate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The type of a secret, as the list draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KvKind {
    /// A cookie.
    Cookie,
    /// A localStorage value.
    LocalStorage,
    /// A saved password.
    Password,
    /// A file.
    File,
}

impl KvKind {
    /// From the broker's tag (`cookie`, `local_storage`, `password`, `file`).
    pub fn from_wire(kind: &str) -> KvKind {
        match kind {
            "cookie" => KvKind::Cookie,
            "local_storage" => KvKind::LocalStorage,
            "password" => KvKind::Password,
            _ => KvKind::File,
        }
    }

    /// "Cookie", "Local storage", "Password", "File".
    pub fn label(self) -> &'static str {
        match self {
            KvKind::Cookie => "Cookie",
            KvKind::LocalStorage => "Local storage",
            KvKind::Password => "Password",
            KvKind::File => "File",
        }
    }

    /// The plural a count uses ("3 cookies").
    pub fn plural(self, n: u32) -> String {
        let (one, many) = match self {
            KvKind::Cookie => ("cookie", "cookies"),
            KvKind::LocalStorage => ("storage value", "storage values"),
            KvKind::Password => ("password", "passwords"),
            KvKind::File => ("file", "files"),
        };
        format!("{n} {}", if n == 1 { one } else { many })
    }

    /// The SF Symbol of the type.
    pub fn symbol(self) -> &'static str {
        match self {
            KvKind::Cookie => "circle.hexagongrid.fill",
            KvKind::LocalStorage => "cylinder.split.1x2",
            KvKind::Password => "key.fill",
            KvKind::File => "doc",
        }
    }

    /// Display order inside a site.
    fn rank(self) -> u8 {
        match self {
            KvKind::Password => 0,
            KvKind::Cookie => 1,
            KvKind::LocalStorage => 2,
            KvKind::File => 3,
        }
    }
}

/// The lock of a row or a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KvLock {
    /// Every item needs approval for each use.
    Locked,
    /// Every item allows unattended access.
    Unlocked,
    /// Some of each.
    Mixed,
}

impl KvLock {
    /// The SF Symbol: `lock.fill` while locked, `lock.open` otherwise.
    pub fn symbol(self) -> &'static str {
        match self {
            KvLock::Locked => "lock.fill",
            KvLock::Unlocked => "lock.open",
            // Some are unlocked: the open lock, drawn dimmer by the shell.
            KvLock::Mixed => "lock.open",
        }
    }
}

fn lock_of(items: &[&KvItem]) -> KvLock {
    let open = items.iter().filter(|i| i.policy.unattended).count();
    if open == 0 {
        KvLock::Locked
    } else if open == items.len() {
        KvLock::Unlocked
    } else {
        KvLock::Mixed
    }
}

/// The list's own state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultState {
    /// The search text.
    pub query: String,
    /// Selected item ids.
    pub selected: Vec<String>,
    /// Open groups (their keys); the rest are closed.
    pub expanded: Vec<String>,
    /// Narrowed to one app (its provider id), as the sidebar picks it.
    #[serde(default)]
    pub app: Option<String>,
}

/// An input to the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum VaultAction {
    /// The search text changed.
    Query {
        /// Text.
        text: String,
    },
    /// Select or deselect one item.
    Toggle {
        /// Item id.
        id: String,
    },
    /// Select or deselect every shown item of an app, a site or the files
    /// of an app (all of them when any is unselected, none when all are).
    ToggleGroup {
        /// Group key.
        key: String,
    },
    /// Select every shown item.
    SelectAll,
    /// Deselect everything.
    Clear,
    /// Open or close a group.
    ToggleOpen {
        /// Group key.
        key: String,
    },
}

/// What the list shows for an item row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultRow {
    /// Item id.
    pub id: String,
    /// Type.
    pub kind: KvKind,
    /// "Cookie".
    pub kind_label: String,
    /// The type's SF Symbol.
    pub symbol: String,
    /// The key: a cookie name, storage key, username or file name.
    pub title: String,
    /// Grey line: the domain, or the file's folder.
    pub subtitle: String,
    /// Grey time: "2 min ago".
    pub updated: String,
    /// Locked (needs approval for every use).
    pub locked: bool,
    /// The lock icon: `lock.fill` or `lock.open`.
    pub lock_symbol: String,
    /// The lock's tooltip and accessibility label.
    pub lock_help: String,
    /// Identity provider: always asks, cannot be unlocked.
    pub identity_provider: bool,
    /// Selected.
    pub selected: bool,
}

/// A site inside an app: its items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSite {
    /// Group key.
    pub key: String,
    /// "github.com".
    pub site: String,
    /// Items (all of them, even while closed).
    pub count: u32,
    /// "12 cookies, 2 storage values".
    pub counts: String,
    /// Grey time of the newest save.
    pub updated: String,
    /// Selection of its items.
    pub selected: Tri,
    /// Lock of its items.
    pub lock: KvLock,
    /// Items a click on the lock unlocks (locked, not identity providers).
    pub unlock_ids: Vec<String>,
    /// Items a click on the lock locks (unlocked).
    pub lock_ids: Vec<String>,
    /// Open.
    pub open: bool,
    /// Its rows (empty while closed, unless searching).
    pub rows: Vec<VaultRow>,
}

/// The files of an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFiles {
    /// Group key.
    pub key: String,
    /// Files.
    pub count: u32,
    /// Selection of its items.
    pub selected: Tri,
    /// Lock of its items.
    pub lock: KvLock,
    /// Items a click on the lock unlocks (locked, not identity providers).
    pub unlock_ids: Vec<String>,
    /// Items a click on the lock locks (unlocked).
    pub lock_ids: Vec<String>,
    /// Open.
    pub open: bool,
    /// Its rows (empty while closed, unless searching).
    pub rows: Vec<VaultRow>,
}

/// An app and everything saved from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultApp {
    /// Group key: the provider id (`chrome`).
    pub key: String,
    /// The provider id, for the app's icon.
    pub provider_id: String,
    /// "Google Chrome".
    pub name: String,
    /// Items.
    pub count: u32,
    /// "212 items, 14 unlocked".
    pub summary: String,
    /// Grey time of the newest save.
    pub updated: String,
    /// Selection of its items.
    pub selected: Tri,
    /// Lock of its items.
    pub lock: KvLock,
    /// Items a click on the lock unlocks (locked, not identity providers).
    pub unlock_ids: Vec<String>,
    /// Items a click on the lock locks (unlocked).
    pub lock_ids: Vec<String>,
    /// Open.
    pub open: bool,
    /// Sites with their items, by name.
    pub sites: Vec<VaultSite>,
    /// Files, when it has any.
    pub files: Option<VaultFiles>,
}

/// What the batch bar offers for the current selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSelection {
    /// Selected items.
    pub count: u32,
    /// Their ids, in list order.
    pub ids: Vec<String>,
    /// "3 selected".
    pub title: String,
    /// Some selected item is locked and may be unlocked.
    pub can_unlock: bool,
    /// Some selected item is unlocked.
    pub can_lock: bool,
    /// Selected items that always ask (identity providers).
    pub always_ask: u32,
    /// The ids to send when unlocking (locked, not identity providers).
    pub unlock_ids: Vec<String>,
    /// The ids to send when locking (unlocked).
    pub lock_ids: Vec<String>,
}

/// The list as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultView {
    /// Apps, by name.
    pub apps: Vec<VaultApp>,
    /// Items shown (after search).
    pub shown: u32,
    /// Items in the vault.
    pub total: u32,
    /// "No items yet." and friends.
    pub empty_text: Option<String>,
    /// Domains and keys are hidden until the user confirms with Touch ID.
    pub names_hidden: bool,
    /// What to say about it.
    pub hidden_note: Option<String>,
    /// The button that opens the names.
    pub show_names_label: String,
    /// The search field's prompt.
    pub search_prompt: String,
    /// The selection.
    pub selection: VaultSelection,
    /// Select all is offered.
    pub can_select_all: bool,
}

/// The search prompt.
pub const SEARCH_PROMPT: &str = "Search by domain, key, app or type";

fn tokens(query: &str) -> Vec<String> {
    query.split_whitespace().map(|t| t.to_lowercase()).collect()
}

/// Whether `item` matches every word of the query: a word may be found in
/// the domain, the key (or path), the app or the type.
pub fn matches(item: &KvItem, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let kind = KvKind::from_wire(&item.kind);
    let hay = [
        item.app_display.to_lowercase(),
        item.provider_id.to_lowercase(),
        item.domain.clone().unwrap_or_default().to_lowercase(),
        item.key.to_lowercase(),
        item.path.clone().unwrap_or_default().to_lowercase(),
        kind.label().to_lowercase(),
        item.kind.replace('_', " "),
        if item.policy.unattended {
            "unlocked".into()
        } else {
            "locked".into()
        },
    ];
    words.iter().all(|w| hay.iter().any(|h| h.contains(w)))
}

fn site_key(provider: &str, site: &str) -> String {
    format!("{provider}|{site}")
}

fn files_key(provider: &str) -> String {
    format!("{provider}|\u{1}files")
}

/// The site an item sits under: its registrable domain (`github.com`).
fn site_of_item(i: &KvItem) -> String {
    i.domain
        .as_deref()
        .map(site_of)
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// A conservative registrable domain for a cookie host, an origin or a host
/// (the broker's grouping; see `cua_keyvault::record::site_of`).
pub fn site_of(domain: &str) -> String {
    const MULTI: [&str; 10] = [
        "co.uk", "org.uk", "gov.uk", "ac.uk", "co.jp", "com.au", "com.br", "co.nz", "co.in",
        "com.cn",
    ];
    let d = domain.trim();
    let d = d.split_once("://").map_or(d, |(_, r)| r);
    let d = d.split(['/', '?', '#']).next().unwrap_or(d);
    let d = d.rsplit_once('@').map_or(d, |(_, r)| r);
    let d = match d.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) && !h.contains(':') => h,
        _ => d,
    };
    let host = d.trim_start_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 || host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return host;
    }
    let last_two = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
    if MULTI.contains(&last_two.as_str()) {
        return format!("{}.{last_two}", labels[labels.len() - 3]);
    }
    last_two
}

/// The folder part of a file path ("Default" for `Default/Bookmarks`).
fn folder_of(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default()
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn lock_help(i: &KvItem) -> String {
    if i.identity_provider {
        "Identity providers always ask".into()
    } else if i.policy.unattended {
        "Unlocked: any agent with the Cua Spaces MCP can write this into your Spaces. Click to lock.".into()
    } else {
        "Locked: every use needs your approval. Click to allow unattended access.".into()
    }
}

fn row(i: &KvItem, selected: bool, now: i64) -> VaultRow {
    let kind = KvKind::from_wire(&i.kind);
    let (title, subtitle) = if i.key.is_empty() && i.domain.is_none() {
        // The names are hidden.
        (kind.label().to_string(), String::new())
    } else if kind == KvKind::File {
        (file_name(&i.key), folder_of(&i.key))
    } else {
        let mut sub = i.domain.clone().unwrap_or_default();
        if kind == KvKind::Cookie
            && let Some(p) = i.path.as_deref().filter(|p| *p != "/")
        {
            sub = format!("{sub}{p}");
        }
        (i.key.clone(), sub)
    };
    VaultRow {
        id: i.id.clone(),
        kind,
        kind_label: kind.label().into(),
        symbol: kind.symbol().into(),
        title,
        subtitle,
        updated: ago(now - i.updated_ms as i64),
        locked: !i.policy.unattended,
        lock_symbol: if i.policy.unattended {
            "lock.open"
        } else {
            "lock.fill"
        }
        .into(),
        lock_help: lock_help(i),
        identity_provider: i.identity_provider,
        selected,
    }
}

/// The ids a click on a group's lock acts on: (unlock, lock).
fn lock_targets(items: &[&KvItem]) -> (Vec<String>, Vec<String>) {
    let unlock = items
        .iter()
        .filter(|i| !i.policy.unattended && !i.identity_provider)
        .map(|i| i.id.clone())
        .collect();
    let lock = items
        .iter()
        .filter(|i| i.policy.unattended)
        .map(|i| i.id.clone())
        .collect();
    (unlock, lock)
}

fn tri(selected: usize, of: usize) -> Tri {
    if selected == 0 {
        Tri::Off
    } else if selected == of {
        Tri::On
    } else {
        Tri::Mixed
    }
}

/// The items the list shows: the vault's, filtered by the query, in list
/// order (app, then files last inside it; sites by name; type, then key).
fn shown<'a>(o: &'a KeyvaultOverview, state: &VaultState) -> Vec<&'a KvItem> {
    let words = tokens(&state.query);
    let mut items: Vec<&KvItem> = o
        .items
        .iter()
        .filter(|i| state.app.as_ref().is_none_or(|a| a == &i.provider_id))
        .filter(|i| matches(i, &words))
        .collect();
    items.sort_by(|a, b| {
        let (ka, kb) = (KvKind::from_wire(&a.kind), KvKind::from_wire(&b.kind));
        collate(&a.app_display, &b.app_display)
            .then_with(|| a.provider_id.cmp(&b.provider_id))
            .then_with(|| (ka == KvKind::File).cmp(&(kb == KvKind::File)))
            .then_with(|| collate(&site_of_item(a), &site_of_item(b)))
            .then_with(|| ka.rank().cmp(&kb.rank()))
            .then_with(|| collate(&a.key, &b.key))
            .then_with(|| {
                collate(
                    &a.domain.clone().unwrap_or_default(),
                    &b.domain.clone().unwrap_or_default(),
                )
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    items
}

/// The ids a group key stands for among the shown items.
fn group_ids(o: &KeyvaultOverview, state: &VaultState, key: &str) -> Vec<String> {
    shown(o, state)
        .into_iter()
        .filter(|i| {
            let app = i.provider_id.as_str();
            let is_file = KvKind::from_wire(&i.kind) == KvKind::File;
            key == app
                || (is_file && key == files_key(app))
                || (!is_file && key == site_key(app, &site_of_item(i)))
        })
        .map(|i| i.id.clone())
        .collect()
}

/// Advances the list.
pub fn reduce(o: &KeyvaultOverview, state: &VaultState, action: &VaultAction) -> VaultState {
    let mut s = state.clone();
    match action {
        VaultAction::Query { text } => {
            s.query = text.clone();
            // Selecting what a search hides would act on rows the user
            // cannot see: keep only what is still shown.
            let visible: Vec<String> = shown(o, &s).iter().map(|i| i.id.clone()).collect();
            s.selected.retain(|id| visible.contains(id));
        }
        VaultAction::Toggle { id } => {
            if !o.items.iter().any(|i| &i.id == id) {
                return s;
            }
            match s.selected.iter().position(|x| x == id) {
                Some(n) => {
                    s.selected.remove(n);
                }
                None => s.selected.push(id.clone()),
            }
        }
        VaultAction::ToggleGroup { key } => {
            let ids = group_ids(o, &s, key);
            if ids.is_empty() {
                return s;
            }
            if ids.iter().all(|id| s.selected.contains(id)) {
                s.selected.retain(|id| !ids.contains(id));
            } else {
                for id in ids {
                    if !s.selected.contains(&id) {
                        s.selected.push(id);
                    }
                }
            }
        }
        VaultAction::SelectAll => {
            s.selected = shown(o, &s).iter().map(|i| i.id.clone()).collect();
        }
        VaultAction::Clear => s.selected.clear(),
        VaultAction::ToggleOpen { key } => match s.expanded.iter().position(|k| k == key) {
            Some(n) => {
                s.expanded.remove(n);
            }
            None => s.expanded.push(key.clone()),
        },
    }
    // Keep the selection in list order, without ids that are gone.
    let order: Vec<String> = shown(o, &s).iter().map(|i| i.id.clone()).collect();
    s.selected = order
        .into_iter()
        .filter(|id| s.selected.contains(id))
        .collect();
    s
}

/// Drops what the vault no longer holds from the selection (after a delete
/// or a refresh).
pub fn prune(o: &KeyvaultOverview, state: &VaultState) -> VaultState {
    let mut s = state.clone();
    s.selected.retain(|id| o.items.iter().any(|i| &i.id == id));
    s
}

fn newest(items: &[&KvItem]) -> u64 {
    items.iter().map(|i| i.updated_ms).max().unwrap_or(0)
}

/// The list as drawn.
pub fn view(o: &KeyvaultOverview, state: &VaultState, now: i64) -> VaultView {
    let items = shown(o, state);
    let searching = !state.query.trim().is_empty();
    let names_hidden = !o.items.is_empty() && !o.names_visible;
    let open = |key: &str| searching || state.expanded.iter().any(|k| k == key);
    let sel = |i: &KvItem| state.selected.contains(&i.id);

    // app key -> (name, items)
    let mut by_app: BTreeMap<String, (String, Vec<&KvItem>)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for i in &items {
        if !by_app.contains_key(&i.provider_id) {
            order.push(i.provider_id.clone());
        }
        by_app
            .entry(i.provider_id.clone())
            .or_insert_with(|| (i.app_display.clone(), Vec::new()))
            .1
            .push(i);
    }
    let mut apps = Vec::new();
    for key in order {
        let (name, group) = by_app.remove(&key).expect("keyed above");
        let (files, rest): (Vec<&KvItem>, Vec<&KvItem>) = group
            .iter()
            .copied()
            .partition(|i| KvKind::from_wire(&i.kind) == KvKind::File);
        let mut sites: Vec<VaultSite> = Vec::new();
        let mut cur: Option<(String, Vec<&KvItem>)> = None;
        let flush = |cur: &mut Option<(String, Vec<&KvItem>)>, sites: &mut Vec<VaultSite>| {
            if let Some((site, its)) = cur.take() {
                let k = site_key(&key, &site);
                let mut kinds: Vec<(KvKind, u32)> = Vec::new();
                for i in &its {
                    let kk = KvKind::from_wire(&i.kind);
                    match kinds.iter_mut().find(|(x, _)| *x == kk) {
                        Some((_, n)) => *n += 1,
                        None => kinds.push((kk, 1)),
                    }
                }
                kinds.sort_by_key(|(k, _)| k.rank());
                let is_open = open(&k);
                let (unlock_ids, lock_ids) = lock_targets(&its);
                sites.push(VaultSite {
                    selected: tri(its.iter().filter(|i| sel(i)).count(), its.len()),
                    lock: lock_of(&its),
                    unlock_ids,
                    lock_ids,
                    count: its.len() as u32,
                    counts: kinds
                        .iter()
                        .map(|(kk, n)| kk.plural(*n))
                        .collect::<Vec<_>>()
                        .join(", "),
                    updated: ago(now - newest(&its) as i64),
                    open: is_open,
                    rows: if is_open {
                        its.iter().map(|i| row(i, sel(i), now)).collect()
                    } else {
                        vec![]
                    },
                    site: if site.is_empty() {
                        "Unnamed".into()
                    } else {
                        site
                    },
                    key: k,
                });
            }
        };
        for i in rest {
            let site = site_of_item(i);
            match cur.as_mut() {
                Some((s, its)) if *s == site => its.push(i),
                _ => {
                    flush(&mut cur, &mut sites);
                    cur = Some((site, vec![i]));
                }
            }
        }
        flush(&mut cur, &mut sites);
        let files_group = (!files.is_empty()).then(|| {
            let k = files_key(&key);
            let is_open = open(&k);
            let (unlock_ids, lock_ids) = lock_targets(&files);
            VaultFiles {
                count: files.len() as u32,
                selected: tri(files.iter().filter(|i| sel(i)).count(), files.len()),
                lock: lock_of(&files),
                unlock_ids,
                lock_ids,
                open: is_open,
                rows: if is_open {
                    files.iter().map(|i| row(i, sel(i), now)).collect()
                } else {
                    vec![]
                },
                key: k,
            }
        });
        let unlocked = group.iter().filter(|i| i.policy.unattended).count();
        let n = group.len();
        let (unlock_ids, lock_ids) = lock_targets(&group);
        apps.push(VaultApp {
            unlock_ids,
            lock_ids,
            provider_id: key.clone(),
            name,
            count: n as u32,
            summary: if unlocked == 0 {
                format!("{n} item{}", if n == 1 { "" } else { "s" })
            } else {
                format!(
                    "{n} item{}, {unlocked} unlocked",
                    if n == 1 { "" } else { "s" }
                )
            },
            updated: ago(now - newest(&group) as i64),
            selected: tri(group.iter().filter(|i| sel(i)).count(), n),
            lock: lock_of(&group),
            open: open(&key),
            sites,
            files: files_group,
            key,
        });
    }

    let selected: Vec<&KvItem> = items.iter().copied().filter(|i| sel(i)).collect();
    let unlock_ids: Vec<String> = selected
        .iter()
        .filter(|i| !i.policy.unattended && !i.identity_provider)
        .map(|i| i.id.clone())
        .collect();
    let lock_ids: Vec<String> = selected
        .iter()
        .filter(|i| i.policy.unattended)
        .map(|i| i.id.clone())
        .collect();
    let selection = VaultSelection {
        count: selected.len() as u32,
        ids: selected.iter().map(|i| i.id.clone()).collect(),
        title: format!("{} selected", selected.len()),
        can_unlock: !unlock_ids.is_empty(),
        can_lock: !lock_ids.is_empty(),
        always_ask: selected
            .iter()
            .filter(|i| i.identity_provider && !i.policy.unattended)
            .count() as u32,
        unlock_ids,
        lock_ids,
    };
    let empty_text = if !apps.is_empty() {
        None
    } else if o.items.is_empty() {
        Some(
            "Nothing saved yet. Teleport an app with Save to Keyvault, or import passwords.".into(),
        )
    } else {
        Some("No matches".into())
    };
    VaultView {
        shown: items.len() as u32,
        total: o.items.len() as u32,
        can_select_all: !items.is_empty(),
        apps,
        empty_text,
        names_hidden,
        hidden_note: names_hidden
            .then(|| "Names are hidden. Confirm with Touch ID to see what is saved.".into()),
        show_names_label: "Show Items".into(),
        search_prompt: SEARCH_PROMPT.into(),
        selection,
    }
}

/// The broker request that locks `ids`.
pub fn lock_command(ids: &[String]) -> KvCommand {
    KvCommand::SetLocked {
        item_ids: ids.to_vec(),
        locked: true,
    }
}

/// The broker request that unlocks `ids` (the daemon asks for Touch ID once
/// for the batch).
pub fn unlock_command(ids: &[String]) -> KvCommand {
    KvCommand::SetLocked {
        item_ids: ids.to_vec(),
        locked: false,
    }
}

/// The broker request that deletes `ids` and wipes their copies in Spaces.
pub fn delete_command(ids: &[String]) -> KvCommand {
    KvCommand::DeleteItems {
        item_ids: ids.to_vec(),
    }
}

/// What the Keyvault holds for one app that a teleport can send (passwords
/// never are): how many, when the newest was saved, and their ids.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvVaultSource {
    /// Items that can be sent.
    pub count: u32,
    /// When the newest was saved, Unix ms (0: none).
    pub newest_ms: i64,
    /// Their ids, in list order.
    pub ids: Vec<String>,
    /// The app's saved passwords (ids), never part of `ids`: they are sent
    /// only when ticked in the review.
    #[serde(default)]
    pub password_ids: Vec<String>,
}

/// The saved items of `provider_id` a teleport can send.
pub fn vault_source(o: &KeyvaultOverview, provider_id: &str) -> KvVaultSource {
    let state = VaultState {
        app: Some(provider_id.into()),
        ..Default::default()
    };
    let all = shown(o, &state);
    let password_ids: Vec<String> = all
        .iter()
        .filter(|i| KvKind::from_wire(&i.kind) == KvKind::Password)
        .map(|i| i.id.clone())
        .collect();
    let items: Vec<&KvItem> = all
        .into_iter()
        .filter(|i| KvKind::from_wire(&i.kind) != KvKind::Password)
        .collect();
    KvVaultSource {
        password_ids,
        count: items.len() as u32,
        newest_ms: items.iter().map(|i| i.updated_ms as i64).max().unwrap_or(0),
        ids: items.iter().map(|i| i.id.clone()).collect(),
    }
}

/// The unlock prompt: what the user is agreeing to before the daemon asks
/// for Touch ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlockPrompt {
    /// "Allow unattended access?"
    pub title: String,
    /// What it allows.
    pub message: String,
    /// "3 items" (a batch), or the one item's name.
    pub subject: String,
    /// "Deny".
    pub deny: String,
    /// "Allow".
    pub allow: String,
    /// "Never ask again".
    pub never_ask: String,
}

/// The unlock prompt for `count` items (`name` is the one item's name, for
/// a single item). `None` when the user chose "Never ask again": unlocking
/// then goes straight to the Touch ID check the broker asks for.
pub fn unlock_prompt(o: &KeyvaultOverview, count: u32, name: Option<&str>) -> Option<UnlockPrompt> {
    if o.status.as_ref().and_then(|s| s.skip_unlock_prompt) == Some(true) {
        return None;
    }
    Some(unlock_prompt_always(count, name))
}

/// The prompt itself, whatever the setting says.
pub fn unlock_prompt_always(count: u32, name: Option<&str>) -> UnlockPrompt {
    let one = count == 1;
    UnlockPrompt {
        title: "Allow unattended access?".into(),
        message: format!(
            "This will allow any agent with access to the Cua Spaces MCP to write {} into your connected Spaces. The secrets are not sent directly to any agents.",
            if one { "this key" } else { "these keys" }
        ),
        subject: match (one, name) {
            (true, Some(n)) if !n.is_empty() => n.to_string(),
            _ => format!("{count} items"),
        },
        deny: "Deny".into(),
        allow: "Allow".into(),
        never_ask: "Never ask again".into(),
    }
}

/// The delete confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvDeleteConfirm {
    /// "Delete 3 items?"
    pub title: String,
    /// What deleting does.
    pub message: String,
    /// "Delete".
    pub confirm: String,
    /// "Cancel".
    pub cancel: String,
}

/// The delete confirmation for `count` items, saying when live copies in
/// Spaces will be wiped too (`live_copies`: Spaces holding any of them).
pub fn delete_confirm(count: u32, live_copies: u32) -> KvDeleteConfirm {
    let one = count == 1;
    let mut message = format!(
        "{} removed from the Keyvault and cannot be recovered.",
        if one { "It is" } else { "They are" }
    );
    if live_copies > 0 {
        message.push_str(&format!(
            " Copies delivered to {} will be wiped as well.",
            if live_copies == 1 {
                "a Space".to_string()
            } else {
                format!("{live_copies} Spaces")
            }
        ));
    }
    KvDeleteConfirm {
        title: if one {
            "Delete this item?".into()
        } else {
            format!("Delete {count} items?")
        },
        message,
        confirm: "Delete".into(),
        cancel: "Cancel".into(),
    }
}

/// How many Spaces hold a live copy of any of `ids` (for
/// [`delete_confirm`]).
pub fn live_copy_spaces(o: &KeyvaultOverview, ids: &[String], now: i64) -> u32 {
    let mut targets: Vec<&str> = o
        .deliveries
        .iter()
        .filter(|d| super::view::live_delivery(d, now) && d.items.iter().any(|i| ids.contains(i)))
        .map(|d| d.target.as_str())
        .collect();
    targets.sort_unstable();
    targets.dedup();
    targets.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyvault::wire::{KvDelivery, KvItemPolicy, KvStatus};

    const NOW: i64 = 1_800_000_000_000;

    fn item(id: &str, app: (&str, &str), kind: &str, domain: Option<&str>, key: &str) -> KvItem {
        KvItem {
            id: id.into(),
            kind: kind.into(),
            provider_id: app.0.into(),
            app_display: app.1.into(),
            domain: domain.map(str::to_string),
            key: key.into(),
            path: None,
            source: "Default".into(),
            session: false,
            expires_ms: None,
            bytes: 10,
            blob: None,
            identity_provider: false,
            policy: KvItemPolicy {
                allowed_targets: vec![],
                ttl_secs: 3600,
                unattended: false,
            },
            created_ms: (NOW - 3_600_000) as u64,
            updated_ms: (NOW - 120_000) as u64,
            rev: 1,
            record_digest: "00".into(),
        }
    }

    const CHROME: (&str, &str) = ("chrome", "Google Chrome");
    const SLACK: (&str, &str) = ("slack", "Slack");

    fn vault() -> KeyvaultOverview {
        let mut items = vec![
            item("c1", CHROME, "cookie", Some(".github.com"), "user_session"),
            item("c2", CHROME, "cookie", Some(".github.com"), "logged_in"),
            item("c3", CHROME, "cookie", Some("api.github.com"), "_gh_sess"),
            item("p1", CHROME, "password", Some("https://github.com"), "octo"),
            item("c4", CHROME, "cookie", Some(".notion.so"), "token_v2"),
            item(
                "l1",
                CHROME,
                "local_storage",
                Some("https://notion.so"),
                "theme",
            ),
            item("f1", CHROME, "file", None, "Default/Bookmarks"),
            item("f2", CHROME, "file", None, "Local State"),
            item("s1", SLACK, "file", None, "storage/root-state.json"),
            item("s2", SLACK, "cookie", Some(".slack.com"), "d"),
        ];
        items[0].policy.unattended = true;
        let mut google = item("g1", CHROME, "cookie", Some(".google.com"), "SID");
        google.identity_provider = true;
        items.push(google);
        KeyvaultOverview {
            availability: "ready".into(),
            names_visible: true,
            items_total: items.len() as u32,
            items,
            ..Default::default()
        }
    }

    fn st() -> VaultState {
        VaultState::default()
    }

    fn ids(v: &VaultView) -> Vec<String> {
        v.selection.ids.clone()
    }

    #[test]
    fn items_group_by_app_then_site_with_files_apart() {
        let v = view(&vault(), &st(), NOW);
        assert_eq!(
            v.apps.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Google Chrome", "Slack"]
        );
        let chrome = &v.apps[0];
        assert_eq!(chrome.count, 9);
        assert_eq!(
            chrome
                .sites
                .iter()
                .map(|s| s.site.as_str())
                .collect::<Vec<_>>(),
            ["github.com", "google.com", "notion.so"],
            "api.github.com and .github.com are one site"
        );
        assert_eq!(chrome.sites[0].count, 4);
        assert_eq!(chrome.sites[0].counts, "1 password, 3 cookies");
        assert_eq!(chrome.sites[2].counts, "1 cookie, 1 storage value");
        assert_eq!(chrome.files.as_ref().unwrap().count, 2);
        assert_eq!(chrome.summary, "9 items, 1 unlocked");
        assert_eq!(v.apps[1].summary, "2 items");
        assert!(v.empty_text.is_none());
        assert_eq!((v.shown, v.total), (11, 11));
        // Grey times, and no value anywhere.
        assert_eq!(chrome.updated, "2 min ago");
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("record_digest"));
    }

    #[test]
    fn groups_open_on_demand_and_while_searching() {
        let mut s = st();
        let v = view(&vault(), &s, NOW);
        assert!(v.apps[0].sites[0].rows.is_empty(), "closed: no rows");
        assert!(!v.apps[0].sites[0].open);
        s = reduce(
            &vault(),
            &s,
            &VaultAction::ToggleOpen {
                key: "chrome|github.com".into(),
            },
        );
        let v = view(&vault(), &s, NOW);
        let rows = &v.apps[0].sites[0].rows;
        assert_eq!(
            rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["octo", "_gh_sess", "logged_in", "user_session"],
            "a password first, then cookies by name"
        );
        assert_eq!(rows[0].symbol, "key.fill");
        assert_eq!(rows[1].subtitle, "api.github.com");
        assert_eq!(rows[3].subtitle, ".github.com");
        assert!(rows[3].lock_symbol == "lock.open" && rows[0].lock_symbol == "lock.fill");
        s = reduce(
            &vault(),
            &s,
            &VaultAction::ToggleOpen {
                key: "chrome|github.com".into(),
            },
        );
        assert!(!view(&vault(), &s, NOW).apps[0].sites[0].open);
        // Searching opens what matches.
        s = reduce(
            &vault(),
            &s,
            &VaultAction::Query {
                text: "user_session".into(),
            },
        );
        let v = view(&vault(), &s, NOW);
        assert!(v.apps[0].sites[0].open);
        assert_eq!(v.apps[0].sites[0].rows.len(), 1);
    }

    #[test]
    fn search_matches_domain_key_app_and_type() {
        let o = vault();
        let q = |text: &str| {
            let s = reduce(&o, &st(), &VaultAction::Query { text: text.into() });
            view(&o, &s, NOW)
        };
        assert_eq!(q("notion").shown, 2, "by domain");
        assert_eq!(q("token_v2").shown, 1, "by key");
        assert_eq!(q("slack").shown, 2, "by app");
        assert_eq!(q("google chrome").shown, 9, "by app name");
        assert_eq!(q("password").shown, 1, "by type");
        assert_eq!(q("local storage").shown, 1, "by type, two words");
        assert_eq!(q("cookie github").shown, 3, "type and domain together");
        assert_eq!(q("bookmarks").shown, 1, "a file by path");
        assert_eq!(q("unlocked").shown, 1, "by lock");
        let none = q("nothing-like-this");
        assert_eq!(none.shown, 0);
        assert_eq!(none.empty_text.as_deref(), Some("No matches"));
        assert!(none.apps.is_empty());
        assert_eq!(q("  ").shown, 11, "blank shows everything");
    }

    #[test]
    fn selecting_items_a_site_or_an_app_and_the_batch_bar() {
        let o = vault();
        let mut s = st();
        s = reduce(&o, &s, &VaultAction::Toggle { id: "c2".into() });
        s = reduce(&o, &s, &VaultAction::Toggle { id: "p1".into() });
        let v = view(&o, &s, NOW);
        assert_eq!(v.selection.count, 2);
        assert_eq!(v.selection.title, "2 selected");
        assert_eq!(v.apps[0].sites[0].selected, Tri::Mixed);
        assert_eq!(v.apps[0].selected, Tri::Mixed);
        // A whole site: every item of it (mixed selects the rest).
        s = reduce(
            &o,
            &s,
            &VaultAction::ToggleGroup {
                key: "chrome|github.com".into(),
            },
        );
        let v = view(&o, &s, NOW);
        assert_eq!(v.apps[0].sites[0].selected, Tri::On);
        assert_eq!(ids(&v).len(), 4);
        // And again: none of it.
        s = reduce(
            &o,
            &s,
            &VaultAction::ToggleGroup {
                key: "chrome|github.com".into(),
            },
        );
        assert_eq!(view(&o, &s, NOW).selection.count, 0);
        // A whole app, files included.
        s = reduce(
            &o,
            &s,
            &VaultAction::ToggleGroup {
                key: "chrome".into(),
            },
        );
        let v = view(&o, &s, NOW);
        assert_eq!(v.selection.count, 9);
        assert_eq!(v.apps[0].selected, Tri::On);
        assert_eq!(v.apps[1].selected, Tri::Off);
        // Only the files of an app.
        s = reduce(
            &o,
            &st(),
            &VaultAction::ToggleGroup {
                key: "chrome|\u{1}files".into(),
            },
        );
        assert_eq!(ids(&view(&o, &s, NOW)), ["f1", "f2"]);
        // Select all, clear, and unknown ids are ignored.
        s = reduce(&o, &s, &VaultAction::SelectAll);
        assert_eq!(view(&o, &s, NOW).selection.count, 11);
        s = reduce(&o, &s, &VaultAction::Toggle { id: "ghost".into() });
        assert_eq!(view(&o, &s, NOW).selection.count, 11);
        s = reduce(&o, &s, &VaultAction::Clear);
        assert_eq!(view(&o, &s, NOW).selection.count, 0);
    }

    #[test]
    fn a_search_never_leaves_hidden_rows_selected_and_groups_select_only_what_is_shown() {
        let o = vault();
        let mut s = reduce(&o, &st(), &VaultAction::SelectAll);
        s = reduce(
            &o,
            &s,
            &VaultAction::Query {
                text: "notion".into(),
            },
        );
        assert_eq!(
            ids(&view(&o, &s, NOW)),
            ["c4", "l1"],
            "what the search hides is deselected"
        );
        // With a search on, selecting a whole app selects only its matches.
        s = reduce(
            &o,
            &st(),
            &VaultAction::Query {
                text: "cookie".into(),
            },
        );
        s = reduce(
            &o,
            &s,
            &VaultAction::ToggleGroup {
                key: "chrome".into(),
            },
        );
        let v = view(&o, &s, NOW);
        assert!(
            v.selection
                .ids
                .iter()
                .all(|id| id.starts_with('c') || id.starts_with('g'))
        );
        assert!(!v.selection.ids.contains(&"p1".to_string()));
    }

    #[test]
    fn lock_state_and_what_the_batch_can_do() {
        let o = vault();
        let v = view(&o, &st(), NOW);
        assert_eq!(
            v.apps[0].sites[0].lock,
            KvLock::Mixed,
            "github has one unlocked cookie"
        );
        assert_eq!(v.apps[0].sites[1].lock, KvLock::Locked);
        assert_eq!(v.apps[1].lock, KvLock::Locked);
        // A click on a group's lock acts on its eligible items.
        assert_eq!(v.apps[0].sites[0].unlock_ids.len(), 3, "github: 3 locked");
        assert_eq!(v.apps[0].sites[0].lock_ids, ["c1"]);
        assert!(
            v.apps[0].sites[1].unlock_ids.is_empty(),
            "an identity provider never unlocks"
        );
        assert_eq!(v.apps[0].lock_ids, ["c1"]);
        assert_eq!(KvLock::Locked.symbol(), "lock.fill");
        assert_eq!(KvLock::Unlocked.symbol(), "lock.open");
        // Everything selected: c1 is unlocked, the identity provider always
        // asks, the rest can be unlocked.
        let s = reduce(&o, &st(), &VaultAction::SelectAll);
        let sel = view(&o, &s, NOW).selection;
        assert!(sel.can_lock && sel.can_unlock);
        assert_eq!(sel.lock_ids, ["c1"]);
        assert_eq!(sel.unlock_ids.len(), 9);
        assert!(!sel.unlock_ids.contains(&"g1".to_string()));
        assert_eq!(sel.always_ask, 1);
        assert_eq!(
            unlock_command(&sel.unlock_ids),
            KvCommand::SetLocked {
                item_ids: sel.unlock_ids.clone(),
                locked: false
            }
        );
        assert_eq!(
            lock_command(&sel.lock_ids),
            KvCommand::SetLocked {
                item_ids: vec!["c1".into()],
                locked: true
            }
        );
        // Only the identity provider: nothing to unlock.
        let s = reduce(&o, &st(), &VaultAction::Toggle { id: "g1".into() });
        let sel = view(&o, &s, NOW).selection;
        assert!(!sel.can_unlock && !sel.can_lock);
        // Rows say why.
        let mut open = st();
        open.expanded = vec!["chrome|google.com".into()];
        let r = &view(&o, &open, NOW).apps[0].sites[1].rows[0];
        assert!(r.identity_provider);
        assert_eq!(r.lock_help, "Identity providers always ask");
    }

    #[test]
    fn the_unlock_prompt_says_what_it_allows_and_is_skipped_only_by_the_setting() {
        let mut o = vault();
        let p = unlock_prompt(&o, 1, Some("user_session")).unwrap();
        assert_eq!(p.title, "Allow unattended access?");
        assert_eq!(
            p.message,
            "This will allow any agent with access to the Cua Spaces MCP to write this key into your connected Spaces. The secrets are not sent directly to any agents."
        );
        assert_eq!(p.subject, "user_session");
        assert_eq!(
            (p.deny.as_str(), p.allow.as_str(), p.never_ask.as_str()),
            ("Deny", "Allow", "Never ask again")
        );
        let batch = unlock_prompt(&o, 3, None).unwrap();
        assert!(
            batch
                .message
                .contains("write these keys into your connected Spaces")
        );
        assert_eq!(batch.subject, "3 items");
        for text in [&p.title, &p.message, &p.subject, &batch.message] {
            assert!(
                !text.contains('\u{2014}') && !text.contains('\u{2013}'),
                "no dashes: {text}"
            );
        }
        // "Never ask again" skips the prompt, never the Touch ID check.
        o.status = Some(KvStatus {
            version: "0".into(),
            initialized: true,
            unlocked: true,
            disabled: false,
            caller_first_party: true,
            caller_display: "Cua".into(),
            items: 11,
            pending: 0,
            unlock_policy: None,
            auto_wipe: None,
            os_protector_available: true,
            passphrase_available: true,
            unlock_protectors: vec![],
            browse_until_ms: None,
            skip_unlock_prompt: Some(true),
            reset_notice: None,
        });
        assert!(unlock_prompt(&o, 1, None).is_none());
        assert!(unlock_prompt_always(1, None).message.contains("this key"));
    }

    #[test]
    fn deleting_names_the_copies_it_will_wipe() {
        let one = delete_confirm(1, 0);
        assert_eq!(one.title, "Delete this item?");
        assert!(one.message.starts_with("It is removed"));
        let many = delete_confirm(4, 2);
        assert_eq!(many.title, "Delete 4 items?");
        assert!(
            many.message
                .contains("Copies delivered to 2 Spaces will be wiped")
        );
        assert!(delete_confirm(2, 1).message.contains("a Space"));
        let mut o = vault();
        o.deliveries = vec![
            KvDelivery {
                import_id: "i1".into(),
                target: "dev-1".into(),
                provider_id: "chrome".into(),
                items: vec!["c1".into()],
                caller_fp: "x".into(),
                delivered_ms: 1,
                expires_ms: 0,
                wiped: false,
            },
            KvDelivery {
                import_id: "i2".into(),
                target: "dev-1".into(),
                provider_id: "chrome".into(),
                items: vec!["c2".into()],
                caller_fp: "x".into(),
                delivered_ms: 1,
                expires_ms: 0,
                wiped: false,
            },
            KvDelivery {
                import_id: "i3".into(),
                target: "dev-2".into(),
                provider_id: "chrome".into(),
                items: vec!["c3".into()],
                caller_fp: "x".into(),
                delivered_ms: 1,
                expires_ms: 0,
                wiped: true,
            },
        ];
        assert_eq!(
            live_copy_spaces(&o, &["c1".into(), "c2".into(), "c3".into()], NOW),
            1
        );
        assert_eq!(live_copy_spaces(&o, &["c4".into()], NOW), 0);
        assert_eq!(
            delete_command(&["c1".into()]),
            KvCommand::DeleteItems {
                item_ids: vec!["c1".into()]
            }
        );
    }

    #[test]
    fn hidden_names_group_by_app_and_offer_to_show_them() {
        let mut o = vault();
        o.names_visible = false;
        for i in &mut o.items {
            i.domain = None;
            i.key = String::new();
            i.path = None;
        }
        let v = view(&o, &st(), NOW);
        assert!(v.names_hidden);
        assert_eq!(v.show_names_label, "Show Items");
        assert!(v.hidden_note.as_deref().unwrap().contains("Touch ID"));
        assert_eq!(v.apps[0].count, 9, "counts survive");
        // Rows still render (type only), and nothing names a site.
        assert!(v.apps[0].sites.iter().all(|s| s.site == "Unnamed"));
        let empty = view(&KeyvaultOverview::default(), &st(), NOW);
        assert!(!empty.names_hidden);
        assert!(
            empty
                .empty_text
                .as_deref()
                .unwrap()
                .starts_with("Nothing saved yet")
        );
    }

    #[test]
    fn the_vault_source_of_an_app_leaves_out_passwords() {
        let o = vault();
        let src = vault_source(&o, "chrome");
        assert_eq!(
            src.count, 8,
            "nine Chrome items less the password... and the IdP cookie stays"
        );
        assert!(!src.ids.contains(&"p1".to_string()));
        assert_eq!(src.newest_ms, (NOW - 120_000));
        assert_eq!(vault_source(&o, "slack").count, 2);
        assert_eq!(vault_source(&o, "arc"), KvVaultSource::default());
    }

    #[test]
    fn site_grouping_covers_hosts_origins_and_cookie_domains() {
        assert_eq!(site_of(".github.com"), "github.com");
        assert_eq!(site_of("https://gist.github.com:8443/x"), "github.com");
        assert_eq!(site_of("www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_of("localhost"), "localhost");
        assert_eq!(site_of("http://127.0.0.1:8000"), "127.0.0.1");
    }

    #[test]
    fn the_selection_survives_a_refresh_only_for_items_that_still_exist() {
        let o = vault();
        let s = reduce(&o, &st(), &VaultAction::SelectAll);
        let mut after = o.clone();
        after.items.retain(|i| i.id != "c1");
        assert_eq!(prune(&after, &s).selected.len(), 10);
    }
}
