// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Capability tokens: macaroon-style HMAC chains (Birgisson et al.,
//! "Macaroons: Cookies with Contextual Caveats", NDSS 2014).
//!
//! A token is an id, a list of first-party caveats and a chained tag:
//! `sig0 = HMAC(root, id)`, `sig_i = HMAC(sig_{i-1}, caveat_i)`. Anyone
//! holding a token can append a caveat (attenuate) but cannot remove one,
//! because that would need the previous tag. Only the vault holds `root`,
//! which is random per daemon run, so every token dies when the daemon
//! restarts.
//!
//! Every caveat must hold (conjunction). Set-valued caveats (`item`,
//! `target`, `action`) therefore intersect, so attenuation can only narrow.
//! An unknown caveat fails closed.
//!
//! Wire form: `cuakv1.<base64url(JSON {id, caveats})>.<base64url(tag)>`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::crypto::{SecretKey, b64url_decode, b64url_encode, ct_eq};
use crate::{Error, Result};

/// Token prefix (versioned).
pub const TOKEN_PREFIX: &str = "cuakv1";

/// What a token lets its holder do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Deliver an item into a target Space.
    Teleport,
    /// Read an item's metadata (labels, counts, cookie names; never values).
    Describe,
    /// Sign in to a site in a target Space with a saved password (the
    /// Keyvault fills the form; the password is never handed out).
    Login,
}

impl Action {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Teleport => "teleport",
            Action::Describe => "describe",
            Action::Login => "login",
        }
    }

    /// Parses a wire name.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "teleport" => Some(Action::Teleport),
            "describe" => Some(Action::Describe),
            "login" => Some(Action::Login),
            _ => None,
        }
    }
}

/// One first-party caveat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caveat {
    /// The grant this token was minted from (checked against revocation).
    Grant(String),
    /// The verified caller fingerprint the token is bound to.
    Caller(String),
    /// Allowed actions.
    Actions(BTreeSet<Action>),
    /// Allowed vault item ids.
    Items(BTreeSet<String>),
    /// Allowed targets (Space names). Display and narrowing only; the
    /// authoritative binding is [`Caveat::TargetIds`], because a name is
    /// mutable (red-team F1).
    Targets(BTreeSet<String>),
    /// Allowed target identities: the immutable Space/sandbox ids the grant
    /// was pinned to at approval time. Delivery resolves the requested name
    /// to its current id and it must be in this set, so a name that now maps
    /// to a different Space fails closed (red-team F1).
    TargetIds(BTreeSet<String>),
    /// Unix seconds after which the token is dead.
    NotAfter(u64),
    /// The vault's revocation epoch at mint time (the kill switch bumps it).
    Epoch(u64),
    /// Single use: this nonce is accepted once.
    Once(String),
}

impl Caveat {
    /// The canonical string form (what the tag chains over).
    pub fn encode(&self) -> String {
        fn set<'a>(it: impl Iterator<Item = &'a str>) -> String {
            it.collect::<Vec<_>>().join(",")
        }
        match self {
            Caveat::Grant(g) => format!("grant={g}"),
            Caveat::Caller(c) => format!("caller={c}"),
            Caveat::Actions(a) => format!("action={}", set(a.iter().map(|a| a.as_str()))),
            Caveat::Items(i) => format!("item={}", set(i.iter().map(String::as_str))),
            Caveat::Targets(t) => format!("target={}", set(t.iter().map(String::as_str))),
            Caveat::TargetIds(t) => format!("targetid={}", set(t.iter().map(String::as_str))),
            Caveat::NotAfter(t) => format!("exp={t}"),
            Caveat::Epoch(e) => format!("epoch={e}"),
            Caveat::Once(n) => format!("once={n}"),
        }
    }

    /// Parses the canonical form. Unknown keys and malformed values are
    /// errors (fail closed).
    pub fn decode(s: &str) -> Result<Self> {
        let (k, v) = s
            .split_once('=')
            .ok_or_else(|| Error::Capability(format!("malformed caveat {s:?}")))?;
        let set = |v: &str| -> BTreeSet<String> {
            v.split(',')
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect()
        };
        let num = |v: &str| -> Result<u64> {
            v.parse()
                .map_err(|_| Error::Capability(format!("malformed caveat {s:?}")))
        };
        let reject_separators = |v: &str| -> Result<()> {
            if v.is_empty() || v.contains(',') {
                Err(Error::Capability(format!("malformed caveat {s:?}")))
            } else {
                Ok(())
            }
        };
        Ok(match k {
            "grant" => {
                reject_separators(v)?;
                Caveat::Grant(v.into())
            }
            "caller" => {
                reject_separators(v)?;
                Caveat::Caller(v.into())
            }
            "action" => {
                let mut out = BTreeSet::new();
                for a in v.split(',').filter(|x| !x.is_empty()) {
                    out.insert(
                        Action::parse(a)
                            .ok_or_else(|| Error::Capability(format!("unknown action {a:?}")))?,
                    );
                }
                Caveat::Actions(out)
            }
            "item" => Caveat::Items(set(v)),
            "target" => Caveat::Targets(set(v)),
            "targetid" => Caveat::TargetIds(set(v)),
            "exp" => Caveat::NotAfter(num(v)?),
            "epoch" => Caveat::Epoch(num(v)?),
            "once" => {
                reject_separators(v)?;
                Caveat::Once(v.into())
            }
            other => {
                return Err(Error::Capability(format!(
                    "unknown caveat {other:?} (fail closed)"
                )));
            }
        })
    }
}

