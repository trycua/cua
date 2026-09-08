//! Pure exact-target decisions for macOS-style background input (v1).
//!
//! Background mutation must carry one exact `(pid, CGWindowID)` target from
//! request to postcondition. The platform shell gathers fresh facts about that
//! target immediately before dispatch and asks this module for exactly one
//! decision: execute the requested route, or refuse with a stable
//! machine-readable reason. The shell never improvises a second actuator after
//! a refusal, and a sibling window's state can never confirm the requested
//! action.
//!
//! This module owns no platform objects and performs no I/O, so the complete
//! state-policy matrix is testable in ordinary CI. See
//! `docs/macos-background-input-v1-plan.md` for the design contract.

use serde_json::{json, Value};

/// The canonical target key: the requested `(pid, CGWindowID)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactWindowTarget {
    pub pid: i32,
    pub window_id: u32,
}

/// WindowServer's attribution of the requested CGWindowID, resolved
/// immediately before the decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowServerOwnership {
    /// The window exists and the requested pid owns it.
    SamePid,
    /// WindowServer has no record of the id — closed, stale, or fabricated.
    NotFound,
    /// The window exists but another process owns it (out-of-process panels).
    ForeignPid { owner_pid: i32 },
}

/// Whether an explicitly addressed element was proven to belong to the
/// requested window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementAncestry {
    /// The action does not address a retained element.
    NotAddressed,
    /// The live element ascends to an AX window whose CGWindowID is the
    /// requested one.
    ProvenDescendant,
    /// The live element ascends to a different window of the same process.
    OutsideTargetWindow,
    /// Ancestry could not be resolved (dead element, SPI failure). An address
    /// the shell cannot re-prove is not an exact target.
    Unproven,
}

/// Fresh facts about one exact target, gathered by the platform shell.
///
/// Unknown facts must stay unknown: gatherers must not guess a value merely to
/// unlock a route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundTargetFacts {
    pub window_server: WindowServerOwnership,
    /// The requested CGWindowID is claimed by one of the application's fresh
    /// `AXWindows` (mapped through `_AXUIElementGetWindow`). Absent means
    /// off-Space or AX-unresolved: observation-only.
    pub ax_window_present: bool,
    /// `AXMinimized` on the exact target window. `None` means the attribute
    /// could not be read — an unproven fact, which fails closed for pointer
    /// and keyboard routes (it never unlocks them).
    pub target_minimized: Option<bool>,
    /// `AXHidden` on the owning application. `None` fails closed like
    /// `target_minimized`.
    pub app_hidden: Option<bool>,
    /// Same-pid top-level windows, other than the target, that could receive
    /// process-scoped keyboard input (enumerated from WindowServer so an
    /// off-Space sibling that AX cannot see still counts; proven-minimized
    /// siblings are excluded because they cannot be the key window).
    pub competing_keyboard_destinations: usize,
    /// Ancestry proof for an explicitly addressed element, when one exists.
    pub element: ElementAncestry,
}

/// The background action classes v1 distinguishes. Route order and trust are
/// documented in the plan's routing ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundAction {
    /// Semantic AX action or value mutation on an exact element
    /// (`AXPress`, `AXConfirm`, selection, `AXValue`).
    AxSemantic,
    /// Window-local routed pointer (stamped CGWindowID; real pointer unmoved).
    WindowPointer,
    /// Process-scoped text insertion (AX selected-text write or CGEvent
    /// keystrokes) with a target-bound value readback.
    InsertText,
    /// Process-scoped key/hotkey without an action-specific verifier.
    GenericKey,
}

impl BackgroundAction {
    fn is_pid_keyboard(self) -> bool {
        matches!(
            self,
            BackgroundAction::InsertText | BackgroundAction::GenericKey
        )
    }
}

