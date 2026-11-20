// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Conservative text insertion for the exact currently focused UIA Document.
//!
//! This is a capability probe for no-element foreground typing. It never
//! activates a window or selects a target. Before the write, every failed
//! predicate leaves the existing input route available. Once SetValue starts,
//! failures are unverifiable and must not be retried through another route.

use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};

use windows::core::{IUnknown, Interface};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern, SupportedTextSelection_Single,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
    UIA_BackgroundColorAttributeId, UIA_DocumentControlTypeId, UIA_FontNameAttributeId,
    UIA_FontSizeAttributeId, UIA_FontWeightAttributeId, UIA_ForegroundColorAttributeId,
    UIA_IsItalicAttributeId, UIA_StrikethroughStyleAttributeId, UIA_StyleIdAttributeId,
    UIA_TextPatternId, UIA_UnderlineStyleAttributeId, UIA_ValuePatternId, UIA_TEXTATTRIBUTE_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, IsWindow, GA_ROOT,
};

use super::windows_enum::{run_focused_document_uia, UiaDeadlineError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusedDocumentTypeOutcome {
    NotApplicable,
    Confirmed,
    Unknown(&'static str),
}

const MAX_TYPED_DOCUMENT_UNITS: i32 = 1 << 20;

/// Formatting that SetValue would flatten when it differs across the document.
const FORMAT_ATTRIBUTES: [UIA_TEXTATTRIBUTE_ID; 9] = [
    UIA_FontNameAttributeId,
    UIA_FontSizeAttributeId,
    UIA_FontWeightAttributeId,
    UIA_IsItalicAttributeId,
    UIA_UnderlineStyleAttributeId,
    UIA_StrikethroughStyleAttributeId,
    UIA_ForegroundColorAttributeId,
    UIA_BackgroundColorAttributeId,
    UIA_StyleIdAttributeId,
];

const OBJECT_REPLACEMENT: char = '\u{fffc}';

static PENDING_WRITE: AtomicBool = AtomicBool::new(false);

pub(crate) fn try_type_focused_document_append(
    target_hwnd: u64,
    expected_pid: Option<u32>,
    text: &str,
) -> FocusedDocumentTypeOutcome {
    if text.is_empty() {
        return FocusedDocumentTypeOutcome::NotApplicable;
    }

    let text = text.to_owned();
    let state = Arc::new(AtomicU8::new(0));
    let worker_state = Arc::clone(&state);
    match run_focused_document_uia(move |cancelled| {
        let outcome = try_type_focused_document_append_unbounded(
            target_hwnd,
            expected_pid,
            &text,
            &cancelled,
            &worker_state,
        );
        PENDING_WRITE.store(false, Ordering::Release);
        outcome
    }) {
        Ok(outcome) => outcome,
        Err(error) => focused_document_worker_error(
            error,
            state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                .unwrap_or_else(|state| state),
            PENDING_WRITE.load(Ordering::Acquire),
        ),
    }
}

fn try_type_focused_document_append_unbounded(
    target_hwnd: u64,
    expected_pid: Option<u32>,
    text: &str,
    cancelled: &AtomicBool,
    state: &AtomicU8,
) -> FocusedDocumentTypeOutcome {
    use windows::core::BSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomationValuePattern};

    let thread_id = unsafe { GetCurrentThreadId() };
    let coinit = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if coinit.is_err() {
        return focused_document_not_applicable(thread_id, Some(coinit.0), "co_initialize_failed");
    }
    // Declared before UIA interfaces so they drop before CoUninitialize.
    let _com = ComApartment;
    if cancelled.load(Ordering::Acquire) {
        return focused_document_not_applicable(
            thread_id,
            Some(coinit.0),
            "cancelled_before_probe",
        );
    }

    let not_applicable = |stage| focused_document_not_applicable(thread_id, Some(coinit.0), stage);
    let outcome = (|| {
        let target = HWND(target_hwnd as *mut _);
        if !exact_foreground_target(target, expected_pid) {
            return not_applicable("target_not_exact_foreground");
        }
        let Some(target_pid) = crate::win32::windows::window_owner_pid(target_hwnd) else {
            return not_applicable("target_pid_unavailable");
        };
        if expected_pid.is_some_and(|pid| pid != target_pid) {
            return not_applicable("target_pid_mismatch");
        }

        let Ok(uia) = (unsafe {
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        }) else {
            return not_applicable("uia_create_failed");
        };
        let Ok(focused) = (unsafe { uia.GetFocusedElement() }) else {
            return not_applicable("focused_element_unavailable");
        };
        if !focused_document_matches_target(&focused, target, target_pid) {
            return not_applicable("focused_element_not_exact_document");
        }

        let Ok(value_pattern) = (unsafe { focused.GetCurrentPattern(UIA_ValuePatternId) }) else {
            return not_applicable("value_pattern_unavailable");
        };
        let Ok(value_pattern) = value_pattern.cast::<IUIAutomationValuePattern>() else {
            return not_applicable("value_pattern_cast_failed");
        };
        let Ok(initial_read_only) = (unsafe { value_pattern.CurrentIsReadOnly() }) else {
            return not_applicable("value_read_only_state_unavailable");
        };
        let Ok(text_pattern) = (unsafe { focused.GetCurrentPattern(UIA_TextPatternId) }) else {
            return not_applicable("text_pattern_unavailable");
        };
        let Ok(text_pattern) = text_pattern.cast::<IUIAutomationTextPattern>() else {
            return not_applicable("text_pattern_cast_failed");
        };
        let probed = unsafe { text_pattern.DocumentRange() }
            .and_then(|range| unsafe { range.GetText(MAX_TYPED_DOCUMENT_UNITS + 1) });
        if !probed.is_ok_and(|text| within_size_cap(text.len())) {
            return not_applicable("document_over_size_cap");
        }
        let Ok(initial_value) = (unsafe { value_pattern.CurrentValue() }) else {
            return not_applicable("value_unavailable");
        };
        let initial_value = initial_value.to_string();
        // SetValue rewrites the whole document as plain text, so content it
        // cannot carry back must keep the key route.
        if initial_value.contains(OBJECT_REPLACEMENT)
            || !document_is_plain_text(&uia, &text_pattern)
        {
            return not_applicable("document_has_rich_content");
        }
        let single_text_selection = unsafe { text_pattern.SupportedTextSelection() }
            .is_ok_and(|selection| selection == SupportedTextSelection_Single);
        let document_matches = unsafe { text_pattern.DocumentRange() }
            .and_then(|range| unsafe { range.GetText(-1) })
            .is_ok_and(|value| value == initial_value);
        let read_selection = || -> Option<(String, String)> {
            let selection = unsafe { text_pattern.GetSelection() }
                .ok()
                .filter(|selections| unsafe { selections.Length() }.ok() == Some(1))
                .and_then(|selections| unsafe { selections.GetElement(0) }.ok())?;
            let before = unsafe { text_pattern.DocumentRange() }
                .and_then(|document| unsafe { document.Clone() })
                .ok()?;
            unsafe {
                before.MoveEndpointByRange(
                    TextPatternRangeEndpoint_End,
                    &selection,
                    TextPatternRangeEndpoint_Start,
                )
            }
            .ok()?;
            let prefix = unsafe { before.GetText(-1) }.ok()?.to_string();
            let selected = unsafe { selection.GetText(-1) }.ok()?.to_string();
            Some((prefix, selected))
        };
        let probed_selection = read_selection();
        let mapped = probed_selection
            .as_ref()
            .and_then(|(prefix, selected)| split_at_selection(&initial_value, prefix, selected));
        if !focused_document_eligible(
            exact_foreground_target(target, Some(target_pid)),
            !initial_read_only.as_bool(),
            mapped.is_some(),
            single_text_selection,
            document_matches,
        ) {
            return not_applicable("initial_eligibility_not_met");
        }

        let Ok(focused_before_write) = (unsafe { uia.GetFocusedElement() }) else {
            return not_applicable("focused_element_changed_before_write");
        };
        if !exact_foreground_target(target, Some(target_pid))
            || !unsafe { uia.CompareElements(&focused, &focused_before_write) }
                .is_ok_and(|same| same.as_bool())
            || !focused_document_matches_target(&focused_before_write, target, target_pid)
        {
            return not_applicable("target_or_focus_changed_before_write");
        }
        let Ok(read_only_before_write) = (unsafe { value_pattern.CurrentIsReadOnly() }) else {
            return not_applicable("value_read_only_state_unavailable_before_write");
        };
        let Ok(value_before_write) = (unsafe { value_pattern.CurrentValue() }) else {
            return not_applicable("value_unavailable_before_write");
        };
        if read_only_before_write.as_bool() || value_before_write != initial_value {
            return not_applicable("value_changed_or_read_only_before_write");
        }
        if !unsafe { text_pattern.DocumentRange() }
            .and_then(|range| unsafe { range.GetText(-1) })
            .is_ok_and(|value| value == initial_value)
        {
            return not_applicable("document_changed_before_write");
        }
        if !same_selection(probed_selection.as_ref(), read_selection().as_ref()) {
            return not_applicable("selection_changed_before_write");
        }
        // If the UIA deadline fired during pre-write checks, do not start a
        // write after the caller has already returned an unknown result.
        PENDING_WRITE.store(true, Ordering::Release);
        if cancelled.load(Ordering::Acquire)
            || state
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return not_applicable("cancelled_before_write");
        }

        let Some((prefix, rest)) = mapped else {
            return not_applicable("selection_not_mapped");
        };
        let caret_text = format!("{prefix}{text}");
        let expected_value = format!("{caret_text}{rest}");
        let write_result = (|| -> Result<(), &'static str> {
            let value = BSTR::from(expected_value.as_str());
            unsafe { value_pattern.SetValue(&value) }
                .map_err(|_| "ValuePattern.SetValue failed after dispatch")?;
            let actual_value = unsafe { value_pattern.CurrentValue() }
                .map_err(|_| "ValuePattern read-back failed after dispatch")?;
            if !normalized_text_matches(&expected_value, &actual_value.to_string()) {
                return Err("ValuePattern read-back did not match after dispatch");
            }

            let document_range = unsafe { text_pattern.DocumentRange() }
                .map_err(|_| "TextPattern DocumentRange failed after dispatch")?;
            // Text written at the end keeps the caret at the document end, so
            // an append needs no character count to place it.
            let mut landed = rest
                .is_empty()
                .then(|| unsafe { document_range.Clone() })
                .transpose()
                .map_err(|_| "TextPattern range clone failed after dispatch")?;
            for units in caret_unit_counts(&caret_text, &actual_value.to_string()) {
                if landed.is_some() {
                    break;
                }
                let range = unsafe { document_range.Clone() }
                    .map_err(|_| "TextPattern range clone failed after dispatch")?;
                unsafe {
                    range.MoveEndpointByRange(
                        TextPatternRangeEndpoint_End,
                        &document_range,
                        TextPatternRangeEndpoint_Start,
                    )
                }
                .map_err(|_| "TextPattern caret collapse failed after dispatch")?;
                unsafe {
                    range.MoveEndpointByUnit(
                        TextPatternRangeEndpoint_End,
                        TextUnit_Character,
                        units,
                    )
                }
                .map_err(|_| "TextPattern caret move failed after dispatch")?;
                let reached = unsafe { range.GetText(-1) }
                    .map_err(|_| "TextPattern caret read-back failed after dispatch")?;
                if normalized_text_matches(&caret_text, &reached.to_string()) {
                    landed = Some(range);
                }
            }
            let Some(caret_range) = landed else {
                return Err("TextPattern caret did not land after the typed text");
            };
            unsafe {
                caret_range.MoveEndpointByRange(
                    TextPatternRangeEndpoint_Start,
                    &caret_range,
                    TextPatternRangeEndpoint_End,
                )
            }
            .map_err(|_| "TextPattern caret collapse failed after dispatch")?;
            unsafe { caret_range.Select() }
                .map_err(|_| "TextPattern caret placement failed after dispatch")?;

            let selections = unsafe { text_pattern.GetSelection() }
                .map_err(|_| "TextPattern selection read-back failed after dispatch")?;
            if unsafe { selections.Length() }
                .map_err(|_| "TextPattern selection count failed after dispatch")?
                != 1
            {
                return Err("TextPattern did not report exactly one caret after dispatch");
            }
            let selection = unsafe { selections.GetElement(0) }
                .map_err(|_| "TextPattern caret read-back failed after dispatch")?;
            let collapsed = unsafe {
                selection.CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    &selection,
                    TextPatternRangeEndpoint_End,
                )
            }
            .map_err(|_| "TextPattern caret comparison failed after dispatch")?;
            let fresh_document_range = unsafe { text_pattern.DocumentRange() }
                .map_err(|_| "TextPattern verification range failed after dispatch")?;
            let before_caret = unsafe { fresh_document_range.Clone() }
                .map_err(|_| "TextPattern range clone failed after dispatch")?;
            unsafe {
                before_caret.MoveEndpointByRange(
                    TextPatternRangeEndpoint_End,
                    &selection,
                    TextPatternRangeEndpoint_Start,
                )
            }
            .map_err(|_| "TextPattern caret range failed after dispatch")?;
            let before_text = unsafe { before_caret.GetText(-1) }
                .map_err(|_| "TextPattern caret text failed after dispatch")?;
            if collapsed != 0 || !normalized_text_matches(&caret_text, &before_text.to_string()) {
                return Err("TextPattern caret was not right after the typed text after dispatch");
            }
            let final_value = unsafe { value_pattern.CurrentValue() }
                .map_err(|_| "Final ValuePattern read-back failed after dispatch")?;
            if !normalized_text_matches(&expected_value, &final_value.to_string()) {
                return Err("ValuePattern changed during caret placement");
            }
            if !exact_foreground_target(target, Some(target_pid)) {
                return Err("Foreground identity changed after dispatch");
            }
            let focused_after = unsafe { uia.GetFocusedElement() }
                .map_err(|_| "Focused element read-back failed after dispatch")?;
            let same_element = unsafe { uia.CompareElements(&focused, &focused_after) }
                .map_err(|_| "Focused element identity comparison failed after dispatch")?;
            if !same_element.as_bool()
                || !focused_document_matches_target(&focused_after, target, target_pid)
            {
                return Err("Focused Document target changed after dispatch");
            }
            Ok(())
        })();
        classify_focused_document_write(true, write_result)
    })();

    outcome
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

