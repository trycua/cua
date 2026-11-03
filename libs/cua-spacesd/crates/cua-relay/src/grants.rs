// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who has reached each account machine, and whose access was taken away
//! (CUA-1416, CUA-1417).
//!
//! A machine mints its own side-channel tickets (the media WebSocket,
//! tunnels, the volume) for the caller of an authorized account request.
//! The relay can't verify them; it forwards them and the machine checks the
//! HMAC and the expiry. That alone let a ticket minted before a revoke
//! outlive the revoke for its whole lifetime, which the client picks (up to
//! a day). So the relay remembers:
//!
//! - **seen**: for each machine, the accounts it authorized there in the
//!   last [`TICKET_HORIZON_MS`], with the role they had. Revokes re-check
//!   exactly these accounts, so a change cuts only the accounts that lost
//!   access, not every client of the machine.
//! - **revoked**: for each machine, the accounts that lost access (or
//!   dropped to a lower role) and when. A ticket names the account it was
//!   minted for (`p`, signed by the machine, so a guest can't change it).
//!   Its use is refused while it may predate the revoke: with the minting
//!   assertion's `iat` in the ticket (`a`), exactly; without it (older
//!   spacesd), until the account has been authorized again at its old role
//!   or the mark outlives every ticket that could predate it.
//!
//! Both are persisted next to the directory so a relay restart can't
//! reopen the hole, and both forget entries after the horizon.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::directory::Role;
use crate::oidc::Identity;
use crate::storage::{store, Storage};

/// Longest a machine-minted ticket can live (spacesd's `MAX_TICKET_TTL`, a
/// day) plus clock slack: no ticket minted before a revoke can still be
/// valid after this.
pub const TICKET_HORIZON_MS: u64 = (24 * 60 * 60 + 10 * 60) * 1000;

/// How often a still-active account's `seen` time is persisted (it is
/// always current in memory).
const SEEN_PERSIST_EVERY_MS: u64 = 60 * 60 * 1000;

/// A role as a rank: more is more access.
pub fn rank(role: Role) -> u8 {
    match role {
        Role::Viewer => 1,
        Role::Shared => 2,
        Role::Owner => 3,
    }
}

fn digest_hex(s: &str) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, s.as_bytes()).as_ref())
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// An account the relay authorized on a machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Seen {
    /// Account id (`Identity::account`).
    pub account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Role rank when last authorized ([`rank`]).
    pub role: u8,
    /// When last authorized (ms).
    pub at: u64,
}

impl Seen {
    /// The identity to re-check access with.
    pub fn identity(&self, user: &str) -> Identity {
        Identity {
            user: user.to_owned(),
            account: self.account.clone(),
            email: self.email.clone(),
            name: self.name.clone(),
            auth_time: None,
            mfa: false,
            orgs: Vec::new(),
        }
    }
}

/// An account's access to a machine was revoked or lowered.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Revoked {
    /// When (ms).
    pub at: u64,
    /// The highest role rank it had before, within the horizon.
    pub role: u8,
}

/// A ticket's claims, read without verifying it (the machine does that).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketInfo {
    /// The account it was minted for (`p`, without a `viewer:` prefix).
    pub user: String,
    /// The minting call's relay assertion `iat` (seconds), when the
    /// machine records it.
    pub issued: Option<u64>,
}

#[derive(Deserialize)]
struct TicketPayload {
    #[serde(default)]
    p: String,
    #[serde(default)]
    a: Option<u64>,
}

