// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Drift guard: every agent the composer's @ picker offers to start in a
//! Space (`summon.candidates`) is a harness `agent_add` knows, so picking it
//! adds a member instead of failing with "unknown harness".

use cua_spaces_app_core::connectors::ConnectorsInput;
use cua_spaces_app_core::summon::{SummonInput, candidates, plan};

fn input() -> SummonInput {
    SummonInput {
        members: vec![],
        people: vec![],
        can_add: true,
        connectors: ConnectorsInput::default(),
    }
}

#[test]
fn every_agent_the_picker_starts_is_a_real_harness() {
    let c = candidates(&input());
    assert!(!c.is_empty());
    for c in &c {
        assert!(
            cua_agents::harness::harness(&c.harness).is_some(),
            "@{} starts harness {:?}, which agent_add doesn't know (known: {:?})",
            c.handle,
            c.harness,
            cua_agents::harness::ids()
        );
    }
}

#[test]
fn mentioning_codex_adds_the_openai_codex_harness() {
    let p = plan(&input(), "@codex look at this");
    let step = p
        .steps
        .iter()
        .find(|s| s.handle == "codex")
        .expect("a step for @codex");
    assert_eq!(step.harness, "openai-codex");
}
