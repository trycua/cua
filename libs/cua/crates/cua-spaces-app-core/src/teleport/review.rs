// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The review's choice of what to send.
//!
//! The consent review is the vault list again: for a browser the user picks
//! which sites (domains) and which other items to send instead of the whole
//! profile, with counts per site; and they can choose to send from the saved
//! Keyvault items of the app instead of reading the live app (no fresh
//! capture, so the host's Keychain is never asked).
//!
//! What was picked last time for an app and a Space is remembered (a list of
//! sites per `app|space`); a first review starts from a minimal default:
//! only the sites that look like they keep a sign-in, and never an identity
//! provider (its session unlocks other apps). Pure shaping: the SDK's
//! approval check (`cua_teleport::ux`) enforces what is sent.

use crate::keyvault::vault::site_of;
use crate::keyvault::wire::{KvDomainCount, KvInventory};
use crate::util::collate;
use serde::{Deserialize, Serialize};

/// Where the review sends from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendSource {
    /// Read the live app now (a browser's cookies are decrypted, so macOS asks
    /// for Keychain access).
    #[default]
    Live,
    /// Send the items saved in the Keyvault for this app. Nothing is read from
    /// the host.
    Vault,
}

/// What the user picked last time for one app and Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedChoice {
    /// `<app>|<space>`.
    pub key: String,
    /// The sites (registrable domains) that were sent.
    pub domains: Vec<String>,
}

/// The key a choice is remembered under.
pub fn remember_key(app: &str, space: &str) -> String {
    format!("{app}|{space}")
}

/// What was picked last time for `key`, if anything.
pub fn remembered(choices: &[RememberedChoice], key: &str) -> Option<Vec<String>> {
    choices
        .iter()
        .find(|c| c.key == key)
        .map(|c| c.domains.clone())
}

/// `choices` with `domains` remembered for `key` (replacing any earlier
/// choice; the list stays bounded).
pub fn remember(
    choices: &[RememberedChoice],
    key: &str,
    domains: &[String],
) -> Vec<RememberedChoice> {
    const MAX: usize = 64;
    let mut out: Vec<RememberedChoice> = choices.iter().filter(|c| c.key != key).cloned().collect();
    out.push(RememberedChoice {
        key: key.into(),
        domains: domains.to_vec(),
    });
    let excess = out.len().saturating_sub(MAX);
    out.drain(..excess);
    out
}

/// One site of a browser, with its counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDomain {
    /// The site (`github.com`).
    pub domain: String,
    /// "12 cookies, 2 storage values".
    pub counts: String,
    /// Items the site holds.
    pub count: u32,
    /// It looks like it keeps a sign-in.
    pub signin: bool,
    /// An identity provider: its session unlocks other apps.
    pub identity_provider: bool,
    /// Chosen to send.
    pub selected: bool,
    /// Can be chosen: it holds something that can be sent. A site whose
    /// cookies are all unreadable is greyed out.
    pub selectable: bool,
    /// Cookies that cannot be read, shown greyed with `unavailable_note`.
    pub unavailable: u32,
    /// "3 cookies cannot be sent: Chrome protects them with ...", or empty.
    pub unavailable_note: String,
}

/// A non-cookie consent line the user can turn off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewToggle {
    /// The consent item's key.
    pub key: String,
    /// Label.
    pub label: String,
    /// Detail.
    pub detail: String,
    /// Bytes that leave.
    pub bytes: u64,
    /// A secret.
    pub sensitive: bool,
    /// Sent (not turned off).
    pub selected: bool,
}

/// The saved Keyvault items of the app being teleported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSource {
    /// The app has saved items that can be sent (passwords never are).
    pub available: bool,
    /// How many.
    pub items: u32,
    /// "saved 2 days ago".
    pub saved: String,
    /// The items chosen to send (ids).
    pub selected: Vec<String>,
    /// The app's saved passwords (ids), sent only when ticked.
    #[serde(default)]
    pub password_ids: Vec<String>,
}

/// The review's choices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewChoice {
    /// Where to send from.
    pub source: SendSource,
    /// The browser's sites with counts (none until read, or for an app that
    /// is not a browser).
    pub domains: Vec<KvDomainCount>,
    /// The sites chosen.
    pub selected_domains: Vec<String>,
    /// The sites list's search text.
    pub query: String,
    /// Consent items (keys) turned off.
    pub excluded: Vec<String>,
    /// The saved items.
    pub vault: VaultSource,
    /// The sites were asked for (a failure counts: the review then sends
    /// what the plan lists).
    #[serde(default)]
    pub loaded: bool,
    /// The saved passwords are ticked to send (off unless the user ticks
    /// them; never remembered).
    #[serde(default)]
    pub include_passwords: bool,
}

/// The sites the minimal default picks: those that look like they keep a
/// sign-in, never an identity provider.
pub fn default_domains(domains: &[KvDomainCount]) -> Vec<String> {
    domains
        .iter()
        .filter(|d| d.signin && !d.identity_provider && sendable(d))
        .map(|d| d.domain.clone())
        .collect()
}

