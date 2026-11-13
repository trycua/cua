// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account's pool keys as one device keeps them in step with the
//! relay's keyring ([`super::seal`]), and the pool log sealed with them
//! ([`SealedLog`]).
//!
//! Every signed-in machine's agent does the same small upkeep, so there is
//! nothing to set up and no single machine the keys depend on:
//!
//! - it certifies its pool key with its device key and registers it;
//! - the first device of the account makes the first generation;
//! - a device that holds the keys wraps them to any device of the account
//!   that has none (a new machine gets the keys for the runs to come, and
//!   the earlier ones, so it can take over);
//! - when a device that holds them has left (revoked, or re-keyed), the
//!   next generation is made and wrapped only to the devices still there,
//!   and the relay refuses anything sealed under an older generation from
//!   then on (`rekey`), so nothing written later is readable by the device
//!   that left;
//! - when no device that holds the keys is left at all (every one was
//!   revoked), the keys are gone with them: the next device starts a new
//!   chain and the relay drops the log nobody can read.
//!
//! A device whose keys are not shared yet while devices that hold them are
//! still enrolled (asleep, switched off, or gone without being revoked)
//! waits for one of them: it never starts a new chain on its own, because
//! only the person can tell an asleep laptop from a lost one (and the
//! relay, which says when it last saw a device, must not be able to make
//! a device drop the keys by hiding the others). It says which devices hold
//! the keys ([`KeyError::NotShared`]), so the person can wake one of them
//! or revoke the ones they no longer have.
//!
//! A keyring update names the version it replaces; the relay stores it only
//! if that is still the one it has, so two machines doing this at once
//! never lose a wrap.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::seal::{Bundle, Envelope, Keyring, Me, Member, PoolSecret, SealError, aad};
use crate::coordinator::journal::{Journal, JournalError, LogEntry, LogReader};

/// What the relay says about the account's keys.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeyState {
    /// The keyring (none before the first device made it).
    #[serde(default)]
    pub keyring: Option<Keyring>,
    /// Its version (0: none).
    #[serde(default)]
    pub version: u64,
    /// The account's devices that registered a pool key and may hold the
    /// keys (enrolled, not revoked).
    #[serde(default)]
    pub members: Vec<Member>,
}

/// A device of the account that holds the pool's keys.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    /// `dev_…`.
    pub device: String,
    /// What it calls itself (may be empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// When the relay last saw it (Unix seconds): for people only, never
    /// a reason to act on the keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
}

impl Holder {
    /// Its name, or its id.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.device
        } else {
            &self.name
        }
    }
}

