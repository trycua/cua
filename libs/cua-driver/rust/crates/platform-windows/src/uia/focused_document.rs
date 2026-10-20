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

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, SupportedTextSelection_Single,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, UIA_DocumentControlTypeId,
    UIA_TextPatternId, UIA_ValuePatternId,
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
        try_type_focused_document_append_unbounded(
            target_hwnd,
            expected_pid,
            &text,
            &cancelled,
            &worker_state,
        )
    }) {
        Ok(outcome) => outcome,
        Err(_)
            if state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok() =>
        {
            FocusedDocumentTypeOutcome::NotApplicable
        }
        Err(error) => focused_document_worker_error(error),
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
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationValuePattern,
    };

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
        let Ok(initial_value) = (unsafe { value_pattern.CurrentValue() }) else {
            return not_applicable("value_unavailable");
        };
        let initial_value = initial_value.to_string();

        let Ok(text_pattern) = (unsafe { focused.GetCurrentPattern(UIA_TextPatternId) }) else {
            return not_applicable("text_pattern_unavailable");
        };
        let Ok(text_pattern) = text_pattern.cast::<IUIAutomationTextPattern>() else {
            return not_applicable("text_pattern_cast_failed");
        };
        let single_text_selection = unsafe { text_pattern.SupportedTextSelection() }
            .is_ok_and(|selection| selection == SupportedTextSelection_Single);
        let document_matches = unsafe { text_pattern.DocumentRange() }
            .and_then(|range| unsafe { range.GetText(-1) })
            .is_ok_and(|value| value.to_string() == initial_value);
        if !focused_document_eligible(
            exact_foreground_target(target, Some(target_pid)),
            !initial_read_only.as_bool(),
            initial_value.is_empty(),
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
        if read_only_before_write.as_bool() || value_before_write.to_string() != initial_value {
            return not_applicable("value_changed_or_read_only_before_write");
        }
        if !unsafe { text_pattern.DocumentRange() }
            .and_then(|range| unsafe { range.GetText(-1) })
            .is_ok_and(|value| value.to_string() == initial_value)
        {
            return not_applicable("document_changed_before_write");
        }
        // If the UIA deadline fired during pre-write checks, do not start a
        // write after the caller has already returned an unknown result.
        if cancelled.load(Ordering::Acquire)
            || state
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return not_applicable("cancelled_before_write");
        }

        let expected_value = text.to_owned();
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
            let caret_range = unsafe { document_range.Clone() }
                .map_err(|_| "TextPattern range clone failed after dispatch")?;
            unsafe {
                caret_range.MoveEndpointByRange(
                    TextPatternRangeEndpoint_Start,
                    &document_range,
                    TextPatternRangeEndpoint_End,
                )
            }
            .map_err(|_| "TextPattern caret collapse failed after dispatch")?;
            unsafe { caret_range.Select() }
                .map_err(|_| "TextPattern caret placement failed after dispatch")?;

            let fresh_document_range = unsafe { text_pattern.DocumentRange() }
                .map_err(|_| "TextPattern verification range failed after dispatch")?;
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
            let start_at_end = unsafe {
                selection.CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    &fresh_document_range,
                    TextPatternRangeEndpoint_End,
                )
            }
            .map_err(|_| "TextPattern caret start comparison failed after dispatch")?;
            let end_at_end = unsafe {
                selection.CompareEndpoints(
                    TextPatternRangeEndpoint_End,
                    &fresh_document_range,
                    TextPatternRangeEndpoint_End,
                )
            }
            .map_err(|_| "TextPattern caret end comparison failed after dispatch")?;
            if start_at_end != 0 || end_at_end != 0 {
                return Err("TextPattern caret was not at the document end after dispatch");
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
    value_is_empty: bool,
    single_text_selection: bool,
    document_matches: bool,
) -> bool {
    exact_target && writable_value && single_text_selection && value_is_empty && document_matches
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

fn focused_document_worker_error(error: UiaDeadlineError) -> FocusedDocumentTypeOutcome {
    match error {
        UiaDeadlineError::Busy => FocusedDocumentTypeOutcome::Unknown(
            "focused UIA worker is busy; an earlier write may still be in flight",
        ),
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
        classify_focused_document_write, focused_document_eligible, focused_document_worker_error,
        normalize_document_line_endings, normalized_text_matches, FocusedDocumentTypeOutcome,
    };
    use crate::uia::windows_enum::UiaDeadlineError;

    #[test]
    fn focused_document_requires_exact_target_and_safe_value_selection() {
        assert!(focused_document_eligible(true, true, true, true, true));
        assert!(!focused_document_eligible(true, true, true, true, false));
        assert!(!focused_document_eligible(true, true, false, true, true));
        assert!(!focused_document_eligible(true, true, false, true, false));
        assert!(!focused_document_eligible(true, true, false, false, true));
        assert!(!focused_document_eligible(false, true, false, true, true));
        assert!(!focused_document_eligible(true, false, false, true, true));
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
    fn busy_timed_out_or_lost_workers_never_enable_a_second_input_route() {
        assert!(matches!(
            focused_document_worker_error(UiaDeadlineError::Busy),
            FocusedDocumentTypeOutcome::Unknown(_)
        ));
        assert!(matches!(
            focused_document_worker_error(UiaDeadlineError::Timeout),
            FocusedDocumentTypeOutcome::Unknown(_)
        ));
        assert!(matches!(
            focused_document_worker_error(UiaDeadlineError::Unavailable),
            FocusedDocumentTypeOutcome::Unknown(_)
        ));
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