/// Reads a spacesd ticket (`v1.<base64url JSON>.<sig>`). `None` when it is
/// not one (the machine refuses it anyway).
pub fn parse_ticket(ticket: &str) -> Option<TicketInfo> {
    use base64::Engine as _;
    let mut parts = ticket.split('.');
    let (Some("v1"), Some(body), Some(_sig), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .ok()?;
    let payload: TicketPayload = serde_json::from_slice(&raw).ok()?;
    let user = payload
        .p
        .strip_prefix("viewer:")
        .unwrap_or(&payload.p)
        .to_owned();
    Some(TicketInfo {
        user,
        issued: payload.a,
    })
}

/// Every ticket a request carries: the `ticket` query parameter and
/// `cua.ticket.` / legacy `rcdp.v2.ticket.` WebSocket subprotocols.
pub fn tickets_in(uri: &axum::http::Uri, headers: &axum::http::HeaderMap) -> Vec<String> {
    let mut found = Vec::new();
    for pair in uri.query().unwrap_or("").split('&') {
        if let Some(v) = pair.strip_prefix("ticket=") {
            found.push(percent_decode(v));
        }
    }
    for value in headers.get_all(axum::http::header::SEC_WEBSOCKET_PROTOCOL) {
        let Ok(value) = value.to_str() else { continue };
        for protocol in value.split(',').map(str::trim) {
            if let Some(t) = protocol
                .strip_prefix("cua.ticket.")
                .or_else(|| protocol.strip_prefix("rcdp.v2.ticket."))
            {
                found.push(t.to_owned());
            }
        }
    }
    found
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    seen: BTreeMap<String, BTreeMap<String, Seen>>,
    #[serde(default)]
    revoked: BTreeMap<String, BTreeMap<String, Revoked>>,
    /// Accounts erased (account deletion): SHA-256 of the user id -> when
    /// (ms). A ticket minted for them before that stays refused until it
    /// cannot be valid any more, without keeping the id itself.
    #[serde(default)]
    erased: BTreeMap<String, u64>,
    /// The last write failed: the next change writes the file again even
    /// when it would not on its own.
    #[serde(skip)]
    unsaved: bool,
}

impl State {
    fn prune(&mut self, now: u64) {
        let fresh = |at: u64| now.saturating_sub(at) < TICKET_HORIZON_MS;
        for users in self.seen.values_mut() {
            users.retain(|_, s| fresh(s.at));
        }
        self.seen.retain(|_, users| !users.is_empty());
        for users in self.revoked.values_mut() {
            users.retain(|_, r| fresh(r.at));
        }
        self.revoked.retain(|_, users| !users.is_empty());
        self.erased.retain(|_, at| fresh(*at));
    }
}

/// The store (see the module docs).
#[derive(Default)]
pub struct Grants {
    path: Option<PathBuf>,
    state: Mutex<State>,
    /// Where write failures are counted and reported.
    storage: Arc<Storage>,
}

impl Grants {
    /// An in-memory store.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads (or starts) the store persisted at `path`.
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        Self::open_with(path, Arc::default())
    }

    /// [`Grants::open`], reporting writes to `storage`.
    pub fn open_with(path: PathBuf, storage: Arc<Storage>) -> std::io::Result<Self> {
        crate::assertion::remove_stale_temps(&path)?;
        let mut state = match std::fs::read(&path) {
            Ok(raw) => serde_json::from_slice(&raw)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e),
        };
        state.prune(now_ms());
        Ok(Self {
            path: Some(path),
            state: Mutex::new(state),
            storage,
        })
    }

    /// Whether the grants file is writable: no write failed lately without
    /// a successful one since (true in memory).
    pub fn writes_ok(&self) -> bool {
        self.storage.store_ok(store::GRANTS)
    }

    /// Writes the grants file: compact JSON streamed to a temp file that
    /// replaces it. A failed write is counted and logged (store `grants` on
    /// `/readyz` and `/metrics`, `state persist failed`) and retried with
    /// the next change. Nothing is rolled back: a revoke mark is enforced
    /// from memory either way (dropping it would let the old tickets
    /// through again); what a failed write risks is only that a restart
    /// before the next successful one forgets it.
    fn persist(&self, state: &mut State) {
        let _ = self.persist_result(state);
    }

    /// [`Self::persist`], returning the write's outcome.
    fn persist_result(&self, state: &mut State) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let written = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::assertion::write_private_with(path, |out| {
                serde_json::to_writer(&mut *out, &*state).map_err(std::io::Error::other)
            })
        })();
        self.storage.record(store::GRANTS, &written);
        state.unsaved = written.is_err();
        written
    }

    /// Emails the seen rows hold for `account`.
    pub fn emails_of(&self, account: &str) -> Vec<String> {
        let state = self.state.lock().expect("grants");
        state
            .seen
            .values()
            .flat_map(|m| m.iter())
            .filter(|(u, s)| *u == account || s.account == account)
            .filter_map(|(_, s)| s.email.clone())
            .collect()
    }

    /// Account deletion: forgets every row about `account` (its user id,
    /// or any of `emails`): who it reached on any machine (rows on other
    /// people's machines carrying its email or name included) and its
    /// revoke marks. Leaves a PII-free mark (a hash, for the ticket
    /// horizon) so tickets minted for it before now stay refused. Returns
    /// the rows removed. The file is rewritten every call, so a retry after
    /// a failed write finishes the job.
    pub fn erase_account(&self, account: &str, emails: &[String]) -> std::io::Result<usize> {
        let mut state = self.state.lock().expect("grants");
        let mut rows = 0;
        let mut users: BTreeSet<String> = BTreeSet::from([account.to_owned()]);
        for machine in state.seen.values_mut() {
            machine.retain(|user, s| {
                let mine = user == account
                    || s.account == account
                    || s.email
                        .as_deref()
                        .is_some_and(|e| emails.iter().any(|m| m.eq_ignore_ascii_case(e)));
                if mine {
                    users.insert(user.clone());
                    rows += 1;
                }
                !mine
            });
        }
        for machine in state.revoked.values_mut() {
            machine.retain(|user, _| {
                let mine = user == account || users.contains(user);
                rows += usize::from(mine);
                !mine
            });
        }
        state.seen.retain(|_, u| !u.is_empty());
        state.revoked.retain(|_, u| !u.is_empty());
        if rows > 0 {
            let at = now_ms();
            for user in &users {
                state.erased.insert(digest_hex(user), at);
            }
        }
        state.prune(now_ms());
        self.persist_result(&mut state)?;
        Ok(rows)
    }

    /// Whether `user` was erased (account deletion) after `issued` (the
    /// seconds a ticket was minted at; unknown: assume before).
    pub fn erased_since(&self, user: &str, issued: Option<u64>) -> bool {
        let state = self.state.lock().expect("grants");
        if state.erased.is_empty() {
            return false;
        }
        state
            .erased
            .get(&digest_hex(user))
            .is_some_and(|at| issued.is_none_or(|iat| iat.saturating_mul(1000) <= *at))
    }

    /// Records that `who` was authorized on `machine` as `role`; `at` is
    /// taken before the role was computed, so a revoke that races the
    /// request still counts as later.
    pub fn record(&self, machine: &str, who: &Identity, role: Role, at: u64) {
        let mut state = self.state.lock().expect("grants");
        let users = state.seen.entry(machine.to_owned()).or_default();
        let next = Seen {
            account: who.account.clone(),
            email: who.email.clone(),
            name: who.name.clone(),
            role: rank(role),
            at,
        };
        let old = users.get(&who.user).cloned();
        let changed = match &old {
            None => true,
            Some(old) => {
                old.role != next.role
                    || old.email != next.email
                    || old.account != next.account
                    || next.at.saturating_sub(old.at) > SEEN_PERSIST_EVERY_MS
            }
        };
        // The first authorization after a revoke mark is what lets an older
        // spacesd's tickets through again: keep it across a restart.
        let first_since_mark = state
            .revoked
            .get(machine)
            .and_then(|r| r.get(&who.user))
            .is_some_and(|mark| old.as_ref().is_none_or(|o| o.at <= mark.at));
        let users = state.seen.entry(machine.to_owned()).or_default();
        users.insert(who.user.clone(), next);
        if changed || first_since_mark || state.unsaved {
            state.prune(now_ms());
            self.persist(&mut state);
        }
    }

    /// The accounts seen on `machine`.
    pub fn seen_on(&self, machine: &str) -> Vec<(String, Seen)> {
        let state = self.state.lock().expect("grants");
        state
            .seen
            .get(machine)
            .map(|users| users.iter().map(|(u, s)| (u.clone(), s.clone())).collect())
            .unwrap_or_default()
    }

    /// Machines with any seen account.
    pub fn machines(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("grants")
            .seen
            .keys()
            .cloned()
            .collect()
    }

    /// `user` on `machine`: as last seen and its revoke mark.
    pub fn lookup(&self, machine: &str, user: &str) -> (Option<Seen>, Option<Revoked>) {
        let state = self.state.lock().expect("grants");
        (
            state.seen.get(machine).and_then(|u| u.get(user)).cloned(),
            state
                .revoked
                .get(machine)
                .and_then(|u| u.get(user))
                .copied(),
        )
    }

    /// Updates `user`'s role rank on `machine` (it was raised).
    pub fn set_role(&self, machine: &str, user: &str, role: u8) {
        let mut state = self.state.lock().expect("grants");
        if let Some(s) = state.seen.get_mut(machine).and_then(|u| u.get_mut(user)) {
            s.role = role;
            self.persist(&mut state);
        }
    }

    /// Whether `user`'s access to `machine` was revoked at or after `at`.
    pub fn revoked_since(&self, machine: &str, user: &str, at: u64) -> bool {
        self.state
            .lock()
            .expect("grants")
            .revoked
            .get(machine)
            .and_then(|u| u.get(user))
            .is_some_and(|mark| mark.at >= at)
    }

    /// Marks `user`'s access to `machine` as revoked (or lowered to `now`)
    /// at `at`. Tickets it holds that may predate this are refused.
    pub fn revoke(&self, machine: &str, user: &str, before: u8, now: Option<u8>, at: u64) {
        let mut state = self.state.lock().expect("grants");
        let mark = state
            .revoked
            .entry(machine.to_owned())
            .or_default()
            .entry(user.to_owned())
            .or_insert(Revoked { at, role: before });
        mark.at = mark.at.max(at);
        mark.role = mark.role.max(before);
        if let Some(users) = state.seen.get_mut(machine) {
            match now {
                Some(rank) => {
                    if let Some(s) = users.get_mut(user) {
                        s.role = rank;
                    }
                }
                None => {
                    users.remove(user);
                }
            }
        }
        state.prune(now_ms());
        self.persist(&mut state);
    }
}