/// The choice after the inventory arrived: what was picked last time (only
/// the sites that still exist), else the minimal default.
pub fn with_inventory(
    mut c: ReviewChoice,
    inventory: &KvInventory,
    remembered: Option<&[String]>,
) -> ReviewChoice {
    let mut domains = inventory.domains.clone();
    domains.sort_by(|a, b| collate(&a.domain, &b.domain));
    c.selected_domains = match remembered {
        Some(r) => domains
            .iter()
            .filter(|d| r.iter().any(|x| site_of(x) == d.domain))
            .map(|d| d.domain.clone())
            .collect(),
        None => default_domains(&domains),
    };
    c.domains = domains;
    c
}

/// The site holds something that can be sent.
fn sendable(d: &KvDomainCount) -> bool {
    d.cookies + d.local_storage + d.passwords > 0
}

fn unavailable_note(d: &KvDomainCount) -> String {
    if d.unavailable == 0 {
        return String::new();
    }
    format!(
        "{} cookie{} cannot be sent: {}",
        d.unavailable,
        if d.unavailable == 1 { "" } else { "s" },
        d.unavailable_reason
    )
}

fn counts_text(d: &KvDomainCount) -> String {
    let mut parts = Vec::new();
    let mut add = |n: u32, one: &str, many: &str| {
        if n > 0 {
            parts.push(format!("{n} {}", if n == 1 { one } else { many }));
        }
    };
    add(d.cookies, "cookie", "cookies");
    add(d.local_storage, "storage value", "storage values");
    add(d.passwords, "password", "passwords");
    parts.join(", ")
}

/// The sites as the review lists them (filtered by the search text).
pub fn domain_rows(c: &ReviewChoice) -> Vec<ReviewDomain> {
    let words: Vec<String> = c
        .query
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect();
    c.domains
        .iter()
        .filter(|d| words.iter().all(|w| d.domain.to_lowercase().contains(w)))
        .map(|d| ReviewDomain {
            counts: counts_text(d),
            count: d.cookies + d.local_storage + d.passwords,
            signin: d.signin,
            identity_provider: d.identity_provider,
            selected: c.selected_domains.contains(&d.domain),
            selectable: sendable(d),
            unavailable: d.unavailable,
            unavailable_note: unavailable_note(d),
            domain: d.domain.clone(),
        })
        .collect()
}

/// "3 of 12 sites".
pub fn domain_summary(c: &ReviewChoice) -> String {
    format!(
        "{} of {} site{}",
        c.selected_domains.len(),
        c.domains.len(),
        if c.domains.len() == 1 { "" } else { "s" }
    )
}

/// Toggles one site.
pub fn toggle_domain(c: &mut ReviewChoice, domain: &str) {
    if !c.domains.iter().any(|d| d.domain == domain && sendable(d)) {
        return;
    }
    match c.selected_domains.iter().position(|d| d == domain) {
        Some(n) => {
            c.selected_domains.remove(n);
        }
        None => c.selected_domains.push(domain.into()),
    }
    sort_selected(c);
}

/// Selects or clears every site the search shows.
pub fn set_shown_domains(c: &mut ReviewChoice, value: bool) {
    let shown: Vec<String> = domain_rows(c)
        .into_iter()
        .filter(|r| r.selectable)
        .map(|r| r.domain)
        .collect();
    c.selected_domains.retain(|d| !shown.contains(d));
    if value {
        c.selected_domains.extend(shown);
    }
    sort_selected(c);
}

