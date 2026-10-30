// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Telemetry for what just happened to a durable session: the history
//! entries a save recorded become `cua_durable_agent` events (see
//! [`super::super::telemetry`]).

use super::{Hist, Row};
use cua_telemetry::events::{self, DurableAgent, Event};

/// Words for how a drive ended, by the history entry's event.
fn is_drive_failure(event: &str) -> bool {
    matches!(event, "crashed" | "stalled" | "fenced")
}

/// The machine the drive before `h` ran on, when known.
fn previous_node<'a>(r: &'a Row, h: &Hist) -> Option<&'a str> {
    r.history
        .iter()
        .rev()
        .filter(|x| x.epoch < h.epoch && !x.node.is_empty())
        .map(|x| x.node.as_str())
        .next()
}

/// The failure that ended the drive before the one `h` started.
fn previous_failure<'a>(r: &'a Row, h: &Hist) -> Option<&'a Hist> {
    r.history
        .iter()
        .rev()
        .find(|x| is_drive_failure(&x.event) && x.at_ms <= h.at_ms && x.epoch < h.epoch)
}

/// What a permanent failure was, from the recorded message. The message is
/// only matched here, never sent.
fn permanent_failure(error: &str) -> &'static str {
    if error.contains(".cua/setup") {
        "setup_failed"
    } else if error.contains("no durable Space") {
        "space_gone"
    } else {
        "permanent"
    }
}

/// Sends the events for the history entries `r` recorded since it was
/// loaded. `machines` is how many machines the coordinator knows.
pub(super) fn session_changed(r: &Row, machines: u32) {
    let t = cua_telemetry::global();
    let Some(hash) = t.salted_hash("durable_session", &r.name) else {
        return;
    };
    for e in session_events(r, machines, &hash) {
        t.capture(e);
    }
}

