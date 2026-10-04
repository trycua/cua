// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teleport, as the shells present it:
//!
//! - [`flow`]: "Teleport an app..." (pick, options, plan, the consent review
//!   with its secrets acknowledgement, run, done);
//! - [`review`]: the review's choice of what to send (sites with counts,
//!   items, the Keyvault as the source) and what is remembered per Space;
//! - [`windows`]: the window lists (open windows here, a Space's remote
//!   windows), their filters and the primary button;
//! - [`drag`]: dragging a real window to the notch;
//! - [`transfer`]: the overlay over a Space while a teleport uploads.
//!
//! The plan itself (what installs, what leaves this machine) and the final
//! approval check are `cua_teleport::ux`; this module only drives the UI.

pub mod drag;
pub mod flow;
pub mod grid;
pub mod review;
pub mod transfer;
pub mod windows;