/// `3 min ago`, `5 h ago`, `12 days ago`.
fn ago(secs: u64) -> String {
    match secs {
        0..120 => "just now".into(),
        120..7200 => format!("{} min ago", secs / 60),
        7200..172_800 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// What a device whose keys are not shared yet tells the person: which of
/// their devices hold the keys, and the two ways on.
pub fn not_shared_message(holders: &[Holder]) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let list = holders
        .iter()
        .map(|h| {
            let mut about = Vec::new();
            if !h.name.is_empty() {
                about.push(h.device.clone());
            }
            if let Some(t) = h.last_seen {
                about.push(format!("last seen {}", ago(now.saturating_sub(t))));
            }
            if about.is_empty() {
                h.label().to_string()
            } else {
                format!("{} ({})", h.label(), about.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    if holders.is_empty() {
        return "the pool's keys are not shared with this device yet".into();
    }
    format!(
        "the pool's keys are not shared with this device yet. Your pool keys are on: {list}. \
         At least one of them must be online to share them with a new device: open Cua (or start \
         `cua daemon`) on one of them and try again in a few seconds. If you no longer have \
         those devices, revoke them (`cua devices revoke <id>`): this device then makes new \
         pool keys (the pool's job history kept on the relay, sealed under the old keys, is \
         dropped)"
    )
}

/// Why the keys could not be had or kept.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// This device has no keys yet and devices of the account that hold
    /// them are still enrolled: one of them must share them (it does so
    /// on its own while it is online), or the person revokes them.
    #[error("{}", not_shared_message(.holders))]
    NotShared {
        /// The devices that hold the keys.
        holders: Vec<Holder>,
    },
    /// The relay could not be reached or failed for now.
    #[error("{0}")]
    Unavailable(String),
    /// The keyring changed meanwhile (the update named an older version).
    #[error("the pool keyring changed meanwhile")]
    Conflict,
    /// The relay refused.
    #[error("the relay refused the pool keyring: {0}")]
    Refused(String),
    /// The keys themselves.
    #[error(transparent)]
    Seal(#[from] SealError),
}

/// Where the account's keyring and members are kept (the relay).
#[async_trait]
pub trait KeyDirectory: Send + Sync + 'static {
    /// Registers this device's certified pool key.
    async fn register(&self, me: &Member) -> Result<(), KeyError>;
    /// The keyring, its version and the members.
    async fn fetch(&self) -> Result<KeyState, KeyError>;
    /// Stores `ring` if the stored one is still `version` (0: none);
    /// returns the new version. `reset`: replaces a keyring no member can
    /// open, and drops the log sealed under it.
    async fn put(&self, version: u64, ring: &Keyring, reset: bool) -> Result<u64, KeyError>;
    /// When the relay last saw each of the account's devices (Unix
    /// seconds, by `dev_…`), to tell people; empty when it cannot say.
    async fn last_seen(&self) -> HashMap<String, u64> {
        HashMap::new()
    }
}

/// This device's enrolled device key: it certifies the pool key.
pub trait DeviceSigner: Send + Sync + 'static {
    /// `dev_…`.
    fn device(&self) -> String;
    /// Its P-256 public key (uncompressed, base64url).
    fn public_key(&self) -> String;
    /// A fixed-size ECDSA P-256 SHA-256 signature of `message`, base64url.
    fn sign(&self, message: &str) -> Result<String, String>;
}

/// What this device knows about the keys, for `pool` status.
#[derive(Clone, Debug, Default, Serialize)]
pub struct KeysView {
    /// The generation it seals under (0: none yet).
    #[serde(rename = "gen")]
    pub generation: u64,
    /// The devices the keys are wrapped to (names, or ids).
    pub shared_with: Vec<String>,
    /// The account's devices that hold the keys now (enrolled, with the
    /// pool key they are wrapped to): at least one must be online to add
    /// a new device.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holders: Vec<Holder>,
    /// Why it has none, when it has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
}

/// The account's keys on this device.
pub struct PoolKeys {
    account: String,
    signer: Arc<dyn DeviceSigner>,
    secret: PoolSecret,
    dir: Arc<dyn KeyDirectory>,
    bundle: Mutex<Option<Arc<Bundle>>>,
    view: Mutex<KeysView>,
    /// When the relay last saw each device, and when that was asked.
    seen: Mutex<Option<(std::time::Instant, HashMap<String, u64>)>>,
    sync: tokio::sync::Mutex<()>,
}

/// How long what the relay said about when it saw each device is kept.
const SEEN_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// Rounds of fetch-and-update before giving up for now.
const ROUNDS: usize = 8;

impl PoolKeys {
    /// The keys of `account` for the device `signer`, with pool key
    /// `secret`, kept in step with `dir`.
    pub fn new(
        account: &str,
        signer: Arc<dyn DeviceSigner>,
        secret: PoolSecret,
        dir: Arc<dyn KeyDirectory>,
    ) -> Arc<Self> {
        Arc::new(Self {
            account: account.to_string(),
            signer,
            secret,
            dir,
            bundle: Mutex::default(),
            view: Mutex::default(),
            seen: Mutex::default(),
            sync: tokio::sync::Mutex::new(()),
        })
    }

    /// The account.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// This device's id.
    pub fn device(&self) -> String {
        self.signer.device()
    }

    /// This device's certified pool key.
    pub fn member(&self) -> Result<Member, KeyError> {
        let device = self.signer.device();
        let pool_key = self.secret.public();
        let sig = self
            .signer
            .sign(&super::seal::member_message(
                &self.account,
                &device,
                &pool_key,
            ))
            .map_err(KeyError::Unavailable)?;
        Ok(Member {
            device,
            public_key: self.signer.public_key(),
            pool_key,
            sig,
            name: String::new(),
        })
    }

    /// What this device knows about the keys.
    pub fn view(&self) -> KeysView {
        self.view.lock().expect("keys view").clone()
    }

    /// The keys this device has, without asking the relay.
    pub fn cached(&self) -> Option<Arc<Bundle>> {
        self.bundle.lock().expect("keys").clone()
    }

    /// The keys: the ones this device has, or brought in step first.
    pub async fn ensure(&self) -> Result<Arc<Bundle>, KeyError> {
        match self.cached() {
            Some(b) => Ok(b),
            None => self.sync().await,
        }
    }

    fn keep(&self, b: Bundle, ring: &Keyring, members: &[Member]) -> Arc<Bundle> {
        let b = Arc::new(b);
        *self.bundle.lock().expect("keys") = Some(b.clone());
        let names = ring
            .wraps
            .iter()
            .map(|w| {
                members
                    .iter()
                    .find(|m| m.device == w.device && !m.name.is_empty())
                    .map_or_else(|| w.device.clone(), |m| m.name.clone())
            })
            .collect();
        *self.view.lock().expect("keys view") = KeysView {
            generation: b.generation(),
            shared_with: names,
            holders: holders(ring, members),
            waiting: None,
        };
        b
    }

    /// When the relay last saw each device: asked at most once a minute
    /// (a waiting device syncs every second), keeping the last answer when
    /// the relay does not say.
    async fn last_seen(&self) -> HashMap<String, u64> {
        if let Some((at, m)) = self.seen.lock().expect("seen").as_ref()
            && at.elapsed() < SEEN_FOR
        {
            return m.clone();
        }
        let fresh = self.dir.last_seen().await;
        let mut g = self.seen.lock().expect("seen");
        let m = if fresh.is_empty() {
            g.as_ref().map(|(_, m)| m.clone()).unwrap_or_default()
        } else {
            fresh
        };
        *g = Some((std::time::Instant::now(), m.clone()));
        m
    }

    fn waiting(&self, why: &str, holders: &[Holder]) {
        let mut v = self.view.lock().expect("keys view");
        v.waiting = Some(why.to_string());
        v.holders = holders.to_vec();
    }

    /// Brings this device's keys in step with the relay's keyring and does
    /// the upkeep any member does (see the module docs).
    pub async fn sync(&self) -> Result<Arc<Bundle>, KeyError> {
        let _one = self.sync.lock().await;
        let me_member = self.member()?;
        let device = me_member.device.clone();
        let me = Me {
            device: &device,
            secret: &self.secret,
        };
        for _ in 0..ROUNDS {
            let st = self.dir.fetch().await?;
            let members: Vec<Member> = st
                .members
                .into_iter()
                .filter(|m| m.verify(&self.account))
                .collect();
            if !members
                .iter()
                .any(|m| m.device == device && m.pool_key == me_member.pool_key)
            {
                self.dir.register(&me_member).await?;
                continue;
            }
            let Some(ring) = st.keyring else {
                let b = Bundle::first(&self.account);
                let ring = Keyring::wrap(&b, &me, &members)?;
                match self.dir.put(0, &ring, false).await {
                    Ok(_) => {
                        tracing::info!(generation = 1, "this device made the pool's first keys");
                        return Ok(self.keep(b, &ring, &members));
                    }
                    Err(KeyError::Conflict) => continue,
                    Err(e) => return Err(e),
                }
            };
            let b = match ring.open(&self.account, &me, &members) {
                Ok(b) => b,
                Err(SealError::NotShared) => {
                    *self.bundle.lock().expect("keys") = None;
                    let mut holders = holders(&ring, &members);
                    if !holders.is_empty() {
                        // Who they are, for the person: when the relay last
                        // saw each is a hint only (it decides nothing here).
                        let seen = self.last_seen().await;
                        for h in &mut holders {
                            h.last_seen = seen.get(&h.device).copied();
                        }
                        self.waiting(
                            "waiting for one of your devices that has the pool's keys to share them with this one (at least one must be online)",
                            &holders,
                        );
                        return Err(KeyError::NotShared { holders });
                    }
                    // Nobody left holds the keys: start a new chain.
                    let b = Bundle::fresh(&self.account, ring.generation + 1);
                    let fresh = Keyring::wrap(&b, &me, &members)?;
                    match self.dir.put(st.version, &fresh, true).await {
                        Ok(_) => {
                            tracing::warn!(
                                generation = b.generation(),
                                "none of the account's devices that held the pool's keys is left: new keys (the pool log sealed under the old ones is dropped)"
                            );
                            return Ok(self.keep(b, &fresh, &members));
                        }
                        Err(KeyError::Conflict) => continue,
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e.into()),
            };
            if let Some(old) = self.cached() {
                b.continues(&old)?;
            }
            if ring.stale(&members) {
                // A device that holds the keys left: the next generation,
                // only for those still here.
                let next = b.rotate();
                let ring2 = Keyring::wrap(&next, &me, &members)?;
                match self.dir.put(st.version, &ring2, false).await {
                    Ok(_) => {
                        tracing::info!(
                            generation = next.generation(),
                            "a device left: new pool keys for the ones still here"
                        );
                        return Ok(self.keep(next, &ring2, &members));
                    }
                    Err(KeyError::Conflict) => continue,
                    Err(e) => return Err(e),
                }
            }
            let missing: Vec<Member> = members
                .iter()
                .filter(|m| !ring.has(&m.device, &m.pool_key))
                .cloned()
                .collect();
            if !missing.is_empty() {
                let mut ring2 = ring.clone();
                ring2.add(&b, &me, &missing)?;
                match self.dir.put(st.version, &ring2, false).await {
                    Ok(_) => {
                        tracing::info!(
                            devices = missing.len(),
                            "shared the pool's keys with the account's new devices"
                        );
                        return Ok(self.keep(b, &ring2, &members));
                    }
                    Err(KeyError::Conflict) => continue,
                    Err(e) => return Err(e),
                }
            }
            return Ok(self.keep(b, &ring, &members));
        }
        Err(KeyError::Unavailable(
            "the pool keyring kept changing; trying again later".into(),
        ))
    }

    /// Seals `plain` under the newest generation this device has.
    pub async fn seal(&self, aad: &[u8], plain: &[u8]) -> Result<Envelope, KeyError> {
        Ok(self.ensure().await?.seal(aad, plain))
    }

    /// Opens `env` (bringing the keys in step first when it is sealed
    /// under a newer generation than this device has).
    pub async fn open(&self, aad: &[u8], env: &Envelope) -> Result<Vec<u8>, KeyError> {
        let mut b = self.ensure().await?;
        if env.g > b.generation() {
            b = self.sync().await?;
        }
        Ok(b.open(aad, env)?)
    }
}

/// The members `ring` is wrapped to with the pool key they have now.
fn holders(ring: &Keyring, members: &[Member]) -> Vec<Holder> {
    let mut out: Vec<Holder> = Vec::new();
    for w in &ring.wraps {
        if let Some(m) = members
            .iter()
            .find(|m| m.device == w.device && m.pool_key == w.key)
            && !out.iter().any(|h| h.device == m.device)
        {
            out.push(Holder {
                device: m.device.clone(),
                name: m.name.clone(),
                last_seen: None,
            });
        }
    }
    out
}

/// The associated data of a log entry: its position and clear fields.
pub fn entry_aad(account: &str, seq: u64, e: &LogEntry) -> Vec<u8> {
    aad(
        "cua-pool-log/v1",
        &[
            account,
            &seq.to_string(),
            &e.kind,
            &e.job,
            &e.run,
            if e.terminal { "1" } else { "0" },
            &e.node,
            &e.attempt.to_string(),
        ],
    )
}

/// The associated data of a snapshot.
pub fn snapshot_aad(account: &str, seq: u64) -> Vec<u8> {
    aad("cua-pool-snapshot/v1", &[account, &seq.to_string()])
}

/// A pool log whose entries' data and snapshots are sealed with the
/// account's keys before they reach `inner`, and opened after.
pub struct SealedLog<L> {
    inner: L,
    keys: Arc<PoolKeys>,
    /// The last append: (from, as asked, as sealed). A retry of the same
    /// append sends the same bytes, so the relay recognizes it as a retry.
    last: Mutex<Option<LastAppend>>,
}

/// An append as asked (from, entries) and as sealed.
type LastAppend = (u64, Vec<LogEntry>, Vec<LogEntry>);

impl<L> SealedLog<L> {
    /// `inner` sealed with `keys`.
    pub fn new(inner: L, keys: Arc<PoolKeys>) -> Self {
        Self {
            inner,
            keys,
            last: Mutex::default(),
        }
    }

    /// The keys.
    pub fn keys(&self) -> &Arc<PoolKeys> {
        &self.keys
    }

    async fn after_rekey(&self, why: String) -> JournalError {
        *self.last.lock().expect("last append") = None;
        if let Err(e) = self.keys.sync().await {
            tracing::warn!("the pool's keys changed and could not be brought in step: {e}");
        }
        JournalError::Unavailable(format!("the pool's keys changed ({why}); sealing again"))
    }
}

fn unavailable(e: KeyError) -> JournalError {
    JournalError::Unavailable(format!("pool keys: {e}"))
}

#[async_trait]
impl<L: Journal> Journal for SealedLog<L> {
    fn replicated(&self) -> bool {
        self.inner.replicated()
    }

    async fn append(&self, from: u64, entries: Vec<LogEntry>) -> Result<u64, JournalError> {
        let cached = {
            let last = self.last.lock().expect("last append");
            last.as_ref()
                .filter(|(f, plain, _)| *f == from && *plain == entries)
                .map(|(_, _, sealed)| sealed.clone())
        };
        let sealed = match cached {
            Some(s) => s,
            None => {
                let b = self.keys.ensure().await.map_err(unavailable)?;
                let account = self.keys.account().to_string();
                let sealed: Vec<LogEntry> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let seq = from + 1 + i as u64;
                        let plain = serde_json::to_vec(&e.data).unwrap_or_default();
                        let mut s = e.clone();
                        s.data = b.seal(&entry_aad(&account, seq, e), &plain).to_value();
                        s
                    })
                    .collect();
                *self.last.lock().expect("last append") =
                    Some((from, entries.clone(), sealed.clone()));
                sealed
            }
        };
        match self.inner.append(from, sealed).await {
            Err(JournalError::Rekey(why)) => Err(self.after_rekey(why).await),
            r => r,
        }
    }

    async fn snapshot(&self, seq: u64, data: Vec<u8>) -> Result<(), JournalError> {
        let env = self
            .keys
            .seal(&snapshot_aad(self.keys.account(), seq), &data)
            .await
            .map_err(unavailable)?;
        match self.inner.snapshot(seq, env.to_bytes()).await {
            Err(JournalError::Rekey(why)) => Err(self.after_rekey(why).await),
            r => r,
        }
    }
}

#[async_trait]
impl<L: LogReader + Send + Sync> LogReader for SealedLog<L> {
    async fn read(&self, after: u64) -> Result<Option<(Vec<LogEntry>, u64)>, JournalError> {
        let Some((entries, head)) = self.inner.read(after).await? else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(entries.len());
        for mut e in entries {
            let env = Envelope::parse(&e.data).ok_or_else(|| {
                JournalError::Unavailable(format!(
                    "pool log entry {} is not sealed: refusing it",
                    e.seq
                ))
            })?;
            let plain = self
                .keys
                .open(&entry_aad(self.keys.account(), e.seq, &e), &env)
                .await
                .map_err(|err| {
                    JournalError::Unavailable(format!("pool log entry {}: {err}", e.seq))
                })?;
            e.data = serde_json::from_slice(&plain).map_err(|err| {
                JournalError::Unavailable(format!("pool log entry {}: {err}", e.seq))
            })?;
            out.push(e);
        }
        Ok(Some((out, head)))
    }

    async fn snapshot(&self) -> Result<Option<(u64, Vec<u8>)>, JournalError> {
        let Some((seq, bytes)) = LogReader::snapshot(&self.inner).await? else {
            return Ok(None);
        };
        let env = Envelope::from_bytes(&bytes).ok_or_else(|| {
            JournalError::Unavailable("the pool snapshot is not sealed: refusing it".into())
        })?;
        let plain = self
            .keys
            .open(&snapshot_aad(self.keys.account(), seq), &env)
            .await
            .map_err(|e| JournalError::Unavailable(format!("pool snapshot: {e}")))?;
        Ok(Some((seq, plain)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::memlog::MemPool;

    struct Signer(ring::signature::EcdsaKeyPair, String);

    impl Signer {
        fn new() -> Arc<Self> {
            use base64::Engine as _;
            use ring::signature::KeyPair as _;
            let rng = ring::rand::SystemRandom::new();
            let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
            let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
            let pair =
                ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
            let public =
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pair.public_key().as_ref());
            Arc::new(Self(pair, public))
        }
    }

    impl DeviceSigner for Signer {
        fn device(&self) -> String {
            super::super::seal::device_id(&self.1).unwrap()
        }
        fn public_key(&self) -> String {
            self.1.clone()
        }
        fn sign(&self, message: &str) -> Result<String, String> {
            use base64::Engine as _;
            let rng = ring::rand::SystemRandom::new();
            self.0
                .sign(&rng, message.as_bytes())
                .map(|s| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_ref()))
                .map_err(|_| "sign".into())
        }
    }

    /// A journal that keeps the snapshot it is given (as the relay does).
    #[derive(Default)]
    struct Snap(Mutex<Option<(u64, Vec<u8>)>>);

    #[async_trait]
    impl Journal for Snap {
        fn replicated(&self) -> bool {
            true
        }
        async fn append(&self, from: u64, e: Vec<LogEntry>) -> Result<u64, JournalError> {
            Ok(from + e.len() as u64)
        }
        async fn snapshot(&self, seq: u64, data: Vec<u8>) -> Result<(), JournalError> {
            *self.0.lock().unwrap() = Some((seq, data));
            Ok(())
        }
    }

    #[async_trait]
    impl LogReader for Snap {
        async fn read(&self, _after: u64) -> Result<Option<(Vec<LogEntry>, u64)>, JournalError> {
            Ok(None)
        }
        async fn snapshot(&self) -> Result<Option<(u64, Vec<u8>)>, JournalError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn keys_of(pool: &MemPool) -> Arc<PoolKeys> {
        PoolKeys::new(
            "acct",
            Signer::new(),
            PoolSecret::generate(),
            Arc::new(pool.keys("acct")),
        )
    }

    /// A device whose keys are not shared while a device that holds them
    /// is enrolled (offline) waits, naming it, and never makes new keys
    /// on its own: whatever the relay says about when it last saw it.
    #[tokio::test]
    async fn a_new_device_names_the_holders_and_never_starts_new_keys_while_they_are_enrolled() {
        let pool = MemPool::new();
        let first = keys_of(&pool);
        first.sync().await.unwrap();
        // Last seen a year ago: still a holder (only the person revokes).
        pool.saw(&first.device(), 1);
        let new = keys_of(&pool);
        let e = new.sync().await.unwrap_err();
        let KeyError::NotShared { holders } = &e else {
            panic!("{e:?}")
        };
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].device, first.device());
        assert_eq!(holders[0].last_seen, Some(1));
        let msg = e.to_string();
        assert!(msg.contains(&first.device()), "{msg}");
        assert!(msg.contains("days ago"), "{msg}");
        assert!(msg.contains("cua devices revoke"), "{msg}");
        assert_eq!(new.view().holders, *holders);
        assert!(new.view().waiting.is_some());
        let ring = pool.keyring().unwrap();
        assert_eq!(ring.generation, 1, "no new chain");
        // The holder comes online: it shares them; the new device has them.
        first.sync().await.unwrap();
        assert_eq!(new.sync().await.unwrap().generation(), 1);
        let v = new.view();
        assert!(v.waiting.is_none());
        let mut held: Vec<String> = v.holders.iter().map(|h| h.device.clone()).collect();
        held.sort();
        let mut want = vec![first.device(), new.device()];
        want.sort();
        assert_eq!(held, want);
    }

    #[test]
    fn the_message_names_each_holder_once_with_when_it_was_seen() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let m = not_shared_message(&[
            Holder {
                device: "dev_a".into(),
                name: "ada-laptop".into(),
                last_seen: Some(now - 5 * 3600),
            },
            Holder {
                device: "dev_b".into(),
                ..Holder::default()
            },
        ]);
        assert!(
            m.contains("Your pool keys are on: ada-laptop (dev_a, last seen 5 h ago), dev_b."),
            "{m}"
        );
        assert_eq!(
            not_shared_message(&[]),
            "the pool's keys are not shared with this device yet"
        );
    }

    #[tokio::test]
    async fn a_snapshot_is_stored_sealed_and_bound_to_its_position() {
        let pool = MemPool::new();
        let keys = PoolKeys::new(
            "acct",
            Signer::new(),
            PoolSecret::generate(),
            Arc::new(pool.keys("acct")),
        );
        keys.sync().await.unwrap();
        let log = SealedLog::new(Snap::default(), keys);
        let state = b"the coordinator's database: echo secret".to_vec();
        Journal::snapshot(&log, 7, state.clone()).await.unwrap();
        let stored = log.inner.0.lock().unwrap().clone().unwrap();
        assert!(Envelope::from_bytes(&stored.1).is_some(), "stored sealed");
        assert!(!stored.1.windows(6).any(|w| w == b"secret"));
        assert_eq!(LogReader::snapshot(&log).await.unwrap(), Some((7, state)));
        // Moved to another position by the relay: it does not open.
        log.inner.0.lock().unwrap().as_mut().unwrap().0 = 8;
        assert!(LogReader::snapshot(&log).await.is_err());
    }
}
