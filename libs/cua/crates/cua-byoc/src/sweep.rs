// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cloud_sweep`: what Cua left in a cloud. The rules, in one place:
//!
//! - A resource is a candidate only when the cloud shows it tagged
//!   `cua-managed=true` and `cua-owner=<this home's owner>` (the provider
//!   lists by those tags and they are checked again here) AND it is either
//!   recorded in this home's state or carries this owner id. Untagged
//!   resources, other owners' resources and anything Cua did not create
//!   are never candidates, whatever their names.
//! - A candidate is deleted when it has expired (its `cua-expires` tag or
//!   record), when its sandbox is gone (an instance or sandbox no record
//!   names, older than a grace period so a create under way is left
//!   alone), or with `all`. Shared resources (a security group, a
//!   firewall rule) go only with `all` and only once no Cua instance is
//!   left.
//! - A record whose resource the cloud no longer has is forgotten, but only
//!   when the cloud was asked in the record's own region ([`in_scope`]):
//!   a listing covers one connection's region, so a record from another
//!   region (an elastic host placed elsewhere) is "not seen", never "gone".
//!   The automatic sweep never forgets a record at all.
//! - Dry run by default: nothing is deleted.

use std::collections::BTreeMap;

use cua_sandbox_core::byoc::{CloudSweepItem, CloudSweepReport};

use crate::api::{CloudApi, Result};
use crate::audit;
use crate::model::{self, Connection, Resource, now};
use crate::state::Store;

/// A create younger than this is never "orphaned" (it is still booting).
pub const GRACE_SECS: u64 = 30 * 60;

/// What to do with one resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// Delete it (else keep).
    pub delete: bool,
    /// Why.
    pub reason: String,
}

/// The sweeper's decision for `r`, as the cloud shows it with `tags`,
/// given this home's `records`. Pure, so the rules are tested directly.
pub fn verdict(
    r: &Resource,
    tags: &BTreeMap<String, String>,
    owner: &str,
    records: &[Resource],
    all: bool,
    instances_left: bool,
    now: u64,
) -> Verdict {
    let keep = |reason: &str| Verdict {
        delete: false,
        reason: reason.into(),
    };
    let del = |reason: &str| Verdict {
        delete: true,
        reason: reason.into(),
    };
    if !model::is_ours(tags, owner) {
        return keep("not Cua's (tags): never touched");
    }
    let recorded = records.iter().find(|x| {
        x.provider == r.provider
            && x.resource_type == r.resource_type
            && ((!x.id.is_empty() && x.id == r.id) || (!x.name.is_empty() && x.name == r.name))
    });
    let expires = recorded
        .map(|x| x.expires)
        .filter(|e| *e > 0)
        .unwrap_or_else(|| model::tag_expires(tags));
    let shared = !matches!(r.resource_type.as_str(), "instance" | "sandbox");
    if shared {
        return if all && !instances_left {
            del("shared resource, no Cua instance left")
        } else {
            keep("shared by Cua sandboxes here")
        };
    }
    if all {
        return del("--all");
    }
    if expires > 0 && now >= expires {
        return del("expired");
    }
    match recorded {
        Some(x) if x.state == "failed" => del("a failed create left it"),
        // A pool host (`host:<name>`) has no sandbox of its own: its record
        // stays as the cloud returned it (Hetzner: pending) however long it
        // serves. Its TTL ends it (above), and the pool removes it when idle
        // or when it never joined.
        Some(x) if x.sandbox.starts_with("host:") && !x.id.is_empty() => {
            keep("a pool host (its TTL and the pool end it)")
        }
        Some(x) if x.state != "pending" && !x.sandbox.is_empty() && !x.id.is_empty() => {
            keep("its sandbox exists")
        }
        Some(x) if now.saturating_sub(x.created) < GRACE_SECS => keep("being created"),
        Some(_) => del("its sandbox was never created"),
        None => {
            let created = r.created;
            if created > 0 && now.saturating_sub(created) < GRACE_SECS {
                keep("being created elsewhere")
            } else {
                del("no sandbox refers to it")
            }
        }
    }
}

