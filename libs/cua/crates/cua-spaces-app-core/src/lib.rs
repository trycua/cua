// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Spaces app core: every product decision the Spaces apps make,
//! as plain data in and plain data out.
//!
//! Two native shells render it and nothing else:
//!
//! - `apps/cua-spaces` (Tauri): `src-tauri` links this crate; the webview
//!   loads it as wasm (`apps/cua-spaces/core-wasm`) behind `src/model/*.ts`;
//! - `apps/cua-spaces-macos` (SwiftUI): through `cua-sdk`'s UniFFI export
//!   (`native/app_core.rs`), the only FFI crate.
//!
//! Every state machine is a reducer (`reduce(state, action) -> state`) and a
//! projection (`view(state, env) -> view`). Side effects come back as data
//! for the shell to run. The default feature has no I/O and builds for
//! wasm32; `keyvault-client` adds the broker client and `installer` the CLI
//! install step.
//!
//! | Module | What it owns |
//! |---|---|
//! | [`spaces`] | registry rows to Spaces, names, MRU order, the Space list state machine, sidebar and detail |
//! | [`wizard`] | the image catalog, placement and runtime rules, the New Space wizard and its Run on menu (This Mac, your machines, your clouds) |
//! | [`cloud_connect`] | the "Connect a cloud" sheet: providers with their found mark, the region, project or environment, Test and Connect |
//! | [`teleport`] | picker filters and primary action, the review gate, drag-to-notch, transfer overlay |
//! | [`keyvault`] | grouping, consent chips, decisions, categories, the approval sheet, the broker client |
//! | [`onboarding`] | the first-run steps |
//! | [`driver_preview`] | the AI agents page's background computer-use card (its animated miniature) |
//! | [`settings`] | hotkey format, defaults, the settings file, the Settings page |
//! | [`experiments`] | Settings, Experiments: one switch per feature still being built, and what each hides while off |
//! | [`login_item`] | launch at login: the Settings toggle, the first run's Done checkbox, when the app turns it on |
//! | [`about`] | Settings, About: name, version, links, the update controls; the refresh after an update |
//! | [`notch`] | notch geometry, notch mode and tiles |
//! | [`window`] | the main window's chrome (account line, New Space, empty state) and the menu bar item's menu |
//! | [`host`] | "This machine": the roster entry, its page, the host setup form and its validation |
//! | [`devices`] | this device's relay enrollment, the account's devices, approvals (with presence) and the access log |
//! | [`persistent`] | the Agents page: persistent agents, pause and resume, one agent's memory, routines and computer access |
//! | [`drive_page`] | the Drive page: browse the Cua Volume, access requests (with presence) and grants, sync per device, conflicts, Open in Finder |
//! | [`drive_settings`] | Settings, Storage: this machine or an S3-compatible bucket, the Finder volume (a mount on Linux), the block cache |
//! | [`drive_mount_preview`] | the first run's Cua Volume page (its animated miniature) |
//! | [`notifications`] | which daemon notifications to post as system notifications, and the list |
//! | [`share`] | who a Space is shared with (viewer or editor) and the Share sheet |
//! | `installer` | the "Command line" step: plan and install the bundled `cua` (feature `installer`) |
//! | [`agents`] | coding agent runs (labels, order, filter) and the agents on this machine (Settings rows, setup summary) |
//! | [`telemetry`] | which anonymous usage events a UI step means, and sending them (feature `telemetry`) |
//! | [`presence`] | the name and principal id the apps join a Space's presence as |
//! | [`paths`] | home-relative paths |
//! | [`errors`] | the words a failed action shows (a dead daemon in plain words) |
//! | [`dispatch`] | one JSON entry point (`call(method, args)`) for the wasm shim and parity |
//! | [`parity`] | the scripted flows every shell replays |

pub mod about;
pub mod agents;
pub mod billing;
pub mod cloud_connect;
pub mod devices;
pub mod dispatch;
pub mod drive_mount_preview;
pub mod drive_page;
pub mod drive_settings;
pub mod driver_preview;
pub mod errors;
pub mod experiments;
pub mod host;
#[cfg(feature = "installer")]
pub mod installer;
pub mod keyvault;
pub mod login_item;
pub mod model;
pub mod notch;
pub mod notifications;
pub mod onboarding;
pub mod onboarding_preview;
pub mod parity;
pub mod paths;
pub mod persistent;
pub mod presence;
pub mod settings;
pub mod share;
pub mod spaces;
pub mod telemetry;
pub mod teleport;
mod util;
pub mod window;
pub mod wizard;

pub use model::*;
pub use util::{collate, contains_word, parse_rfc3339_ms, to_fixed};

/// Why a core call failed. Only [`dispatch`] and the file helpers fail; the
/// reducers are total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// Unknown method or malformed arguments.
    Invalid(String),
    /// A file could not be read or written.
    Io(String),
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::Invalid(m) => write!(f, "invalid: {m}"),
            CoreError::Io(m) => write!(f, "io: {m}"),
        }
    }
}

impl std::error::Error for CoreError {}
