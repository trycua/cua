// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! API tokens sealed to the account's devices, for clouds that have no
//! OIDC federation (Hetzner, Modal).
//!
//! The device that is given the token seals it with the account's pool
//! data key of the current generation ([`PoolSealer`]: XChaCha20-Poly1305
//! under the key bundle that is HPKE-wrapped to each enrolled device), bound
//! to [`token_aad`] `{acct, conn_id, cloud, generation}`, and puts the
//! ciphertext on the account's connection on Cua's issuer. The issuer holds
//! no key that opens it.
//!
//! Any enrolled device opens it locally, in memory, when it calls the cloud
//! ([`SealedTokens`], the [`SecretSource`] of the clouds that take a token),
//! and keeps it in memory only, for a few minutes. Agents use it only
//! through the daemon's cloud tools; no tool returns it.
//!
//! When a device is revoked, the pool moves to a new key generation. The
//! next device that opens the token re-seals it under the new generation and
//! swaps the ciphertext on the issuer with compare-and-swap on the
//! generation it read, so the revoked device, which never gets the new key,
//! cannot open what is stored from then on.
//!
//! The pool keys are Cua Spaces' (`cua-spaces-cluster`): the Spaces build
//! of `cua` registers its [`PoolSealer`] with [`set_pool_sealer`]. A build
//! without one links these clouds with a token stored on the device only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_sandbox_core::Error;

use crate::api::{Result, Secret, SecretSource};
use crate::federation::{Expected, IssuerClient, SealedBlob, ServerConnection};
use crate::model::Auth;
use crate::state::Store;

/// The associated data's domain.
pub const AAD_DOMAIN: &str = "cua-cloud-token/v1";

/// How long an opened token is kept in memory.
pub const CACHE_FOR: Duration = Duration::from_secs(5 * 60);

/// The associated data a token is sealed with: the issuer account, the
/// connection, the cloud and the key generation, each length-prefixed after
/// [`AAD_DOMAIN`]. A ciphertext moved to another account, connection, cloud
/// or generation does not open.
pub fn token_aad(acct: &str, conn_id: &str, cloud: &str, generation: u64) -> Vec<u8> {
    let generation = generation.to_string();
    let mut out = Vec::with_capacity(96);
    for f in [AAD_DOMAIN, acct, conn_id, cloud, generation.as_str()] {
        out.extend_from_slice(&(f.len() as u32).to_be_bytes());
        out.extend_from_slice(f.as_bytes());
    }
    out
}

/// Why the pool keys could not seal or open a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SealerError {
    /// This device has no pool keys yet: one of `holders` (device names)
    /// must be online to share them.
    NotShared {
        /// The devices that hold the keys.
        holders: Vec<String>,
    },
    /// The relay that keeps the keyring could not be reached.
    Unavailable(String),
    /// It does not open: sealed for another account, connection, cloud or
    /// generation, changed, or under a generation this device never got.
    Refused(String),
}

impl std::fmt::Display for SealerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SealerError::NotShared { holders } if holders.is_empty() => {
                f.write_str("this device does not have the account's pool keys yet")
            }
            SealerError::NotShared { holders } => write!(
                f,
                "this device does not have the account's pool keys yet; they are on {}: open \
                 Cua on one of them and try again in a few seconds",
                holders.join(", ")
            ),
            SealerError::Unavailable(m) => write!(f, "the pool keys are out of reach ({m})"),
            SealerError::Refused(m) => write!(f, "the sealed token does not open here ({m})"),
        }
    }
}

impl std::error::Error for SealerError {}

