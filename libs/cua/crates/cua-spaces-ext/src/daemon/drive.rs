// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which Cua Volume the runtime serves, and the user presence its grants
//! ask for.
//!
//! The backend comes from `$CUA_HOME/volume/config.json` and `CUA_DRIVE_*`
//! ([`cua_volume::config`]): the local store by default; an S3-compatible
//! bucket with keys from the credential store (`drive-keys`) or the
//! environment; or, only when configured, the Cua cloud's bucket with keys
//! vended for the signed-in account. Nothing here does network I/O or reads
//! a key at startup: S3 keys and cloud vends are fetched on first use.
//!
//! A backend this build cannot serve (`s3` without the `s3` feature of cua-spaces-ext)
//! is served as [`cua_volume::backend::Unavailable`], so every call names
//! the problem instead of quietly writing somewhere else.

use std::path::Path;
use std::sync::Arc;

use cua_volume::config::{BackendKind, DriveConfig};
use cua_volume::{Drive, Presence};

/// The Keyvault's user presence (Touch ID, the login password or the vault
/// passphrase) as the drive's: grants and approvals ask the user the same
/// way a Keyvault consent does.
pub struct KeyvaultPresence(pub Arc<dyn cua_keyvault::UserPresence>);

impl Presence for KeyvaultPresence {
    fn confirm(&self, reason: &str) -> Result<(), String> {
        self.0.confirm(reason).map_err(|e| e.to_string())
    }
}

#[cfg(feature = "s3")]
mod s3 {
    use cua_volume::s3::{CredentialSource, S3Credentials, TokenSource};
    use cua_volume::{Error, Result};

    /// Static keys read on first use from the credential store (`drive-keys`,
    /// JSON `{"access_key_id", "secret_access_key"}`), else from
    /// `CUA_DRIVE_S3_ACCESS_KEY_ID` / `CUA_DRIVE_S3_SECRET_ACCESS_KEY`.
    pub struct StoreKeys;

    #[async_trait::async_trait]
    impl CredentialSource for StoreKeys {
        async fn credentials(&self) -> Result<S3Credentials> {
            let stored = tokio::task::spawn_blocking(|| {
                cua_auth::Store::from_env().load_secret(cua_volume::config::S3_SECRET_NAME)
            })
            .await
            .map_err(|e| Error::Backend(e.to_string()))?
            .map_err(|e| Error::Backend(format!("credential store: {e}")))?;
            if let Some(bytes) = stored {
                return serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Backend(format!("drive-keys secret: {e}")));
            }
            let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
            match (
                get("CUA_DRIVE_S3_ACCESS_KEY_ID"),
                get("CUA_DRIVE_S3_SECRET_ACCESS_KEY"),
            ) {
                (Some(a), Some(s)) => Ok(S3Credentials {
                    access_key_id: a,
                    secret_access_key: s,
                    session_token: get("CUA_DRIVE_S3_SESSION_TOKEN"),
                    expires_ms: None,
                }),
                _ => Err(Error::Forbidden(
                    "no S3 keys: save them with `cua volume config set-keys` or set \
                     CUA_DRIVE_S3_ACCESS_KEY_ID and CUA_DRIVE_S3_SECRET_ACCESS_KEY"
                        .into(),
                )),
            }
        }
    }

    /// The credential store's `drive-keys` secret as the drive service's
    /// key store (Settings > Storage saves keys here).
    pub struct CredentialStoreKeys;

    impl cua_volume::service::KeyStore for CredentialStoreKeys {
        fn load(&self) -> Result<Option<(String, String)>> {
            if let Some(bytes) = cua_auth::Store::from_env()
                .load_secret(cua_volume::config::S3_SECRET_NAME)
                .map_err(|e| Error::Backend(format!("credential store: {e}")))?
            {
                let c: S3Credentials = serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Backend(format!("drive-keys secret: {e}")))?;
                return Ok(Some((c.access_key_id, c.secret_access_key)));
            }
            cua_volume::service::EnvKeys.load()
        }

        fn save(&self, access_key_id: &str, secret_access_key: &str) -> Result<()> {
            let body = serde_json::json!({
                "access_key_id": access_key_id,
                "secret_access_key": secret_access_key,
            });
            cua_auth::Store::from_env()
                .save_secret(
                    cua_volume::config::S3_SECRET_NAME,
                    body.to_string().as_bytes(),
                )
                .map_err(|e| Error::Backend(format!("credential store: {e}")))
        }
    }

    /// The Fleet client's bearer (the signed-in account, or client
    /// credentials).
    pub struct FleetTokens(pub cua_fleet::FleetClient);

    #[async_trait::async_trait]
    impl TokenSource for FleetTokens {
        async fn token(&self) -> Result<String> {
            self.0
                .access_token(false)
                .await
                .map_err(|e| Error::Forbidden(format!("Cua cloud sign-in: {e}")))
        }
    }
}

