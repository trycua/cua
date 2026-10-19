// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which backend a cua home's drive uses.
//!
//! `$CUA_HOME/volume/config.json`, overridden by the environment:
//!
//! | backend | where the bytes live | keys |
//! |---|---|---|
//! | `fs` (default) | `$CUA_HOME/volume/data` | none |
//! | `s3` | a versioned bucket (AWS S3, R2, MinIO, BYO) | the credential store's `drive-keys` secret, else `CUA_DRIVE_S3_ACCESS_KEY_ID` / `CUA_DRIVE_S3_SECRET_ACCESS_KEY` |
//! | `cloud` | the Cua cloud bucket | short-lived keys vended by `POST /api/drive/credentials` for the signed-in account |
//!
//! `cloud` is off by default and needs both this setting and the server
//! side's flag; the server answers `drive_disabled` until then. `s3` and
//! `cloud` need a build with the `s3` feature (the `cua` CLI and daemon).
//!
//! ```json
//! {"backend": "s3", "s3": {"endpoint": "http://127.0.0.1:9000", "region": "us-east-1",
//!  "bucket": "cua-volume", "root": "", "path_style": true}}
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Environment variable naming the backend (`fs`, `s3`, `cloud`).
pub const BACKEND_ENV: &str = "CUA_DRIVE_BACKEND";
/// The credential-store secret holding static S3 keys
/// (`{"access_key_id": "...", "secret_access_key": "..."}`).
pub const S3_SECRET_NAME: &str = "drive-keys";

/// The backend kinds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    #[default]
    Fs,
    S3,
    Cloud,
}

impl BackendKind {
    pub fn parse(s: &str) -> Result<BackendKind> {
        match s.trim() {
            "fs" | "local" => Ok(BackendKind::Fs),
            "s3" => Ok(BackendKind::S3),
            "cloud" => Ok(BackendKind::Cloud),
            other => Err(Error::Invalid(format!(
                "drive backend {other:?}: use fs, s3 or cloud"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Fs => "fs",
            BackendKind::S3 => "s3",
            BackendKind::Cloud => "cloud",
        }
    }
}

/// Where an S3-compatible bucket is (no keys: those never go in the file).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default = "default_region")]
    pub region: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub path_style: bool,
}

fn default_region() -> String {
    "us-east-1".into()
}

/// `$CUA_HOME/volume/config.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveConfig {
    #[serde(default)]
    pub backend: BackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Settings>,
    /// Seconds a vended cloud key lives (15 to 60 minutes; default 900).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_ttl_secs: Option<u64>,
}

impl DriveConfig {
    /// The config file of a cua home.
    pub fn path(cua_home: &Path) -> PathBuf {
        crate::state_dir(cua_home).join("config.json")
    }

    /// Reads the file (default when absent), then applies the environment.
    pub fn load(cua_home: &Path) -> Result<DriveConfig> {
        let mut c = match std::fs::read(Self::path(cua_home)) {
            Ok(b) => serde_json::from_slice(&b)
                .map_err(|e| Error::Invalid(format!("drive/config.json: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DriveConfig::default(),
            Err(e) => return Err(e.into()),
        };
        c.apply_env(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))?;
        Ok(c)
    }

    /// Applies `CUA_DRIVE_*` overrides read through `get`.
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<()> {
        if let Some(b) = get(BACKEND_ENV) {
            self.backend = BackendKind::parse(&b)?;
        }
        let mut s3 = self.s3.clone().unwrap_or_else(|| S3Settings {
            region: default_region(),
            ..Default::default()
        });
        let mut touched = false;
        if let Some(v) = get("CUA_DRIVE_S3_ENDPOINT") {
            s3.endpoint = Some(v);
            touched = true;
        }
        if let Some(v) = get("CUA_DRIVE_S3_REGION") {
            s3.region = v;
            touched = true;
        }
        if let Some(v) = get("CUA_DRIVE_S3_BUCKET") {
            s3.bucket = v;
            touched = true;
        }
        if let Some(v) = get("CUA_DRIVE_S3_ROOT") {
            s3.root = v;
            touched = true;
        }
        if let Some(v) = get("CUA_DRIVE_S3_PATH_STYLE") {
            s3.path_style = matches!(v.as_str(), "1" | "true" | "yes");
            touched = true;
        }
        if touched {
            self.s3 = Some(s3);
        }
        Ok(())
    }

    /// The vended key lifetime, clamped to 15-60 minutes.
    pub fn cloud_ttl_secs(&self) -> u64 {
        self.cloud_ttl_secs.unwrap_or(900).clamp(900, 3600)
    }

    /// Writes the file.
    pub fn save(&self, cua_home: &Path) -> Result<()> {
        let p = Self::path(cua_home);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        cua_home::guard_write(&p)?;
        std::fs::write(p, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_file_and_env() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = DriveConfig::default();
        assert_eq!(c.backend, BackendKind::Fs);
        c.apply_env(|_| None).unwrap();
        assert_eq!(c, DriveConfig::default());
        let env = |k: &str| match k {
            "CUA_DRIVE_BACKEND" => Some("s3".to_string()),
            "CUA_DRIVE_S3_ENDPOINT" => Some("http://127.0.0.1:9000".into()),
            "CUA_DRIVE_S3_BUCKET" => Some("b".into()),
            "CUA_DRIVE_S3_PATH_STYLE" => Some("1".into()),
            _ => None,
        };
        c.apply_env(env).unwrap();
        assert_eq!(c.backend, BackendKind::S3);
        let s3 = c.s3.clone().unwrap();
        assert!(s3.path_style && s3.bucket == "b" && s3.region == "us-east-1");
        c.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(DriveConfig::path(dir.path())).unwrap();
        assert!(!text.contains("secret"), "no keys in the file: {text}");
        let mut back: DriveConfig = serde_json::from_str(&text).unwrap();
        back.apply_env(|_| None).unwrap();
        assert_eq!(back, c);
        assert!(
            c.apply_env(|k| (k == BACKEND_ENV).then(|| "ftp".into()))
                .is_err()
        );
        assert_eq!(
            DriveConfig {
                cloud_ttl_secs: Some(10),
                ..c.clone()
            }
            .cloud_ttl_secs(),
            900
        );
    }
}