/// Stable machine-readable refusal codes. These appear in structured tool
/// results; do not rename them without a compatibility review.
pub mod refusal_codes {
    pub const WINDOW_NOT_FOUND: &str = "window_not_found";
    pub const OWNER_PID_MISMATCH: &str = "owner_pid_mismatch";
    pub const OFF_SPACE_OR_AX_UNRESOLVED: &str = "off_space_or_ax_unresolved";
    pub const MINIMIZED_OR_HIDDEN: &str = "minimized_or_hidden_window";
    pub const SAME_PID_KEYBOARD_AMBIGUITY: &str = "same_pid_keyboard_ambiguity";
    pub const ELEMENT_OUTSIDE_TARGET_WINDOW: &str = "element_outside_target_window";
}

/// A typed refusal: no actuator ran and none may run for this decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundRefusal {
    pub code: &'static str,
    pub reason: String,
    /// The safe next route when one actually exists (advice for an explicit
    /// caller decision, never an implicit fallback).
    pub advice: Option<&'static str>,
}

/// Verification binding for an executed route: evidence is acceptable only
/// when it belongs to the exact requested window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetBoundVerification {
    pub window_id: u32,
}

impl TargetBoundVerification {
    /// Whether a readback observed on an element whose resolved window is
    /// `evidence_window_id` may confirm the requested action. Unresolvable
    /// evidence (`None`) is never accepted — it is unverifiable, not proof —
    /// and a sibling window's state can never confirm the requested target.
    pub fn accepts_evidence_from(&self, evidence_window_id: Option<u32>) -> bool {
        evidence_window_id == Some(self.window_id)
    }
}

/// One decision for one gathered-facts snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackgroundInputDecision {
    Execute {
        /// The only evidence source that may report `confirmed`.
        verification: TargetBoundVerification,
    },
    Refuse(BackgroundRefusal),
}

impl BackgroundInputDecision {
    pub fn is_execute(&self) -> bool {
        matches!(self, BackgroundInputDecision::Execute { .. })
    }
}

fn refuse(
    code: &'static str,
    reason: String,
    advice: Option<&'static str>,
) -> BackgroundInputDecision {
    BackgroundInputDecision::Refuse(BackgroundRefusal {
        code,
        reason,
        advice,
    })
}

