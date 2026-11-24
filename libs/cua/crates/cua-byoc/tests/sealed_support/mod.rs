// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Stand-ins for the sealed-token tests of the Hetzner and Modal suites:
//! Cua's issuer with the sealed-token compare-and-swap ([`FakeIssuer`]) and
//! the account's pool keys ([`FakePool`]: a key per generation, a new
//! generation when a device is revoked, which then gets no newer key; an
//! AEAD bound to the associated data, from `ring`). The pool keys
//! themselves are tested in `cua-spaces-cli` (`cloud_tokens`).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cua_byoc::federation::{
    Expected, IssuerAccount, IssuerClient, IssuerError, IssuerLimits, MintedToken, NewConnection,
    SealedBlob, ServerConnection, TokenRequest,
};
use cua_byoc::sealed::{PoolSealer, SealerError};
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::rand::SecureRandom as _;

pub const ACCT: &str = "a1b2c3d4e5f6";

/// Cua's issuer: the account's connections, with the sealed-token swap.
#[derive(Default)]
pub struct FakeIssuer {
    pub conns: Mutex<Vec<ServerConnection>>,
    /// Every change: (what, agent).
    pub changes: Mutex<Vec<(String, Option<String>)>>,
}

impl FakeIssuer {
    pub fn get(&self, id: &str) -> ServerConnection {
        self.conns
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .unwrap()
    }

    pub fn only(&self, cloud: &str) -> ServerConnection {
        self.conns
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.cloud == cloud && c.status != "revoked")
            .cloned()
            .unwrap()
    }

    pub fn approve(&self, id: &str) {
        let mut all = self.conns.lock().unwrap();
        let c = all.iter_mut().find(|c| c.id == id).unwrap();
        c.status = "connected".into();
        c.version += 1;
    }

    fn change(&self, what: &str, agent: Option<&str>) {
        self.changes
            .lock()
            .unwrap()
            .push((what.into(), agent.map(String::from)));
    }
}

#[async_trait]
impl IssuerClient for FakeIssuer {
    async fn token(&self, _req: &TokenRequest) -> Result<MintedToken, IssuerError> {
        Err(IssuerError::Other(
            "sealed connections get no tokens".into(),
        ))
    }

    async fn account(&self) -> Result<IssuerAccount, IssuerError> {
        Ok(IssuerAccount {
            account: ACCT.into(),
            ..Default::default()
        })
    }

    async fn connections(&self) -> Result<Vec<ServerConnection>, IssuerError> {
        Ok(self.conns.lock().unwrap().clone())
    }

    async fn create_connection(
        &self,
        new: &NewConnection,
        agent: Option<&str>,
    ) -> Result<ServerConnection, IssuerError> {
        self.change("create", agent);
        let mut all = self.conns.lock().unwrap();
        let c = ServerConnection {
            id: format!("conn_{}", all.len() + 1),
            account: ACCT.into(),
            cloud: new.cloud.clone(),
            auth: "sealed".into(),
            identifiers: new.identifiers.clone(),
            region: new.region.clone(),
            limits: new.limits.clone(),
            status: if agent.is_some() {
                "pending"
            } else {
                "connected"
            }
            .into(),
            version: 1,
            ..Default::default()
        };
        all.push(c.clone());
        Ok(c)
    }

    async fn set_limits(
        &self,
        id: &str,
        limits: &IssuerLimits,
        agent: Option<&str>,
    ) -> Result<(ServerConnection, bool), IssuerError> {
        self.change("limits", agent);
        let mut all = self.conns.lock().unwrap();
        let c = all.iter_mut().find(|c| c.id == id).unwrap();
        c.limits = limits.clone();
        c.version += 1;
        Ok((c.clone(), false))
    }

    async fn revoke(&self, id: &str, agent: Option<&str>) -> Result<(), IssuerError> {
        self.change("revoke", agent);
        let mut all = self.conns.lock().unwrap();
        let c = all
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| IssuerError::NotFound(id.into()))?;
        c.status = "revoked".into();
        c.sealed_token = None;
        c.version += 1;
        Ok(())
    }

    async fn set_sealed_token(
        &self,
        id: &str,
        token: &SealedBlob,
        expected: Expected,
        agent: Option<&str>,
    ) -> Result<ServerConnection, IssuerError> {
        let mut all = self.conns.lock().unwrap();
        let c = all
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| IssuerError::NotFound(id.into()))?;
        if c.status == "revoked" {
            return Err(IssuerError::Revoked);
        }
        let had = c.sealed_token.as_ref().map_or(0, |b| b.generation);
        if expected.expected_version.is_some_and(|v| v != c.version)
            || expected.expected_generation.is_some_and(|g| g != had)
        {
            return Err(IssuerError::Conflict);
        }
        c.sealed_token = Some(token.clone());
        c.version += 1;
        if agent.is_some() {
            c.status = "pending".into();
        }
        let out = c.clone();
        drop(all);
        self.change(&format!("seal {}", token.generation), agent);
        Ok(out)
    }
}

