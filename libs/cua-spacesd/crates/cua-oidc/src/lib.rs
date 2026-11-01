// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-oidc`: the Cua OIDC issuer (CUA-2046; design in
//! `docs/rfcs/cloud-oidc-federation.md`).
//!
//! A cloud is linked once per account. The customer's AWS, Google Cloud or
//! Azure account trusts this issuer for their account only, and every
//! enrolled device of the account then gets short-lived (15 minute) RS256
//! tokens to exchange with the cloud. No usable cloud secret is ever stored
//! server-side, and the signing key never leaves AWS KMS.
//!
//! Server only: deliberately not the relay (compromising the mesh
//! coordinator must not grant cloud access) and not Keycloak (which can't
//! mint per-device, per-agent, per-purpose claims).
//!
//! # HTTP API
//!
//! Public, no auth:
//! - `GET /a/{acct}/.well-known/openid-configuration`: discovery, issuer
//!   `{base}/a/{acct}`.
//! - `GET /a/{acct}/.well-known/jwks.json`, `GET /.well-known/jwks.json`:
//!   the active, next and previous keys (`kid` = RFC 7638 thumbprint).
//! - `GET /v1/templates/aws.yaml?acct={acct}`: the CloudFormation template
//!   for that account (or the caller's, with a bearer token).
//! - `GET /healthz` (204), `GET /readyz` (204, or 503 when the database is
//!   down).
//!
//! Signed in (`Authorization: Bearer <Keycloak access token>`, audience
//! `cua-oidc`):
//! - `GET /v1/account`: `{account, issuer, subject}`.
//! - `GET /v1/connections`, `GET /v1/connections/{id}`.
//! - `GET /v1/audit?connection=&limit=`: newest first.
//!
//! Signed in plus a device proof (`DPoP: <proof>`, see [`proof`]):
//! - `POST /v1/token` `{connection, purpose, audience, agent?}`.
//! - `POST /v1/connections`, `POST /v1/connections/{id}/limits`,
//!   `/approve`, `/revoke`, `DELETE /v1/connections/{id}`.
//! - `POST /v1/connections/{id}/spend` `{month, usd}`: what this device
//!   spent there this month (list price times runtime of the machines it
//!   created). The connection's month spend is the sum over the account's
//!   devices; `create` tokens stop at the budget.
//! - `POST /v1/connections/{id}/sealed-token` `{sealed_token,
//!   expected_version?, expected_generation?}`: a `sealed` connection's
//!   new ciphertext (a replaced token, or the same token sealed under a
//!   newer pool key generation), swapped in only when the connection is
//!   still as the caller read it (409 `conflict` otherwise).
//!
//! Errors are `{"error": code, "message": plain words}` ([`api::ApiError`]).

pub mod account;
pub mod api;
pub mod devices;
pub mod jwk;
pub mod keycloak;
#[cfg(feature = "kms")]
pub mod kms;
pub mod model;
pub mod pg;
pub mod proof;
pub mod registry;
pub mod signer;
pub mod token;

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The calendar month (UTC) of `secs`, as `YYYY-MM`.
pub fn month_of(secs: u64) -> String {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

/// This calendar month (UTC), as `YYYY-MM`.
pub fn this_month() -> String {
    month_of(now_secs())
}

#[cfg(test)]
mod tests {
    #[test]
    fn months() {
        assert_eq!(super::month_of(0), "1970-01");
        // 2026-10-24T12:00:00Z
        assert_eq!(super::month_of(1_792_843_200), "2026-10");
        // 2024-02-29T23:59:59Z, 2024-03-01T00:00:00Z
        assert_eq!(super::month_of(1_709_251_199), "2024-02");
        assert_eq!(super::month_of(1_709_251_200), "2024-03");
        assert_eq!(super::month_of(1_767_225_599), "2025-12");
        assert_eq!(super::month_of(1_767_225_600), "2026-01");
    }
}
