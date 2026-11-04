// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua sb stream-probe`'s actions are input batches the Space accepts
//! (checked against the RCDP protocol types).

use cua_cli::stream_probe::{events_of, parse_action};
use serde_json::json;

#[test]
fn probe_actions_are_valid_input_batches() {
    for spec in [
        "click:0.1,0.9",
        "type:hi",
        "key:enter+command",
        "scroll:0.5,0.5,3",
    ] {
        let events = events_of(&parse_action(spec).unwrap());
        let n = events.len() as u64;
        let batch: cua_media_protocol::InteractiveInputBatch = serde_json::from_value(json!({
            "session_id": "", "first_sequence": 1, "events": events
        }))
        .unwrap();
        assert_eq!(batch.validate(), Ok(n), "{spec}");
    }
}