/// The audit actor, and the pass, of the daemon's automatic sweep.
pub const AUTO_ACTOR: &str = "auto-sweep";

/// How one sweep runs.
#[derive(Clone, Copy, Debug)]
pub struct SweepOpts<'a> {
    /// Delete nothing.
    pub dry_run: bool,
    /// Every Cua resource, not only expired and orphaned ones.
    pub all: bool,
    /// What the audit log names as the one who deleted (`sweep`,
    /// `auto-sweep`, `disconnect`).
    pub actor: &'a str,
    /// May forget the record of a resource the cloud confirms is gone.
    /// The automatic sweep never does.
    pub forget_missing: bool,
}

/// Whether the cloud, asked through `conn`, can answer for `rec`: the
/// record is from the connection's region (and zone). A record from
/// anywhere else is only unverified when it is not listed.
pub fn in_scope(rec: &Resource, conn: &Connection) -> bool {
    !rec.region.is_empty()
        && rec.region == conn.region
        && (rec.zone.is_empty() || conn.zone.is_empty() || rec.zone == conn.zone)
}

/// Sweeps one connected provider.
pub async fn sweep_provider(
    provider: &dyn CloudApi,
    conn: &Connection,
    store: &Store,
    opts: SweepOpts<'_>,
) -> Result<Vec<CloudSweepItem>> {
    let SweepOpts {
        dry_run,
        all,
        actor,
        forget_missing,
    } = opts;
    let owner = store.owner_id().map_err(cua_sandbox_core::Error::Io)?;
    let records: Vec<Resource> = store
        .resources()
        .map_err(cua_sandbox_core::Error::Io)?
        .into_iter()
        .filter(|r| r.provider == provider.name())
        .collect();
    // Every region this home recorded something in, and the connection's:
    // a host or group made in another region is listed (and deleted) there,
    // never taken for gone because the connection's region does not have it.
    let mut regions: Vec<String> = vec![conn.region.clone()];
    for r in &records {
        if !r.region.is_empty() && !regions.contains(&r.region) {
            regions.push(r.region.clone());
        }
    }
    let conns: Vec<Connection> = regions
        .iter()
        .map(|region| {
            let mut c = conn.clone();
            if *region != conn.region {
                c.region = region.clone();
                c.zone = String::new();
            }
            c
        })
        .collect();
    let mut listed: Vec<Resource> = Vec::new();
    let mut swept: Vec<&Connection> = Vec::new();
    for c in &conns {
        match provider.list_owned(c, &owner).await {
            Ok(rows) => {
                swept.push(c);
                for mut r in rows {
                    if r.region.is_empty() {
                        r.region = c.region.clone();
                    }
                    if !listed.iter().any(|x| same(x, &r)) {
                        listed.push(r);
                    }
                }
            }
            // The connection's own region must answer; another recorded
            // region that does not is only left unchecked.
            Err(e) if c.region == conn.region => return Err(e),
            Err(e) => tracing::warn!(region = c.region, "not swept: {e}"),
        }
    }
    let conn_for = |region: &str| -> &Connection {
        swept
            .iter()
            .copied()
            .find(|c| c.region == region)
            .unwrap_or(conn)
    };
    let t = now();
    let instances_left = listed.iter().any(|r| {
        matches!(r.resource_type.as_str(), "instance" | "sandbox")
            && !verdict(r, &r.tags, &owner, &records, all, true, t).delete
            && r.state != "terminated"
    });
    let mut out = Vec::new();
    // Instances and sandboxes first, shared resources after them.
    let mut ordered: Vec<&Resource> = listed.iter().collect();
    ordered.sort_by_key(|r| !matches!(r.resource_type.as_str(), "instance" | "sandbox"));
    for r in ordered {
        if r.state == "terminated" {
            continue;
        }
        let v = verdict(r, &r.tags, &owner, &records, all, instances_left, t);
        let mut item = CloudSweepItem {
            resource: r.wire(t),
            action: if v.delete {
                "delete".into()
            } else {
                "keep".into()
            },
            reason: v.reason.clone(),
        };
        if v.delete && !dry_run {
            match provider.delete(conn_for(&r.region), r, &owner).await {
                Ok(()) => {
                    item.action = "deleted".into();
                    store.audit(audit::of_resource(
                        "resource_deleted",
                        r,
                        actor,
                        "ok",
                        &v.reason,
                    ));
                    // Its relay machine goes too (removed right after this,
                    // with the other relay machines left behind).
                    let machine = records
                        .iter()
                        .find(|x| same(x, r))
                        .map(|x| x.machine.clone())
                        .filter(|m| !m.is_empty())
                        .or_else(|| r.tags.get(model::tag::MACHINE).cloned())
                        .filter(|m| !m.is_empty());
                    for rec in records.iter().filter(|x| same(x, r)) {
                        let _ = store.forget(rec);
                    }
                    if let Some(m) = machine {
                        let _ = store.record(Resource {
                            provider: r.provider.clone(),
                            id: m.clone(),
                            resource_type: model::RELAY_MACHINE.into(),
                            name: m.clone(),
                            machine: m,
                            created: t,
                            state: "orphaned".into(),
                            ..Default::default()
                        });
                    }
                }
                Err(e) => {
                    item.action = "failed".into();
                    item.reason = format!("{}: {e}", v.reason);
                    store.audit(audit::of_resource(
                        "resource_deleted",
                        r,
                        actor,
                        "failed",
                        &item.reason,
                    ));
                }
            }
        }
        out.push(item);
    }
    // Records whose resource is gone from the cloud.
    for rec in records
        .iter()
        .filter(|r| r.resource_type != model::RELAY_MACHINE)
    {
        if rec.id.is_empty() && t.saturating_sub(rec.created) < GRACE_SECS {
            continue;
        }
        if listed.iter().any(|r| same(rec, r)) {
            continue;
        }
        // Not in the listing. Gone only if the cloud, asked where the
        // record says it lives, says so; otherwise it is simply not seen
        // and stays recorded.
        let scoped = swept.iter().any(|c| in_scope(rec, c));
        let gone = rec.id.is_empty()
            || (scoped
                && match provider.describe(conn_for(&rec.region), rec).await {
                    Ok(None) => true,
                    Ok(Some((state, _))) => state == "terminated",
                    Err(_) => false,
                });
        let mut w = rec.wire(t);
        if gone && forget_missing {
            if !dry_run {
                let _ = store.forget(rec);
            }
            w.state = "gone".into();
            out.push(CloudSweepItem {
                resource: w,
                action: if dry_run {
                    "forget".into()
                } else {
                    "forgotten".into()
                },
                reason: "no longer in the cloud".into(),
            });
        } else if !rec.id.is_empty() {
            let reason = if !scoped {
                format!(
                    "recorded in {}, which did not answer (this connection is in {}): not checked, left alone",
                    if rec.region.is_empty() {
                        "an unknown region"
                    } else {
                        &rec.region
                    },
                    conn.region
                )
            } else {
                "not seen in the cloud: its record is left alone".to_string()
            };
            tracing::debug!(resource = %rec.id, "{reason}");
            out.push(CloudSweepItem {
                resource: w,
                action: "keep".into(),
                reason,
            });
        }
    }
    Ok(out)
}