/// The account's pool data keys on this device.
#[async_trait]
pub trait PoolSealer: Send + Sync + 'static {
    /// Seals `plain` under the newest generation, bound to `aad` of that
    /// generation.
    async fn seal(
        &self,
        plain: &[u8],
        aad: &(dyn Fn(u64) -> Vec<u8> + Send + Sync),
    ) -> std::result::Result<SealedBlob, SealerError>;

    /// Opens `blob` (sealed under `blob.generation`) bound to `aad`.
    async fn open(
        &self,
        blob: &SealedBlob,
        aad: &[u8],
    ) -> std::result::Result<zeroize::Zeroizing<Vec<u8>>, SealerError>;

    /// The newest generation (brought in step with the keyring first).
    async fn generation(&self) -> std::result::Result<u64, SealerError>;

    /// The devices that hold the keys now (names, else ids).
    async fn holders(&self) -> std::result::Result<Vec<String>, SealerError>;
}

static SEALER: RwLock<Option<Arc<dyn PoolSealer>>> = RwLock::new(None);

/// Registers this process's pool keys (the Spaces build of `cua` does).
pub fn set_pool_sealer(sealer: Option<Arc<dyn PoolSealer>>) {
    *SEALER.write().unwrap_or_else(|e| e.into_inner()) = sealer;
}

/// This process's pool keys, when it has any.
pub fn pool_sealer() -> Option<Arc<dyn PoolSealer>> {
    SEALER.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The issuer slot [`crate::Clouds`] shares.
pub(crate) type IssuerSlot = Arc<Mutex<Option<Arc<dyn IssuerClient>>>>;

/// The issuer of `slot`: the one set, else from the environment (the
/// `cua auth login` session and this device's key).
pub(crate) fn issuer_of(slot: &IssuerSlot) -> Option<Arc<dyn IssuerClient>> {
    let mut s = slot.lock().expect("issuer");
    if s.is_none() {
        *s =
            crate::federation::HttpIssuer::from_env().map(|i| Arc::new(i) as Arc<dyn IssuerClient>);
    }
    s.clone()
}

/// One opened token, in memory.
struct Cached {
    conn_id: String,
    token: Secret,
    at: Instant,
}

/// The tokens of this home's sealed connections: opened when a cloud call
/// needs one, kept in memory for [`CACHE_FOR`], re-sealed when the pool
/// moved to a newer key generation.
pub struct SealedTokens {
    store: Arc<Store>,
    issuer: IssuerSlot,
    sealer: Mutex<Option<Arc<dyn PoolSealer>>>,
    cache: Mutex<HashMap<String, Cached>>,
}

impl std::fmt::Debug for SealedTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cached: Vec<String> = self.cache.lock().expect("cache").keys().cloned().collect();
        f.debug_struct("SealedTokens")
            .field("cached", &cached)
            .finish_non_exhaustive()
    }
}

/// A plain refusal of a cloud call.
fn refused(provider: &str, why: impl std::fmt::Display) -> Error {
    Error::Cloud(format!("{provider}: the account's sealed token: {why}"))
}

