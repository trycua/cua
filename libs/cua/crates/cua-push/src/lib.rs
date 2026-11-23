// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Push notifications for the Cua Spaces phone app, server side.
//!
//! A person's own daemon says what happened ([`PushEvent`]: an approval
//! waits, a task finished or failed, an agent is blocked); this crate turns
//! that into a short alert for each of the account's registered phones.
//!
//! - [`event`]: the event, the words of each alert and the payload. A
//!   payload carries a fixed phrase, the agent's short name and the ids the
//!   app needs to open the entry. It never carries model text, tool
//!   output, file contents or a secret, whatever the event holds.
//! - [`registry`]: the account's phones (registered by the app with the
//!   token its operating system gave it) and its preferences, kept in one
//!   JSON file written atomically.
//! - [`provider`]: the [`PushProvider`] interface, a [`MockProvider`] for
//!   tests and [`NoProvider`] (nothing configured).
//! - [`apns`]: the Apple provider. It is complete, and idle until the
//!   owner supplies the APNs key (see `ApnsConfig::from_env`).
//! - [`hub`]: the registry and a provider together: registers, applies the
//!   preferences, deduplicates events and drops tokens the provider says
//!   are gone.
//!
//! The relay (`cua-relay`) serves the hub over HTTP; the daemon reports
//! events to the relay with its signed-in account.

pub mod apns;
pub mod event;
pub mod hub;
pub mod provider;
pub mod registry;

pub use event::{EventKind, Payload, PushEvent};
pub use hub::{Delivery, PushHub};
pub use provider::{LogProvider, MockProvider, NoProvider, ProviderError, PushProvider};
pub use registry::{Platform, Preferences, Registration, Registry};
