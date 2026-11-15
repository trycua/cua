//! Trees of Spaces: a top-level Space and the Spaces created from it
//! (children, their children, ...).
//!
//! **Record.** Each child keeps, in its [`SpaceMeta`]:
//!
//! - `parent`: its parent's canonical id;
//! - `root`: the top-level Space of its tree (a top-level Space has none:
//!   it is its own root);
//! - `depth`: 1 for a child of a top-level Space, 2 for a grandchild, ...;
//! - `origin`: how it was made: `user` (top-level), `child`, or
//!   `fork-batch:<id>`.
//!
//! The record fields are what counts. A label `cua.parent=<id>` mirrors the
//! parent so older clients, listings and cloud sweeps can see it. Links are
//! by id, never by name: a parent re-created under the same name is not
//! the old one.
//!
//! **Depth.** A child may be at most [`DEFAULT_DEPTH_LIMIT`] levels below a
//! top-level Space unless its root's `depth_limit` says more, and never
//! more than [`MAX_DEPTH`].
//!
//! **Cascade.** Deleting, stopping or suspending a Space does the same to
//! its subtree, children first ([`Spaces::delete_tree`]); starting one
//! starts its subtree after it. `keep_children` detaches the children
//! instead: each becomes a top-level Space with its own subtree.
//!
//! **Network.** The tree is the network boundary ([`crate::network`]):
//! a child's settings are no more open than its parent's, its members reach
//! each other, and other trees are blocked.

use std::collections::{BTreeMap, BTreeSet};

use crate::registry::SpaceMeta;
use crate::{Error, Result, Spaces};

/// How deep a tree goes below its top-level Space unless its root says
/// more (user → child → grandchild).
pub const DEFAULT_DEPTH_LIMIT: u32 = 2;
/// How deep a tree may ever go.
pub const MAX_DEPTH: u32 = 4;
/// The label that mirrors a child's parent.
pub const PARENT_LABEL: &str = "cua.parent";
/// `origin` of a top-level Space.
pub const ORIGIN_USER: &str = "user";
/// `origin` of a child Space.
pub const ORIGIN_CHILD: &str = "child";

/// A Space's place in its tree, as listings show it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct TreeLink {
    /// Its parent's id; `None` for a top-level Space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Its tree's top-level Space; `None` for a top-level Space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// 0 for a top-level Space, 1 for its children, ...
    pub depth: u32,
    /// `user`, `child` or `fork-batch:<id>`.
    pub origin: String,
}

impl TreeLink {
    /// The link a record keeps.
    pub fn of(meta: &SpaceMeta) -> TreeLink {
        TreeLink {
            parent: meta.parent.clone(),
            root: meta.root.clone(),
            depth: meta.depth,
            origin: meta.origin.clone().unwrap_or_else(|| {
                if meta.parent.is_some() {
                    ORIGIN_CHILD
                } else {
                    ORIGIN_USER
                }
                .into()
            }),
        }
    }
}

/// The depth limit of a tree whose root keeps `root_meta`.
pub fn depth_limit(root_meta: Option<&SpaceMeta>) -> u32 {
    root_meta
        .and_then(|m| m.depth_limit)
        .unwrap_or(DEFAULT_DEPTH_LIMIT)
        .min(MAX_DEPTH)
}

/// The depth a new child of a parent at `parent_depth` gets, or why it
/// can't be made.
pub fn child_depth(parent: &str, parent_depth: u32, limit: u32) -> Result<u32> {
    let depth = parent_depth + 1;
    if depth > limit {
        return Err(Error::invalid(format!(
            "{parent} can't have children: its tree is {limit} level{} deep already (the most \
             is {MAX_DEPTH}; the tree's top-level Space sets it)",
            if limit == 1 { "" } else { "s" }
        )));
    }
    Ok(depth)
}

/// The direct children of `id` in `all`.
pub fn children_of(all: &BTreeMap<String, SpaceMeta>, id: &str) -> Vec<String> {
    all.iter()
        .filter(|(_, m)| m.parent.as_deref() == Some(id))
        .map(|(k, _)| k.clone())
        .collect()
}