/// Why a ticket is refused.
pub const TICKET_REVOKED: &str =
    "your access to this machine changed after this ticket was issued; reconnect to get a new one";

/// Whether a ticket for an account with mark `revoked` may be used, given
/// how it was last seen and its role now (`None`: no access).
pub fn ticket_allowed(
    ticket: &TicketInfo,
    seen: Option<&Seen>,
    revoked: Option<Revoked>,
    role_now: Option<u8>,
) -> bool {
    let Some(mark) = revoked else {
        return true;
    };
    match ticket.issued {
        // Minted from an assertion issued after the revoke (relay clock).
        Some(iat) => iat.saturating_mul(1000) > mark.at,
        // Older spacesd: no mint time. Allowed only once the account was
        // authorized again since, at no lower a role than it lost.
        None => seen.is_some_and(|s| s.at > mark.at) && role_now.is_some_and(|r| r >= mark.role),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(p: &str, a: Option<u64>) -> String {
        use base64::Engine as _;
        let mut payload = serde_json::json!({"s": "media", "r": "m1", "p": p, "e": 1, "n": "x"});
        if let Some(a) = a {
            payload["a"] = a.into();
        }
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        format!("v1.{body}.sig")
    }

    fn who(user: &str) -> Identity {
        Identity {
            user: user.into(),
            account: user.into(),
            email: Some(format!("{user}@example.com")),
            name: None,
            auth_time: None,
            mfa: false,
            orgs: Vec::new(),
        }
    }

    #[test]
    fn parses_the_principal_and_mint_time() {
        let t = parse_ticket(&ticket("bob", None)).unwrap();
        assert_eq!(t.user, "bob");
        assert_eq!(t.issued, None);
        let t = parse_ticket(&ticket("viewer:bob", Some(42))).unwrap();
        assert_eq!(t.user, "bob");
        assert_eq!(t.issued, Some(42));
        assert!(parse_ticket("t0").is_none());
        assert!(parse_ticket("v1.!!.x").is_none());
    }

    #[test]
    fn finds_tickets_in_the_query_and_subprotocols() {
        let uri: axum::http::Uri = "/media?x=1&ticket=v1.a%2Eb.c".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::SEC_WEBSOCKET_PROTOCOL,
            "cua.media.v1, cua.ticket.v1.d.e".parse().unwrap(),
        );
        headers.append(
            axum::http::header::SEC_WEBSOCKET_PROTOCOL,
            "rcdp.v2.ticket.v1.f.g".parse().unwrap(),
        );
        assert_eq!(
            tickets_in(&uri, &headers),
            vec!["v1.a.b.c", "v1.d.e", "v1.f.g"]
        );
    }

    #[test]
    fn a_ticket_from_before_a_revoke_is_refused() {
        let t = now_ms();
        let g = Grants::in_memory();
        g.record("m", &who("bob"), Role::Shared, t + 1_000);
        let (seen, mark) = g.lookup("m", "bob");
        let legacy = TicketInfo {
            user: "bob".into(),
            issued: None,
        };
        assert!(ticket_allowed(&legacy, seen.as_ref(), mark, Some(2)));
        g.revoke("m", "bob", 2, None, t + 2_000);
        let (seen, mark) = g.lookup("m", "bob");
        assert!(seen.is_none(), "forgotten as a user of the machine");
        assert!(!ticket_allowed(&legacy, seen.as_ref(), mark, None));
        // With the mint time: before the revoke refused, after allowed.
        let old = TicketInfo {
            user: "bob".into(),
            issued: Some(t / 1000),
        };
        let new = TicketInfo {
            user: "bob".into(),
            issued: Some(t / 1000 + 3),
        };
        assert!(!ticket_allowed(&old, None, mark, Some(2)));
        assert!(ticket_allowed(&new, None, mark, Some(2)));
    }

    #[test]
    fn a_legacy_ticket_needs_the_old_role_back() {
        let t = now_ms();
        let g = Grants::in_memory();
        g.record("m", &who("bob"), Role::Shared, t + 1_000);
        // Editor lowered to viewer.
        g.revoke("m", "bob", 2, Some(1), t + 2_000);
        let legacy = TicketInfo {
            user: "bob".into(),
            issued: None,
        };
        g.record("m", &who("bob"), Role::Viewer, t + 3_000);
        let (seen, mark) = g.lookup("m", "bob");
        assert!(
            !ticket_allowed(&legacy, seen.as_ref(), mark, Some(1)),
            "an editor-era ticket must not work for a viewer"
        );
        // Made an editor again and authorized since: allowed.
        g.record("m", &who("bob"), Role::Shared, t + 4_000);
        let (seen, mark) = g.lookup("m", "bob");
        assert!(ticket_allowed(&legacy, seen.as_ref(), mark, Some(2)));
    }

    #[test]
    fn persists_and_forgets_after_the_horizon() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grants.json");
        let g = Grants::open(path.clone()).unwrap();
        let now = now_ms();
        g.record("m", &who("bob"), Role::Shared, now);
        g.revoke("m", "bob", 2, None, now);
        g.record("m", &who("old"), Role::Viewer, now - TICKET_HORIZON_MS - 1);
        let g = Grants::open(path).unwrap();
        assert!(
            g.lookup("m", "bob").1.is_some(),
            "revoke survives a restart"
        );
        assert!(g.lookup("m", "old").0.is_none(), "stale entry pruned");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_is_loud_keeps_the_revoke_and_is_retried() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let storage = Arc::new(Storage::new(Some(state.clone()), Default::default()));
        let path = state.join("relay.grants.json");
        let g = Grants::open_with(path.clone(), storage.clone()).unwrap();
        let now = now_ms();
        g.record("m", &who("bob"), Role::Shared, now);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains('\n'), "compact JSON: {raw}");
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o500)).unwrap();
        g.revoke("m", "bob", 2, None, now + 1);
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        // The revoke holds in memory either way.
        assert!(g.revoked_since("m", "bob", now + 1));
        // Root ignores directory permissions; nothing more to check then.
        if storage.failures(store::GRANTS) > 0 {
            assert!(!g.writes_ok());
            let (ready, body) = storage.report();
            assert!(!ready);
            assert_eq!(body["failing"], serde_json::json!(["grants"]));
            assert!(
                Grants::open(path.clone())
                    .unwrap()
                    .lookup("m", "bob")
                    .1
                    .is_none(),
                "not on disk yet"
            );
            // The next authorization of anyone, even one that would not
            // write on its own, saves the mark.
            g.record("m", &who("carol"), Role::Shared, now + 2);
            g.record("m", &who("carol"), Role::Shared, now + 3);
        }
        assert!(g.writes_ok());
        assert!(
            Grants::open(path).unwrap().lookup("m", "bob").1.is_some(),
            "the revoke reached the disk"
        );
    }
}
