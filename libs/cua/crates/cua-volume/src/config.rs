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
//!
//! `s3` needs a build with the `s3` feature (the `cua` CLI and daemon). The
//! Cua cloud backend (`cloud`) has ended: a config or environment that names
//! it fails with [`CLOUD_BACKEND_ENDED`], and the drive is served as
//! unavailable until it names `fs` or `s3`.
//!
//! ```json
//! {"backend": "s3", "s3": {"endpoint": "http://127.0.0.1:9000", "region": "us-east-1",
//!  "bucket": "cua-volume", "root": "", "path_style": true}}
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Environment variable naming the backend (`fs`, `s3`).
pub const BACKEND_ENV: &str = "CUA_DRIVE_BACKEND";
/// The credential-store secret holding static S3 keys
/// (`{"access_key_id": "...", "secret_access_key": "..."}`).
pub const S3_SECRET_NAME: &str = "drive-keys";

/// What a config or environment that names the removed `cloud` backend gets.
pub const CLOUD_BACKEND_ENDED: &str = "the Cua cloud drive backend has ended; use fs or s3";

/// The backend kinds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    #[default]
    Fs,
    S3,
}

impl BackendKind {
    pub fn parse(s: &str) -> Result<BackendKind> {
        match s.trim() {
            "fs" | "local" => Ok(BackendKind::Fs),
            "s3" => Ok(BackendKind::S3),
            "cloud" => Err(Error::Invalid(CLOUD_BACKEND_ENDED.into())),
            other => Err(Error::Invalid(format!(
                "drive backend {other:?}: use fs or s3"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Fs => "fs",
            BackendKind::S3 => "s3",
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
}

impl DriveConfig {
    /// The config file of a cua home.
    pub fn path(cua_home: &Path) -> PathBuf {
        crate::state_dir(cua_home).join("config.json")
    }

    /// Reads the file (default when absent), then applies the environment.
    pub fn load(cua_home: &Path) -> Result<DriveConfig> {
        let mut c = match std::fs::read(Self::path(cua_home)) {
            Ok(b) => Self::parse(&b)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DriveConfig::default(),
            Err(e) => return Err(e.into()),
        };
        c.apply_env(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))?;
        Ok(c)
    }

    /// Parses the file's bytes. A file that names the removed `cloud`
    /// backend fails with [`CLOUD_BACKEND_ENDED`].
    fn parse(bytes: &[u8]) -> Result<DriveConfig> {
        let bad = |e: serde_json::Error| Error::Invalid(format!("drive/config.json: {e}"));
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(bad)?;
        if value.get("backend").and_then(|b| b.as_str()) == Some("cloud") {
            return Err(Error::Invalid(CLOUD_BACKEND_ENDED.into()));
        }
        serde_json::from_value(value).map_err(bad)
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
    }

    #[test]
    fn the_ended_cloud_backend_is_refused_with_its_own_message() {
        let ended = Error::Invalid(CLOUD_BACKEND_ENDED.into());
        assert_eq!(BackendKind::parse("cloud").unwrap_err(), ended);
        let mut c = DriveConfig::default();
        assert_eq!(
            c.apply_env(|k| (k == BACKEND_ENV).then(|| "cloud".into()))
                .unwrap_err(),
            ended
        );
        assert_eq!(
            DriveConfig::parse(br#"{"backend":"cloud","cloud_ttl_secs":900}"#).unwrap_err(),
            ended
        );
        // An old file's unrelated keys do not matter.
        assert_eq!(
            DriveConfig::parse(br#"{"backend":"fs","cloud_ttl_secs":900}"#)
                .unwrap()
                .backend,
            BackendKind::Fs
        );
    }
}