/// Decide whether one background action may run against one exact target.
///
/// The rules encode the v1 state-policy matrix:
/// - a stale or foreign CGWindowID refuses every mutation;
/// - a target absent from fresh `AXWindows` (off-Space or AX-unresolved) is
///   observation-only;
/// - an addressed element must prove ancestry to the requested window for
///   every route, including semantic AX;
/// - semantic AX actions remain available for minimized/hidden targets;
/// - the routed pointer requires a visible (possibly occluded) target; and
/// - process-scoped keyboard additionally requires that the target is the
///   only eligible same-pid keyboard destination, because the transport
///   addresses a process, not a window.
pub fn decide_background_input(
    target: ExactWindowTarget,
    facts: &BackgroundTargetFacts,
    action: BackgroundAction,
) -> BackgroundInputDecision {
    match &facts.window_server {
        WindowServerOwnership::NotFound => {
            return refuse(
                refusal_codes::WINDOW_NOT_FOUND,
                format!(
                    "WindowServer has no record of window {} (closed or stale); \
                     re-enumerate windows and re-target",
                    target.window_id
                ),
                None,
            );
        }
        WindowServerOwnership::ForeignPid { owner_pid } => {
            return refuse(
                refusal_codes::OWNER_PID_MISMATCH,
                format!(
                    "window {} is owned by pid {owner_pid}, not requested pid {}; \
                     re-target the actual owner",
                    target.window_id, target.pid
                ),
                None,
            );
        }
        WindowServerOwnership::SamePid => {}
    }

    match facts.element {
        ElementAncestry::NotAddressed | ElementAncestry::ProvenDescendant => {}
        ElementAncestry::OutsideTargetWindow | ElementAncestry::Unproven => {
            return refuse(
                refusal_codes::ELEMENT_OUTSIDE_TARGET_WINDOW,
                format!(
                    "the addressed element could not be proven to belong to window {}; \
                     take a fresh get_window_state snapshot and re-address it",
                    target.window_id
                ),
                Some("get_window_state"),
            );
        }
    }

    if !facts.ax_window_present {
        return refuse(
            refusal_codes::OFF_SPACE_OR_AX_UNRESOLVED,
            format!(
                "window {} is not among the process's current AXWindows (another Space, \
                 or its AX surface is unresolved); background input is refused so a \
                 sibling window cannot receive it. Observation (capture/list_windows) \
                 remains available",
                target.window_id
            ),
            Some("foreground"),
        );
    }

    // Pointer/keyboard require PROVEN not-minimized and not-hidden: an
    // unreadable attribute (`None`) is an unknown fact and unknown facts
    // never unlock a route. Semantic AX stays available either way.
    let visibility_disproven =
        !(facts.target_minimized == Some(false) && facts.app_hidden == Some(false));
    match action {
        BackgroundAction::AxSemantic => BackgroundInputDecision::Execute {
            verification: TargetBoundVerification {
                window_id: target.window_id,
            },
        },
        BackgroundAction::WindowPointer => {
            if visibility_disproven {
                return refuse(
                    refusal_codes::MINIMIZED_OR_HIDDEN,
                    format!(
                        "window {} is minimized or its application is hidden (or that \
                         state could not be proven); the routed pointer is refused in \
                         v1. Use an exact element action (click/set_value by element) \
                         instead",
                        target.window_id
                    ),
                    Some("accessibility"),
                );
            }
            BackgroundInputDecision::Execute {
                verification: TargetBoundVerification {
                    window_id: target.window_id,
                },
            }
        }
        BackgroundAction::InsertText | BackgroundAction::GenericKey => {
            if visibility_disproven {
                return refuse(
                    refusal_codes::MINIMIZED_OR_HIDDEN,
                    format!(
                        "window {} is minimized or its application is hidden (or that \
                         state could not be proven); raw key input is refused in v1. \
                         Use an exact element action (set_value, click \
                         action:\"confirm\"/\"press\") instead",
                        target.window_id
                    ),
                    Some("accessibility"),
                );
            }
            debug_assert!(action.is_pid_keyboard());
            if facts.competing_keyboard_destinations > 0 {
                return refuse(
                    refusal_codes::SAME_PID_KEYBOARD_AMBIGUITY,
                    format!(
                        "pid {} owns {} other eligible top-level window(s); process-scoped \
                         key events cannot be proven to reach window {} and could mutate a \
                         sibling window. Use an exact element action, the page tool for \
                         browser content, or delivery_mode:\"foreground\"",
                        target.pid, facts.competing_keyboard_destinations, target.window_id
                    ),
                    Some("accessibility"),
                );
            }
            BackgroundInputDecision::Execute {
                verification: TargetBoundVerification {
                    window_id: target.window_id,
                },
            }
        }
    }
}

/// The exact-window resolution string reported by the read-only capability
/// section.
fn exact_window_status(facts: &BackgroundTargetFacts) -> &'static str {
    match &facts.window_server {
        WindowServerOwnership::NotFound => "not_found",
        WindowServerOwnership::ForeignPid { .. } => "owner_mismatch",
        WindowServerOwnership::SamePid if facts.ax_window_present => "matched",
        WindowServerOwnership::SamePid => "ax_unresolved",
    }
}

