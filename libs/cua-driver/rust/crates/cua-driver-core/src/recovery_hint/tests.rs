use super::*;

const WINDOW: &Value = &Value::Null;

fn seed(args: Value) -> Seed {
    Seed::from_args(&args)
}

fn window_seed() -> Seed {
    seed(json!({"pid": 4242, "window_id": 17, "session": "run-1"}))
}

fn structured(value: Value) -> Map<String, Value> {
    value.as_object().expect("object").clone()
}

fn hint_for(tool: &str, seed: &Seed, is_error: bool, value: Value) -> RecoveryHint {
    recovery_for(tool, seed, is_error, &structured(value))
        .unwrap_or_else(|| panic!("no hint for {tool}"))
}

fn call_value(hint: &RecoveryHint) -> Value {
    hint.next_call.as_ref().expect("next_call").to_value()
}

#[test]
fn every_covered_code_has_a_short_non_empty_hint() {
    let with_window = window_seed();
    let without = Seed::default();
    let mut count = 0;
    let mut too_long = Vec::new();
    for code in covered_codes() {
        count += 1;
        for seed in [&with_window, &without] {
            let hint = hint_for("click", seed, true, json!({ "code": code }));
            assert!(!hint.hint.trim().is_empty(), "{code}");
            if hint.hint.chars().count() > MAX_HINT_CHARS {
                too_long.push(format!("{code}: {}", hint.hint));
            }
        }
        // The browser refusal shape carries the same code one level down.
        let nested = hint_for(
            "browser_click",
            &with_window,
            false,
            json!({"status": "refused", "refusal": {"code": code, "message": "m"}}),
        );
        assert!(!nested.hint.is_empty(), "{code}");
    }
    assert!(too_long.is_empty(), "{too_long:#?}");
    assert!(count >= 50, "table shrank to {count} codes");
}

#[test]
fn every_hint_variant_stays_within_the_length_limit() {
    let seed = window_seed();
    let variants = [
        (
            "drag",
            json!({"code": "background_unavailable", "reason": "drag_and_drop_session"}),
        ),
        (
            "drag",
            json!({"code": "background_unavailable", "reason": "electron_main_process_unavailable"}),
        ),
        (
            "click",
            json!({"code": "background_unavailable", "reason": "pointer_reading_toolkit"}),
        ),
        (
            "click",
            json!({"code": "background_unavailable", "reason": "windowserver_connection_invalid"}),
        ),
        (
            "click",
            json!({"code": "background_unavailable", "window": {"pid": 1, "window_id": 2}}),
        ),
        ("invoke_menu", json!({"code": "background_unavailable"})),
        ("browser_prepare", json!({"code": "background_unavailable"})),
        ("scroll", json!({"code": "background_unavailable"})),
        (
            "click",
            json!({"code": "background_unavailable", "escalation": {"recommended": "page"}}),
        ),
        (
            "click",
            json!({"code": "background_unavailable", "escalation": {"recommended": "px"}}),
        ),
        (
            "click",
            json!({"code": "background_unavailable", "escalation": {"recommended": "foreground"}}),
        ),
        (
            "browser_prepare",
            json!({"code": "browser_consent_required"}),
        ),
        (
            "get_browser_state",
            json!({"code": "browser_consent_required"}),
        ),
        (
            "browser_click",
            json!({"code": "browser_input_trust_unavailable"}),
        ),
        (
            "type_text",
            json!({"code": "type_text_incomplete", "delivered_chars": 12}),
        ),
    ];
    let too_long: Vec<String> = variants
        .into_iter()
        .filter_map(|(tool, value)| {
            let hint = hint_for(tool, &seed, true, value.clone());
            (hint.hint.chars().count() > MAX_HINT_CHARS).then(|| {
                format!(
                    "{tool} {value}: {} chars: {}",
                    hint.hint.chars().count(),
                    hint.hint
                )
            })
        })
        .collect();
    assert!(too_long.is_empty(), "{too_long:#?}");
}

