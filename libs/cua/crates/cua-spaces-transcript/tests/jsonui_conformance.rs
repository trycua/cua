//! The Rust third of the cross-language JSON-UI proof, plus the honesty rules
//! stated as tests rather than as documentation.
//!
//! Rust writes the fixtures, so the reproduction test alone would be circular.
//! It is here because the Swift and TypeScript suites compare against *files*,
//! and a file is only evidence if something re-derives it: this test is what
//! fails when someone edits a fixture by hand instead of changing the parser.
//!
//! The tests that are not circular are the other three. They are ports of
//! `GoldenTests.testObservedElementsAreQuotableFromTheirFrame` and
//! `testRunningStatusIsAlwaysLabelledInferred` in `libs/spaces-sdk-swift`, and
//! a set of refusals: the parser must decline to see structure that is not
//! there, which is the half of the contract a golden cannot express.

use cua_spaces_transcript::cast::RenderedFrame;
use cua_spaces_transcript::conformance::{INSTANTS, instants_json, jsonui_fixture_name};
use cua_spaces_transcript::parser::{Element, PermissionMode, ToolCallStatus};
use cua_spaces_transcript::{
    CastPlayer, ClaudeCodeParser, JSONUI_SCHEMA, Provenance, render_jsonui_document,
};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spaces-contract/fixtures/transcript")
        .canonicalize()
        .expect("conformance fixtures are checked in")
}

fn cast_text(cast: &str) -> String {
    std::fs::read_to_string(fixtures().join(format!("casts/{cast}.cast"))).expect("cast fixture")
}

#[test]
fn every_jsonui_fixture_is_reproduced_byte_for_byte() {
    let root = fixtures();
    assert_eq!(INSTANTS.len(), 11);
    for (cast, instant, time_ms, purpose) in INSTANTS {
        let actual =
            render_jsonui_document(&cast_text(cast), cast, instant, *time_ms).expect("render");
        let expected =
            std::fs::read_to_string(root.join("jsonui").join(jsonui_fixture_name(cast, instant)))
                .expect("jsonui fixture");
        assert_eq!(actual, expected, "{cast}@{instant} ({purpose})");
    }
}

#[test]
fn the_instant_index_names_the_jsonui_schema() {
    let expected = std::fs::read_to_string(fixtures().join("instants.json")).expect("index");
    assert_eq!(instants_json(), expected);
    assert!(expected.contains(JSONUI_SCHEMA));
}

/// Parsing is a pure function of the bytes: the same cast parsed twice is the
/// same document, or a fixture diff would be noise rather than signal.
#[test]
fn parsing_is_stable() {
    let text = cast_text("claude-basic-turn");
    let first = render_jsonui_document(&text, "claude-basic-turn", "complete", 13500).unwrap();
    let second = render_jsonui_document(&text, "claude-basic-turn", "complete", 13500).unwrap();
    assert_eq!(first, second);
}

/// Every cast must say which build drew it. Claude Code's rendering changes
/// between releases, so a fixture whose CLI version is unknown cannot be
/// re-derived or argued with.
#[test]
fn every_cast_records_the_cli_version_that_drew_it() {
    for (cast, _, _, _) in INSTANTS {
        let player = CastPlayer::parse(&cast_text(cast)).expect("cast");
        assert!(
            player.recording.header.cli_version.is_some(),
            "{cast} has no recorded CLI version"
        );
    }
}

/// The honesty rule, enforced rather than documented.
///
/// Anything the parser emits unqualified must be quotable from the frame it
/// came from. This takes every `observed` element in every fixture instant and
/// checks that its principal strings actually appear on that screen. An element
/// that passes this cannot be a hallucinated card.
#[test]
fn observed_elements_are_quotable_from_their_frame() {
    for (cast, instant, time_ms, _) in INSTANTS {
        let player = CastPlayer::parse(&cast_text(cast)).expect("cast");
        let frame = player.frame(*time_ms as f64 / 1000.0);
        let flat = frame.text().replace('\u{a0}', " ");
        for element in ClaudeCodeParser::new().parse(&frame).elements {
            if element.provenance() != Provenance::Observed {
                continue;
            }
            for quote in element.quotable_strings() {
                assert!(
                    flat.contains(&quote),
                    "{cast}@{instant}: an `observed` {} claims {quote:?} \
                     but that text is not on the frame.",
                    element.type_name()
                );
            }
        }
    }
}

