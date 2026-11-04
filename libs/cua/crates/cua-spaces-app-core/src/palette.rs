// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The command palette (⌘K) and the app's keyboard shortcuts.
//!
//! [`keybindings`] are the shared UI's own file
//! (`apps/cua-spaces-web/src/lib/keybindings.json`), read here so the
//! SwiftUI app binds the same keys as the React UI. [`rank`] orders the
//! palette's commands for a query, the same way in every shell.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

const KEYBINDINGS_JSON: &str =
    include_str!("../../../../../apps/cua-spaces-web/src/lib/keybindings.json");

/// One command's default key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keybinding {
    /// `commandPalette.toggle`, `nav.spaces`, ...
    pub command: String,
    /// `mod+k`, `mod+shift+l`: `mod` is ⌘ on macOS and Ctrl elsewhere.
    pub key: String,
}

/// The default keyboard shortcuts, in the shared file's order.
pub fn keybindings() -> Vec<Keybinding> {
    serde_json::from_str(KEYBINDINGS_JSON).expect("keybindings.json is a list of {command, key}")
}

/// What the palette matches a query against, for one command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteEntry {
    /// "Actions", "Go to", "Spaces".
    pub group: String,
    /// "New Space".
    pub label: String,
    /// Secondary text ("Linux").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Extra words it answers to ("create add").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
}

/// How well `query` fuzzy-matches `text`: its characters must appear in
/// order. Matches at the start of a word and runs of consecutive characters
/// score higher, gaps lower. 0: no match.
pub fn fuzzy_score(text: &str, query: &str) -> f64 {
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return 1.0;
    }
    let mut score = 0.0;
    let mut from = 0usize;
    let mut prev: isize = -2;
    for ch in &q {
        let Some(at) = t[from.min(t.len())..]
            .iter()
            .position(|c| c == ch)
            .map(|i| i + from)
        else {
            return 0.0;
        };
        let word_start = at == 0
            || matches!(t[at - 1], '-' | '_' | '/' | '.' | ':')
            || t[at - 1].is_whitespace();
        score += 1.0;
        if at as isize == prev + 1 {
            score += 4.0;
        }
        if word_start {
            score += 6.0;
        }
        score -= f64::min(3.0, (at - from) as f64 * 0.1);
        prev = at as isize;
        from = at + 1;
    }
    if t.starts_with(&q) {
        score += 10.0;
    }
    f64::max(score, 0.01)
}

/// The best score for one query word across label, hint and keywords (the
/// label counts most).
fn word_score(e: &PaletteEntry, word: &str) -> f64 {
    let label = fuzzy_score(&e.label, word) * 1.5;
    let hint = fuzzy_score(e.hint.as_deref().unwrap_or(""), word);
    let keywords = fuzzy_score(e.keywords.as_deref().unwrap_or(""), word);
    label.max(hint).max(keywords)
}

/// The entries that match every word of `query`, best first, as indices
/// into `entries`. Groups stay together, ordered by their best match; an
/// empty query keeps every entry in its order.
pub fn rank(entries: &[PaletteEntry], query: &str) -> Vec<u32> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return (0..entries.len() as u32).collect();
    }
    let scored: Vec<(usize, f64)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let mut score = 0.0;
            for w in &words {
                let s = word_score(e, w);
                if s == 0.0 {
                    return None;
                }
                score += s;
            }
            Some((i, score))
        })
        .collect();
    let group_best = |g: &str| {
        scored
            .iter()
            .filter(|(i, _)| entries[*i].group == g)
            .map(|(_, s)| *s)
            .fold(0.0, f64::max)
    };
    let first_index = |g: &str| {
        scored
            .iter()
            .find(|(i, _)| entries[*i].group == g)
            .map_or(usize::MAX, |(i, _)| *i)
    };
    let mut sorted = scored.clone();
    sorted.sort_by(|(ai, a), (bi, b)| {
        let (ga, gb) = (&entries[*ai].group, &entries[*bi].group);
        group_best(gb)
            .partial_cmp(&group_best(ga))
            .unwrap_or(Ordering::Equal)
            // Ties between groups keep the first group's place.
            .then_with(|| {
                if ga == gb {
                    Ordering::Equal
                } else {
                    first_index(ga).cmp(&first_index(gb))
                }
            })
            .then_with(|| b.partial_cmp(a).unwrap_or(Ordering::Equal))
            .then_with(|| ai.cmp(bi))
    });
    sorted.into_iter().map(|(i, _)| i as u32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(group: &str, label: &str, keywords: &str) -> PaletteEntry {
        PaletteEntry {
            group: group.into(),
            label: label.into(),
            hint: None,
            keywords: (!keywords.is_empty()).then(|| keywords.into()),
        }
    }

    #[test]
    fn the_shared_keybindings_have_the_navigation_keys() {
        let keys: Vec<(String, String)> = keybindings()
            .into_iter()
            .map(|k| (k.command, k.key))
            .collect();
        for (command, key) in [
            ("commandPalette.toggle", "mod+k"),
            ("nav.spaces", "mod+1"),
            ("nav.machines", "mod+2"),
            ("nav.agents", "mod+3"),
            ("nav.keyvault", "mod+4"),
        ] {
            assert!(
                keys.contains(&(command.into(), key.into())),
                "{command} {key} in {keys:?}"
            );
        }
    }

    #[test]
    fn fuzzy_scores_prefer_word_starts_and_runs() {
        assert_eq!(fuzzy_score("Machines", ""), 1.0);
        assert_eq!(fuzzy_score("Machines", "xz"), 0.0);
        assert!(fuzzy_score("Machines", "mac") > fuzzy_score("Open Linux machine", "mac"));
        assert!(fuzzy_score("New Space", "ns") > 0.0);
        assert!(fuzzy_score("New Space", "sp") > fuzzy_score("Unspecified", "sp"));
        assert!(fuzzy_score("Lock Keyvault", "lkv") > 0.0);
        assert_eq!(fuzzy_score("Lock Keyvault", "vkl"), 0.0);
        assert!(fuzzy_score("Spaces", "spa") > fuzzy_score("Settings and spare", "spa"));
        assert!(fuzzy_score("Keyvault", "key") > fuzzy_score("Keyvault", "kvt"));
    }

    #[test]
    fn ranks_matches_best_first_with_groups_together() {
        let entries = vec![
            entry("Actions", "New Space", "create add"),
            entry("Go to", "Spaces", "page"),
            entry("Go to", "Machines", "page"),
            entry("Spaces", "Open dev box", "linux running"),
            entry("Spaces", "Stop dev box", "linux running"),
        ];
        assert_eq!(rank(&entries, ""), [0, 1, 2, 3, 4]);
        assert_eq!(rank(&entries, "  "), [0, 1, 2, 3, 4]);
        assert_eq!(rank(&entries, "machines"), [2]);
        assert_eq!(rank(&entries, "dev stop"), [4]);
        // "spa": Spaces (Go to) beats New Space; Go to's group comes first.
        assert_eq!(rank(&entries, "spa"), [1, 0]);
        assert_eq!(rank(&entries, "zzz"), Vec::<u32>::new());
    }
}