fn focused_document_not_applicable(
    thread_id: u32,
    coinit_hresult: Option<i32>,
    stage: &'static str,
) -> FocusedDocumentTypeOutcome {
    tracing::debug!(
        target: "focused_document_type",
        thread_id,
        coinit_hresult = ?coinit_hresult.map(|hr| format!("0x{:08X}", hr as u32)),
        stage,
        "focused UIA type path not applicable"
    );
    FocusedDocumentTypeOutcome::NotApplicable
}

fn focused_document_matches_target(
    element: &IUIAutomationElement,
    target: HWND,
    target_pid: u32,
) -> bool {
    if unsafe { element.CurrentProcessId() }.ok() != Some(target_pid as i32)
        || !unsafe { element.CurrentControlType() }
            .is_ok_and(|control_type| control_type.0 == UIA_DocumentControlTypeId.0)
        || !unsafe { element.CurrentIsEnabled() }.is_ok_and(|enabled| enabled.as_bool())
        || !unsafe { element.CurrentHasKeyboardFocus() }.is_ok_and(|focused| focused.as_bool())
    {
        return false;
    }
    let Ok(element_hwnd) = (unsafe { element.CurrentNativeWindowHandle() }) else {
        return false;
    };
    !element_hwnd.0.is_null()
        && crate::win32::windows::window_owner_pid(element_hwnd.0 as usize as u64)
            == Some(target_pid)
        && unsafe { GetAncestor(element_hwnd, GA_ROOT) } == target
}

