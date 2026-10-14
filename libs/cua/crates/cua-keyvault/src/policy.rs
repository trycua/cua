// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pure policy decisions: who may do what, with no I/O.

use crate::caller::{CallerIdentity, Signing};
use crate::model::{Grant, MAX_GRANT_SECS, MAX_RULE_SECS, Meta, UnattendedRule};
use crate::{Error, Result};

/// How a teleport was authorized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authority {
    /// A capability token minted from this grant.
    Grant(String),
    /// A user-authored unattended rule.
    Rule(String),
    /// A first-party caller, confirmed by user presence just now.
    Interactive,
}

impl Authority {
    /// Audit detail.
    pub fn describe(&self) -> String {
        match self {
            Authority::Grant(g) => format!("grant={g}"),
            Authority::Rule(r) => format!("rule={r}"),
            Authority::Interactive => "interactive (user presence)".into(),
        }
    }
}

/// Only first-party callers.
pub fn require_first_party(caller: &CallerIdentity, what: &str) -> Result<()> {
    if caller.first_party {
        Ok(())
    } else {
        Err(Error::Forbidden(format!(
            "{what} is only available to the Cua app and CLI; {} must request access instead",
            caller.display()
        )))
    }
}

/// Whether a caller's identity is strong enough for an unattended rule:
/// team-signed code verified by the OS.
pub fn rule_eligible(caller: &CallerIdentity) -> bool {
    caller.os_verified && matches!(caller.signing, Signing::Signed { .. })
}

/// The first rule that lets `caller_fp` teleport `item` to `target`.
pub fn matching_rule<'a>(
    meta: &'a Meta,
    caller_fp: &str,
    item: &str,
    target: &str,
    now_ms: u64,
) -> Option<&'a UnattendedRule> {
    meta.rules
        .iter()
        .find(|r| r.matches(caller_fp, item, target, now_ms))
}

/// Validates a new rule's shape and clamps its lifetime.
pub fn validate_rule(rule: &mut UnattendedRule, meta: &Meta, now_ms: u64) -> Result<()> {
    if rule.items.is_empty() || rule.targets.is_empty() || rule.callers.is_empty() {
        return Err(Error::Invalid(
            "a rule names at least one item, one target and one caller".into(),
        ));
    }
    for i in &rule.items {
        let item = meta
            .items
            .get(i)
            .ok_or_else(|| Error::NotFound(format!("item {i}")))?;
        if !item.policy.unattended {
            return Err(Error::Forbidden(format!(
                "item {i} does not allow unattended teleport (turn it on in its policy first)"
            )));
        }
        for t in &rule.targets {
            if t != "*" && !item.policy.allows_target(t) {
                return Err(Error::Forbidden(format!(
                    "item {i} may not go to target {t}"
                )));
            }
        }
    }
    if rule.not_after_ms <= now_ms {
        return Err(Error::Invalid("the rule is already expired".into()));
    }
    rule.not_after_ms = rule.not_after_ms.min(now_ms + MAX_RULE_SECS * 1000);
    Ok(())
}

/// Whether a request can be answered without asking the user: it names
/// items, every one of them is unlocked (the user allowed unattended access
/// when they unlocked it) and is not an identity provider, every target is
/// one each item may go to, and the actions are only the ones unlocking
/// covers (writing the item into a Space, or signing in with it).
pub fn unattended_eligible(
    items: &[crate::model::ItemMeta],
    targets: &[String],
    actions: &[crate::capability::Action],
) -> bool {
    use crate::capability::Action;
    !items.is_empty()
        && !targets.is_empty()
        && actions
            .iter()
            .all(|a| matches!(a, Action::Teleport | Action::Login))
        && items.iter().all(|i| {
            i.policy.unattended
                && !i.identity_provider
                && targets.iter().all(|t| i.policy.allows_target(t))
        })
}

/// Clamps a grant lifetime.
pub fn clamp_grant_secs(secs: u64) -> u64 {
    secs.clamp(30, MAX_GRANT_SECS)
}