/// A capability token.
#[derive(Clone, PartialEq, Eq)]
pub struct Capability {
    id: String,
    caveats: Vec<String>,
    tag: [u8; 32],
}

impl std::fmt::Debug for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The tag is a bearer secret: never printed.
        f.debug_struct("Capability")
            .field("id", &self.id)
            .field("caveats", &self.caveats)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct Body {
    id: String,
    caveats: Vec<String>,
}

impl Capability {
    /// Mints a token under `root`.
    pub fn mint(root: &SecretKey, id: &str, caveats: &[Caveat]) -> Self {
        let mut cap = Self {
            id: id.to_string(),
            caveats: Vec::new(),
            tag: root.mac(id.as_bytes()),
        };
        for c in caveats {
            cap = cap.attenuate(c);
        }
        cap
    }

    /// Appends a caveat. Needs no key: the new tag chains from the old one.
    pub fn attenuate(mut self, caveat: &Caveat) -> Self {
        let enc = caveat.encode();
        let k = SecretKey::from_bytes(&self.tag).expect("tags are 32 bytes");
        self.tag = k.mac(enc.as_bytes());
        self.caveats.push(enc);
        self
    }

    /// Token id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Parsed caveats.
    pub fn caveats(&self) -> Result<Vec<Caveat>> {
        self.caveats.iter().map(|c| Caveat::decode(c)).collect()
    }

    /// The wire form.
    pub fn encode(&self) -> String {
        let body = serde_json::to_vec(&Body {
            id: self.id.clone(),
            caveats: self.caveats.clone(),
        })
        .expect("serializable");
        format!(
            "{TOKEN_PREFIX}.{}.{}",
            b64url_encode(&body),
            b64url_encode(&self.tag)
        )
    }

