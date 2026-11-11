// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-spacesd phone`: the machine owner's side of phones.
//!
//! A phone enrolled on the account reaches what a Space is: the thread, the
//! picture of the screen and, on a desktop this computer shares as a Space, the
//! pointer and the keyboard when the account's role allows touching it (the
//! same rule as for every app). It never reaches the computer's own services.
//! There is no second consent: sharing the desktop is the consent. What the
//! owner has here is a switch and a log:
//!
//! ```text
//! cua-spacesd phone status       whether phones are answered
//! cua-spacesd phone off          answer no phone (the kill switch)
//! cua-spacesd phone on           answer phones again
//! cua-spacesd phone audit [-n 50] the log
//! ```
//!
//! It reads and writes the folder the running daemon reads on every call, so
//! `off` takes effect with the phone's next call and cuts a running picture of
//! the screen within a second.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use cua_spacesd_server::phone_control::{PhoneControl, Refused};

#[derive(Args, Debug, Clone)]
pub struct PhoneArgs {
    /// The daemon's data directory (default: the one `join` uses).
    #[arg(long, env = "CUA_ENV_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand, Debug, Clone)]
enum Action {
    /// Whether this computer answers phones.
    Status,
    /// Answer no phone, now (the kill switch).
    Off,
    /// Answer phones again.
    On,
    /// The last lines of the log of what phones did.
    Audit {
        #[arg(short, default_value_t = 50)]
        n: usize,
    },
}

pub fn run(args: PhoneArgs) -> i32 {
    let dir = args
        .data_dir
        .unwrap_or_else(cua_spacesd_server::config::default_data_dir);
    let store = PhoneControl::in_data_dir(&dir);
    match go(&store, args.action) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("cua-spacesd phone: {e}");
            1
        }
    }
}

fn go(store: &PhoneControl, action: Action) -> Result<(), Refused> {
    match action {
        Action::Status => {
            if store.phones_off() {
                println!(
                    "Phones are off on this computer. Turn them on with: cua-spacesd phone on"
                );
            } else {
                println!(
                    "Phones are answered on this computer (when the mobile experiment is on)."
                );
                println!("Stop them with: cua-spacesd phone off");
            }
        }
        Action::Off => {
            store.set_off(true)?;
            println!("This computer answers no phone now. A running picture of the screen stops within a second.");
        }
        Action::On => {
            store.set_off(false)?;
            println!("This computer answers phones again.");
        }
        Action::Audit { n } => {
            for line in store.audit_tail(n) {
                println!("{line}");
            }
        }
    }
    Ok(())
}