#[test]
fn the_table_covers_every_code_agents_hit_most() {
    let covered: Vec<&str> = covered_codes().collect();
    for code in [
        "background_unavailable",
        "browser_consent_required",
        "browser_requires_setup",
        "browser_ref_stale",
        "browser_binding_stale",
        "browser_tab_required",
        "browser_tab_not_found",
        "browser_input_trust_unavailable",
        "browser_input_incomplete",
        "browser_action_unavailable",
        "browser_wrong_target_refused",
        "browser_route_unavailable",
        "stale_element_token",
        "element_not_visible",
        "element_bounds_unavailable",
        "target_changed",
        "stale_target",
        "stale_geometry",
        "desktop_changed",
        "point_outside_window",
        "px_frame_mismatch",
        "window_target_not_found",
        "ambiguous_window_target",
        "window_id_required",
        "window_id_not_found",
        "window_minimized",
        "permission_required",
        "permission_denied",
        "os_permission_prompt_requires_trusted_host",
        "session_not_started",
        "session_ended",
        "session_required",
        "menu_path_unavailable",
        "option_not_found",
        "type_text_incomplete",
        "typed_output_mismatch",
        "set_value_unavailable",
        "paste_target_is_browser_chrome",
        "popup_keyboard_grab",
        "lane_busy",
        "primary_target_busy",
        "invalid_arguments",
    ] {
        assert!(covered.contains(&code), "no hint for {code}");
    }
    let mut seen = std::collections::HashSet::new();
    for code in covered {
        assert!(seen.insert(code), "{code} appears in two rules");
    }
}

#[test]
fn a_drag_refused_by_macos_drag_and_drop_names_a_non_drag_route_and_the_foreground_condition() {
    let hint = hint_for(
        "drag",
        &window_seed(),
        true,
        json!({
            "code": "background_unavailable",
            "effect": "no_drop",
            "reason": "drag_and_drop_session",
            "event_kind": "mouse_drag",
        }),
    );
    assert_eq!(
        hint.hint,
        "No drop landed: macOS drag-and-drop needs the real pointer. Try a route that is not a drag first \
         (Move button, menu item, shortcut). Retry with delivery_mode:\"foreground\" \
         only if the user allows the window to come forward."
    );
    assert_eq!(
        call_value(&hint),
        json!({
            "tool": "get_window_state",
            "arguments": {"pid": 4242, "window_id": 17, "session": "run-1"},
        })
    );
}

#[test]
fn a_next_call_never_asks_for_the_foreground() {
    let seed = window_seed();
    for (tool, value) in [
        (
            "drag",
            json!({"code": "background_unavailable", "reason": "drag_and_drop_session"}),
        ),
        ("click", json!({"code": "background_unavailable"})),
        ("invoke_menu", json!({"code": "background_unavailable"})),
        ("browser_prepare", json!({"code": "background_unavailable"})),
        ("scroll", json!({"code": "background_unavailable"})),
        ("click", json!({"code": "target_occluded"})),
        ("click", json!({"code": "window_minimized"})),
    ] {
        let hint = hint_for(tool, &seed, true, value);
        if let Some(call) = &hint.next_call {
            let rendered = call.to_value().to_string();
            assert!(!rendered.contains("foreground"), "{tool}: {rendered}");
            assert!(!rendered.contains("bring_to_front"), "{tool}: {rendered}");
            assert!(call.arguments.get("delivery_mode").is_none(), "{tool}");
        }
    }
}

#[test]
fn a_desktop_point_over_a_background_app_points_at_the_window_target() {
    let hint = hint_for(
        "click",
        &seed(json!({"x": 100, "y": 200, "scope": "desktop"})),
        true,
        json!({
            "code": "background_unavailable",
            "event_kind": "click",
            "window": {"pid": 811, "window_id": 659, "app_name": "Notes"},
        }),
    );
    assert!(hint
        .hint
        .contains("target {kind:\"window\", pid, window_id}"));
    assert_eq!(
        call_value(&hint),
        json!({
            "tool": "get_window_state",
            "arguments": {"pid": 811, "window_id": 659, "include_accessibility_tree": false},
        })
    );
}

#[test]
fn a_menu_item_disabled_in_the_background_points_at_the_control_in_the_window() {
    let hint = hint_for(
        "invoke_menu",
        &window_seed(),
        true,
        json!({
            "status": "refused",
            "code": "background_unavailable",
            "refusal": {"code": "background_unavailable", "message": "disabled"},
        }),
    );
    assert!(hint
        .hint
        .contains("bring_to_front first only if the user allows"));
    assert_eq!(call_value(&hint)["tool"], "get_window_state");
}