/// The backend `config` names, for a runtime whose Fleet client is `fleet`.
pub fn backend(
    config: &DriveConfig,
    home: &Path,
    fleet: Option<&cua_fleet::FleetClient>,
) -> Arc<dyn cua_volume::Backend> {
    let data = cua_volume::state_dir(home).join("data");
    match config.backend {
        BackendKind::Fs => Arc::new(cua_volume::fs::FsBackend::new(data)),
        #[cfg(feature = "s3")]
        BackendKind::S3 => {
            use cua_volume::s3::{S3Backend, S3Config};
            let Some(s) = config.s3.clone().filter(|s| !s.bucket.is_empty()) else {
                return Arc::new(cua_volume::backend::Unavailable(
                    "the drive is set to s3 but no bucket is configured (drive/config.json `s3.bucket` or CUA_DRIVE_S3_BUCKET)".into(),
                ));
            };
            let cfg = S3Config {
                endpoint: s.endpoint,
                region: s.region,
                bucket: s.bucket,
                root: s.root,
                path_style: s.path_style,
            };
            match S3Backend::new(cfg, Arc::new(s3::StoreKeys)) {
                Ok(b) => Arc::new(b),
                Err(e) => Arc::new(cua_volume::backend::Unavailable(e.to_string())),
            }
        }
        #[cfg(feature = "s3")]
        BackendKind::Cloud => {
            use cua_volume::s3::{CloudBackend, CloudVendor, VendRequest};
            let Some(fleet) = fleet else {
                return Arc::new(cua_volume::backend::Unavailable(
                    "the drive is set to cloud but this runtime is not signed in to Cua (`cua auth login`)".into(),
                ));
            };
            let vendor = CloudVendor::new(
                &fleet.config().base_url,
                Arc::new(s3::FleetTokens(fleet.clone())),
                VendRequest {
                    principal: "user".into(),
                    space: None,
                    ttl_secs: config.cloud_ttl_secs(),
                },
            );
            match vendor {
                Ok(v) => Arc::new(CloudBackend::new(Arc::new(v))),
                Err(e) => Arc::new(cua_volume::backend::Unavailable(e.to_string())),
            }
        }
        #[cfg(not(feature = "s3"))]
        other => {
            let _ = fleet;
            Arc::new(cua_volume::backend::Unavailable(format!(
                "the drive is set to {} but this build has no S3 support; use the `cua` daemon",
                other.as_str()
            )))
        }
    }
}

/// Where the drive service keeps S3 keys: the credential store when this
/// build has S3 support, else the environment only.
pub fn keys() -> Arc<dyn cua_volume::service::KeyStore> {
    #[cfg(feature = "s3")]
    {
        Arc::new(s3::CredentialStoreKeys)
    }
    #[cfg(not(feature = "s3"))]
    {
        Arc::new(cua_volume::service::EnvKeys)
    }
}

/// The drive of the cua home `home`: the configured backend, grants and
/// audit under `<home>/volume`, and `presence` for widening access.
pub fn open(
    home: &Path,
    fleet: Option<&cua_fleet::FleetClient>,
    presence: Arc<dyn Presence>,
) -> Drive {
    let backend: Arc<dyn cua_volume::Backend> = match DriveConfig::load(home) {
        Ok(config) => backend(&config, home, fleet),
        Err(e) => {
            tracing::warn!(error = %e, "drive: config unreadable; the drive refuses every call");
            Arc::new(cua_volume::backend::Unavailable(format!(
                "drive/config.json: {e}"
            )))
        }
    };
    Drive::new(backend, cua_volume::state_dir(home)).with_presence(presence)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Yes;
    impl Presence for Yes {
        fn confirm(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn the_local_store_is_the_default_and_bad_config_is_loud() {
        let home = tempfile::tempdir().unwrap();
        let d = open(home.path(), None, Arc::new(Yes));
        assert_eq!(d.backend().kind(), "fs");
        std::fs::create_dir_all(home.path().join("volume")).unwrap();
        std::fs::write(home.path().join("volume/config.json"), "{not json").unwrap();
        let d = open(home.path(), None, Arc::new(Yes));
        assert_eq!(d.backend().kind(), "unavailable");
        std::fs::write(
            home.path().join("volume/config.json"),
            r#"{"backend":"cloud"}"#,
        )
        .unwrap();
        let d = open(home.path(), None, Arc::new(Yes));
        assert_eq!(d.backend().kind(), "unavailable", "cloud needs a sign-in");
    }

    /// Opt-in (run by `crates/cua-volume/tests/run-minio.sh`, which sets a
    /// temporary CUA_HOME and the file credential store): the s3 config
    /// with keys from the environment reaches a real bucket.
    #[cfg(feature = "s3")]
    #[tokio::test]
    async fn the_s3_config_reaches_the_bucket() {
        let Ok(endpoint) = std::env::var("CUA_DRIVE_S3_TEST_ENDPOINT") else {
            eprintln!("skipped: set CUA_DRIVE_S3_TEST_ENDPOINT (run-minio.sh)");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let config = DriveConfig {
            backend: BackendKind::S3,
            s3: Some(cua_volume::config::S3Settings {
                endpoint: Some(endpoint),
                region: "us-east-1".into(),
                bucket: "cua-volume-test".into(),
                root: format!("daemon-{:08x}/", rand::random::<u32>()),
                path_style: true,
            }),
            cloud_ttl_secs: None,
        };
        let b = backend(&config, home.path(), None);
        assert_eq!(b.kind(), "s3");
        let d = Drive::new(b, home.path().join("volume")).with_presence(Arc::new(Yes));
        let s = d.session(cua_volume::Context::agent("ada", None));
        s.write(
            "agents/ada/m.md",
            b"hi".to_vec(),
            cua_volume::Condition::None,
        )
        .await
        .unwrap();
        assert_eq!(s.read("agents/ada/m.md", None).await.unwrap().0, b"hi");
    }
}