/// True when the document range has no embedded objects and no formatting
/// attribute that varies across it.
fn document_is_plain_text(uia: &IUIAutomation, text_pattern: &IUIAutomationTextPattern) -> bool {
    let Ok(document) = (unsafe { text_pattern.DocumentRange() }) else {
        return false;
    };
    if !unsafe { document.GetChildren() }
        .and_then(|children| unsafe { children.Length() })
        .is_ok_and(|count| count == 0)
    {
        return false;
    }
    let Ok(mixed) = (unsafe { uia.ReservedMixedAttributeValue() }) else {
        return false;
    };
    FORMAT_ATTRIBUTES.iter().all(|&attribute| {
        unsafe { document.GetAttributeValue(attribute) }.is_ok_and(|value| {
            IUnknown::try_from(&value)
                .ok()
                .is_none_or(|value| value != mixed)
        })
    })
}

fn exact_foreground_target(target: HWND, expected_pid: Option<u32>) -> bool {
    !target.0.is_null()
        && unsafe { IsWindow(target) }.as_bool()
        && unsafe { GetForegroundWindow() } == target
        && crate::win32::windows::window_owner_pid(target.0 as usize as u64)
            .is_some_and(|pid| expected_pid.is_none_or(|expected| expected == pid))
}

fn focused_document_eligible(
    exact_target: bool,
    writable_value: bool,
    selection_mapped: bool,
    single_text_selection: bool,
    document_matches: bool,
) -> bool {
    exact_target && writable_value && single_text_selection && selection_mapped && document_matches
}

