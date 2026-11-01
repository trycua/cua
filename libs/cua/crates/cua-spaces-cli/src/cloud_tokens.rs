// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account's pool keys ([`cua_spaces_cluster::pool::keys`]) as the
//! sealer of cloud API tokens ([`cua_byoc::sealed::PoolSealer`]): a Hetzner
//! or Modal token is sealed with the data key of the pool's newest
//! generation (XChaCha20-Poly1305, under the bundle HPKE-wrapped to each
//! enrolled device) and opened by any device that holds the keys. A device
//! that was revoked never gets a newer generation, so it cannot open a token
//! re-sealed after it left.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use cua_byoc::federation::SealedBlob;
use cua_byoc::sealed::{PoolSealer, SealerError};
use cua_spaces_cluster::pool::keys::{KeyError, PoolKeys};
use cua_spaces_cluster::pool::seal::{Envelope, SealError};

/// How the sealer finds this device's pool keys (per call: the signed-in
/// account may change).
pub type KeysFn = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Arc<PoolKeys>, SealerError>> + Send>>
        + Send
        + Sync,
>;

/// [`PoolSealer`] over this device's [`PoolKeys`].
pub struct PoolKeysSealer {
    keys: KeysFn,
}

impl PoolKeysSealer {
    /// The keys `keys` finds.
    pub fn new(keys: KeysFn) -> Self {
        PoolKeysSealer { keys }
    }

    /// Always `keys` (tests).
    pub fn fixed(keys: Arc<PoolKeys>) -> Self {
        Self::new(Arc::new(move || {
            let k = keys.clone();
            Box::pin(async move { Ok(k) })
        }))
    }
}

fn key_error(e: KeyError) -> SealerError {
    match e {
        KeyError::NotShared { holders } => SealerError::NotShared {
            holders: holders.iter().map(|h| h.label().to_string()).collect(),
        },
        KeyError::Seal(SealError::UnknownGeneration(g)) => SealerError::Refused(format!(
            "it is sealed under key generation {g}, which this device does not have"
        )),
        KeyError::Seal(e) => SealerError::Refused(e.to_string()),
        KeyError::Unavailable(m) | KeyError::Refused(m) => SealerError::Unavailable(m),
        KeyError::Conflict => SealerError::Unavailable("the pool keyring kept changing".into()),
    }
}

#[async_trait]
impl PoolSealer for PoolKeysSealer {
    async fn seal(
        &self,
        plain: &[u8],
        aad: &(dyn Fn(u64) -> Vec<u8> + Send + Sync),
    ) -> Result<SealedBlob, SealerError> {
        let keys = (self.keys)().await?;
        // In step with the relay first: never seal under a generation a
        // revoked device still has.
        let bundle = keys.sync().await.map_err(key_error)?;
        let generation = bundle.generation();
        let env = bundle.seal(&aad(generation), plain);
        Ok(SealedBlob {
            ciphertext: env.to_bytes(),
            generation,
        })
    }