#[test]
fn a_stale_element_token_asks_for_a_fresh_read_of_the_same_window() {
    let hint = hint_for(
        "click",
        &window_seed(),
        true,
        json!({"code": "stale_element_token"}),
    );
    assert_eq!(
        hint.hint,
        "That element_token comes from an older read. Read the window again for fresh tokens, then act \
         with the new one."
    );
    assert_eq!(
        call_value(&hint),
        json!({
            "tool": "get_window_state",
            "arguments": {"pid": 4242, "window_id": 17, "session": "run-1"},
        })
    );
}

#[test]
fn a_window_target_comes_from_the_typed_target_when_the_call_has_no_top_level_ids() {
    let hint = hint_for(
        "click",
        &seed(json!({"target": {"kind": "window", "pid": 9, "window_id": 4}, "x": 1, "y": 2})),
        true,
        json!({"code": "stale_geometry"}),
    );
    assert_eq!(
        call_value(&hint),
        json!({"tool": "get_window_state", "arguments": {"pid": 9, "window_id": 4}})
    );
}

#[test]
fn without_a_known_window_the_hint_stands_alone() {
    for code in [
        "stale_element_token",
        "window_target_not_found",
        "target_changed",
        "menu_path_unavailable",
    ] {
        let hint = hint_for("click", &Seed::default(), true, json!({ "code": code }));
        assert!(hint.next_call.is_none(), "{code}");
        assert!(!hint.hint.is_empty());
    }
}

#[test]
fn window_lookup_refusals_list_the_app_windows() {
    for code in [
        "window_target_not_found",
        "window_id_not_found",
        "px_window_not_found",
        "window_id_required",
        "ambiguous_window_target",
        "window_minimized",
    ] {
        let hint = hint_for("click", &window_seed(), true, json!({ "code": code }));
        assert_eq!(
            call_value(&hint),
            json!({"tool": "list_windows", "arguments": {"pid": 4242}}),
            "{code}"
        );
    }
}

#[test]
fn a_point_outside_the_window_asks_for_the_screenshot_only() {
    let hint = hint_for(
        "click",
        &window_seed(),
        true,
        json!({"code": "point_outside_window"}),
    );
    assert!(hint
        .hint
        .contains("pixels of that window's latest screenshot"));
    assert_eq!(
        call_value(&hint),
        json!({
            "tool": "get_window_state",
            "arguments": {
                "pid": 4242,
                "window_id": 17,
                "session": "run-1",
                "include_accessibility_tree": false,
            },
        })
    );
}

#[test]
fn permission_codes_split_into_policy_refusals_and_missing_os_grants() {
    let policy = hint_for(
        "click",
        &window_seed(),
        true,
        json!({"status": "refused", "refusal": {"code": "permission_denied", "message": "no"}}),
    );
    assert!(policy.hint.contains("Do not retry it or work around it"));
    assert!(policy.next_call.is_none());

    let os = hint_for(
        "check_permissions",
        &Seed::default(),
        true,
        json!({"status": "refused", "refusal": {"code": "os_permission_prompt_requires_trusted_host", "message": "m"}}),
    );
    assert_eq!(
        call_value(&os),
        json!({"tool": "check_permissions", "arguments": {"prompt": false}})
    );
}

#[test]
fn session_codes_name_start_session() {
    let ended = hint_for(
        "click",
        &seed(json!({"session": "run-1"})),
        true,
        json!({"status": "refused", "refusal": {"code": "session_ended", "message": "m"}}),
    );
    assert_eq!(
        call_value(&ended),
        json!({"tool": "start_session", "arguments": {"session": "run-1"}})
    );
    let missing = hint_for(
        "escalate_session",
        &Seed::default(),
        true,
        json!({"code": "session_not_started"}),
    );
    assert_eq!(
        call_value(&missing),
        json!({"tool": "start_session", "arguments": {}})
    );
}

#[test]
fn an_incomplete_type_text_names_how_much_landed() {
    let hint = hint_for(
        "type_text",
        &window_seed(),
        true,
        json!({"code": "type_text_incomplete", "delivered_chars": 12, "requested_chars": 40}),
    );
    assert_eq!(
        hint.hint,
        "Only the first 12 characters landed. Read the field to confirm, then type only the missing rest; \
         do not resend it all."
    );
    assert_eq!(
        call_value(&hint),
        json!({
            "tool": "get_window_state",
            "arguments": {"pid": 4242, "window_id": 17, "session": "run-1", "since": "latest"},
        })
    );
}