/// The parser must never report something as running unless it has said the
/// status is its own conclusion. "Running" is a reading of an absence and is
/// never a word on the screen.
#[test]
fn running_status_is_always_labelled_inferred() {
    for (cast, instant, time_ms, _) in INSTANTS {
        let player = CastPlayer::parse(&cast_text(cast)).expect("cast");
        let parsed = ClaudeCodeParser::new().parse(&player.frame(*time_ms as f64 / 1000.0));
        for element in &parsed.elements {
            match element {
                Element::ToolCall(call) if call.status == ToolCallStatus::Running => {
                    assert_eq!(
                        call.status_provenance,
                        Provenance::Inferred,
                        "{cast}@{instant}: running is never on screen"
                    );
                }
                Element::Subagent(subagent) if subagent.status == ToolCallStatus::Running => {
                    assert_eq!(
                        subagent.status_provenance,
                        Provenance::Inferred,
                        "{cast}@{instant}: running is never on screen"
                    );
                }
                _ => {}
            }
        }
    }
}

/// Nothing on screen is ever dropped: every row either belongs to an element or
/// lands in a `rawText` run.
#[test]
fn every_non_blank_row_is_covered_by_some_element() {
    for (cast, instant, time_ms, _) in INSTANTS {
        let player = CastPlayer::parse(&cast_text(cast)).expect("cast");
        let frame = player.frame(*time_ms as f64 / 1000.0);
        let parsed = ClaudeCodeParser::new().parse(&frame);
        for (row, line) in frame.lines().iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            assert!(
                parsed
                    .elements
                    .iter()
                    .any(|element| element.region().first_row <= row
                        && row <= element.region().last_row),
                "{cast}@{instant}: row {row} ({line:?}) is in no element"
            );
        }
    }
}

// MARK: - Refusals
//
// The half of the contract a golden cannot express: the parser declining to see
// structure that is not on the screen. Each of these is a shape that *nearly*
// matches a rule, and a looser rule would swallow it.

/// Build a frame out of literal rows, on the alternate screen where the Claude
/// Code UI lives.
fn frame_of(rows: &[&str]) -> RenderedFrame {
    let mut data = String::from("\u{1b}[?1049h\u{1b}[H");
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            data.push_str("\r\n");
        }
        data.push_str(row);
    }
    let escaped = serde_json::to_string(&data).expect("escape");
    let cast = format!(
        "{{\"version\":2,\"width\":100,\"height\":{}}}\n[0.1,\"o\",{escaped}]\n",
        rows.len().max(6)
    );
    CastPlayer::parse(&cast).expect("synthetic cast").frame(1.0)
}

fn parse_rows(rows: &[&str]) -> Vec<Element> {
    ClaudeCodeParser::new().parse(&frame_of(rows)).elements
}

/// Prose is not a tool call. A sentence that happens to contain a bracket, and
/// a lower-case word that happens to end in one, are both messages.
#[test]
fn prose_is_not_a_tool_call() {
    assert_eq!(
        cua_spaces_transcript::parser::tool_call_pattern("I ran it (twice)"),
        None
    );
    assert_eq!(
        cua_spaces_transcript::parser::tool_call_pattern("done"),
        None
    );
    assert_eq!(
        cua_spaces_transcript::parser::tool_call_pattern("read(calc.py)"),
        None
    );
    let elements = parse_rows(&["⏺ I checked the file (it was fine)"]);
    assert!(matches!(elements[0], Element::AgentMessage(_)));
}

/// A question mark without options is prose. The rule is structural, and a
/// lexical one would let any rhetorical question adopt an unrelated list.
#[test]
fn a_question_mark_without_options_is_not_a_question() {
    let elements = parse_rows(&["  Should I keep going?", "", "  I will wait."]);
    assert!(
        !elements.iter().any(|e| matches!(e, Element::Question(_))),
        "{elements:#?}"
    );
}

/// Two rules with nothing prompt-shaped between them are a divider, not the
/// composer. Claiming otherwise would report an empty composer on every frame
/// that draws a separator.
#[test]
fn two_rules_without_a_prompt_are_not_a_composer() {
    let rule = "─".repeat(40);
    let elements = parse_rows(&[&rule, "  some banner text", &rule]);
    assert!(
        !elements.iter().any(|e| matches!(e, Element::Composer(_))),
        "{elements:#?}"
    );

    let with_prompt = parse_rows(&[&rule, "❯ hello", &rule]);
    assert!(
        with_prompt
            .iter()
            .any(|e| matches!(e, Element::Composer(_)))
    );
}

