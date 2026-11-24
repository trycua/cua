// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The probe contract: `x-cua-relay-route`.
//!
//! Monitoring must not depend on the wording of a refusal. The text a
//! person reads ("sign in: …") changes with the relay's mode and with
//! copy edits; on 4 Nov 2026 turning phone support on changed it and two
//! probes that matched it paged falsely (INC-27). Probes match the status
//! code and this header instead.
//!
//! Every response of the client route (`/m/<id>/…`, host mode, and any
//! other path the relay hands to a computer) and of `/healthz` and
//! `/readyz` carries the header, with one value of [`RouteMarker`]. The
//! values are stable: changing or removing one is a breaking change for
//! every probe (see `tests/probe_contract.rs`, and "Probe contract" in the
//! README). A computer cannot set it: whatever a computer sends under that
//! name is replaced with `forwarded`.

use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;

use crate::conn_metrics::{Offline, RelayStatus};

/// The header name.
pub const HEADER: &str = "x-cua-relay-route";

/// What answered a request, and why, in one fixed word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteMarker {
    /// `/healthz` and a ready `/readyz`: the relay is up.
    Ok,
    /// `/readyz` answering 503: the state volume or a store is failing.
    NotReady,
    /// The computer answered; status and body are its own.
    Forwarded,
    /// 401 from the relay: no credential, or one it does not accept. The
    /// refusal an anonymous probe of `/m/<unknown id>/…` gets in every mode.
    RefusedAuth,
    /// 403 from the relay: signed in but not allowed (not shared, stopped
    /// sharing, revoked ticket, old computer for a phone).
    RefusedDenied,
    /// 429 or 503 from the relay: a plan, rate or capacity limit.
    RefusedLimit,
    /// Any other 4xx from the relay (bad request, not found).
    Refused,
    /// The computer is not connected (502).
    Offline,
    /// Any other 5xx from the relay (tunnel or handshake failure).
    Error,
    /// The relay answered itself without refusing: a redirect (legacy share
    /// URL, share sign-in) or a page it serves.
    Relay,
}

impl RouteMarker {
    /// Every value, for tests and docs.
    pub const ALL: [RouteMarker; 10] = [
        RouteMarker::Ok,
        RouteMarker::NotReady,
        RouteMarker::Forwarded,
        RouteMarker::RefusedAuth,
        RouteMarker::RefusedDenied,
        RouteMarker::RefusedLimit,
        RouteMarker::Refused,
        RouteMarker::Offline,
        RouteMarker::Error,
        RouteMarker::Relay,
    ];

    /// The header value. Fixed: see the module docs.
    pub const fn as_str(self) -> &'static str {
        match self {
            RouteMarker::Ok => "ok",
            RouteMarker::NotReady => "not-ready",
            RouteMarker::Forwarded => "forwarded",
            RouteMarker::RefusedAuth => "refused-auth",
            RouteMarker::RefusedDenied => "refused-denied",
            RouteMarker::RefusedLimit => "refused-limit",
            RouteMarker::Refused => "refused",
            RouteMarker::Offline => "offline",
            RouteMarker::Error => "error",
            RouteMarker::Relay => "relay",
        }
    }

    /// The marker of a relay-made response with HTTP status `status`.
    pub fn of_refusal(status: StatusCode) -> Self {
        match status.as_u16() {
            401 => RouteMarker::RefusedAuth,
            403 => RouteMarker::RefusedDenied,
            429 | 503 => RouteMarker::RefusedLimit,
            s if (400..500).contains(&s) => RouteMarker::Refused,
            s if s >= 500 => RouteMarker::Error,
            _ => RouteMarker::Relay,
        }
    }
}

/// Header: a random id, new for every process start. Two answers with the
/// same id come from the same relay process; a new id means it restarted.
pub const BOOT_HEADER: &str = "x-cua-relay-boot";
/// Header: when this process started, in Unix seconds.
pub const STARTED_HEADER: &str = "x-cua-relay-started";
/// Header: the relay's crate version.
pub const VERSION_HEADER: &str = "x-cua-relay-version";

