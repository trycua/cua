// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Usage telemetry of durable sessions and durable Spaces: the one place the
//! coordinator talks to `cua-telemetry`.
//!
//! What leaves the machine is the event `cua_durable_agent` and nothing else:
//! fixed words, small counts and a per-install salted hash of the session so
//! the events of one session can be put together. Never a session or Space
//! name, a machine name, an address, a prompt, a path or an error message;
//! `cua_telemetry::events::durable_agent` takes no free text, and
//! `cua_telemetry::schema::validate` drops a payload that has any. It follows
//! the install's telemetry setting like every other event (off with
//! `DO_NOT_TRACK=1`, `CUA_TELEMETRY=0` or `cua telemetry off`).

use cua_telemetry::events::{self, DurableAgent};

/// Captures one `cua_durable_agent` event for the session or Space `name`
/// (hashed here). Does nothing when telemetry is off.
pub(super) fn emit(name: &str, build: impl FnOnce(&str) -> DurableAgent<'_>) {
    let t = cua_telemetry::global();
    let Some(hash) = t.salted_hash("durable_session", name) else {
        return;
    };
    let d = build(&hash);
    if let Some(e) = events::durable_agent(&d) {
        t.capture(e);
    }
}

/// A durable Space was placed again on a machine (the one it was on lost
/// it); its disk starts fresh.
pub(super) fn space_recreated(name: &str, failover: bool, machines: u32) {
    emit(name, |session| DurableAgent {
        phase: "space_recreated",
        harness: "other",
        on: "auto",
        reason: "none",
        end: "none",
        failure: "none",
        failover,
        machines,
        session,
        ..Default::default()
    });
}

/// A lease was refused: it is held by another holder, or the caller's token
/// is no longer the current one.
pub(super) fn lease_refused(name: &str, machines: u32) {
    emit(name, |session| DurableAgent {
        phase: "refused",
        harness: "other",
        on: "auto",
        reason: "none",
        end: "none",
        failure: "lease_refused",
        machines,
        session,
        ..Default::default()
    });
}
