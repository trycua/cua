// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Drift guard (CUA-2857): every harness "Start an agent here" offers is a
//! harness the registry knows by that exact id, so `agent_add` accepts what
//! the dialog sends.

use std::path::Path;

fn offered() -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../apps/cua-spaces-web/src/components/add-agent/add-agent-sheet.tsx");
    let text = std::fs::read_to_string(&src).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
    let line = text
        .lines()
        .find(|l| l.contains("export const START_HARNESSES"))
        .expect("START_HARNESSES in the Start an agent here sheet");
    let list = line
        .split('[')
        .nth(1)
        .and_then(|r| r.split(']').next())
        .expect("a list");
    list.split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[test]
fn every_offered_harness_is_a_registry_id_agent_add_takes_verbatim() {
    let ids = offered();
    assert!(ids.len() >= 4, "{ids:?}");
    for id in &ids {
        let h =
            cua_agents::harness::harness(id).unwrap_or_else(|| panic!("{id:?} is not a harness"));
        assert_eq!(h.id, id, "the dialog must send the canonical id");
        assert!(h.ready, "{id:?} is offered but not ready");
    }
}