/// The agent's prose *about* an error is a message, not an error banner.
/// Matching on the word "error" anywhere would turn every explanation into one.
#[test]
fn prose_about_an_error_is_not_an_error_banner() {
    let elements = parse_rows(&["⏺ The read failed with an error, so I stopped."]);
    assert!(
        matches!(elements[0], Element::AgentMessage(_)),
        "{elements:#?}"
    );

    let banner = parse_rows(&["  Error: ENOENT: no such file or directory"]);
    assert!(matches!(banner[0], Element::Error(_)), "{banner:#?}");
}

/// An unrecognised mode footer keeps its words rather than being mapped onto
/// the nearest case we happen to have. That is the degradation path Claude Code
/// changing its wording is supposed to take.
#[test]
fn an_unknown_mode_footer_keeps_its_words() {
    let elements = parse_rows(&["  ⏸ interstellar mode on (shift+tab to cycle)"]);
    let Element::ModeIndicator(mode) = &elements[0] else {
        panic!("expected a mode indicator, got {elements:#?}");
    };
    assert_eq!(mode.mode, PermissionMode::Unrecognised);
    assert_eq!(mode.raw_text, "⏸ interstellar mode on (shift+tab to cycle)");
    assert_eq!(mode.hint.as_deref(), Some("shift+tab to cycle"));
    assert_eq!(mode.provenance, Provenance::Unrecognised);
}

/// A frame that is not on the alternate screen is not the agent UI. The whole
/// frame degrades to raw text with a null layout profile rather than being
/// mis-parsed against a layout it was never drawn for.
#[test]
fn a_frame_outside_the_alternate_screen_degrades_to_raw_text() {
    let cast = "{\"version\":2,\"width\":80,\"height\":6}\n\
                [0.1,\"o\",\"⏺ Update(calc.py)\\r\\n\"]\n";
    let parsed = ClaudeCodeParser::new().parse(&CastPlayer::parse(cast).unwrap().frame(1.0));
    assert_eq!(parsed.layout_profile, None);
    assert!(!parsed.is_alternate_screen);
    assert!(
        parsed
            .elements
            .iter()
            .all(|element| matches!(element, Element::RawText(_)))
    );
}

/// A folded activity row is a tool call the parser inferred, never one the
/// agent named, and its status is `unknown` rather than `running`: an absent
/// result under a folded row is not evidence of anything.
#[test]
fn a_folded_activity_row_is_inferred_and_unknown() {
    let elements = parse_rows(&["  Read 1 file"]);
    let Element::ToolCall(call) = &elements[0] else {
        panic!("expected a tool call, got {elements:#?}");
    };
    assert_eq!(call.provenance, Provenance::Inferred);
    assert_eq!(call.status, ToolCallStatus::Unknown);
    assert_eq!(call.status_provenance, Provenance::Inferred);
    assert_eq!(call.name, "Read");
}

/// A shell call titled with the agent's own description yields an observed
/// message and an inferred `Bash` call: the word "Bash" is nowhere on screen.
#[test]
fn a_shell_call_names_bash_as_an_inference() {
    let elements = parse_rows(&["⏺ Check the Python version", "  ⎿  $ python3 --version"]);
    assert!(
        matches!(elements[0], Element::AgentMessage(_)),
        "{elements:#?}"
    );
    let Element::ToolCall(call) = &elements[1] else {
        panic!("expected a tool call, got {elements:#?}");
    };
    assert_eq!(call.name, "Bash");
    assert_eq!(call.provenance, Provenance::Inferred);
    assert_eq!(call.argument_summary.as_deref(), Some("python3 --version"));
    assert_eq!(call.status, ToolCallStatus::Running);
    assert_eq!(call.status_provenance, Provenance::Inferred);
}

/// A named call with no result yet is observed in its name and inferred in its
/// status. One flag for both would either overstate the status or understate
/// the name.
#[test]
fn a_named_call_with_no_result_splits_its_provenance() {
    let elements = parse_rows(&["⏺ Read(calc.py)"]);
    let Element::ToolCall(call) = &elements[0] else {
        panic!("expected a tool call, got {elements:#?}");
    };
    assert_eq!(call.name, "Read");
    assert_eq!(call.argument_summary.as_deref(), Some("calc.py"));
    assert_eq!(call.provenance, Provenance::Observed);
    assert_eq!(call.status, ToolCallStatus::Running);
    assert_eq!(call.status_provenance, Provenance::Inferred);
}