pub(crate) fn same(a: &Resource, b: &Resource) -> bool {
    a.provider == b.provider
        && a.resource_type == b.resource_type
        && ((!a.id.is_empty() && a.id == b.id) || (!a.name.is_empty() && a.name == b.name))
}

/// A report from the rows of every provider.
pub fn report(dry_run: bool, resources: Vec<CloudSweepItem>) -> CloudSweepReport {
    CloudSweepReport { dry_run, resources }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tags;

    fn inst(id: &str, tags: BTreeMap<String, String>, created: u64) -> Resource {
        Resource {
            provider: "aws".into(),
            id: id.into(),
            resource_type: "instance".into(),
            created,
            tags,
            ..Default::default()
        }
    }

    const NOW: u64 = 1_000_000;

    #[test]
    fn untagged_and_foreign_resources_are_never_touched() {
        let mine = tags("me", "s", "h", 0);
        let theirs = tags("someone-else", "s", "h", 0);
        let mut unmanaged = mine.clone();
        unmanaged.remove(model::tag::MANAGED);
        for (t, why) in [
            (BTreeMap::new(), "untagged"),
            (theirs, "other owner"),
            (unmanaged, "no managed tag"),
        ] {
            let r = inst("i-x", t.clone(), 1);
            // Even with all=true, even recorded, even expired.
            let recorded = vec![Resource {
                expires: 1,
                ..r.clone()
            }];
            for all in [false, true] {
                let v = verdict(&r, &t, "me", &recorded, all, false, NOW);
                assert!(!v.delete, "{why} all={all}: {v:?}");
            }
        }
    }

    #[test]
    fn ours_go_when_expired_orphaned_or_all() {
        let live = tags("me", "s", "h", NOW + 100);
        let expired = tags("me", "s", "h", NOW - 1);
        let rec_live = Resource {
            sandbox: "aws:space-1".into(),
            state: "running".into(),
            ..inst("i-1", live.clone(), NOW - 10)
        };
        // Recorded with its sandbox, not expired: kept.
        assert!(
            !verdict(
                &rec_live,
                &live,
                "me",
                std::slice::from_ref(&rec_live),
                false,
                true,
                NOW
            )
            .delete
        );
        // Expired: deleted.
        assert!(verdict(&rec_live, &expired, "me", &[], false, true, NOW).delete);
        // Unrecorded and old: an orphan of this owner.
        let old = inst("i-2", live.clone(), NOW - GRACE_SECS - 1);
        assert_eq!(
            verdict(&old, &live, "me", &[], false, true, NOW).reason,
            "no sandbox refers to it"
        );
        // Unrecorded but young: being created.
        let young = inst("i-3", live.clone(), NOW - 5);
        assert!(!verdict(&young, &live, "me", &[], false, true, NOW).delete);
        // all: everything ours.
        assert!(
            verdict(
                &rec_live,
                &live,
                "me",
                std::slice::from_ref(&rec_live),
                true,
                true,
                NOW
            )
            .delete
        );
    }

    /// A joined pool host is never an orphan to the automatic sweep, however
    /// long it serves (its record keeps the state the cloud returned at
    /// create, Hetzner's "pending"); its TTL still ends it.
    #[test]
    fn a_pool_host_is_kept_until_its_ttl() {
        let live = tags("me", "host:hetzner-1", "h", NOW + 3600);
        let host = Resource {
            provider: "hetzner".into(),
            sandbox: "host:hetzner-1".into(),
            state: "pending".into(),
            ..inst("171928710", live.clone(), NOW - GRACE_SECS - 60)
        };
        let recs = std::slice::from_ref(&host);
        let v = verdict(&host, &live, "me", recs, false, true, NOW);
        assert!(!v.delete, "{v:?}");
        let expired = tags("me", "host:hetzner-1", "h", NOW - 1);
        let gone = Resource {
            expires: NOW - 1,
            ..host.clone()
        };
        assert!(
            verdict(
                &gone,
                &expired,
                "me",
                std::slice::from_ref(&gone),
                false,
                true,
                NOW
            )
            .delete
        );
    }

    #[test]
    fn what_a_failed_create_left_goes_without_the_grace_period() {
        let t = tags("me", "s", "c", NOW + 3600);
        let r = Resource {
            state: "failed".into(),
            sandbox: "aws:s".into(),
            created: NOW - 5,
            ..inst("i-9", t.clone(), NOW - 5)
        };
        let v = verdict(&r, &t, "me", std::slice::from_ref(&r), false, true, NOW);
        assert!(v.delete, "{v:?}");
        assert_eq!(v.reason, "a failed create left it");
    }

    #[test]
    fn shared_resources_go_only_with_all_and_no_instance_left() {
        let t = tags("me", "", "", 0);
        let sg = Resource {
            provider: "aws".into(),
            id: "sg-1".into(),
            resource_type: "security_group".into(),
            ..Default::default()
        };
        assert!(!verdict(&sg, &t, "me", &[], false, false, NOW).delete);
        assert!(!verdict(&sg, &t, "me", &[], true, true, NOW).delete);
        assert!(verdict(&sg, &t, "me", &[], true, false, NOW).delete);
    }
}
