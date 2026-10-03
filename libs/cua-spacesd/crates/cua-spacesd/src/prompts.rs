// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-spacesd prompts`: answer SecurityAgent / keychain dialogs from the
//! command line (macOS Space VMs). Agents reach it through `space_bash`:
//!
//! ```text
//! "/Applications/Cua Spacesd.app/Contents/MacOS/cua-spacesd" prompts unblock --wait 10
//! ```
//!
//! The same engine runs in the daemon as a background watcher; this is the
//! on-demand form (and `scan` is the read-only one).

use std::time::Duration;

use clap::{Args, Subcommand, ValueEnum};
use cua_spacesd_prompts::{scan, unblock, Options};

/// `prompts` arguments.
#[derive(Args, Debug)]
pub struct PromptsArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand, Debug)]
enum Action {
    /// List the dialogs on screen and what `unblock` would press (read-only).
    Scan(CommonArgs),
    /// Answer the dialogs on screen with the guest password.
    Unblock(UnblockArgs),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Class {
    /// Locked-keychain and keychain-item access prompts.
    Keychain,
    /// Admin authorization panels ("wants to make changes").
    Authorization,
}

#[derive(Args, Debug)]
struct CommonArgs {
    /// Dialog classes to handle (repeatable). Default: keychain.
    #[arg(long = "class", value_enum)]
    classes: Vec<Class>,
}

#[derive(Args, Debug)]
struct UnblockArgs {
    #[command(flatten)]
    common: CommonArgs,
    /// Report what would be pressed; touch nothing.
    #[arg(long)]
    dry_run: bool,
    /// Wait up to this many seconds for a dialog to appear.
    #[arg(long, default_value_t = 0)]
    wait: u64,
}

fn options(common: &CommonArgs, dry_run: bool, wait: u64) -> Options {
    let mut options = Options {
        dry_run,
        wait: Duration::from_secs(wait.min(120)),
        ..Options::default()
    };
    if !common.classes.is_empty() {
        options.classes = common
            .classes
            .iter()
            .map(|c| match c {
                Class::Keychain => "keychain".to_owned(),
                Class::Authorization => "authorization".to_owned(),
            })
            .collect();
    }
    options
}

/// Runs the subcommand; the exit code is 0 when nothing is left blocking,
/// 1 when a dialog remains or the call could not act, 2 on output failure.
pub fn run(args: PromptsArgs) -> i32 {
    let report = match &args.action {
        Action::Scan(common) => scan(&options(common, true, 0)),
        Action::Unblock(u) => unblock(&options(&u.common, u.dry_run, u.wait)),
    };
    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("cannot render the report: {error}");
            return 2;
        }
    }
    let blocked = report.error.is_some()
        || match &args.action {
            Action::Scan(_) => !report.dialogs.is_empty(),
            Action::Unblock(u) if u.dry_run => !report.dialogs.is_empty(),
            Action::Unblock(_) => report.remaining > 0,
        };
    i32::from(blocked)
}