    async fn open(
        &self,
        blob: &SealedBlob,
        aad: &[u8],
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, SealerError> {
        let env = Envelope::from_bytes(&blob.ciphertext)
            .ok_or_else(|| SealerError::Refused("it is not sealed pool data".into()))?;
        if env.g != blob.generation {
            return Err(SealerError::Refused(
                "its generation does not match the one recorded".into(),
            ));
        }
        let keys = (self.keys)().await?;
        keys.open(aad, &env)
            .await
            .map(zeroize::Zeroizing::new)
            .map_err(key_error)
    }

    async fn generation(&self) -> Result<u64, SealerError> {
        let keys = (self.keys)().await?;
        Ok(keys.sync().await.map_err(key_error)?.generation())
    }

    async fn holders(&self) -> Result<Vec<String>, SealerError> {
        let keys = (self.keys)().await?;
        if keys.cached().is_none() {
            keys.sync().await.map_err(key_error)?;
        }
        Ok(keys
            .view()
            .holders
            .iter()
            .map(|h| h.label().to_string())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_byoc::sealed::token_aad;
    use cua_spaces_cluster::pool::memlog::MemPool;
    use cua_spaces_cluster::pool::seal::PoolSecret;

    const ACCOUNT: &str = "acct";

    struct Device {
        id: String,
        keys: Arc<PoolKeys>,
        sealer: PoolKeysSealer,
    }

    fn device(pool: &MemPool) -> Device {
        let (key, _) = cua_host::DeviceKey::generate().unwrap();
        let id = key.id();
        let keys = PoolKeys::new(
            ACCOUNT,
            Arc::new(crate::pool_ext::DeviceKeySigner(key)),
            PoolSecret::generate(),
            Arc::new(pool.keys(ACCOUNT)),
        );
        Device {
            id,
            sealer: PoolKeysSealer::fixed(keys.clone()),
            keys,
        }
    }

    fn aad_for(conn: &'static str) -> impl Fn(u64) -> Vec<u8> + Send + Sync {
        move |g| token_aad("a1b2c3d4e5f6", conn, "hetzner", g)
    }

    #[tokio::test]
    async fn a_token_opens_on_every_holder_and_only_with_its_own_associated_data() {
        let pool = MemPool::new();
        let (laptop, desktop) = (device(&pool), device(&pool));
        laptop.keys.sync().await.unwrap();
        desktop.keys.sync().await.unwrap_err();
        // The laptop shares the keys with the desktop that registered.
        laptop.keys.sync().await.unwrap();
        desktop.keys.sync().await.unwrap();

        let blob = laptop
            .sealer
            .seal(b"hcloud-token-123", &aad_for("conn_1"))
            .await
            .unwrap();
        assert_eq!(blob.generation, 1);
        assert!(
            !blob
                .ciphertext
                .windows(b"hcloud".len())
                .any(|w| w == b"hcloud"),
            "the ciphertext carries no plain token"
        );
        let opened = desktop
            .sealer
            .open(&blob, &token_aad("a1b2c3d4e5f6", "conn_1", "hetzner", 1))
            .await
            .unwrap();
        assert_eq!(opened.as_slice(), b"hcloud-token-123");

        // Another account, connection, cloud or generation: refused.
        for aad in [
            token_aad("zzzzzzzzzzzz", "conn_1", "hetzner", 1),
            token_aad("a1b2c3d4e5f6", "conn_2", "hetzner", 1),
            token_aad("a1b2c3d4e5f6", "conn_1", "modal", 1),
            token_aad("a1b2c3d4e5f6", "conn_1", "hetzner", 2),
        ] {
            assert!(matches!(
                desktop.sealer.open(&blob, &aad).await,
                Err(SealerError::Refused(_))
            ));
        }
        // A blob whose recorded generation was changed: refused.
        let mut moved = blob.clone();
        moved.generation = 2;
        assert!(matches!(
            desktop
                .sealer
                .open(&moved, &token_aad("a1b2c3d4e5f6", "conn_1", "hetzner", 2))
                .await,
            Err(SealerError::Refused(_))
        ));
        let mut names = laptop.sealer.holders().await.unwrap();
        names.sort();
        let mut want = vec![laptop.id.clone(), desktop.id.clone()];
        want.sort();
        assert_eq!(names, want);
    }

    #[tokio::test]
    async fn a_revoked_device_cannot_open_the_token_sealed_after_it_left() {
        let pool = MemPool::new();
        let (laptop, stolen) = (device(&pool), device(&pool));
        laptop.keys.sync().await.unwrap();
        stolen.keys.sync().await.unwrap_err();
        laptop.keys.sync().await.unwrap();
        stolen.keys.sync().await.unwrap();
        let old = laptop
            .sealer
            .seal(b"hcloud-token-123", &aad_for("conn_1"))
            .await
            .unwrap();
        assert_eq!(old.generation, 1);

        // The stolen device is revoked: the next sync makes generation 2
        // for the devices still there, and the token is sealed under it.
        pool.revoke(&stolen.id);
        assert_eq!(laptop.sealer.generation().await.unwrap(), 2);
        let new = laptop
            .sealer
            .seal(b"hcloud-token-123", &aad_for("conn_1"))
            .await
            .unwrap();
        assert_eq!(new.generation, 2);
        assert_eq!(
            laptop
                .sealer
                .open(&new, &token_aad("a1b2c3d4e5f6", "conn_1", "hetzner", 2))
                .await
                .unwrap()
                .as_slice(),
            b"hcloud-token-123"
        );
        let r = stolen
            .sealer
            .open(&new, &token_aad("a1b2c3d4e5f6", "conn_1", "hetzner", 2))
            .await;
        assert!(r.is_err(), "the revoked device opened generation 2");
        assert_eq!(laptop.sealer.holders().await.unwrap(), vec![laptop.id]);
    }

    #[tokio::test]
    async fn a_device_without_the_keys_says_who_has_them() {
        let pool = MemPool::new();
        let (laptop, fresh) = (device(&pool), device(&pool));
        laptop.keys.sync().await.unwrap();
        match fresh.sealer.generation().await {
            Err(SealerError::NotShared { holders }) => assert_eq!(holders, vec![laptop.id]),
            other => panic!("{other:?}"),
        }
    }
}
