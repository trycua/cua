//! Requests recorded from real harnesses (see each fixture's `note`) parse
//! into the conversation the scenario engine expects: the first asks for the
//! scripted tool call, the second (carrying the real tool result) ends the
//! script by quoting it.

use cua_mock_llm::scenario::{Failures, Step, plan};
use cua_mock_llm::{anthropic, openai};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/recorded/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(
        v["note"]
            .as_str()
            .unwrap()
            .contains("recorded from the real")
    );
    v["body"].clone()
}

#[test]
fn claude_agent_acp_requests() {
    let f = Failures::default();
    let first = plan(&anthropic::convo(&fixture("claude-agent-acp-0.81.1-0")), &f);
    assert!(
        matches!(first.steps.last(), Some(Step::Call { name, input }) if name == "Bash"
            && input["command"].as_str().unwrap().contains("hello-from-mock")),
        "{first:?}"
    );
    let second = plan(&anthropic::convo(&fixture("claude-agent-acp-0.81.1-1")), &f);
    assert_eq!(
        second.steps,
        vec![Step::Text("Done. Last tool output: hello-from-mock".into())]
    );
}

#[test]
fn codex_acp_requests() {
    let f = Failures::default();
    let first = plan(&openai::responses_convo(&fixture("codex-acp-1.13.1-0")), &f);
    assert!(
        matches!(first.steps.last(), Some(Step::Call { name, .. }) if name == "exec_command"),
        "{first:?}"
    );
    let second = plan(&openai::responses_convo(&fixture("codex-acp-1.13.1-1")), &f);
    let Some(Step::Text(t)) = second.steps.last() else {
        panic!("{second:?}")
    };
    assert!(t.contains("codex-was-here"), "{t}");
}
