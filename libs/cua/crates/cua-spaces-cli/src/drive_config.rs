// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua volume config` in the Cua Spaces build: the Cua Volume's backend
//! (local, S3-compatible, the Cua cloud) and its S3 keys.

use std::io::{Read as _, Write};

use cua_cli::drive_cmd::ConfigCmd;
use cua_cli::util::line;
use cua_sdk::CuaError;

/// `cua volume config`.
pub fn config(cmd: ConfigCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    use cua_volume::config::{BackendKind, DriveConfig, S3Settings};
    let home = cua_cli::util::cua_home();
    let err = |e: cua_volume::Error| CuaError::InvalidArgument(e.to_string());
    match cmd {
        ConfigCmd::Show => {
            let c = DriveConfig::load(&home).map_err(err)?;
            let keys = cua_auth::Store::from_env()
                .load_secret(cua_volume::config::S3_SECRET_NAME)
                .ok()
                .flatten()
                .is_some()
                || std::env::var_os("CUA_DRIVE_S3_ACCESS_KEY_ID").is_some();
            if json {
                let mut v = serde_json::to_value(&c).unwrap_or_default();
                v["s3_keys_saved"] = keys.into();
                v["config_file"] = DriveConfig::path(&home).display().to_string().into();
                line(out, v.to_string());
            } else {
                line(out, format!("backend: {}", c.backend.as_str()));
                match c.backend {
                    BackendKind::Fs => {
                        line(out, format!("store: {}", home.join("drive/data").display()))
                    }
                    _ => {
                        if let Some(s) = &c.s3 {
                            line(
                                out,
                                format!(
                                    "bucket: {} ({})",
                                    s.bucket,
                                    s.endpoint.as_deref().unwrap_or("AWS")
                                ),
                            );
                        }
                        if c.backend == BackendKind::S3 {
                            line(
                                out,
                                format!(
                                    "keys: {}",
                                    if keys {
                                        "saved"
                                    } else {
                                        "missing (cua volume config set-keys)"
                                    }
                                ),
                            );
                        } else {
                            line(
                                out,
                                "keys: vended by the Cua cloud for the signed-in account (off until the cloud enables Cua Volume)",
                            );
                        }
                    }
                }
            }
            Ok(0)
        }
        ConfigCmd::Set {
            backend,
            endpoint,
            region,
            bucket,
            root,
            path_style,
        } => {
            let mut c = DriveConfig::load(&home).unwrap_or_default();
            c.backend = BackendKind::parse(&backend).map_err(err)?;
            if c.backend == BackendKind::S3 {
                let mut s = c.s3.clone().unwrap_or(S3Settings {
                    region: "us-east-1".into(),
                    ..Default::default()
                });
                if endpoint.is_some() {
                    s.endpoint = endpoint;
                }
                if let Some(r) = region {
                    s.region = r;
                }
                if let Some(b) = bucket {
                    s.bucket = b;
                }
                if let Some(r) = root {
                    s.root = r;
                }
                s.path_style = path_style || s.path_style;
                if s.bucket.is_empty() {
                    return Err(CuaError::InvalidArgument(
                        "--backend s3 needs --bucket".into(),
                    ));
                }
                c.s3 = Some(s);
            }
            c.save(&home).map_err(err)?;
            if json {
                line(out, serde_json::to_string(&c).unwrap_or_default());
            } else {
                line(
                    out,
                    format!(
                        "drive backend set to {}; restart the daemon (cua daemon restart) to use it",
                        c.backend.as_str()
                    ),
                );
            }
            Ok(0)
        }
        ConfigCmd::SetKeys => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| CuaError::Internal(format!("stdin: {e}")))?;
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
            let (Some(id), Some(secret)) = (lines.next(), lines.next()) else {
                return Err(CuaError::InvalidArgument(
                    "pipe the access key id and the secret, one per line".into(),
                ));
            };
            let body = serde_json::json!({"access_key_id": id, "secret_access_key": secret});
            cua_auth::Store::from_env()
                .save_secret(
                    cua_volume::config::S3_SECRET_NAME,
                    body.to_string().as_bytes(),
                )
                .map_err(|e| CuaError::Internal(format!("credential store: {e}")))?;
            line(
                out,
                if json {
                    "{\"saved\":true}"
                } else {
                    "saved the drive's S3 keys in the credential store"
                },
            );
            Ok(0)
        }
    }
}
