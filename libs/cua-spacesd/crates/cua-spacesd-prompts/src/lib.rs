// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Unblocks macOS security prompts inside a Space VM.
//!
//! SecurityAgent raises "<app> wants to use the 'login' keychain", "wants to
//! use your confidential information stored in 'Chrome Safe Storage'" and
//! admin authorization panels. In the Spaces images the guest account
//! (`lume`) and its login keychain share one known password, and the
//! dialogs are ordinary accessibility trees (measured on macOS 26: the
//! SecurityAgent process runs as the Space user, its window exposes
//! `AXSecureTextField`, `OK` and `Allow` buttons that take `AXValue` and
//! `AXPress`). This crate reads them and answers them the way a person at the
//! keyboard would, so an agent is never stuck behind one.
//!
//! This is the fallback, not the fix: the first line of defence is that no
//! prompt is raised (keychain repair in the image, ACL-free Safe Storage
//! installs in teleport). Every answer is logged (what asked, what was
//! pressed; never the password).
//!
//! # Safety boundary
//!
//! Answering security prompts for whoever raised them is a privilege gadget
//! on a real machine, so [`guard`] refuses anywhere but an Apple Virtual
//! Machine, and a password is only ever the one the image provisioned
//! (`CUA_SPACESD_PROMPT_PASSWORD`, `CUA_GUEST_PASSWORD`, `CUA_SUDO_PW`, or
//! `lume` for the account named `lume`). Admin authorization panels are only
//! answered when the caller names that class explicitly, and the destructive
//! keychain recovery alerts are never answered.

pub mod classify;
mod engine;
mod guard;

#[cfg(target_os = "macos")]
mod ax;

pub use classify::Kind;
pub use engine::{
    run_watcher, scan, unblock, Action, DialogReport, Options, Report, WatcherConfig,
};
pub use guard::{guard, Guard, Secret};