/// What identifies this relay process: for tests, canaries and dashboards
/// that must tell a restart from a failure (a rollout from an outage).
/// Random and public by design: it carries no secret and grants nothing.
#[derive(Debug)]
pub struct Boot {
    id: String,
    started: u64,
}

impl Boot {
    /// The random id of this process (32 hex digits).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Unix seconds at which this process started.
    pub fn started(&self) -> u64 {
        self.started
    }
}

/// This process's identity, fixed at the first call. `main` calls it at
/// start, so the time is the process start.
pub fn boot() -> &'static Boot {
    static BOOT: std::sync::OnceLock<Boot> = std::sync::OnceLock::new();
    BOOT.get_or_init(|| Boot {
        id: hex::encode(rand::random::<[u8; 16]>()),
        started: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    })
}

/// Adds the boot identity headers to `response`.
pub(crate) fn stamp_boot(response: &mut Response) {
    let boot = boot();
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(boot.id()) {
        headers.insert(BOOT_HEADER, v);
    }
    if let Ok(v) = HeaderValue::from_str(&boot.started().to_string()) {
        headers.insert(STARTED_HEADER, v);
    }
    headers.insert(
        VERSION_HEADER,
        HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    );
}

/// `GET /healthz` on every listener (main, probe port, STUN-only): 204,
/// `ok`, and the boot identity ([`BOOT_HEADER`], [`STARTED_HEADER`],
/// [`VERSION_HEADER`]).
pub async fn healthz() -> Response {
    use axum::response::IntoResponse as _;
    let mut response = StatusCode::NO_CONTENT.into_response();
    stamp(&mut response, RouteMarker::Ok);
    stamp_boot(&mut response);
    response
}

/// Sets the header on `response`, replacing any value already there.
pub(crate) fn stamp(response: &mut Response, marker: RouteMarker) {
    response
        .headers_mut()
        .insert(HEADER, HeaderValue::from_static(marker.as_str()));
}

/// Stamps a finished client-route response. A forwarded response was
/// already stamped `forwarded` (by `harden_response`, over anything the
/// computer sent); everything else the relay made itself.
pub(crate) fn stamp_client_route(response: &mut Response) {
    if response.headers().contains_key(HEADER) {
        return;
    }
    let ext = response.extensions();
    let marker = if ext.get::<Offline>().is_some() {
        RouteMarker::Offline
    } else if let Some(RelayStatus(status)) = ext.get::<RelayStatus>() {
        // Also for gRPC refusals, sent as HTTP 200 with a grpc-status: the
        // relay's own status is what the marker reports.
        RouteMarker::of_refusal(*status)
    } else {
        RouteMarker::Relay
    };
    stamp(response, marker);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire values are the contract. A change here breaks probes in
    /// trycua/cloud: update them in the same change, never after.
    #[test]
    fn values_are_fixed() {
        let values: Vec<_> = RouteMarker::ALL.iter().map(|m| m.as_str()).collect();
        assert_eq!(
            values,
            [
                "ok",
                "not-ready",
                "forwarded",
                "refused-auth",
                "refused-denied",
                "refused-limit",
                "refused",
                "offline",
                "error",
                "relay",
            ]
        );
        assert_eq!(HEADER, "x-cua-relay-route");
    }

    #[test]
    fn boot_identity_is_stable_within_a_process() {
        let a = boot();
        let b = boot();
        assert_eq!(a.id(), b.id());
        assert_eq!(a.id().len(), 32);
        assert!(a.id().bytes().all(|c| c.is_ascii_hexdigit()));
        assert!(a.started() > 1_700_000_000);
    }

    #[test]
    fn refusal_classes() {
        let of = |s: u16| RouteMarker::of_refusal(StatusCode::from_u16(s).unwrap());
        assert_eq!(of(401), RouteMarker::RefusedAuth);
        assert_eq!(of(403), RouteMarker::RefusedDenied);
        assert_eq!(of(429), RouteMarker::RefusedLimit);
        assert_eq!(of(503), RouteMarker::RefusedLimit);
        assert_eq!(of(400), RouteMarker::Refused);
        assert_eq!(of(404), RouteMarker::Refused);
        assert_eq!(of(502), RouteMarker::Error);
        assert_eq!(of(308), RouteMarker::Relay);
    }
}