    /// Parses the wire form (does not verify).
    pub fn decode(s: &str) -> Result<Self> {
        let mut parts = s.split('.');
        let (Some(p), Some(body), Some(tag), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Error::Capability("malformed token".into()));
        };
        if p != TOKEN_PREFIX {
            return Err(Error::Capability("unknown token version".into()));
        }
        let body: Body = serde_json::from_slice(&b64url_decode(body)?)
            .map_err(|_| Error::Capability("malformed token body".into()))?;
        let tag: [u8; 32] = b64url_decode(tag)?
            .try_into()
            .map_err(|_| Error::Capability("malformed token tag".into()))?;
        if body.id.is_empty() || body.caveats.len() > 64 {
            return Err(Error::Capability("malformed token body".into()));
        }
        Ok(Self {
            id: body.id,
            caveats: body.caveats,
            tag,
        })
    }

    /// Verifies the chain under `root`, then every caveat against `ctx`.
    /// Returns the grant id the token was minted from.
    pub fn verify(&self, root: &SecretKey, ctx: &mut VerifyContext<'_>) -> Result<String> {
        let mut tag = root.mac(self.id.as_bytes());
        for c in &self.caveats {
            let k = SecretKey::from_bytes(&tag).expect("tags are 32 bytes");
            tag = k.mac(c.as_bytes());
        }
        if !ct_eq(&tag, &self.tag) {
            return Err(Error::Capability("token signature is invalid".into()));
        }
        let mut grant = None;
        let mut bound_caller = false;
        let mut bounded_time = false;
        let mut has_epoch = false;
        let mut bound_target_id = false;
        let mut once = Vec::new();
        for c in self.caveats()? {
            match c {
                Caveat::Grant(g) => {
                    if grant.as_ref().is_some_and(|x| x != &g) {
                        return Err(Error::Capability("conflicting grant caveats".into()));
                    }
                    if !(ctx.grant_is_live)(&g) {
                        return Err(Error::Capability(format!(
                            "grant {g} is revoked or expired"
                        )));
                    }
                    grant = Some(g);
                }
                Caveat::Caller(c) => {
                    if !ct_eq(c.as_bytes(), ctx.caller.as_bytes()) {
                        return Err(Error::Capability(
                            "token is bound to a different caller".into(),
                        ));
                    }
                    bound_caller = true;
                }
                Caveat::Actions(a) => {
                    if !a.contains(&ctx.action) {
                        return Err(Error::Capability(format!(
                            "token does not allow {}",
                            ctx.action.as_str()
                        )));
                    }
                }
                Caveat::Items(i) => {
                    if !i.contains(ctx.item) {
                        return Err(Error::Capability("token does not cover this item".into()));
                    }
                }
                Caveat::Targets(t) => match ctx.target {
                    Some(target) if t.contains(target) => {}
                    _ => {
                        return Err(Error::Capability("token does not cover this target".into()));
                    }
                },
                Caveat::TargetIds(t) => {
                    match ctx.target_id {
                        Some(id) if t.contains(id) => {}
                        _ => {
                            return Err(Error::Capability(
                                "the target's identity is not the one this grant was pinned to (rebinding)".into(),
                            ));
                        }
                    }
                    bound_target_id = true;
                }
                Caveat::NotAfter(t) => {
                    if ctx.now >= t {
                        return Err(Error::Capability("token expired".into()));
                    }
                    bounded_time = true;
                }
                Caveat::Epoch(e) => {
                    has_epoch = true;
                    if e != ctx.epoch {
                        return Err(Error::Capability(
                            "token predates a revocation (kill switch)".into(),
                        ));
                    }
                }
                Caveat::Once(n) => once.push(n),
            }
        }
        // Structural minimums: every token the vault accepts is bound to a
        // grant, a caller and an expiry. A hand-built token without them is
        // refused even if its chain is valid.
        let grant = grant.ok_or_else(|| Error::Capability("token names no grant".into()))?;
        if !bound_caller || !bounded_time || !has_epoch || !bound_target_id {
            return Err(Error::Capability(
                "token is not bound to a caller, an expiry, a revocation epoch and a target identity".into(),
            ));
        }
        // Consume single-use nonces last, only once everything else passed.
        for n in once {
            if !(ctx.consume_nonce)(&n) {
                return Err(Error::Capability("token was already used (replay)".into()));
            }
        }
        Ok(grant)
    }
}