/// Character counts to try for the caret after `caret_text`: the text as
/// typed with each line break as one unit, then the same span of the written
/// document as the provider stores it, where a CRLF may count as two.
fn caret_unit_counts(caret_text: &str, written: &str) -> Vec<i32> {
    let typed = normalize_document_line_endings(caret_text);
    let stored = stored_prefix(written, typed.chars().count());
    let mut counts = Vec::new();
    for text in [typed.as_str(), stored] {
        for units in [text.encode_utf16().count(), text.chars().count()] {
            let units = units as i32;
            if !counts.contains(&units) {
                counts.push(units);
            }
        }
    }
    counts
}

/// The leading part of `written` that holds `line_units` characters once
/// each CRLF, CR or LF counts as one line break.
fn stored_prefix(written: &str, line_units: usize) -> &str {
    let mut seen = 0;
    let mut chars = written.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if seen == line_units {
            return &written[..index];
        }
        seen += 1;
        if ch == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
            chars.next();
        }
    }
    written
}

fn within_size_cap(probed_units: usize) -> bool {
    probed_units <= MAX_TYPED_DOCUMENT_UNITS as usize
}

fn same_selection(probed: Option<&(String, String)>, now: Option<&(String, String)>) -> bool {
    probed.is_some() && probed == now
}

