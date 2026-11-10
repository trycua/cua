// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua experiments`: features that are still being built, one switch each.
//!
//! The switches live in the cua home, where the daemon and the Spaces apps
//! read them. `CUA_EXPERIMENTS`
//! turns experiments on (`ci-runners`) or off (`-ci-runners`) for one
//! process.

use std::io::Write;

use clap::Subcommand;
use cua_sdk::CuaError;
use cua_vmm::experiments::{self, Gate};

use crate::util::line;

/// `cua experiments`.
#[derive(Subcommand, Debug, Clone)]
pub enum ExperimentsCmd {
    /// Every experiment and whether it is on .
    #[command(after_help = "Examples:
  cua experiments list
  cua experiments list --json")]
    List,
    /// Turn an experiment on.
    #[command(after_help = "Examples:
  cua experiments enable ci-runners")]
    Enable {
        /// The experiment's name, as `cua experiments` lists it.
        name: String,
    },
    /// Turn an experiment off. Nothing it made is deleted.
    #[command(after_help = "Examples:
  cua experiments disable ci-runners")]
    Disable {
        /// The experiment's name, as `cua experiments` lists it.
        name: String,
    },
}

fn gate(name: &str) -> Result<Gate, CuaError> {
    let t = name.trim().to_ascii_lowercase().replace('_', "-");
    if experiments::RETIRED.contains(&t.as_str()) {
        return Err(CuaError::InvalidArgument(format!(
            "{t} is always on now; it has no switch"
        )));
    }
    Gate::parse(name).ok_or_else(|| {
        let names: Vec<_> = Gate::ALL.iter().map(|g| g.name()).collect();
        CuaError::InvalidArgument(format!(
            "no experiment named {name:?}; they are: {}",
            names.join(", ")
        ))
    })
}

/// Runs `cmd`.
pub fn run(cmd: &ExperimentsCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let home = cua_home::cua_home();
    match cmd {
        ExperimentsCmd::List => list(&home, json, out),
        ExperimentsCmd::Enable { name } | ExperimentsCmd::Disable { name } => {
            let on = matches!(cmd, ExperimentsCmd::Enable { .. });
            let g = gate(name)?;
            experiments::set_switch(&home, g, on)
                .map_err(|e| CuaError::Internal(format!("keep the {} switch: {e}", g.name())))?;
            if json {
                return list(&home, true, out);
            }
            line(
                out,
                format!("{} is {}.", g.title(), if on { "on" } else { "off" }),
            );
            // The environment can still decide for this process.
            let now = experiments::enabled_in(&home, g);
            if now != on {
                line(
                    out,
                    format!(
                        "It stays {} here: {} says so.",
                        if now { "on" } else { "off" },
                        experiments::EXPERIMENTS_ENV
                    ),
                );
            }
            Ok(0)
        }
    }
}

fn list(home: &std::path::Path, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    if json {
        let rows: Vec<_> = Gate::ALL
            .iter()
            .map(|g| {
                serde_json::json!({
                    "name": g.name(),
                    "title": g.title(),
                    "summary": g.summary(),
                    "on": experiments::enabled_in(home, *g),
                })
            })
            .collect();
        line(out, serde_json::Value::Array(rows).to_string());
        return Ok(0);
    }
    for g in Gate::ALL {
        let state = if experiments::enabled_in(home, *g) {
            "on "
        } else {
            "off"
        };
        line(out, format!("{state}  {:<16}  {}", g.name(), g.summary()));
    }
    line(out, "");
    line(
        out,
        "Turn one on with `cua experiments enable <name>`; they are also in the app, Settings, Advanced, Labs.",
    );
    Ok(0)
}
