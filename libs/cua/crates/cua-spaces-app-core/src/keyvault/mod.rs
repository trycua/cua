// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault, as the shells present it. Everything is derived from what
//! the broker returned; nothing here decides access (the broker does), and
//! nothing here sees a secret value (the broker never returns one).
//!
//! - [`wire`]: the broker's redacted records and the page overview;
//! - [`vault`]: the vault list (items grouped by app and site, with locks,
//!   multi-select and search), the unlock prompt and the delete confirm;
//! - [`view`]: recent decisions, live access, waiting requests, the page
//!   chrome;
//! - [`browse`]: the sidebar (All Items, Waiting, Access, Recent, one row
//!   per app) and the panes for the lists other than the vault;
//! - [`approval`]: the approval sheet (nothing selected by default);
//! - [`credential`]: the setup and unlock form (Touch ID or a passphrase);
//! - `client` (feature `keyvault-client`): the broker client both shells run.

pub mod approval;
pub mod browse;
#[cfg(feature = "keyvault-client")]
pub mod client;
pub mod credential;
pub mod vault;
pub mod view;
pub mod wire;

pub use wire::*;
