//! Who is whose child, as the spawn API needs it: every question a Space
//! asks on its channel is answered inside its own subtree, so a sibling,
//! an ancestor or another tree reads the same as a Space that does not
//! exist.

use std::collections::{BTreeMap, BTreeSet};

/// The parent links of the Spaces this host knows.
pub trait Tree: Send + Sync {
    /// The parent of `id`, when it is a child.
    fn parent(&self, id: &str) -> Option<String>;
    /// Every Space id this host knows.
    fn ids(&self) -> Vec<String>;
}

/// A snapshot of parent links (tests, and one listing's worth of records).
#[derive(Debug, Default, Clone)]
pub struct Links(pub BTreeMap<String, Option<String>>);

impl Tree for Links {
    fn parent(&self, id: &str) -> Option<String> {
        self.0.get(id).cloned().flatten()
    }
    fn ids(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

/// Steps followed up the parent links before giving up (a cycle, or a
/// tree deeper than any capability allows).
const MAX_WALK: usize = 16;

/// True when `id` is below `ancestor` (not `ancestor` itself).
pub fn is_descendant(tree: &dyn Tree, ancestor: &str, id: &str) -> bool {
    let mut cur = id.to_owned();
    for _ in 0..MAX_WALK {
        match tree.parent(&cur) {
            Some(p) if p == ancestor => return true,
            Some(p) => cur = p,
            None => return false,
        }
    }
    false
}

/// Every Space below `ancestor`, deepest first (the order to delete in).
pub fn subtree(tree: &dyn Tree, ancestor: &str) -> Vec<String> {
    let mut below: Vec<(usize, String)> = tree
        .ids()
        .into_iter()
        .filter_map(|id| depth_below(tree, ancestor, &id).map(|d| (d, id)))
        .collect();
    below.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    below.into_iter().map(|(_, id)| id).collect()
}

/// How many steps `id` is below `ancestor`, when it is.
fn depth_below(tree: &dyn Tree, ancestor: &str, id: &str) -> Option<usize> {
    let mut cur = id.to_owned();
    let mut seen = BTreeSet::new();
    for step in 1..=MAX_WALK {
        let p = tree.parent(&cur)?;
        if !seen.insert(p.clone()) {
            return None;
        }
        if p == ancestor {
            return Some(step);
        }
        cur = p;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(pairs: &[(&str, Option<&str>)]) -> Links {
        Links(
            pairs
                .iter()
                .map(|(c, p)| (c.to_string(), p.map(str::to_string)))
                .collect(),
        )
    }

    #[test]
    fn only_descendants_are_in_a_subtree() {
        let t = links(&[
            ("root", None),
            ("a", Some("root")),
            ("a1", Some("a")),
            ("b", Some("root")),
            ("other", None),
            ("o1", Some("other")),
        ]);
        assert!(is_descendant(&t, "root", "a1"));
        assert!(is_descendant(&t, "a", "a1"));
        // Not itself, not a sibling, not an ancestor, not another tree.
        assert!(!is_descendant(&t, "a", "a"));
        assert!(!is_descendant(&t, "a", "b"));
        assert!(!is_descendant(&t, "a", "root"));
        assert!(!is_descendant(&t, "a", "o1"));
        assert_eq!(subtree(&t, "root"), vec!["a1", "a", "b"]);
        assert_eq!(subtree(&t, "a"), vec!["a1"]);
        assert!(subtree(&t, "a1").is_empty());
    }

    #[test]
    fn a_cycle_is_no_ones_subtree() {
        let t = links(&[("x", Some("y")), ("y", Some("x"))]);
        assert!(subtree(&t, "z").is_empty());
        assert!(!is_descendant(&t, "z", "x"));
    }
}
