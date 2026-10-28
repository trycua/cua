//! Leak-check inventories. An [`Inventory`] lists everything a Spaces
//! deployment can leave behind (Spaces, VMs, containers, processes, state
//! records, temp disks, relay registrations, cloud instances and volumes);
//! [`Inventory::leaks`] diffs a baseline against the end state and reports
//! every item that does not belong to a live, wanted Space.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// One thing that exists.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Item {
    /// What it is: `space`, `process:qemu`, `container`, `record:gateway`,
    /// `file:overlays`, `port`, `aws:instance`, `hetzner:server`, ...
    pub kind: String,
    /// A stable identity (never a pid for a thing a restart re-creates).
    pub id: String,
    /// The Space (name or id) it belongs to, when it belongs to one.
    #[serde(default)]
    pub owner: Option<String>,
    /// Human detail for the report (pid, size, age); not compared.
    #[serde(default)]
    pub detail: String,
}

impl Item {
    pub fn new(kind: &str, id: &str) -> Self {
        Item {
            kind: kind.into(),
            id: id.into(),
            owner: None,
            detail: String::new(),
        }
    }

    pub fn owned_by(mut self, owner: &str) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }

    fn key(&self) -> (&str, &str) {
        (&self.kind, &self.id)
    }
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.kind, self.id)?;
        if let Some(o) = &self.owner {
            write!(f, " (of {o})")?;
        }
        if !self.detail.is_empty() {
            write!(f, " [{}]", self.detail)?;
        }
        Ok(())
    }
}

/// Everything that exists on one host (or in one cloud scope) at one time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    /// Where it was taken: a host name, a `CUA_HOME`, `aws:us-west-2`.
    pub scope: String,
    pub label: String,
    pub items: BTreeSet<Item>,
    /// Collectors that could not run (so a clean diff is not trusted blindly).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// What is allowed to exist after a run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Wanted {
    /// Spaces the scenario deliberately keeps (name or id).
    #[serde(default)]
    pub spaces: BTreeSet<String>,
    /// Kinds tolerated while `spaces` is non-empty (their ports, processes).
    #[serde(default)]
    pub tolerate_kinds: BTreeSet<String>,
    /// `kind` -> id prefixes the deployment intends to keep (a warm pool's
    /// hosts, a cache dir).
    #[serde(default)]
    pub keep: BTreeMap<String, Vec<String>>,
}

/// Why an item counts as a leak.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Leak {
    pub item: Item,
    pub reason: String,
}

/// The outcome of a diff.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub scope: String,
    pub leaks: Vec<Leak>,
    /// Items of the baseline that disappeared (informational).
    pub vanished: Vec<Item>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.leaks.is_empty()
    }

    /// `Err` with the full report text when anything leaked.
    pub fn assert_clean(&self, context: &str) -> Result<(), String> {
        if self.is_clean() {
            Ok(())
        } else {
            Err(format!("{context}\n{self}"))
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}: {} leak(s)", self.scope, self.leaks.len())?;
        for l in &self.leaks {
            writeln!(f, "  LEAK {}: {}", l.item, l.reason)?;
        }
        for w in &self.warnings {
            writeln!(f, "  warning: {w}")?;
        }
        Ok(())
    }
}

impl Inventory {
    pub fn new(scope: &str, label: &str) -> Self {
        Inventory {
            scope: scope.into(),
            label: label.into(),
            ..Default::default()
        }
    }

    pub fn add(&mut self, item: Item) {
        self.items.insert(item);
    }

    pub fn extend(&mut self, other: Inventory) {
        self.items.extend(other.items);
        self.warnings.extend(other.warnings);
    }

    pub fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Item> {
        self.items.iter().filter(move |i| i.kind == kind)
    }

    pub fn ids(&self, kind: &str) -> BTreeSet<String> {
        self.of_kind(kind).map(|i| i.id.clone()).collect()
    }