#[test]
fn an_unattached_browser_names_the_attach_call() {
    let seed = seed(json!({"pid": 659, "window_id": 12, "session": "web"}));
    let hint = hint_for(
        "get_browser_state",
        &seed,
        false,
        json!({"status": "refused", "refusal": {"code": "browser_consent_required", "message": "m"}}),
    );
    assert!(hint.hint.starts_with("Attach to this browser window first"));
    assert_eq!(
        call_value(&hint),
        json!({
            "tool": "browser_prepare",
            "arguments": {
                "pid": 659,
                "window_id": 12,
                "session": "web",
                "strategy": {"kind": "existing_profile"},
            },
        })
    );
}

#[test]
fn the_producers_own_attach_call_wins_over_the_rebuilt_one() {
    let hint = hint_for(
        "get_browser_state",
        &Seed::default(),
        false,
        json!({"status": "refused", "refusal": {
            "code": "browser_consent_required",
            "message": "m",
            "detail": {"next_call": {"tool": "browser_prepare", "arguments": {"pid": 1, "window_id": 7}}},
        }}),
    );
    assert_eq!(
        call_value(&hint),
        json!({"tool": "browser_prepare", "arguments": {"pid": 1, "window_id": 7}})
    );
}

#[test]
fn a_refused_browser_prepare_leads_to_the_accessibility_tree() {
    let hint = hint_for(
        "browser_prepare",
        &window_seed(),
        false,
        json!({"status": "refused", "refusal": {"code": "browser_consent_required", "message": "m"}}),
    );
    assert!(hint.hint.contains("needs authorization"));
    assert_eq!(call_value(&hint)["tool"], "get_window_state");

    let background = hint_for(
        "browser_prepare",
        &window_seed(),
        true,
        json!({"code": "background_unavailable", "status": "refused", "refusal": {"code": "background_unavailable", "message": "m"}}),
    );
    assert!(background.hint.starts_with("Not blocked:"));
    assert_eq!(call_value(&background)["tool"], "get_window_state");
}

#[test]
fn browser_refusals_point_at_a_fresh_read_of_the_bound_tab() {
    let seed = seed(json!({"target_id": "t1", "tab_id": "tab9", "ref": "e4", "session": "web"}));
    for code in ["browser_ref_stale", "browser_action_unavailable"] {
        let hint = hint_for(
            "browser_click",
            &seed,
            false,
            json!({"status": "refused", "refusal": {"code": code, "message": "m"}}),
        );
        assert_eq!(
            call_value(&hint),
            json!({
                "tool": "get_browser_state",
                "arguments": {"target_id": "t1", "tab_id": "tab9", "session": "web"},
            }),
            "{code}"
        );
    }
}

#[test]
fn rebinding_a_browser_needs_the_native_window() {
    let with_window = hint_for(
        "browser_click",
        &seed(json!({"pid": 5, "window_id": 6, "target_id": "t", "tab_id": "x"})),
        false,
        json!({"status": "refused", "refusal": {"code": "browser_binding_stale", "message": "m"}}),
    );
    assert_eq!(
        call_value(&with_window),
        json!({"tool": "get_browser_state", "arguments": {"pid": 5, "window_id": 6}})
    );
    let without = hint_for(
        "browser_click",
        &seed(json!({"target_id": "t", "tab_id": "x"})),
        false,
        json!({"status": "refused", "refusal": {"code": "browser_binding_stale", "message": "m"}}),
    );
    assert!(without.next_call.is_none());
}

#[test]
fn trusted_input_refusals_name_the_alternative_route_the_producer_gave() {
    let hint = hint_for(
        "browser_click",
        &Seed::default(),
        false,
        json!({"status": "refused", "refusal": {
            "code": "browser_input_trust_unavailable",
            "message": "m",
            "detail": {"alternative_route": "dom_event"},
        }}),
    );
    assert!(hint.hint.contains("input_route:\"dom_event\""));
    assert!(hint
        .hint
        .contains("only if the user allows the window to come forward"));
}

#[test]
fn incomplete_browser_typing_reports_the_counts() {
    let hint = hint_for(
        "browser_type",
        &seed(json!({"target_id": "t", "tab_id": "x"})),
        false,
        json!({"status": "refused", "refusal": {
            "code": "browser_input_incomplete",
            "message": "m",
            "detail": {"requested_chars": 40, "delivered_chars": 12},
        }}),
    );
    assert_eq!(
        hint.hint,
        "Typing stopped after 12 of 40 characters. Read the field with get_browser_state, then type only what is missing."
    );
}