/// What a request is checked against.
pub struct VerifyContext<'a> {
    /// Now, Unix seconds.
    pub now: u64,
    /// The verified caller's fingerprint on this connection.
    pub caller: &'a str,
    /// Requested action.
    pub action: Action,
    /// Requested item id.
    pub item: &'a str,
    /// Requested target name, if any.
    pub target: Option<&'a str>,
    /// The requested target's freshly-resolved immutable id (red-team F1).
    /// Checked against the [`Caveat::TargetIds`] the grant was pinned to.
    pub target_id: Option<&'a str>,
    /// The vault's current revocation epoch.
    pub epoch: u64,
    /// Whether a grant id exists, is not revoked and is not expired.
    pub grant_is_live: &'a dyn Fn(&str) -> bool,
    /// Records a single-use nonce; false if it was seen before.
    pub consume_nonce: &'a mut dyn FnMut(&str) -> bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;

    fn set<T: Ord + Clone>(v: &[T]) -> BTreeSet<T> {
        v.iter().cloned().collect()
    }

    fn base(root: &SecretKey) -> Capability {
        Capability::mint(
            root,
            "tok1",
            &[
                Caveat::Grant("g1".into()),
                Caveat::Caller("fp-a".into()),
                Caveat::Actions(set(&[Action::Teleport])),
                Caveat::Items(set(&["i1".to_string(), "i2".to_string()])),
                Caveat::Targets(set(&["space-1".to_string()])),
                Caveat::TargetIds(set(&["sandbox-1".to_string()])),
                Caveat::NotAfter(1_000),
                Caveat::Epoch(3),
                Caveat::Once("n1".into()),
            ],
        )
    }

    fn check(root: &SecretKey, cap: &Capability, f: impl FnOnce(&mut Ctx)) -> Result<String> {
        let mut c = Ctx::default();
        f(&mut c);
        let live = |g: &str| !c.revoked.contains(g);
        let mut seen = c.seen.clone();
        let mut consume = |n: &str| seen.insert(n.to_string());
        let mut ctx = VerifyContext {
            now: c.now,
            caller: &c.caller,
            action: c.action,
            item: &c.item,
            target: c.target.as_deref(),
            target_id: c.target_id.as_deref(),
            epoch: c.epoch,
            grant_is_live: &live,
            consume_nonce: &mut consume,
        };
        cap.verify(root, &mut ctx)
    }

    struct Ctx {
        now: u64,
        caller: String,
        action: Action,
        item: String,
        target: Option<String>,
        target_id: Option<String>,
        epoch: u64,
        revoked: HashSet<String>,
        seen: HashSet<String>,
    }
    impl Default for Ctx {
        fn default() -> Self {
            Self {
                now: 10,
                caller: "fp-a".into(),
                action: Action::Teleport,
                item: "i1".into(),
                target: Some("space-1".into()),
                target_id: Some("sandbox-1".into()),
                epoch: 3,
                revoked: HashSet::new(),
                seen: HashSet::new(),
            }
        }
    }

    #[test]
    fn valid_token_verifies_and_round_trips_the_wire_form() {
        let root = SecretKey::generate().unwrap();
        let cap = base(&root);
        let wire = cap.encode();
        assert!(wire.starts_with("cuakv1."));
        let back = Capability::decode(&wire).unwrap();
        assert_eq!(back, cap);
        assert_eq!(check(&root, &back, |_| {}).unwrap(), "g1");
        assert!(!format!("{cap:?}").contains(&wire.rsplit('.').next().unwrap().to_string()));
    }

    #[test]
    fn every_scope_is_enforced() {
        let root = SecretKey::generate().unwrap();
        let cap = base(&root);
        assert!(check(&root, &cap, |c| c.caller = "fp-b".into()).is_err());
        assert!(check(&root, &cap, |c| c.action = Action::Describe).is_err());
        assert!(check(&root, &cap, |c| c.item = "i3".into()).is_err());
        assert!(check(&root, &cap, |c| c.target = Some("space-2".into())).is_err());
        assert!(check(&root, &cap, |c| c.target = None).is_err());
        // Red-team F1: the same name resolving to a different immutable id (a
        // renamed or re-created Space) fails closed.
        assert!(check(&root, &cap, |c| c.target_id = Some("sandbox-2".into())).is_err());
        assert!(check(&root, &cap, |c| c.target_id = None).is_err());
        assert!(
            check(&root, &cap, |c| c.now = 1_000).is_err(),
            "expiry is exclusive"
        );
        assert!(check(&root, &cap, |c| c.epoch = 4).is_err(), "kill switch");
        assert!(
            check(&root, &cap, |c| {
                c.revoked.insert("g1".into());
            })
            .is_err()
        );
        assert!(
            check(&root, &cap, |c| {
                c.seen.insert("n1".into());
            })
            .is_err(),
            "replay"
        );
        assert!(
            check(&SecretKey::generate().unwrap(), &cap, |_| {}).is_err(),
            "other root"
        );
    }

    #[test]
    fn caveats_cannot_be_removed_or_edited() {
        let root = SecretKey::generate().unwrap();
        let cap = base(&root);
        let mut stripped = cap.clone();
        stripped.caveats.retain(|c| !c.starts_with("exp="));
        assert!(check(&root, &stripped, |c| c.now = 5_000).is_err());
        let mut widened = cap.clone();
        for c in &mut widened.caveats {
            if c.starts_with("item=") {
                *c = "item=i1,i2,i3".into();
            }
        }
        assert!(check(&root, &widened, |c| c.item = "i3".into()).is_err());
    }

    #[test]
    fn tokens_without_binding_are_refused() {
        let root = SecretKey::generate().unwrap();
        let bare = Capability::mint(&root, "t", &[Caveat::Grant("g".into())]);
        assert!(check(&root, &bare, |_| {}).is_err());
        // Red-team F6: a token without an epoch caveat would survive the kill
        // switch, so it is refused even with every other binding present.
        let no_epoch = Capability::mint(
            &root,
            "t",
            &[
                Caveat::Grant("g1".into()),
                Caveat::Caller("fp-a".into()),
                Caveat::NotAfter(1_000),
                Caveat::TargetIds(set(&["sandbox-1".to_string()])),
            ],
        );
        assert!(check(&root, &no_epoch, |_| {}).is_err());
        // Red-team F1: a token with no target-id caveat is refused even with
        // every other binding present, so nothing can deliver to an unpinned
        // (mutable-name) target.
        let no_target_id = Capability::mint(
            &root,
            "t",
            &[
                Caveat::Grant("g1".into()),
                Caveat::Caller("fp-a".into()),
                Caveat::NotAfter(1_000),
                Caveat::Epoch(3),
            ],
        );
        assert!(check(&root, &no_target_id, |_| {}).is_err());
        let unknown = Capability {
            caveats: vec!["frobnicate=1".into()],
            ..Capability::mint(&root, "t", &[])
        };
        assert!(
            Capability::decode(&unknown.encode())
                .unwrap()
                .caveats()
                .is_err()
        );
    }

    #[test]
    fn replay_is_refused_within_one_verifier() {
        let root = SecretKey::generate().unwrap();
        let cap = base(&root);
        let live = |_: &str| true;
        let mut seen = HashSet::new();
        let mut consume = |n: &str| seen.insert(n.to_string());
        let mut ctx = VerifyContext {
            now: 10,
            caller: "fp-a",
            action: Action::Teleport,
            item: "i1",
            target: Some("space-1"),
            target_id: Some("sandbox-1"),
            epoch: 3,
            grant_is_live: &live,
            consume_nonce: &mut consume,
        };
        assert!(cap.verify(&root, &mut ctx).is_ok());
        assert!(cap.verify(&root, &mut ctx).is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        /// Attenuation only narrows: whatever a caveat appended by the holder
        /// says, a request the attenuated token accepts is also accepted by
        /// the original.
        #[test]
        fn attenuation_never_widens(item in "i[0-9]", target in "space-[0-9]", now in 0u64..2_000,
                                    extra_items in proptest::collection::btree_set("i[0-9]", 0..4),
                                    extra_exp in 0u64..3_000) {
            let root = SecretKey::generate().unwrap();
            let cap = base(&root);
            let narrowed = cap.clone()
                .attenuate(&Caveat::Items(extra_items.clone()))
                .attenuate(&Caveat::NotAfter(extra_exp));
            let accepted_narrow = check(&root, &narrowed, |c| {
                c.item = item.clone(); c.target = Some(target.clone()); c.now = now;
            }).is_ok();
            let accepted_base = check(&root, &cap, |c| {
                c.item = item.clone(); c.target = Some(target.clone()); c.now = now;
            }).is_ok();
            prop_assert!(!accepted_narrow || accepted_base);
            if accepted_narrow {
                prop_assert!(extra_items.contains(&item) && now < extra_exp);
            }
        }

        #[test]
        fn random_tampering_fails(pos in any::<usize>(), byte in any::<u8>()) {
            let root = SecretKey::generate().unwrap();
            let wire = base(&root).encode();
            let mut bytes = wire.clone().into_bytes();
            let i = pos % bytes.len();
            prop_assume!(bytes[i] != byte);
            bytes[i] = byte;
            if let Ok(s) = String::from_utf8(bytes)
                && let Ok(cap) = Capability::decode(&s)
                && s != wire
            {
                // A different string may still decode to the same token
                // (base64 padding bits); only a changed token must fail.
                let orig = Capability::decode(&wire).unwrap();
                if cap != orig {
                    let refused = check(&root, &cap, |_| ()).is_err();
                    prop_assert!(refused);
                }
            }
        }
    }
}