    /// Diffs this end state against `baseline`.
    ///
    /// An item leaks when it is not in the baseline, is not a wanted Space,
    /// does not belong to a wanted Space, is not kept by `wanted.keep`, and
    /// (while Spaces are wanted) is not of a tolerated kind. Independently,
    /// an item that names an owner Space which no longer exists in this
    /// inventory leaks even if the baseline had it, because it outlived its
    /// Space.
    pub fn leaks(&self, baseline: &Inventory, wanted: &Wanted) -> Report {
        let before: BTreeSet<(&str, &str)> = baseline.items.iter().map(Item::key).collect();
        let live_spaces: BTreeSet<&str> = self
            .items
            .iter()
            .filter(|i| i.kind == "space")
            .flat_map(|i| [i.id.as_str()])
            .collect();
        let mut report = Report {
            scope: self.scope.clone(),
            warnings: self.warnings.clone(),
            ..Default::default()
        };
        for item in &self.items {
            let kept = wanted
                .keep
                .get(&item.kind)
                .is_some_and(|ps| ps.iter().any(|p| item.id.starts_with(p.as_str())));
            if kept {
                continue;
            }
            let is_wanted_space = item.kind == "space" && wanted.spaces.contains(&item.id);
            let of_wanted = item
                .owner
                .as_deref()
                .is_some_and(|o| wanted.spaces.contains(o));
            if let Some(o) = item.owner.as_deref()
                && !live_spaces.contains(o)
                && !of_wanted
                && item.kind != "space"
            {
                report.leaks.push(Leak {
                    item: item.clone(),
                    reason: format!("outlived its Space {o}"),
                });
                continue;
            }
            if before.contains(&item.key()) || is_wanted_space || of_wanted {
                continue;
            }
            if !wanted.spaces.is_empty() && wanted.tolerate_kinds.contains(&item.kind) {
                continue;
            }
            report.leaks.push(Leak {
                item: item.clone(),
                reason: if item.owner.is_some() {
                    "belongs to a Space that is not wanted".into()
                } else {
                    "not in the baseline and not wanted".into()
                },
            });
        }
        let after: BTreeSet<(&str, &str)> = self.items.iter().map(Item::key).collect();
        report.vanished = baseline
            .items
            .iter()
            .filter(|i| !after.contains(&i.key()))
            .cloned()
            .collect();
        report
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Inventory, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// The two-way cross-check of a Store against a provider API: ids only in
/// `store_kind` (the Store says it exists, the cloud does not) and ids only
/// in `cloud_kind` (the cloud has it, the Store does not). Both lists empty
/// means the two agree. `ignore_cloud` are cloud ids that legitimately have
/// no Store record (pre-existing resources of other owners).
pub fn cross_check(
    inv: &Inventory,
    store_kind: &str,
    cloud_kind: &str,
    ignore_cloud: &BTreeSet<String>,
) -> Report {
    let store = inv.ids(store_kind);
    let cloud = inv.ids(cloud_kind);
    let mut r = Report {
        scope: format!("{} vs {store_kind}/{cloud_kind}", inv.scope),
        ..Default::default()
    };
    for id in store.difference(&cloud) {
        r.leaks.push(Leak {
            item: Item::new(store_kind, id),
            reason: format!("the Store has it, the cloud ({cloud_kind}) does not"),
        });
    }
    for id in cloud.difference(&store) {
        if ignore_cloud.contains(id) {
            continue;
        }
        r.leaks.push(Leak {
            item: Item::new(cloud_kind, id),
            reason: format!("the cloud has it, the Store ({store_kind}) does not"),
        });
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(items: &[Item]) -> Inventory {
        let mut i = Inventory::new("t", "x");
        for it in items {
            i.add(it.clone());
        }
        i
    }

    #[test]
    fn new_unwanted_items_leak_and_baseline_items_do_not() {
        let base = inv(&[Item::new("process:cua-daemon", "cua daemon")]);
        let end = inv(&[
            Item::new("process:cua-daemon", "cua daemon"),
            Item::new("process:qemu", "qemu -name a"),
            Item::new("file:overlays", "overlays/a.qcow2"),
        ]);
        let r = end.leaks(&base, &Wanted::default());
        assert_eq!(r.leaks.len(), 2);
        assert!(r.assert_clean("seed 7").unwrap_err().contains("seed 7"));
    }

    #[test]
    fn wanted_spaces_and_their_children_are_fine() {
        let base = inv(&[]);
        let end = inv(&[
            Item::new("space", "keep"),
            Item::new("process:qemu", "qemu a").owned_by("keep"),
            Item::new("port", "127.0.0.1:9").detail("fwd"),
        ]);
        let mut w = Wanted::default();
        w.spaces.insert("keep".into());
        assert_eq!(end.leaks(&base, &w).leaks.len(), 1);
        w.tolerate_kinds.insert("port".into());
        assert!(end.leaks(&base, &w).is_clean());
    }

    #[test]
    fn a_child_outliving_its_space_leaks_even_from_the_baseline() {
        let child = Item::new("process:qemu", "qemu gone").owned_by("gone");
        let r = inv(std::slice::from_ref(&child))
            .leaks(&inv(std::slice::from_ref(&child)), &Wanted::default());
        assert_eq!(r.leaks.len(), 1);
        assert!(r.leaks[0].reason.contains("outlived"));
    }

    #[test]
    fn keep_prefixes_allow_a_pool() {
        let end = inv(&[Item::new("aws:instance", "i-pool1")]);
        let mut w = Wanted::default();
        assert_eq!(end.leaks(&inv(&[]), &w).leaks.len(), 1);
        w.keep.insert("aws:instance".into(), vec!["i-pool".into()]);
        assert!(end.leaks(&inv(&[]), &w).is_clean());
    }

    #[test]
    fn cross_check_is_two_way() {
        let i = inv(&[
            Item::new("store:host", "h1"),
            Item::new("store:host", "h2"),
            Item::new("aws:instance", "h2"),
            Item::new("aws:instance", "h3"),
            Item::new("aws:instance", "other"),
        ]);
        let ig: BTreeSet<String> = ["other".to_string()].into();
        let r = cross_check(&i, "store:host", "aws:instance", &ig);
        let ids: Vec<_> = r.leaks.iter().map(|l| l.item.id.as_str()).collect();
        assert_eq!(ids, ["h1", "h3"]);
    }

    #[test]
    fn json_roundtrip() {
        let i = inv(&[Item::new("space", "a").detail("x")]);
        assert_eq!(Inventory::from_json(&i.to_json()).unwrap(), i);
    }
}
