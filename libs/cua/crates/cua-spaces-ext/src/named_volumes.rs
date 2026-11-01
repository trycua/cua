// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Named volumes of the Cua Volume: the folders `volumes/<name>/` that
//! Spaces mount ([`cua_spaces::named_volumes`]).
//!
//! In-process operations of the [`crate::DriveExtension`] (not MCP tools:
//! an agent mounts what its Space was created with and cannot make or drop
//! volumes). They act as the user, who owns the whole Cua Volume.
//!
//! | Operation | Arguments | Result |
//! |---|---|---|
//! | `volumes.list` | | `{volumes: [{name, mounted_in}]}` |
//! | `volumes.create` | `name` | `{name}`; refused when it exists |
//! | `volumes.delete` | `name`, `force` | `{deleted, files}`; refused while mounted unless forced |
//! | `volumes.exists` | `names` | `{}`; `not_found` names the first missing volume |
//! | `volume.attach` | `space` | the volume mounted in `space`, with its binds |

use cua_spaces::extension::BoxFuture;
use cua_spaces::mcp::ToolOutcome;
use cua_spaces::{Error, Result, Spaces};
use cua_volume::{Context, path};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{DriveResult as _, SpacesDrive as _};

/// The operations this module serves.
pub const OPS: &[&str] = &[
    "volumes.list",
    "volumes.create",
    "volumes.delete",
    "volumes.exists",
    "volume.attach",
];

#[derive(Deserialize)]
struct Name {
    name: String,
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
struct Names {
    #[serde(default)]
    names: Vec<String>,
}

#[derive(Deserialize)]
struct SpaceArg {
    space: String,
}

fn volume_names(entries: Vec<cua_volume::Entry>) -> Vec<String> {
    entries
        .into_iter()
        .filter(|e| e.folder)
        .map(|e| e.name)
        .collect()
}

async fn existing(spaces: &Spaces) -> Result<Vec<String>> {
    let user = spaces.drive().session(Context::user());
    // No `volumes/` folder yet: no volumes.
    match user.ls(path::VOLUMES).await {
        Ok(entries) => Ok(volume_names(entries)),
        Err(cua_volume::Error::NotFound(_)) => Ok(vec![]),
        Err(e) => Err(crate::drive_err(e)),
    }
}

/// Runs operation `op`.
pub fn call<'a>(
    spaces: &'a Spaces,
    op: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<ToolOutcome>> {
    Box::pin(async move {
        let args = if args.is_null() { json!({}) } else { args };
        let parse = |what: &str| Error::invalid(format!("{op}: {what}"));
        match op {
            "volumes.list" => {
                let users = spaces.volume_users()?;
                let volumes: Vec<Value> = existing(spaces)
                    .await?
                    .into_iter()
                    .map(|n| {
                        let mounted_in = users.get(&n).cloned().unwrap_or_default();
                        json!({"name": n, "mounted_in": mounted_in})
                    })
                    .collect();
                Ok(ToolOutcome::json(&json!({ "volumes": volumes })))
            }
            "volumes.create" => {
                let a: Name = serde_json::from_value(args).map_err(|e| parse(&e.to_string()))?;
                let folder = path::volume_folder(&a.name).drive()?;
                if existing(spaces).await?.contains(&a.name) {
                    return Err(crate::drive_err(cua_volume::Error::Precondition(format!(
                        "volume {} already exists",
                        a.name
                    ))));
                }
                spaces
                    .drive()
                    .session(Context::user())
                    .mkdir(&folder)
                    .await
                    .drive()?;
                Ok(ToolOutcome::json(&json!({ "name": a.name })))
            }
            "volumes.delete" => {
                let a: Name = serde_json::from_value(args).map_err(|e| parse(&e.to_string()))?;
                let folder = path::volume_folder(&a.name).drive()?;
                if !existing(spaces).await?.contains(&a.name) {
                    return Err(crate::drive_err(cua_volume::Error::NotFound(format!(
                        "volume {}",
                        a.name
                    ))));
                }
                let users = spaces.volume_users()?.remove(&a.name).unwrap_or_default();
                if !users.is_empty() && !a.force {
                    return Err(crate::drive_err(cua_volume::Error::Precondition(format!(
                        "volume {} is mounted in {}; delete those Spaces first, or force the delete",
                        a.name,
                        users.join(", ")
                    ))));
                }
                let files = spaces
                    .drive()
                    .session(Context::user())
                    .remove_tree(&folder)
                    .await
                    .drive()?;
                Ok(ToolOutcome::json(
                    &json!({ "deleted": a.name, "files": files }),
                ))
            }
            "volumes.exists" => {
                let a: Names = serde_json::from_value(args).map_err(|e| parse(&e.to_string()))?;
                if a.names.is_empty() {
                    return Ok(ToolOutcome::json(&json!({})));
                }
                let have = existing(spaces).await?;
                for n in &a.names {
                    if !have.contains(n) {
                        return Err(crate::drive_err(cua_volume::Error::NotFound(format!(
                            "volume {n} does not exist; create it first (Spaces.create_volume, \
                             `cua volume create`)"
                        ))));
                    }
                }
                Ok(ToolOutcome::json(&json!({})))
            }
            "volume.attach" => {
                let a: SpaceArg =
                    serde_json::from_value(args).map_err(|e| parse(&e.to_string()))?;
                let info = spaces.volume_attach(&a.space).await?;
                match info {
                    Some(i) => Ok(ToolOutcome::json(&i)),
                    None => Err(Error::CapabilityMissing {
                        space: a.space,
                        feature: cua_spaces::volume::VOLUME_FEATURE.into(),
                        limitation: "the Space has no Cua Volume mount".into(),
                    }),
                }
            }
            other => Err(Error::invalid(format!("unknown operation {other}"))),
        }
    })
}
