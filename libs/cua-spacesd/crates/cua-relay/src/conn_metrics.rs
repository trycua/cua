// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Connection outcome counters for `/metrics` (`cua_relay_connections_total`).
//!
//! The only labels are the fixed `kind` and `outcome` sets below: never an
//! account, computer id, address, path or user agent. One middleware
//! classifies the final response of the two connection routes (a computer
//! opening its tunnel, and a request forwarded to a computer), so the exit
//! points of those handlers stay untouched. Responses the relay itself
//! produced carry a [`RelayStatus`] extension; anything else (a 101, or
//! the computer's own response) is a success for the relay.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::http::StatusCode;
use axum::response::Response;

/// Which connection route handled the request. Set on the response by the
/// route's handler; other routes (health, API, share web) stay uncounted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Machine,
    Client,
}

/// The status of a response the relay generated itself (a refusal or an
/// internal failure), also when it is sent as a gRPC status over HTTP 200.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RelayStatus(pub(crate) StatusCode);

/// A response of a connection route that is not a connection attempt
/// (an unrouted path, the share sign-in redirect and session exchange).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Uncounted;

/// The target computer is not connected (a sleeping Mac, not a relay fault).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Offline;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Ok,
    Rejected,
    Offline,
    Error,
}

/// Classifies a finished response, or `None` when it is not a connection.
pub(crate) fn classify(response: &Response) -> Option<(Kind, Outcome)> {
    let ext = response.extensions();
    let kind = *ext.get::<Kind>()?;
    if ext.get::<Uncounted>().is_some() {
        return None;
    }
    let outcome = if ext.get::<Offline>().is_some() {
        Outcome::Offline
    } else {
        match ext.get::<RelayStatus>() {
            Some(RelayStatus(s)) if s.is_server_error() => Outcome::Error,
            Some(RelayStatus(s)) if s.is_client_error() => Outcome::Rejected,
            _ => Outcome::Ok,
        }
    };
    // A computer cannot be offline when it opens its own tunnel.
    let outcome = match (kind, outcome) {
        (Kind::Machine, Outcome::Offline) => Outcome::Error,
        (_, o) => o,
    };
    Some((kind, outcome))
}

/// Process-lifetime counters.
#[derive(Default)]
pub(crate) struct ConnMetrics {
    machine: [AtomicU64; 3],
    client: [AtomicU64; 4],
}

/// A label value and the outcome it stands for.
type Series = (&'static str, Outcome);

const MACHINE: [Series; 3] = [
    ("ok", Outcome::Ok),
    ("rejected", Outcome::Rejected),
    ("error", Outcome::Error),
];
const CLIENT: [Series; 4] = [
    ("ok", Outcome::Ok),
    ("rejected", Outcome::Rejected),
    ("offline", Outcome::Offline),
    ("error", Outcome::Error),
];

impl ConnMetrics {
    pub(crate) fn record(&self, kind: Kind, outcome: Outcome) {
        let (slots, table): (&[AtomicU64], &[Series]) = match kind {
            Kind::Machine => (&self.machine, &MACHINE),
            Kind::Client => (&self.client, &CLIENT),
        };
        if let Some(i) = table.iter().position(|(_, o)| *o == outcome) {
            slots[i].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Prometheus text, every series present from process start.
    pub(crate) fn render(&self) -> String {
        let mut out = String::from(
            "# HELP cua_relay_connections_total Connection attempts handled by the relay, by kind and outcome.\n\
             # TYPE cua_relay_connections_total counter\n",
        );
        let groups: [(&str, &[AtomicU64], &[Series]); 2] = [
            ("machine", &self.machine, &MACHINE),
            ("client", &self.client, &CLIENT),
        ];
        for (kind, slots, table) in groups {
            for (i, (name, _)) in table.iter().enumerate() {
                let n = slots[i].load(Ordering::Relaxed);
                out.push_str(&format!(
                    "cua_relay_connections_total{{kind=\"{kind}\",outcome=\"{name}\"}} {n}\n"
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn response(kind: Option<Kind>, status: Option<u16>, offline: bool, http: u16) -> Response {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::from_u16(http).unwrap();
        if let Some(k) = kind {
            r.extensions_mut().insert(k);
        }
        if let Some(s) = status {
            r.extensions_mut()
                .insert(RelayStatus(StatusCode::from_u16(s).unwrap()));
        }
        if offline {
            r.extensions_mut().insert(Offline);
        }
        r
    }

    #[test]
    fn classifies_machine_connects() {
        let c = |s, h| classify(&response(Some(Kind::Machine), s, false, h));
        assert_eq!(c(None, 101), Some((Kind::Machine, Outcome::Ok)));
        for s in [400, 401, 403, 409, 429] {
            assert_eq!(c(Some(s), s), Some((Kind::Machine, Outcome::Rejected)));
        }
        assert_eq!(c(Some(500), 500), Some((Kind::Machine, Outcome::Error)));
        assert_eq!(c(Some(503), 503), Some((Kind::Machine, Outcome::Error)));
    }

    #[test]
    fn classifies_client_requests() {
        let c = |s, off, h| classify(&response(Some(Kind::Client), s, off, h));
        // The computer's own answers pass through and are not the relay's fault.
        assert_eq!(c(None, false, 200), Some((Kind::Client, Outcome::Ok)));
        assert_eq!(c(None, false, 404), Some((Kind::Client, Outcome::Ok)));
        assert_eq!(c(None, false, 500), Some((Kind::Client, Outcome::Ok)));
        for s in [400, 401, 403, 404, 429] {
            assert_eq!(
                c(Some(s), false, s),
                Some((Kind::Client, Outcome::Rejected))
            );
        }
        // gRPC refusals ride on HTTP 200 but keep the relay's own status.
        assert_eq!(
            c(Some(401), false, 200),
            Some((Kind::Client, Outcome::Rejected))
        );
        assert_eq!(
            c(Some(502), true, 502),
            Some((Kind::Client, Outcome::Offline))
        );
        for s in [500, 502, 503, 504] {
            assert_eq!(c(Some(s), false, s), Some((Kind::Client, Outcome::Error)));
        }
    }

    #[test]
    fn other_routes_are_not_counted() {
        let mut r = response(Some(Kind::Client), Some(404), false, 404);
        r.extensions_mut().insert(Uncounted);
        assert_eq!(classify(&r), None);
        assert_eq!(classify(&response(None, Some(401), false, 401)), None);
        assert_eq!(classify(&response(None, None, false, 200)), None);
    }

    #[test]
    fn renders_every_series_at_zero_and_counts() {
        let m = ConnMetrics::default();
        let text = m.render();
        assert_eq!(text.matches("cua_relay_connections_total{").count(), 7);
        assert!(text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .all(|l| l.ends_with(" 0")));
        m.record(Kind::Client, Outcome::Offline);
        m.record(Kind::Machine, Outcome::Offline);
        let text = m.render();
        assert!(
            text.contains("{kind=\"client\",outcome=\"offline\"} 1"),
            "{text}"
        );
        assert!(!text.contains("machine\",outcome=\"offline"), "{text}");
    }
}