fn sort_selected(c: &mut ReviewChoice) {
    let order: Vec<&str> = c.domains.iter().map(|d| d.domain.as_str()).collect();
    c.selected_domains
        .sort_by_key(|d| order.iter().position(|x| x == d).unwrap_or(usize::MAX));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(domain: &str, cookies: u32, ls: u32, signin: bool, idp: bool) -> KvDomainCount {
        KvDomainCount {
            domain: domain.into(),
            cookies,
            session_cookies: 0,
            local_storage: ls,
            passwords: 0,
            signin,
            identity_provider: idp,
            unavailable: 0,
            unavailable_reason: String::new(),
        }
    }

    fn inventory() -> KvInventory {
        KvInventory {
            provider_id: "chrome".into(),
            app_display: "Google Chrome".into(),
            domains: vec![
                d("notion.so", 4, 2, true, false),
                d("github.com", 12, 0, true, false),
                d("google.com", 30, 0, true, true),
                d("doubleclick.net", 9, 0, false, false),
            ],
            notes: vec![],
        }
    }

    #[test]
    fn the_first_review_starts_minimal_and_never_picks_an_identity_provider() {
        let c = with_inventory(ReviewChoice::default(), &inventory(), None);
        assert_eq!(
            c.domains
                .iter()
                .map(|x| x.domain.as_str())
                .collect::<Vec<_>>(),
            ["doubleclick.net", "github.com", "google.com", "notion.so"],
            "listed by name"
        );
        assert_eq!(c.selected_domains, ["github.com", "notion.so"]);
        let rows = domain_rows(&c);
        assert_eq!(rows[1].counts, "12 cookies");
        assert_eq!(rows[3].counts, "4 cookies, 2 storage values");
        assert!(rows[2].identity_provider && !rows[2].selected);
        assert_eq!(domain_summary(&c), "2 of 4 sites");
    }

    #[test]
    fn what_was_picked_last_time_wins_over_the_default() {
        let remembered = vec![
            "google.com".to_string(),
            "gone.test".to_string(),
            "api.github.com".to_string(),
        ];
        let c = with_inventory(ReviewChoice::default(), &inventory(), Some(&remembered));
        assert_eq!(
            c.selected_domains,
            ["github.com", "google.com"],
            "a remembered host maps to its site; a site that no longer exists is dropped"
        );
        // Remembering replaces the earlier choice for the same app and Space,
        // and keeps others.
        let key = remember_key("chrome", "aurora");
        let a = remember(&[], &key, &["github.com".into()]);
        let b = remember(&a, "chrome|dev-1", &["notion.so".into()]);
        let c2 = remember(&b, &key, &["notion.so".into(), "github.com".into()]);
        assert_eq!(remembered_of(&c2, &key), vec!["notion.so", "github.com"]);
        assert_eq!(remembered_of(&c2, "chrome|dev-1"), vec!["notion.so"]);
        assert!(super::remembered(&c2, "chrome|other").is_none());
        // Bounded.
        let mut many = vec![];
        for i in 0..100 {
            many = remember(&many, &format!("a|{i}"), &[]);
        }
        assert_eq!(many.len(), 64);
        assert_eq!(many.last().unwrap().key, "a|99");
    }

    fn remembered_of(c: &[RememberedChoice], key: &str) -> Vec<String> {
        super::remembered(c, key).unwrap()
    }

    #[test]
    fn toggling_and_searching_sites() {
        let mut c = with_inventory(ReviewChoice::default(), &inventory(), None);
        toggle_domain(&mut c, "google.com");
        assert_eq!(
            c.selected_domains,
            ["github.com", "google.com", "notion.so"]
        );
        toggle_domain(&mut c, "github.com");
        toggle_domain(&mut c, "nope.test");
        assert_eq!(c.selected_domains, ["google.com", "notion.so"]);
        // Select all shown follows the search.
        c.query = "git".into();
        assert_eq!(domain_rows(&c).len(), 1);
        set_shown_domains(&mut c, true);
        assert_eq!(
            c.selected_domains,
            ["github.com", "google.com", "notion.so"]
        );
        set_shown_domains(&mut c, false);
        assert_eq!(
            c.selected_domains,
            ["google.com", "notion.so"],
            "the rest are kept"
        );
        c.query.clear();
        set_shown_domains(&mut c, false);
        assert!(c.selected_domains.is_empty());
        set_shown_domains(&mut c, true);
        assert_eq!(c.selected_domains.len(), 4);
    }

    #[test]
    fn unreadable_cookies_are_greyed_with_why_and_never_selected() {
        let mut bank = d("bank.test", 0, 0, true, false);
        bank.unavailable = 3;
        bank.unavailable_reason = "Chrome protects it with app-bound encryption".into();
        let mut mixed = d("mixed.test", 2, 0, true, false);
        mixed.unavailable = 1;
        mixed.unavailable_reason = "Chrome protects it with app-bound encryption".into();
        let inv = KvInventory {
            provider_id: "chrome".into(),
            domains: vec![bank, mixed, d("plain.test", 1, 0, true, false)],
            ..Default::default()
        };
        let mut c = with_inventory(ReviewChoice::default(), &inv, None);
        // Only what can be sent is picked by default.
        assert_eq!(c.selected_domains, ["mixed.test", "plain.test"]);
        let rows = domain_rows(&c);
        let by = |n: &str| rows.iter().find(|r| r.domain == n).unwrap();
        assert!(!by("bank.test").selectable);
        assert_eq!(
            by("bank.test").unavailable_note,
            "3 cookies cannot be sent: Chrome protects it with app-bound encryption"
        );
        assert!(by("mixed.test").selectable);
        assert_eq!(
            by("mixed.test").count,
            2,
            "the unreadable one is not counted"
        );
        assert!(
            by("mixed.test")
                .unavailable_note
                .starts_with("1 cookie cannot")
        );
        assert!(by("plain.test").unavailable_note.is_empty());
        // It cannot be toggled on, singly or with select all.
        toggle_domain(&mut c, "bank.test");
        set_shown_domains(&mut c, true);
        assert_eq!(c.selected_domains, ["mixed.test", "plain.test"]);
    }
}