/// Every Space below `id`, deepest first (children before their parents),
/// so a cascade never leaves a child without its parent. Safe against a
/// cycle in a hand-edited record.
pub fn descendants(all: &BTreeMap<String, SpaceMeta>, id: &str) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::from([id.to_string()]);
    let mut levels: Vec<Vec<String>> = Vec::new();
    let mut frontier = vec![id.to_string()];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for p in &frontier {
            for c in children_of(all, p) {
                if seen.insert(c.clone()) {
                    next.push(c);
                }
            }
        }
        if !next.is_empty() {
            levels.push(next.clone());
        }
        frontier = next;
    }
    levels.into_iter().rev().flatten().collect()
}

/// [`descendants`] grouped by level, deepest level first: each level can
/// go at once.
pub fn levels(all: &BTreeMap<String, SpaceMeta>, id: &str) -> Vec<Vec<String>> {
    let mut by_depth: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let base = all.get(id).map_or(0, |m| m.depth);
    for d in descendants(all, id) {
        let depth = all.get(&d).map_or(base + 1, |m| m.depth.max(base + 1));
        by_depth.entry(depth).or_default().push(d);
    }
    by_depth.into_values().rev().collect()
}

/// The members of the tree `id` belongs to: its root and everything below
/// it.
pub fn members(all: &BTreeMap<String, SpaceMeta>, id: &str) -> Vec<String> {
    let root = all
        .get(id)
        .and_then(|m| m.root.clone())
        .unwrap_or_else(|| id.to_string());
    let mut out = vec![root.clone()];
    out.extend(descendants(all, &root));
    out
}

/// `all` with the children of `id` detached: each becomes a top-level
/// Space, and the root and depth of everything below it follow.
pub fn detach(all: &mut BTreeMap<String, SpaceMeta>, id: &str) -> Vec<String> {
    let kids = children_of(all, id);
    for kid in &kids {
        let shift = all.get(kid).map_or(1, |m| m.depth.max(1));
        let below = descendants(all, kid);
        if let Some(m) = all.get_mut(kid) {
            m.parent = None;
            m.root = None;
            m.depth = 0;
            m.labels.remove(PARENT_LABEL);
        }
        for d in below {
            if let Some(m) = all.get_mut(&d) {
                m.root = Some(kid.clone());
                m.depth = m.depth.saturating_sub(shift).max(1);
            }
        }
    }
    kids
}

impl Spaces {
    /// The root of the tree a new child of `parent` joins (`None` without
    /// a parent: it is its own).
    pub(crate) fn tree_root_of(&self, parent: Option<&str>) -> Option<String> {
        let parent = parent.map(str::trim).filter(|p| !p.is_empty())?;
        let id = self.resolve(parent).ok()?.to_string();
        Some(
            self.inner
                .registry
                .meta(&id)
                .and_then(|m| m.root)
                .unwrap_or(id),
        )
    }

    /// The direct children of a Space (canonical ids).
    pub fn children(&self, space: &str) -> Result<Vec<String>> {
        let id = self.resolve(space)?.to_string();
        Ok(children_of(&self.inner.registry.all_meta(), &id))
    }

    /// Every Space below a Space, children before their parents.
    pub fn descendants(&self, space: &str) -> Result<Vec<String>> {
        let id = self.resolve(space)?.to_string();
        Ok(descendants(&self.inner.registry.all_meta(), &id))
    }

    /// The members of a Space's tree: its root and everything below it.
    pub fn tree_members(&self, space: &str) -> Result<Vec<String>> {
        let id = self.resolve(space)?.to_string();
        Ok(members(&self.inner.registry.all_meta(), &id))
    }

    /// A Space's place in its tree.
    pub fn tree_link(&self, space: &str) -> Result<TreeLink> {
        let id = self.resolve(space)?.to_string();
        Ok(self
            .inner
            .registry
            .meta(&id)
            .map(|m| TreeLink::of(&m))
            .unwrap_or_else(|| TreeLink {
                origin: ORIGIN_USER.into(),
                ..Default::default()
            }))
    }

