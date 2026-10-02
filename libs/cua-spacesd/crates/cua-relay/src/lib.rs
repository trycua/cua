// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Reverse tunnel for cua-spacesd machines behind NAT.
//!
//! - [`client`]: the machine side (`cua-spacesd join`), one outbound
//!   WebSocket multiplexed with yamux, every stream spliced into the local
//!   spacesd listener.
//! - [`server`]: the `cua-relay` service that authenticates machines by
//!   relay token and forwards clients' HTTP/1.1, h2 (gRPC) and WebSocket
//!   traffic into the matching machine.
//!
//! - [`api`], [`directory`], [`oidc`], [`assertion`]: the account mode
//!   (cua.ai login): machine directory, account-token validation and the
//!   relay-signed principal assertions machines verify.
//! - [`devices`], [`device_api`]: enrollment of the client devices of an
//!   account (second factor, TTL, audit log).
//!
//! Handshake: `GET <relay>/relay/v1/connect` upgraded to WebSocket with
//! `authorization: Bearer <relay-token>`, `x-cua-machine-id` (persisted,
//! `[a-z0-9-]{8,64}`) and `x-cua-env-driver-version`. The relay then opens
//! one yamux stream per client connection; the machine opens heartbeat
//! streams (one byte, echoed).

pub mod api;
pub mod assertion;
pub mod client;
pub mod device_api;
pub mod devices;
pub mod directory;
pub mod mux;
pub mod oidc;
pub mod server;
pub mod ws;

/// Machine connect path on the relay.
pub const CONNECT_PATH: &str = "/relay/v1/connect";
/// Header carrying the persistent machine id.
pub const MACHINE_ID_HEADER: &str = "x-cua-machine-id";
/// Header carrying the spacesd version.
pub const VERSION_HEADER: &str = "x-cua-env-driver-version";
/// Byte written and echoed on heartbeat streams.
pub const HEARTBEAT_BYTE: u8 = 0x68;

/// Machine ids are DNS-label safe so host mode works: `[a-z0-9-]{8,64}`.
pub fn valid_machine_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

#[cfg(test)]
mod tests {
    #[test]
    fn machine_ids() {
        assert!(super::valid_machine_id("0123abcd"));
        assert!(super::valid_machine_id(
            &uuid::Uuid::new_v4().simple().to_string()
        ));
        assert!(!super::valid_machine_id("short"));
        assert!(!super::valid_machine_id("UPPERCASE1"));
        assert!(!super::valid_machine_id("-leading-dash"));
        assert!(!super::valid_machine_id("../../etc/passwd"));
    }
}