impl SealedTokens {
    pub(crate) fn new(store: Arc<Store>, issuer: IssuerSlot) -> Self {
        SealedTokens {
            store,
            issuer,
            sealer: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Uses `sealer` instead of the process's ([`pool_sealer`]).
    pub(crate) fn set_sealer(&self, sealer: Arc<dyn PoolSealer>) {
        *self.sealer.lock().expect("sealer") = Some(sealer);
        self.cache.lock().expect("cache").clear();
    }

    /// The pool keys this home seals with.
    pub fn sealer(&self) -> Option<Arc<dyn PoolSealer>> {
        self.sealer
            .lock()
            .expect("sealer")
            .clone()
            .or_else(pool_sealer)
    }

    /// The issuer.
    pub(crate) fn issuer(&self) -> Option<Arc<dyn IssuerClient>> {
        issuer_of(&self.issuer)
    }

    /// Forgets `provider`'s opened token (a disconnect, a new token).
    pub fn forget(&self, provider: &str) {
        self.cache.lock().expect("cache").remove(provider);
    }

    fn sealed_conn(&self, provider: &str) -> Option<String> {
        match self.store.connection(provider).ok()??.auth {
            Auth::Sealed { conn_id } => Some(conn_id),
            _ => None,
        }
    }

    /// Opens `server`'s token (refused when it has none).
    pub async fn open_server(&self, server: &ServerConnection) -> Result<Secret> {
        let provider = server.cloud.as_str();
        let blob = server.sealed_token.as_ref().ok_or_else(|| {
            refused(
                provider,
                format!(
                    "none is stored yet; give it with `cua cloud connect {provider} \
                     --token-stdin`"
                ),
            )
        })?;
        let sealer = self.sealer().ok_or_else(|| {
            refused(
                provider,
                "this cua cannot open it (it has no pool keys: use the cua of the Cua Spaces app)",
            )
        })?;
        let aad = token_aad(&server.account, &server.id, provider, blob.generation);
        let plain = sealer
            .open(blob, &aad)
            .await
            .map_err(|e| refused(provider, e))?;
        let token = std::str::from_utf8(&plain)
            .map_err(|_| refused(provider, "it is not text"))?
            .trim()
            .to_string();
        Ok(Secret::new(token))
    }

    /// The account's connection `conn_id` on the issuer.
    async fn server(&self, provider: &str, conn_id: &str) -> Result<ServerConnection> {
        let issuer = self.issuer().ok_or_else(|| {
            refused(
                provider,
                "this device is not signed in to Cua (run `cua auth login`)",
            )
        })?;
        let all = issuer
            .connections()
            .await
            .map_err(|e| refused(provider, e))?;
        let s = all.into_iter().find(|c| c.id == conn_id).ok_or_else(|| {
            refused(
                provider,
                format!(
                    "the account's link is gone (disconnected on another device); link it \
                         again with `cua cloud connect {provider} --token-stdin`"
                ),
            )
        })?;
        match s.status.as_str() {
            "revoked" => Err(refused(
                provider,
                format!(
                    "{provider} was disconnected; link it again with `cua cloud connect \
                     {provider} --token-stdin`"
                ),
            )),
            "pending" => Err(refused(
                provider,
                "an agent put this token in, so a person approves it in the Cua Spaces app \
                 first (Computers, the cloud, Approve)",
            )),
            _ => Ok(s),
        }
    }

    /// Seals `token` for `server` under the newest generation and swaps it
    /// in, if `server` is still as read. `agent`: the agent that gives it
    /// (it then waits for a person's approval).
    pub async fn put(
        &self,
        server: &ServerConnection,
        token: &Secret,
        agent: Option<&str>,
    ) -> Result<ServerConnection> {
        let provider = server.cloud.as_str();
        let issuer = self.issuer().ok_or_else(|| {
            refused(
                provider,
                "this device is not signed in to Cua (run `cua auth login`)",
            )
        })?;
        let sealer = self.sealer().ok_or_else(|| {
            refused(
                provider,
                "this cua cannot seal it (it has no pool keys: use the cua of the Cua Spaces app)",
            )
        })?;
        let (acct, id) = (server.account.clone(), server.id.clone());
        let cloud = provider.to_string();
        let blob = sealer
            .seal(token.expose().as_bytes(), &move |g| {
                token_aad(&acct, &id, &cloud, g)
            })
            .await
            .map_err(|e| refused(provider, e))?;
        let expected = Expected {
            expected_version: Some(server.version),
            expected_generation: Some(server.sealed_token.as_ref().map_or(0, |b| b.generation)),
        };
        let out = issuer
            .set_sealed_token(&server.id, &blob, expected, agent)
            .await
            .map_err(|e| refused(provider, e))?;
        self.forget(provider);
        Ok(out)
    }

    /// Re-seals `server`'s token when the pool moved to a newer generation
    /// than it is sealed under (a device was revoked). Another device that
    /// did it first wins; this one then reads the new one next time.
    async fn reseal_if_stale(&self, server: &ServerConnection, token: &Secret) {
        let (Some(sealer), Some(blob)) = (self.sealer(), server.sealed_token.as_ref()) else {
            return;
        };
        let Ok(newest) = sealer.generation().await else {
            return;
        };
        if newest <= blob.generation {
            return;
        }
        match self.put(server, token, None).await {
            Ok(s) => tracing::info!(
                cloud = %server.cloud,
                connection = %server.id,
                generation = s.sealed_token.as_ref().map_or(0, |b| b.generation),
                "re-sealed the cloud token under the pool's new keys"
            ),
            Err(e) => tracing::info!(
                cloud = %server.cloud,
                connection = %server.id,
                "the cloud token was not re-sealed now ({})",
                e
            ),
        }
    }

    /// `provider`'s token, opened now or from memory, re-sealed first when
    /// the pool has newer keys.
    async fn open(&self, provider: &str, conn_id: &str) -> Result<Secret> {
        if let Some(c) = self.cache.lock().expect("cache").get(provider)
            && c.conn_id == conn_id
            && c.at.elapsed() < CACHE_FOR
        {
            return Ok(c.token.clone());
        }
        let server = self.server(provider, conn_id).await?;
        let token = self.open_server(&server).await?;
        self.reseal_if_stale(&server, &token).await;
        self.cache.lock().expect("cache").insert(
            provider.to_string(),
            Cached {
                conn_id: conn_id.to_string(),
                token: token.clone(),
                at: Instant::now(),
            },
        );
        Ok(token)
    }

    /// For `cloud_status`: re-seals `server`'s token when the pool has
    /// newer keys than it is sealed under (opening it to do so).
    pub(crate) async fn upkeep(&self, server: &ServerConnection) {
        let (Some(sealer), Some(blob)) = (self.sealer(), server.sealed_token.as_ref()) else {
            return;
        };
        if sealer.generation().await.is_ok_and(|g| g > blob.generation)
            && let Ok(token) = self.open_server(server).await
        {
            self.reseal_if_stale(server, &token).await;
            self.forget(&server.cloud);
        }
    }
}

#[async_trait]
impl SecretSource for SealedTokens {
    async fn secret(&self, provider: &str) -> Option<Result<Secret>> {
        let conn_id = self.sealed_conn(provider)?;
        Some(self.open(provider, &conn_id).await)
    }

    fn sealed(&self, provider: &str) -> bool {
        self.sealed_conn(provider).is_some()
    }
}

impl From<SealerError> for Error {
    fn from(e: SealerError) -> Self {
        Error::Cloud(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_associated_data_names_account_connection_cloud_and_generation() {
        let a = token_aad("acct1", "conn_1", "hetzner", 3);
        assert_ne!(a, token_aad("acct2", "conn_1", "hetzner", 3));
        assert_ne!(a, token_aad("acct1", "conn_2", "hetzner", 3));
        assert_ne!(a, token_aad("acct1", "conn_1", "modal", 3));
        assert_ne!(a, token_aad("acct1", "conn_1", "hetzner", 4));
        // Length-prefixed: fields cannot run into each other.
        assert_ne!(
            token_aad("ab", "c", "hetzner", 1),
            token_aad("a", "bc", "hetzner", 1)
        );
        assert!(
            a.windows(AAD_DOMAIN.len())
                .any(|w| w == AAD_DOMAIN.as_bytes())
        );
    }

    #[test]
    fn secrets_and_blobs_never_print() {
        let s = Secret::new("hcloud-SECRET-123");
        assert_eq!(format!("{s:?}"), "Secret(redacted)");
        let b = SealedBlob {
            ciphertext: b"opaque-but-private".to_vec(),
            generation: 2,
        };
        let d = format!("{b:?}");
        assert!(!d.contains("opaque"), "{d}");
        assert!(d.contains("generation: 2"), "{d}");
    }
}