/// Build the additive read-only `background_input` capability report from the
/// same pure facts that gate every action.
///
/// Semantics: `available` means the route's prerequisites are currently
/// proven, not that a future call is guaranteed to succeed; every action
/// revalidates instead of trusting this report. The report contains no window
/// titles, control values, or private symbol addresses.
pub fn background_input_capability_report(
    target: ExactWindowTarget,
    facts: &BackgroundTargetFacts,
    one_shot_capture_available: Option<bool>,
) -> Value {
    let route_entry = |route: &str, action: BackgroundAction| -> Value {
        match decide_background_input(target, facts, action) {
            BackgroundInputDecision::Execute { .. } => {
                json!({ "route": route, "status": "available" })
            }
            BackgroundInputDecision::Refuse(refusal) => {
                json!({ "route": route, "status": "refused", "reason": refusal.code })
            }
        }
    };
    json!({
        "exact_window": {
            "status": exact_window_status(facts),
            "pid": target.pid,
            "window_id": target.window_id,
        },
        "routes": [
            route_entry("accessibility", BackgroundAction::AxSemantic),
            route_entry("window_pointer", BackgroundAction::WindowPointer),
            route_entry("pid_keyboard", BackgroundAction::GenericKey),
        ],
        "observation": {
            "one_shot_capture": match one_shot_capture_available {
                Some(true) => "available",
                Some(false) => "unavailable",
                None => "unknown",
            },
            "frame_freshness": "unknown",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: ExactWindowTarget = ExactWindowTarget {
        pid: 42,
        window_id: 700,
    };

    fn matched_facts() -> BackgroundTargetFacts {
        BackgroundTargetFacts {
            window_server: WindowServerOwnership::SamePid,
            ax_window_present: true,
            target_minimized: Some(false),
            app_hidden: Some(false),
            competing_keyboard_destinations: 0,
            element: ElementAncestry::NotAddressed,
        }
    }

    const ALL_ACTIONS: [BackgroundAction; 4] = [
        BackgroundAction::AxSemantic,
        BackgroundAction::WindowPointer,
        BackgroundAction::InsertText,
        BackgroundAction::GenericKey,
    ];

    fn code_of(decision: BackgroundInputDecision) -> &'static str {
        match decision {
            BackgroundInputDecision::Refuse(refusal) => refusal.code,
            BackgroundInputDecision::Execute { .. } => panic!("expected a refusal"),
        }
    }

    /// Regression for the proven same-PID wrong-target case: target window A
    /// requested while sibling window B is the process's focused/eligible
    /// window. Process-scoped keyboard must refuse before dispatch — an
    /// explicit `window_id` is an address, not delivery proof.
    #[test]
    fn two_window_process_refuses_background_keyboard_for_both_text_and_keys() {
        let facts = BackgroundTargetFacts {
            competing_keyboard_destinations: 1,
            ..matched_facts()
        };
        for action in [BackgroundAction::InsertText, BackgroundAction::GenericKey] {
            assert_eq!(
                code_of(decide_background_input(TARGET, &facts, action)),
                refusal_codes::SAME_PID_KEYBOARD_AMBIGUITY,
                "{action:?} must refuse while a sibling keyboard destination exists"
            );
        }
        // Semantic AX and the stamped window-local pointer remain available:
        // they are window-addressed, not process-addressed.
        for action in [
            BackgroundAction::AxSemantic,
            BackgroundAction::WindowPointer,
        ] {
            assert!(
                decide_background_input(TARGET, &facts, action).is_execute(),
                "{action:?} is window-addressed and stays available"
            );
        }
    }

    /// A sibling window's state can never satisfy the requested target's
    /// verification plan, and unresolvable evidence is not proof.
    #[test]
    fn sibling_readback_never_confirms_the_requested_window() {
        let verification = TargetBoundVerification { window_id: 700 };
        assert!(verification.accepts_evidence_from(Some(700)));
        assert!(!verification.accepts_evidence_from(Some(701)), "sibling");
        assert!(!verification.accepts_evidence_from(None), "unresolvable");
    }

    #[test]
    fn stale_window_id_refuses_every_mutation() {
        let facts = BackgroundTargetFacts {
            window_server: WindowServerOwnership::NotFound,
            ax_window_present: false,
            ..matched_facts()
        };
        for action in ALL_ACTIONS {
            assert_eq!(
                code_of(decide_background_input(TARGET, &facts, action)),
                refusal_codes::WINDOW_NOT_FOUND,
                "{action:?}"
            );
        }
    }

    #[test]
    fn foreign_owner_refuses_every_mutation() {
        let facts = BackgroundTargetFacts {
            window_server: WindowServerOwnership::ForeignPid { owner_pid: 900 },
            ..matched_facts()
        };
        for action in ALL_ACTIONS {
            assert_eq!(
                code_of(decide_background_input(TARGET, &facts, action)),
                refusal_codes::OWNER_PID_MISMATCH,
                "{action:?}"
            );
        }
    }

    /// Off-Space / AX-unresolved targets are observation-only: no route may
    /// send input, and no sibling window may be used instead.
    #[test]
    fn ax_unresolved_target_is_observation_only() {
        let facts = BackgroundTargetFacts {
            ax_window_present: false,
            ..matched_facts()
        };
        for action in ALL_ACTIONS {
            assert_eq!(
                code_of(decide_background_input(TARGET, &facts, action)),
                refusal_codes::OFF_SPACE_OR_AX_UNRESOLVED,
                "{action:?}"
            );
        }
    }

    /// Minimized/hidden targets retain exact semantic AX actions; raw keys and
    /// the routed pointer refuse rather than restoring the window.
    #[test]
    fn minimized_or_hidden_keeps_semantic_ax_and_refuses_keys_and_pointer() {
        for facts in [
            BackgroundTargetFacts {
                target_minimized: Some(true),
                ..matched_facts()
            },
            BackgroundTargetFacts {
                app_hidden: Some(true),
                ..matched_facts()
            },
        ] {
            assert!(
                decide_background_input(TARGET, &facts, BackgroundAction::AxSemantic).is_execute()
            );
            for action in [
                BackgroundAction::WindowPointer,
                BackgroundAction::InsertText,
                BackgroundAction::GenericKey,
            ] {
                assert_eq!(
                    code_of(decide_background_input(TARGET, &facts, action)),
                    refusal_codes::MINIMIZED_OR_HIDDEN,
                    "{action:?}"
                );
            }
        }
    }

    /// Unknown visibility fails closed: when minimized/hidden state could not
    /// be read, the routed pointer and raw keys refuse exactly as if the
    /// window were minimized, while exact semantic AX actions remain allowed.
    #[test]
    fn unknown_visibility_fails_closed_for_pointer_and_keyboard() {
        for facts in [
            BackgroundTargetFacts {
                target_minimized: None,
                ..matched_facts()
            },
            BackgroundTargetFacts {
                app_hidden: None,
                ..matched_facts()
            },
        ] {
            assert!(
                decide_background_input(TARGET, &facts, BackgroundAction::AxSemantic).is_execute()
            );
            for action in [
                BackgroundAction::WindowPointer,
                BackgroundAction::InsertText,
                BackgroundAction::GenericKey,
            ] {
                assert_eq!(
                    code_of(decide_background_input(TARGET, &facts, action)),
                    refusal_codes::MINIMIZED_OR_HIDDEN,
                    "{action:?}"
                );
            }
        }
    }

    /// The singleton rule: with no competing keyboard destination, exact
    /// background text/keys are permitted and verification is target-bound.
    #[test]
    fn singleton_window_permits_background_keyboard_with_target_bound_verification() {
        let facts = matched_facts();
        for action in [BackgroundAction::InsertText, BackgroundAction::GenericKey] {
            match decide_background_input(TARGET, &facts, action) {
                BackgroundInputDecision::Execute { verification } => {
                    assert_eq!(verification.window_id, TARGET.window_id);
                }
                other => panic!("{action:?} expected Execute, got {other:?}"),
            }
        }
    }

    /// An addressed element that cannot be proven to descend from the
    /// requested window refuses every route — a cached element under the right
    /// cache key is not sufficient after a lifecycle or Space transition.
    #[test]
    fn unproven_element_ancestry_refuses_all_routes() {
        for element in [
            ElementAncestry::OutsideTargetWindow,
            ElementAncestry::Unproven,
        ] {
            let facts = BackgroundTargetFacts {
                element,
                ..matched_facts()
            };
            for action in ALL_ACTIONS {
                assert_eq!(
                    code_of(decide_background_input(TARGET, &facts, action)),
                    refusal_codes::ELEMENT_OUTSIDE_TARGET_WINDOW,
                    "{action:?} with {element:?}"
                );
            }
        }
    }

    #[test]
    fn proven_element_ancestry_executes_semantic_ax_even_when_minimized() {
        let facts = BackgroundTargetFacts {
            element: ElementAncestry::ProvenDescendant,
            target_minimized: Some(true),
            ..matched_facts()
        };
        assert!(decide_background_input(TARGET, &facts, BackgroundAction::AxSemantic).is_execute());
    }

    /// Refusal precedence: exactness failures are reported before state or
    /// cardinality failures so the caller fixes the most fundamental fact.
    #[test]
    fn refusal_precedence_owner_before_element_before_scope_before_state() {
        let worst = BackgroundTargetFacts {
            window_server: WindowServerOwnership::ForeignPid { owner_pid: 9 },
            ax_window_present: false,
            target_minimized: Some(true),
            app_hidden: Some(true),
            competing_keyboard_destinations: 3,
            element: ElementAncestry::OutsideTargetWindow,
        };
        assert_eq!(
            code_of(decide_background_input(
                TARGET,
                &worst,
                BackgroundAction::InsertText
            )),
            refusal_codes::OWNER_PID_MISMATCH
        );
        let bad_element = BackgroundTargetFacts {
            window_server: WindowServerOwnership::SamePid,
            ..worst.clone()
        };
        assert_eq!(
            code_of(decide_background_input(
                TARGET,
                &bad_element,
                BackgroundAction::InsertText
            )),
            refusal_codes::ELEMENT_OUTSIDE_TARGET_WINDOW
        );
        let unresolved = BackgroundTargetFacts {
            element: ElementAncestry::NotAddressed,
            ..bad_element
        };
        assert_eq!(
            code_of(decide_background_input(
                TARGET,
                &unresolved,
                BackgroundAction::InsertText
            )),
            refusal_codes::OFF_SPACE_OR_AX_UNRESOLVED
        );
        let minimized = BackgroundTargetFacts {
            ax_window_present: true,
            ..unresolved
        };
        assert_eq!(
            code_of(decide_background_input(
                TARGET,
                &minimized,
                BackgroundAction::InsertText
            )),
            refusal_codes::MINIMIZED_OR_HIDDEN
        );
    }

    #[test]
    fn capability_report_shape_is_stable() {
        let report = background_input_capability_report(TARGET, &matched_facts(), Some(true));
        assert_eq!(report["exact_window"]["status"], "matched");
        assert_eq!(report["exact_window"]["pid"], 42);
        assert_eq!(report["exact_window"]["window_id"], 700);
        assert_eq!(report["routes"][0]["route"], "accessibility");
        assert_eq!(report["routes"][0]["status"], "available");
        assert_eq!(report["routes"][2]["route"], "pid_keyboard");
        assert_eq!(report["routes"][2]["status"], "available");
        assert_eq!(report["observation"]["one_shot_capture"], "available");
        assert_eq!(report["observation"]["frame_freshness"], "unknown");
    }

    #[test]
    fn capability_report_refusals_carry_stable_reason_codes() {
        let facts = BackgroundTargetFacts {
            ax_window_present: false,
            ..matched_facts()
        };
        let report = background_input_capability_report(TARGET, &facts, Some(false));
        assert_eq!(report["exact_window"]["status"], "ax_unresolved");
        for route in report["routes"].as_array().unwrap() {
            assert_eq!(route["status"], "refused");
            assert_eq!(route["reason"], refusal_codes::OFF_SPACE_OR_AX_UNRESOLVED);
        }
        assert_eq!(report["observation"]["one_shot_capture"], "unavailable");

        let two_windows = BackgroundTargetFacts {
            competing_keyboard_destinations: 1,
            ..matched_facts()
        };
        let report = background_input_capability_report(TARGET, &two_windows, None);
        assert_eq!(report["routes"][0]["status"], "available");
        assert_eq!(report["routes"][2]["status"], "refused");
        assert_eq!(
            report["routes"][2]["reason"],
            refusal_codes::SAME_PID_KEYBOARD_AMBIGUITY
        );
        assert_eq!(report["observation"]["one_shot_capture"], "unknown");
    }
}