fn split_at_selection(value: &str, prefix: &str, selected: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix(prefix)?.strip_prefix(selected)?;
    Some((prefix.to_owned(), rest.to_owned()))
}

fn classify_focused_document_write(
    set_value_attempted: bool,
    result: Result<(), &'static str>,
) -> FocusedDocumentTypeOutcome {
    if !set_value_attempted {
        return FocusedDocumentTypeOutcome::NotApplicable;
    }
    match result {
        Ok(()) => FocusedDocumentTypeOutcome::Confirmed,
        Err(stage) => FocusedDocumentTypeOutcome::Unknown(stage),
    }
}

fn normalize_document_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn normalized_text_matches(expected: &str, actual: &str) -> bool {
    normalize_document_line_endings(expected) == normalize_document_line_endings(actual)
}

fn focused_document_worker_error(
    error: UiaDeadlineError,
    state: u8,
    pending_write: bool,
) -> FocusedDocumentTypeOutcome {
    if pending_write {
        return FocusedDocumentTypeOutcome::Unknown("focused UIA write is still in flight");
    }
    if state == 0 {
        return FocusedDocumentTypeOutcome::NotApplicable;
    }
    match error {
        UiaDeadlineError::Busy => FocusedDocumentTypeOutcome::NotApplicable,
        UiaDeadlineError::Timeout => {
            FocusedDocumentTypeOutcome::Unknown("focused UIA call timed out")
        }
        UiaDeadlineError::Unavailable => {
            FocusedDocumentTypeOutcome::Unknown("focused UIA worker ended without a result")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        caret_unit_counts, classify_focused_document_write, focused_document_eligible,
        focused_document_worker_error, normalize_document_line_endings, normalized_text_matches,
        same_selection, split_at_selection, within_size_cap, FocusedDocumentTypeOutcome,
        MAX_TYPED_DOCUMENT_UNITS,
    };
    use crate::uia::windows_enum::UiaDeadlineError;

    #[test]
    fn focused_document_requires_exact_target_and_safe_value_selection() {
        assert!(focused_document_eligible(true, true, true, true, true));
        assert!(!focused_document_eligible(false, true, true, true, true));
        assert!(!focused_document_eligible(true, false, true, true, true));
        assert!(!focused_document_eligible(true, true, true, false, true));
        assert!(!focused_document_eligible(true, true, true, true, false));
        assert!(!focused_document_eligible(true, true, false, true, true));
        assert!(!focused_document_eligible(true, true, false, true, false));
        assert!(!focused_document_eligible(true, true, false, false, true));
        assert!(!focused_document_eligible(false, true, false, true, true));
        assert!(!focused_document_eligible(true, false, false, true, true));
    }

    #[test]
    fn the_size_probe_admits_the_cap_and_refuses_one_unit_more() {
        let cap = MAX_TYPED_DOCUMENT_UNITS as usize;
        assert!(within_size_cap(0));
        assert!(within_size_cap(cap));
        assert!(!within_size_cap(cap + 1));
    }

    #[test]
    fn a_selection_that_moved_before_the_write_sends_the_text_by_keys() {
        let world = ("hello ".to_owned(), "world".to_owned());
        let caret = ("hello world".to_owned(), String::new());
        assert!(same_selection(Some(&world), Some(&world.clone())));
        assert!(!same_selection(Some(&world), Some(&caret)));
        assert!(!same_selection(Some(&world), None));
        assert!(!same_selection(None, None));
    }

    #[test]
    fn the_caret_tries_utf16_units_before_scalar_values_when_they_differ() {
        assert_eq!(caret_unit_counts("line1\rab", "line1\rab tail"), vec![8]);
        assert_eq!(caret_unit_counts("a\r\nb", "a\rb tail"), vec![3]);
        assert_eq!(
            caret_unit_counts("x \u{1f642} y", "x \u{1f642} y tail"),
            vec![6, 5]
        );
    }

    #[test]
    fn the_caret_also_tries_the_stored_text_where_a_crlf_counts_as_two() {
        assert_eq!(caret_unit_counts("a\nb", "a\r\nb tail"), vec![3, 4]);
        assert_eq!(caret_unit_counts("a\n", "a\r\nb"), vec![2, 3]);
        assert_eq!(stored_prefix("a\r\nb\r\nc", 3), "a\r\nb");
        assert_eq!(stored_prefix("a\r\n", 2), "a\r\n");
        assert_eq!(stored_prefix("ab", 5), "ab");
    }

    #[test]
    fn a_selection_maps_onto_the_value_only_when_prefix_selection_and_rest_rebuild_it() {
        assert_eq!(
            split_at_selection("line1\rab", "line1\rab", ""),
            Some(("line1\rab".to_owned(), String::new()))
        );
        assert_eq!(
            split_at_selection("hello world", "hello ", ""),
            Some(("hello ".to_owned(), "world".to_owned()))
        );
        assert_eq!(
            split_at_selection("xyz tail", "", "xyz"),
            Some((String::new(), " tail".to_owned()))
        );
        assert_eq!(split_at_selection("hello", "help", ""), None);
        assert_eq!(split_at_selection("hello", "he", "x"), None);
    }

    #[test]
    fn only_fully_verified_post_write_state_is_confirmed() {
        assert_eq!(
            classify_focused_document_write(false, Err("probe failed")),
            FocusedDocumentTypeOutcome::NotApplicable
        );
        assert_eq!(
            classify_focused_document_write(true, Err("caret failed")),
            FocusedDocumentTypeOutcome::Unknown("caret failed")
        );
        assert_eq!(
            classify_focused_document_write(true, Ok(())),
            FocusedDocumentTypeOutcome::Confirmed
        );
    }

    #[test]
    fn busy_during_pending_write_disallows_native_fallback() {
        assert!(matches!(
            focused_document_worker_error(UiaDeadlineError::Busy, 0, true),
            FocusedDocumentTypeOutcome::Unknown(_)
        ));
        assert_eq!(
            focused_document_worker_error(UiaDeadlineError::Busy, 0, false),
            FocusedDocumentTypeOutcome::NotApplicable
        );
    }

    #[test]
    fn worker_errors_before_write_allow_native_fallback() {
        for error in [
            UiaDeadlineError::Busy,
            UiaDeadlineError::Timeout,
            UiaDeadlineError::Unavailable,
        ] {
            assert_eq!(
                focused_document_worker_error(error, 0, false),
                FocusedDocumentTypeOutcome::NotApplicable
            );
        }
    }

    #[test]
    fn worker_errors_after_write_never_enable_a_second_input_route() {
        for error in [UiaDeadlineError::Timeout, UiaDeadlineError::Unavailable] {
            assert!(matches!(
                focused_document_worker_error(error, 1, false),
                FocusedDocumentTypeOutcome::Unknown(_)
            ));
        }
    }

    #[test]
    fn line_ending_checks_preserve_blank_lines_and_non_ascii_text() {
        assert_eq!(
            normalize_document_line_endings("one\r\n\r\ntwo\rthree\n"),
            "one\n\ntwo\nthree\n"
        );
        assert!(normalized_text_matches(
            "one\r\n\r\ntwo\r\u{676d}\u{5dde}\u{4e1c}",
            "one\n\ntwo\n\u{676d}\u{5dde}\u{4e1c}"
        ));
        assert!(!normalized_text_matches("one\n\ntwo", "one\ntwo"));
    }
}