#[test]
fn unverifiable_actions_read_what_changed() {
    let seed = window_seed();
    let read_changes = json!({
        "tool": "get_window_state",
        "arguments": {"pid": 4242, "window_id": 17, "session": "run-1", "since": "latest"},
    });
    for tool in [
        "click",
        "double_click",
        "right_click",
        "drag",
        "type_text",
        "press_key",
        "hotkey",
        "scroll",
        "set_value",
        "invoke_menu",
        "move_cursor",
    ] {
        for effect in ["unverifiable", "suspected_noop"] {
            let hint = hint_for(
                tool,
                &seed,
                false,
                json!({"effect": effect, "route": "synthetic_events"}),
            );
            assert!(!hint.hint.is_empty(), "{tool} {effect}");
            assert!(
                hint.hint.chars().count() <= MAX_HINT_CHARS,
                "{tool} {effect}"
            );
            assert_eq!(call_value(&hint), read_changes, "{tool} {effect}");
        }
    }
    let click = hint_for("click", &seed, false, json!({"effect": "unverifiable"}));
    assert_eq!(
        click.hint,
        "The click was sent but nothing confirmed it. Read what changed before clicking again; a repeat \
         can toggle or double-fire."
    );
    let drag = hint_for("drag", &seed, false, json!({"effect": "unverifiable"}));
    assert!(drag.hint.contains("start the press inside the item"));
    let noop = hint_for("click", &seed, false, json!({"effect": "suspected_noop"}));
    assert!(noop.hint.starts_with("The click probably did nothing."));
}

#[test]
fn unverifiable_browser_actions_read_the_page() {
    let seed = seed(json!({"target_id": "t1", "tab_id": "tab9"}));
    for tool in ["browser_click", "browser_pointer", "browser_type"] {
        let hint = hint_for(tool, &seed, false, json!({"effect": "unverifiable"}));
        assert_eq!(
            call_value(&hint),
            json!({
                "tool": "get_browser_state",
                "arguments": {"target_id": "t1", "tab_id": "tab9"},
            }),
            "{tool}"
        );
    }
    let typed = hint_for(
        "browser_type",
        &seed,
        false,
        json!({"effect": "unverifiable"}),
    );
    assert!(typed
        .hint
        .starts_with("The text was typed but not read back."));
}

#[test]
fn confirmed_results_and_other_tools_get_no_hint() {
    let seed = window_seed();
    assert!(recovery_for(
        "click",
        &seed,
        false,
        &structured(json!({"effect": "confirmed"}))
    )
    .is_none());
    assert!(recovery_for(
        "click",
        &seed,
        false,
        &structured(json!({"effect": "partial"}))
    )
    .is_none());
    // Not an action tool: its `effect` field means something else.
    assert!(recovery_for(
        "get_window_state",
        &seed,
        false,
        &structured(json!({"effect": "unverifiable"}))
    )
    .is_none());
    // A success payload that happens to carry a `code` is not a refusal.
    assert!(recovery_for(
        "click",
        &seed,
        false,
        &structured(json!({"code": "stale_element_token"}))
    )
    .is_none());
    // An unknown code has no hint.
    assert!(recovery_for(
        "click",
        &seed,
        true,
        &structured(json!({"code": "something_new"}))
    )
    .is_none());
}

#[test]
fn a_refused_action_result_is_read_by_its_error_code() {
    let hint = hint_for(
        "click",
        &window_seed(),
        false,
        json!({"effect": "refused", "route": "synthetic_events", "error": {"code": "point_outside_window"}}),
    );
    assert_eq!(
        call_value(&hint)["arguments"]["include_accessibility_tree"],
        false
    );
}

fn error_with(structured: Value) -> ToolResult {
    ToolResult::error("refused").with_structured(structured)
}