/// The events for the history entries `r` recorded since it was loaded.
/// `session` is the salted hash of the session's name.
pub(super) fn session_events(r: &Row, machines: u32, session: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let mut push = |d: DurableAgent<'_>| out.extend(events::durable_agent(&d));
    for h in &r.fresh {
        let base = || DurableAgent {
            harness: &r.spec.label,
            on: &r.spec.on,
            attempt: h.attempt,
            max_attempts: r.max_attempts,
            reason: "none",
            end: "none",
            failure: "none",
            machines,
            session,
            ..Default::default()
        };
        // A drive that ended on another machine than the drive before it.
        let ends_a_drive =
            is_drive_failure(&h.event) || matches!(h.event.as_str(), "done" | "permanent");
        if ends_a_drive
            && !h.node.is_empty()
            && h.attempt > 1
            && previous_node(r, h).is_some_and(|n| n != h.node)
        {
            push(DurableAgent {
                phase: "failed_over",
                failover: true,
                reason: if h.cause.is_empty() { "none" } else { &h.cause },
                ..base()
            });
        }
        match h.event.as_str() {
            "started" if h.attempt <= 1 => push(DurableAgent {
                phase: "started",
                ..base()
            }),
            "started" => {
                let prev = previous_failure(r, h);
                push(DurableAgent {
                    phase: "restarted",
                    reason: prev
                        .map(|p| p.cause.as_str())
                        .filter(|c| !c.is_empty())
                        .unwrap_or("other"),
                    recovery_time_ms: prev.map_or(0, |p| (h.at_ms - p.at_ms).max(0) as u64),
                    ..base()
                });
            }
            "fenced" => push(DurableAgent {
                phase: "refused",
                failure: "epoch_refused",
                reason: "fenced",
                ..base()
            }),
            "exhausted" => push(DurableAgent {
                phase: "exhausted",
                end: "failed",
                failure: "exhausted",
                reason: r
                    .history
                    .iter()
                    .rev()
                    .find(|x| is_drive_failure(&x.event))
                    .map(|x| x.cause.as_str())
                    .filter(|c| !c.is_empty())
                    .unwrap_or("other"),
                ..base()
            }),
            "permanent" => push(DurableAgent {
                phase: "ended",
                end: "failed",
                failure: permanent_failure(&r.error),
                reason: "permanent",
                ..base()
            }),
            "done" => push(DurableAgent {
                phase: "ended",
                end: "done",
                ..base()
            }),
            "stopped" => push(DurableAgent {
                phase: "ended",
                end: "stopped",
                ..base()
            }),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::sessions::sstate;
    use cua_telemetry::schema;

    /// Text a person or a machine could put anywhere a session carries text.
    const PII: &[&str] = &[
        "alices-macbook-pro.local",
        "/Users/alice/work/secret-repo",
        "alice@example.com",
        "192.168.1.44",
        "Fix the failing tests in acme-billing",
        "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ];

    fn hostile_row() -> Row {
        let mut r = Row {
            name: "alice-nightly-acme".into(),
            state: sstate::RUNNING.into(),
            max_attempts: 5,
            error: format!("{} {} {}", PII[0], PII[1], PII[5]),
            ..Default::default()
        };
        r.spec.label = PII[2].into();
        r.spec.on = PII[0].into();
        r.spec.state_dir = PII[1].into();
        r.spec.argv = vec!["node".into(), PII[4].into()];
        r.spec.env.insert("KEY".into(), PII[5].into());
        // drive 1 crashes on one machine, drive 2 runs on another and is
        // fenced, then it runs out of attempts, and a later one finishes.
        r.attempts = 1;
        r.epoch = 1;
        r.node = PII[0].into();
        r.note("started", None, PII[4]);
        r.note_cause("crashed", Some(137), PII[1], "process_exit");
        r.attempts = 2;
        r.epoch = 2;
        r.node = PII[3].into();
        r.note("started", None, "");
        r.note_cause("fenced", Some(75), PII[2], "fenced");
        r.note("exhausted", None, PII[5]);
        r.note("permanent", Some(78), PII[1]);
        r.note("done", Some(0), PII[4]);
        r.note("stopped", None, PII[2]);
        r
    }

    /// The payload of a session's events holds only vocabulary words, small
    /// numbers and the session's hash: no free text, whatever the session
    /// carries (its name, machine, label, error, driver command and
    /// environment).
    #[test]
    fn events_carry_no_free_text() {
        let r = hostile_row();
        let hash = "0123456789abcdef";
        let evs = session_events(&r, 3, hash);
        assert!(evs.len() >= 7, "every phase is reported: {}", evs.len());
        for e in &evs {
            let spec = schema::EVENTS
                .iter()
                .find(|s| s.name == e.name)
                .expect("declared event");
            let json = serde_json::to_string(&e.props).unwrap();
            for frag in [
                "alice", "macbook", "secret", "acme", "example", "192.168", "sk-ant", "Users",
                "failing",
            ] {
                assert!(!json.contains(frag), "{frag} leaked into {json}");
            }
            // Every property is declared and every value is in its
            // vocabulary: the same check the client runs before queueing.
            for (k, v) in &e.props {
                let p = spec.props.iter().find(|p| p.name == k).unwrap_or_else(|| {
                    panic!("undeclared property {k}");
                });
                match p.kind {
                    schema::Kind::Enum(vocab) => {
                        assert!(vocab.contains(&v.as_str().unwrap()), "{k}={v}");
                    }
                    schema::Kind::Bool => assert!(v.is_boolean(), "{k}"),
                    schema::Kind::Count { max } => assert!(v.as_u64().unwrap() <= max, "{k}"),
                    schema::Kind::SaltedHash => assert_eq!(v, hash),
                    other => panic!("free-form property kind {other:?} on {k}"),
                }
            }
        }
        // The pinned placement is a word, never which machine.
        assert!(evs.iter().all(|e| e.props["placement"] == "host"));
    }

    #[test]
    fn a_restart_names_why_and_how_long_recovery_took() {
        let mut r = Row {
            name: "n".into(),
            max_attempts: 5,
            ..Default::default()
        };
        r.spec.label = "claude-code".into();
        r.attempts = 1;
        r.epoch = 1;
        r.note("started", None, "");
        r.node = "m1".into();
        r.note_cause("stalled", None, "wedged", "stall");
        r.history.last_mut().unwrap().at_ms = 1_000;
        r.attempts = 2;
        r.epoch = 2;
        r.node = String::new();
        r.note("started", None, "");
        r.history.last_mut().unwrap().at_ms = 4_500;
        r.fresh = r.history[2..].to_vec();
        let evs = session_events(&r, 2, "0123456789abcdef");
        assert_eq!(evs.len(), 1);
        let p = &evs[0].props;
        assert_eq!(p["phase"], "restarted");
        assert_eq!(p["reason"], "stall");
        assert_eq!(p["recovery_time_ms"], 3_500);
        assert_eq!(p["attempt"], 2);
        assert_eq!(p["harness"], "claude-code");
        assert_eq!(p["placement"], "auto");
    }

    #[test]
    fn a_drive_on_another_machine_is_a_failover() {
        let mut r = Row {
            name: "n".into(),
            max_attempts: 5,
            ..Default::default()
        };
        r.attempts = 1;
        r.epoch = 1;
        r.node = "m1".into();
        r.note_cause("crashed", None, "x", "host_down");
        r.attempts = 2;
        r.epoch = 2;
        r.node = "m2".into();
        r.fresh.clear();
        r.note("done", Some(0), "");
        let evs = session_events(&r, 2, "0123456789abcdef");
        let phases: Vec<_> = evs.iter().map(|e| e.props["phase"].clone()).collect();
        assert_eq!(phases, ["failed_over", "ended"]);
        assert_eq!(evs[0].props["failover"], true);
        assert_eq!(evs[0].props["reason"], "none");
    }
}