/// The account's pool keys: a key per generation and the devices that are
/// members now.
#[derive(Default)]
pub struct FakePool {
    keys: Mutex<BTreeMap<u64, [u8; 32]>>,
    members: Mutex<BTreeMap<String, String>>,
}

fn random_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut k).unwrap();
    k
}

impl FakePool {
    pub fn new() -> Arc<Self> {
        let p = Arc::new(FakePool::default());
        p.keys.lock().unwrap().insert(1, random_key());
        p
    }

    /// A device of the account named `name`.
    pub fn join(self: &Arc<Self>, name: &str) -> Arc<FakeSealer> {
        let device = format!("dev_{name}");
        self.members
            .lock()
            .unwrap()
            .insert(device.clone(), name.into());
        Arc::new(FakeSealer {
            pool: self.clone(),
            device,
            has: Mutex::default(),
        })
    }

    /// `name` is revoked: a new generation for the devices still there.
    pub fn revoke(&self, name: &str) {
        self.members.lock().unwrap().remove(&format!("dev_{name}"));
        let mut keys = self.keys.lock().unwrap();
        let next = keys.keys().max().copied().unwrap_or(0) + 1;
        keys.insert(next, random_key());
    }
}

/// One device's pool keys.
pub struct FakeSealer {
    pool: Arc<FakePool>,
    device: String,
    /// The keys this device got (a revoked device keeps the old ones).
    has: Mutex<BTreeMap<u64, [u8; 32]>>,
}

impl FakeSealer {
    fn sync(&self) -> Result<u64, SealerError> {
        if !self.pool.members.lock().unwrap().contains_key(&self.device) {
            return Err(SealerError::Unavailable(
                "this device was removed from the account".into(),
            ));
        }
        let keys = self.pool.keys.lock().unwrap().clone();
        let newest = *keys.keys().max().unwrap();
        *self.has.lock().unwrap() = keys;
        Ok(newest)
    }

    fn key(&self, generation: u64) -> Option<[u8; 32]> {
        self.has.lock().unwrap().get(&generation).copied()
    }
}

fn aead(key: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).unwrap())
}

#[async_trait]
impl PoolSealer for FakeSealer {
    async fn seal(
        &self,
        plain: &[u8],
        aad: &(dyn Fn(u64) -> Vec<u8> + Send + Sync),
    ) -> Result<SealedBlob, SealerError> {
        let generation = self.sync()?;
        let key = self.key(generation).unwrap();
        let mut nonce = [0u8; 12];
        ring::rand::SystemRandom::new().fill(&mut nonce).unwrap();
        let mut buf = plain.to_vec();
        aead(&key)
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad(generation)),
                &mut buf,
            )
            .unwrap();
        let mut ciphertext = nonce.to_vec();
        ciphertext.extend(buf);
        Ok(SealedBlob {
            ciphertext,
            generation,
        })
    }

    async fn open(
        &self,
        blob: &SealedBlob,
        aad: &[u8],
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, SealerError> {
        if self.key(blob.generation).is_none() {
            let _ = self.sync();
        }
        let key = self.key(blob.generation).ok_or_else(|| {
            SealerError::Refused(format!(
                "sealed under generation {}, which this device does not have",
                blob.generation
            ))
        })?;
        if blob.ciphertext.len() < 12 {
            return Err(SealerError::Refused("short".into()));
        }
        let (nonce, ct) = blob.ciphertext.split_at(12);
        let mut buf = ct.to_vec();
        let plain = aead(&key)
            .open_in_place(
                Nonce::try_assume_unique_for_key(nonce).unwrap(),
                Aad::from(aad),
                &mut buf,
            )
            .map_err(|_| SealerError::Refused("changed, or another key".into()))?;
        Ok(zeroize::Zeroizing::new(plain.to_vec()))
    }

    async fn generation(&self) -> Result<u64, SealerError> {
        self.sync()
    }

    async fn holders(&self) -> Result<Vec<String>, SealerError> {
        Ok(self
            .pool
            .members
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect())
    }
}