#[test]
fn annotate_adds_both_fields_and_keeps_the_rest() {
    let mut result = error_with(json!({"code": "stale_element_token", "extra": 1}));
    annotate("click", &window_seed(), &mut result);
    let out = result.structured_content.expect("structured");
    assert_eq!(out["code"], "stale_element_token");
    assert_eq!(out["extra"], 1);
    assert!(out["hint"].as_str().unwrap().contains("fresh tokens"));
    assert_eq!(out["next_call"]["tool"], "get_window_state");
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn annotate_never_overwrites_a_producers_hint_or_next_call() {
    let mut result = error_with(json!({
        "code": "background_unavailable",
        "hint": "producer hint",
        "next_call": {"tool": "get_browser_state", "arguments": {"pid": 1, "window_id": 2}},
    }));
    annotate("click", &window_seed(), &mut result);
    let out = result.structured_content.expect("structured");
    assert_eq!(out["hint"], "producer hint");
    assert_eq!(
        out["next_call"],
        json!({"tool": "get_browser_state", "arguments": {"pid": 1, "window_id": 2}})
    );

    // A producer hint without a next_call still gains the call.
    let mut partial = error_with(json!({"code": "stale_element_token", "hint": "mine"}));
    annotate("click", &window_seed(), &mut partial);
    let out = partial.structured_content.expect("structured");
    assert_eq!(out["hint"], "mine");
    assert_eq!(out["next_call"]["tool"], "get_window_state");
}

#[test]
fn annotate_covers_browser_refusals_that_are_not_errors() {
    let mut result = ToolResult::text("refused (browser_ref_stale): m").with_structured(json!({
        "status": "refused",
        "refusal": {"code": "browser_ref_stale", "message": "m"},
    }));
    annotate(
        "browser_click",
        &seed(json!({"target_id": "t", "tab_id": "x"})),
        &mut result,
    );
    let out = result.structured_content.expect("structured");
    assert_eq!(out["refusal"]["code"], "browser_ref_stale");
    assert_eq!(out["next_call"]["tool"], "get_browser_state");
    assert!(
        result.is_error.is_none(),
        "a refusal stays a non-error result"
    );
}

#[test]
fn annotate_covers_unverifiable_action_results() {
    let mut result = ToolResult::text("Posted drag").with_structured(json!({
        "effect": "unverifiable",
        "route": "synthetic_events",
        "summary": "Posted drag",
    }));
    annotate("drag", &window_seed(), &mut result);
    let out = result.structured_content.expect("structured");
    assert_eq!(out["effect"], "unverifiable");
    assert!(out["hint"]
        .as_str()
        .unwrap()
        .starts_with("The drag was posted"));
    assert_eq!(out["next_call"]["arguments"]["since"], "latest");
}

#[test]
fn annotate_leaves_confirmed_results_and_plain_successes_alone() {
    let mut confirmed = ToolResult::text("ok").with_structured(json!({"effect": "confirmed"}));
    annotate("click", &window_seed(), &mut confirmed);
    assert_eq!(
        confirmed.structured_content,
        Some(json!({"effect": "confirmed"}))
    );

    let mut plain = ToolResult::text("ok");
    annotate("click", &window_seed(), &mut plain);
    assert!(plain.structured_content.is_none());
}

#[test]
fn a_missing_required_argument_gains_an_invalid_arguments_code_and_a_hint() {
    let mut result = ToolResult::error("Missing required parameter: from_x");
    annotate("drag", &window_seed(), &mut result);
    let out = result.structured_content.expect("structured");
    assert_eq!(out["code"], "invalid_arguments");
    assert!(out["hint"]
        .as_str()
        .unwrap()
        .contains("required argument is missing"));

    let mut other = ToolResult::error("something else went wrong");
    annotate("drag", &window_seed(), &mut other);
    assert!(
        other.structured_content.is_none(),
        "unrecognised errors stay as they are"
    );
}

#[test]
fn the_seed_reads_only_public_fields() {
    let seed = seed(json!({
        "pid": "not a number",
        "window_id": 5,
        "session": "",
        "target": {"kind": "desktop", "display_id": "primary"},
    }));
    assert_eq!(seed.pid, None);
    assert_eq!(seed.window_id, Some(5));
    assert_eq!(seed.session, None);
    assert!(seed.desktop_target);
    let _ = WINDOW;
}

#[test]
fn a_desktop_scope_action_points_at_the_window_target() {
    let hint = hint_for(
        "click",
        &seed(json!({"scope": "desktop", "x": 1, "y": 2})),
        true,
        json!({"code": "background_unavailable", "effect": "refused"}),
    );
    assert!(hint.hint.starts_with("Desktop input uses the real pointer"));
    assert!(hint
        .hint
        .contains("Foreground only if the user allows the pointer to move."));
    assert!(hint.next_call.is_none(), "no window is known");
}
