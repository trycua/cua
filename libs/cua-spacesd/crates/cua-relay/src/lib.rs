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
//! - [`meter`]: relay allowance metering (live sessions, relay hours;
//!   feature `hosted`, else nothing is metered).
//! - [`devices`], [`device_api`]: enrollment of the client devices of an
//!   account (second factor, TTL, audit log).
//! - [`pool`]: the account's machine pool: the lease that elects which
//!   device coordinates batch work, the job log it appends under its epoch,
//!   and the mailbox clients reach it through.
//! - [`storage`]: the state volume's health: write failures per store,
//!   the pool write budget, the `/readyz` write probe and `/metrics`.
//!
//! Handshake: `GET <relay>/relay/v1/connect` upgraded to WebSocket with
//! `authorization: Bearer <relay-token>`, `x-cua-machine-id` (persisted,
//! `[a-z0-9-]{8,64}`) and `x-cua-env-driver-version`. The relay then opens
//! one yamux stream per client connection; the machine opens heartbeat
//! streams (one byte, echoed).

pub mod api;
pub mod assertion;
pub mod auth_log;
pub mod client;
pub mod clock;
pub mod device_api;
pub mod devices;
pub mod directory;
pub mod grants;
pub mod handover;
pub mod ident;
pub mod mesh;
pub mod mux;
pub mod oidc;
pub mod pairing;
pub mod pool;
pub mod proxies;
pub mod server;
pub mod share_web;
pub mod storage;
mod transport;
pub mod ws;

// Optional hosted-relay features (Cargo feature `hosted`): plan metering,
// Teams and the hosted Spaces API's internal routes. Without it, `free`
// stands in for `meter`, `teams`, `org_access` and `team_sharing`.
// Declared through a macro so that `cargo fmt` does not look for these
// files in a tree without them; CI formats them by name where present.
macro_rules! hosted_modules {
    ($($name:ident),*) => {
        $(
            #[cfg(feature = "hosted")]
            pub mod $name;
        )*
    };
}
hosted_modules!(
    account_erase,
    hosted,
    meter,
    org_access,
    plan_source,
    team_api,
    team_sharing,
    teams
);

#[cfg(not(feature = "hosted"))]
mod free;
#[cfg(not(feature = "hosted"))]
pub use free::{meter, org_access, team_sharing, teams};

/// Machine connect path on the relay.
pub const CONNECT_PATH: &str = "/relay/v1/connect";
/// Header carrying the persistent machine id.
pub const MACHINE_ID_HEADER: &str = "x-cua-machine-id";
/// Header carrying the spacesd version.
pub const VERSION_HEADER: &str = "x-cua-env-driver-version";
/// Byte written and echoed on heartbeat streams.
pub const HEARTBEAT_BYTE: u8 = 0x68;

/// `bytes` bytes from the OS RNG, lowercase hex. For anything a peer must
/// not predict or another machine must not repeat (assertion ids, nonces):
/// a userspace PRNG's state is copied by a memory snapshot into every clone.
pub fn os_random_hex(bytes: usize) -> String {
    use rand::RngCore as _;
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

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