    /// Deletes a Space and its subtree, children first (each level at
    /// once). `keep_children` detaches its children instead: they become
    /// top-level Spaces and keep running. A child that can't be deleted
    /// stops the cascade before its parent goes, so nothing is left without
    /// a parent; deleting again continues it.
    pub async fn delete_tree(&self, space: &str, keep_children: bool) -> Result<String> {
        let id = match self.resolve(space) {
            Ok(id) => id.to_string(),
            // No record names it (a sandbox made by `cua sb fork` or
            // `cua sb create`): it has no tree, and deleting it by name
            // is [`Spaces::delete_copy`]'s.
            Err(Error::NotFound(_)) => return Box::pin(self.delete_this(space)).await,
            Err(e) => return Err(e),
        };
        if keep_children {
            let kids = self
                .inner
                .registry
                .update_all_meta(|all| detach(all, &id))?;
            let message = Box::pin(self.delete_this(space)).await?;
            return Ok(if kids.is_empty() {
                message
            } else {
                format!(
                    "{message} Kept {} child Space{} as top-level: {}.",
                    kids.len(),
                    if kids.len() == 1 { "" } else { "s" },
                    kids.join(", ")
                )
            });
        }
        let all = self.inner.registry.all_meta();
        let mut failed: Vec<String> = Vec::new();
        let mut deleted = 0usize;
        for level in levels(&all, &id) {
            let mut results = Vec::with_capacity(level.len());
            for c in &level {
                results.push((c.clone(), Box::pin(self.delete_this(c)).await));
            }
            for (c, r) in results {
                match r {
                    Ok(_) | Err(Error::NotFound(_)) => {
                        deleted += 1;
                        // A record with no Space behind it goes too.
                        let _ = self
                            .inner
                            .registry
                            .update_meta(&c, |m| *m = SpaceMeta::default());
                    }
                    Err(e) => failed.push(format!("{c} ({e})")),
                }
            }
            if !failed.is_empty() {
                return Err(Error::invalid(format!(
                    "{space} was not deleted: its child Space{} could not be deleted: {}. Delete \
                     again to continue.",
                    if failed.len() == 1 { "" } else { "s" },
                    failed.join("; ")
                )));
            }
        }
        let message = Box::pin(self.delete_this(space)).await?;
        Ok(if deleted == 0 {
            message
        } else {
            format!(
                "{message} Deleted its {deleted} child Space{} too.",
                if deleted == 1 { "" } else { "s" }
            )
        })
    }

    /// Turns a Space's subtree off before the Space itself (`suspend`:
    /// keeping memory). A child that can't be turned off is logged and
    /// left: it is not a reason to keep its parent on.
    pub(crate) async fn power_off_children(&self, space: &str, suspend: bool) {
        let Ok(id) = self.resolve(space).map(|i| i.to_string()) else {
            return;
        };
        let all = self.inner.registry.all_meta();
        for level in levels(&all, &id) {
            let results = futures_util::future::join_all(level.iter().map(|c| async move {
                let r = if suspend {
                    Box::pin(self.suspend_one(c)).await.map(|_| ())
                } else {
                    Box::pin(self.set_power(c, false)).await.map(|_| ())
                };
                (c.clone(), r)
            }))
            .await;
            for (c, r) in results {
                if let Err(e) = r {
                    tracing::warn!(space = %c, parent = %id, error = %e, "a child Space was not turned off with its parent");
                }
            }
        }
    }