/// Drops expired grants and rules and wiped deliveries older than a day.
pub fn prune(meta: &mut Meta, now_ms: u64) -> usize {
    let before = meta.grants.len() + meta.rules.len() + meta.deliveries.len();
    let day = 24 * 3600 * 1000;
    meta.grants
        .retain(|g: &Grant| g.is_live(now_ms) || now_ms.saturating_sub(g.not_after_ms) < day);
    meta.rules
        .retain(|r| now_ms < r.not_after_ms || now_ms.saturating_sub(r.not_after_ms) < day);
    meta.deliveries
        .retain(|d| !d.wiped || now_ms.saturating_sub(d.delivered_ms) < day);
    before - (meta.grants.len() + meta.rules.len() + meta.deliveries.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ItemKind, ItemMeta, ItemPolicy, RuleCaller};

    fn meta_with_item(unattended: bool, targets: Vec<String>) -> Meta {
        let mut m = Meta::default();
        m.items.insert(
            "i1".into(),
            ItemMeta {
                id: "i1".into(),
                kind: ItemKind::Cookie,
                provider_id: "chrome".into(),
                app_display: "Chrome".into(),
                domain: Some("example.test".into()),
                key: "sid".into(),
                path: None,
                source: String::new(),
                session: false,
                expires_ms: None,
                bytes: 0,
                blob: None,
                identity_provider: false,
                policy: ItemPolicy {
                    allowed_targets: targets,
                    ttl_secs: 60,
                    unattended,
                },
                created_ms: 0,
                updated_ms: 0,
                rev: 1,
                record_digest: String::new(),
            },
        );
        m
    }

    fn rule(targets: &[&str], until: u64) -> UnattendedRule {
        UnattendedRule {
            id: "r".into(),
            items: vec!["i1".into()],
            targets: targets.iter().map(|s| s.to_string()).collect(),
            target_ids: Default::default(),
            callers: vec![RuleCaller {
                fp: "fp".into(),
                display: "x".into(),
            }],
            created_ms: 0,
            not_after_ms: until,
            enabled: true,
            note: String::new(),
        }
    }

    #[test]
    fn rules_need_unattended_items_and_allowed_targets_and_are_clamped() {
        let m = meta_with_item(false, vec![]);
        assert!(validate_rule(&mut rule(&["s"], 10_000), &m, 0).is_err());
        let m = meta_with_item(true, vec!["s".into()]);
        assert!(validate_rule(&mut rule(&["t"], 10_000), &m, 0).is_err());
        assert!(validate_rule(&mut rule(&["s"], 0), &m, 5).is_err());
        let mut r = rule(&["s"], u64::MAX);
        validate_rule(&mut r, &m, 0).unwrap();
        assert_eq!(r.not_after_ms, MAX_RULE_SECS * 1000);
    }

    #[test]
    fn unsigned_callers_are_not_rule_eligible() {
        let mut c = CallerIdentity::for_tests("com.example.app", false);
        assert!(rule_eligible(&c));
        c.signing = Signing::Unsigned;
        assert!(!rule_eligible(&c));
        c.signing = Signing::AdHoc {
            identifier: "x".into(),
            cdhash: "y".into(),
        };
        assert!(!rule_eligible(&c));
        // Red-team F14: `Signing::Unknown` is what every Linux (and other
        // non-code-signing OS) caller reports. It is neither `Unsigned` nor
        // `AdHoc`, so confirm it too is barred from unattended rules and
        // long-lived grants, even if a user-writable install path made it look
        // first-party.
        let mut unknown = CallerIdentity::for_tests("x", true);
        unknown.signing = Signing::Unknown;
        unknown.os_verified = true; // even if some OS claimed verification
        assert!(
            !rule_eligible(&unknown),
            "Unknown callers are never rule-eligible"
        );
        let mut linux = CallerIdentity::for_tests("x", true);
        linux.os_verified = false;
        assert!(!rule_eligible(&linux));
    }

    #[test]
    fn third_parties_are_refused_first_party_ops() {
        let c = CallerIdentity::for_tests("com.example.app", false);
        let e = require_first_party(&c, "listing items")
            .unwrap_err()
            .to_string();
        assert!(e.contains("request access"), "{e}");
        assert!(
            require_first_party(&CallerIdentity::for_tests("com.trycua.cua", true), "x").is_ok()
        );
    }
}