    /// Turns a Space's subtree on after the Space itself, parents before
    /// their children.
    pub(crate) async fn power_on_children(&self, space: &str) {
        let Ok(id) = self.resolve(space).map(|i| i.to_string()) else {
            return;
        };
        let all = self.inner.registry.all_meta();
        for level in levels(&all, &id).into_iter().rev() {
            let results = futures_util::future::join_all(level.iter().map(|c| async move {
                let _ = self.migrate_network(c);
                (c.clone(), Box::pin(self.set_power(c, true)).await)
            }))
            .await;
            for (c, r) in results {
                if let Err(e) = r {
                    tracing::warn!(space = %c, parent = %id, error = %e, "a child Space was not turned on with its parent");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> BTreeMap<String, SpaceMeta> {
        let node = |parent: Option<&str>, root: Option<&str>, depth: u32| SpaceMeta {
            parent: parent.map(String::from),
            root: root.map(String::from),
            depth,
            labels: parent
                .map(|p| BTreeMap::from([(PARENT_LABEL.to_string(), p.to_string())]))
                .unwrap_or_default(),
            ..Default::default()
        };
        BTreeMap::from([
            ("local:r".into(), node(None, None, 0)),
            ("local:a".into(), node(Some("local:r"), Some("local:r"), 1)),
            ("local:b".into(), node(Some("local:r"), Some("local:r"), 1)),
            ("local:a1".into(), node(Some("local:a"), Some("local:r"), 2)),
            (
                "local:a11".into(),
                node(Some("local:a1"), Some("local:r"), 3),
            ),
            ("local:other".into(), node(None, None, 0)),
        ])
    }

    #[test]
    fn descendants_come_children_first() {
        let all = tree();
        let d = descendants(&all, "local:r");
        assert_eq!(d.len(), 4);
        let pos = |s: &str| d.iter().position(|x| x == s).unwrap();
        assert!(pos("local:a11") < pos("local:a1"));
        assert!(pos("local:a1") < pos("local:a"));
        assert_eq!(
            levels(&all, "local:r"),
            vec![
                vec!["local:a11".to_string()],
                vec!["local:a1".to_string()],
                vec!["local:a".to_string(), "local:b".to_string()],
            ]
        );
        assert!(descendants(&all, "local:other").is_empty());
        let mut m = members(&all, "local:a1");
        m.sort();
        assert_eq!(
            m,
            ["local:a", "local:a1", "local:a11", "local:b", "local:r"]
        );
        assert_eq!(members(&all, "local:other"), ["local:other"]);
    }

    #[test]
    fn a_cycle_does_not_loop() {
        let mut all = tree();
        all.get_mut("local:r").unwrap().parent = Some("local:a11".into());
        let d = descendants(&all, "local:r");
        assert_eq!(d.len(), 4, "{d:?}");
    }

    #[test]
    fn detaching_makes_each_child_a_top_level_tree() {
        let mut all = tree();
        let kids = detach(&mut all, "local:r");
        assert_eq!(kids, ["local:a", "local:b"]);
        let a = &all["local:a"];
        assert_eq!(
            (a.parent.as_ref(), a.root.as_ref(), a.depth),
            (None, None, 0)
        );
        assert!(!a.labels.contains_key(PARENT_LABEL));
        let a1 = &all["local:a1"];
        assert_eq!(
            a1.parent.as_deref(),
            Some("local:a"),
            "its own parent stays"
        );
        assert_eq!((a1.root.as_deref(), a1.depth), (Some("local:a"), 1));
        assert_eq!(
            (all["local:a11"].root.as_deref(), all["local:a11"].depth),
            (Some("local:a"), 2)
        );
        assert!(children_of(&all, "local:r").is_empty());
    }

    #[test]
    fn depth_is_limited() {
        assert_eq!(child_depth("local:r", 0, DEFAULT_DEPTH_LIMIT).unwrap(), 1);
        assert_eq!(child_depth("local:a", 1, DEFAULT_DEPTH_LIMIT).unwrap(), 2);
        let e = child_depth("local:a1", 2, DEFAULT_DEPTH_LIMIT).unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        assert!(e.to_string().contains("2 levels deep"), "{e}");
        let root = SpaceMeta {
            depth_limit: Some(9),
            ..Default::default()
        };
        assert_eq!(depth_limit(Some(&root)), MAX_DEPTH);
        assert_eq!(depth_limit(None), DEFAULT_DEPTH_LIMIT);
        assert!(child_depth("x", MAX_DEPTH, depth_limit(Some(&root))).is_err());
    }

    #[test]
    fn links_say_where_a_space_is() {
        let all = tree();
        assert_eq!(
            TreeLink::of(&all["local:a1"]),
            TreeLink {
                parent: Some("local:a".into()),
                root: Some("local:r".into()),
                depth: 2,
                origin: ORIGIN_CHILD.into(),
            }
        );
        assert_eq!(TreeLink::of(&all["local:r"]).origin, ORIGIN_USER);
    }
}
