//! Bounded element-token UIA activation. The worker retains the element and
//! the shared UIA permit until the provider returns, including after timeout.

use super::snapshot::RetainedElement;
use super::windows_enum::{
    run_uia_with_deadline_cancelable, ComInit, UiaDeadlineError, SUBTREE_OP_TIMEOUT,
};
use cua_driver_core::action_record::ActionTransport;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) enum ElementClick {
    Semantic {
        message: String,
        transport: ActionTransport,
        failed_calls: Vec<ActionTransport>,
    },
    Miss {
        failed_calls: Vec<ActionTransport>,
    },
}

fn while_active<T>(
    cancelled: &AtomicBool,
    action: impl FnOnce() -> T,
) -> Result<T, UiaDeadlineError> {
    if cancelled.load(Ordering::Acquire) {
        Err(UiaDeadlineError::Timeout)
    } else {
        Ok(action())
    }
}

pub(crate) fn try_click_element(
    element_guard: RetainedElement,
    hwnd: u64,
    idx: usize,
    cx: i32,
    cy: i32,
) -> Result<ElementClick, UiaDeadlineError> {
    run_uia_with_deadline_cancelable(
        "element click",
        SUBTREE_OP_TIMEOUT,
        "an unknown-effect error without another input attempt",
        move |cancelled| click_patterns(element_guard, hwnd, idx, cx, cy, &cancelled),
    )
    .and_then(|result| result)
}

fn click_patterns(
    element_guard: RetainedElement,
    hwnd: u64,
    idx: usize,
    cx: i32,
    cy: i32,
    cancelled: &AtomicBool,
) -> Result<ElementClick, UiaDeadlineError> {
    let _com = ComInit::new();
    // Release the retained interface before uninitializing this apartment.
    let element_guard = element_guard;
    let mut failed_calls = Vec::new();
    let ptr = element_guard.as_ptr();
    use windows::core::Interface;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
        IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, UIA_ExpandCollapsePatternId,
        UIA_InvokePatternId, UIA_SelectionItemPatternId, UIA_TogglePatternId,
    };
    let elem =
        std::mem::ManuallyDrop::new(unsafe { IUIAutomationElement::from_raw(ptr as *mut _) });

    // Try patterns in order of click-semantics specificity:
    //   1. Invoke      - buttons / hyperlinks (canonical)
    //   2. Toggle      - checkboxes (no Invoke)
    //   3. SelectionItem - radio buttons, listbox items
    //   4. ExpandCollapse - combo boxes, tree nodes
    // Each call is wrapped in the UWP foreground-steal bypass.
    //
    // Why this order: PR #1699's WinUI3 control parity tests
    // surfaced these pattern-dispatch gaps. Without these
    // fallthroughs, cua-driver couldn't drive WinUI3
    // CheckBox / RadioButton / ComboBox because PostMessage
    // doesn't reach their handlers (CoreInput dispatcher).
    let invoke_result = while_active(cancelled, || unsafe {
        elem.GetCurrentPattern(UIA_InvokePatternId)
    })?;
    if let Ok(pattern) = invoke_result {
        if let Ok(inv) = pattern.cast::<IUIAutomationInvokePattern>() {
            let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || {
                while_active(cancelled, || unsafe { inv.Invoke() })
            })?;
            match outcome {
                Ok(()) => {
                    return Ok(ElementClick::Semantic {
                        message: format!(
                            "✅ Performed UIA Invoke on [{idx}] (screen ({cx},{cy}))."
                        ),
                        transport: ActionTransport::WindowsUiaInvoke,
                        failed_calls,
                    });
                }
                Err(e) => tracing::debug!(target: "click",
                            "UIA Invoke on [{idx}]: {e}, trying Toggle"),
            }
            failed_calls.push(ActionTransport::WindowsUiaInvoke);
        }
    }
    let toggle_result = while_active(cancelled, || unsafe {
        elem.GetCurrentPattern(UIA_TogglePatternId)
    })?;
    if let Ok(pattern) = toggle_result {
        if let Ok(tg) = pattern.cast::<IUIAutomationTogglePattern>() {
            let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || {
                while_active(cancelled, || unsafe { tg.Toggle() })
            })?;
            if outcome.is_ok() {
                return Ok(ElementClick::Semantic {
                    message: format!("✅ Performed UIA Toggle on [{idx}] (screen ({cx},{cy}))."),
                    transport: ActionTransport::WindowsUiaToggle,
                    failed_calls,
                });
            }
            failed_calls.push(ActionTransport::WindowsUiaToggle);
        }
    }
    let sel_result = while_active(cancelled, || unsafe {
        elem.GetCurrentPattern(UIA_SelectionItemPatternId)
    })?;
    if let Ok(pattern) = sel_result {
        if let Ok(si) = pattern.cast::<IUIAutomationSelectionItemPattern>() {
            let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || {
                while_active(cancelled, || unsafe { si.Select() })
            })?;
            if outcome.is_ok() {
                return Ok(ElementClick::Semantic {
                    message: format!(
                        "✅ Performed UIA SelectionItem.Select on [{idx}] (screen ({cx},{cy}))."
                    ),
                    transport: ActionTransport::WindowsUiaSelection,
                    failed_calls,
                });
            }
            failed_calls.push(ActionTransport::WindowsUiaSelection);
        }
    }
    let exp_result = while_active(cancelled, || unsafe {
        elem.GetCurrentPattern(UIA_ExpandCollapsePatternId)
    })?;
    if let Ok(pattern) = exp_result {
        if let Ok(ec) = pattern.cast::<IUIAutomationExpandCollapsePattern>() {
            let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || {
                while_active(cancelled, || unsafe { ec.Expand() })
            })?;
            if outcome.is_ok() {
                return Ok(ElementClick::Semantic {
                    message: format!(
                        "✅ Performed UIA ExpandCollapse.Expand on [{idx}] (screen ({cx},{cy}))."
                    ),
                    transport: ActionTransport::WindowsUiaExpandCollapse,
                    failed_calls,
                });
            }
            failed_calls.push(ActionTransport::WindowsUiaExpandCollapse);
        }
    }

    Ok(ElementClick::Miss { failed_calls })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_operation_cannot_enter_another_provider_call() {
        let cancelled = AtomicBool::new(false);
        let mut calls = 0;
        assert_eq!(
            while_active(&cancelled, || {
                calls += 1;
                7
            }),
            Ok(7)
        );
        cancelled.store(true, Ordering::Release);
        assert_eq!(
            while_active(&cancelled, || {
                calls += 1;
                9
            }),
            Err(UiaDeadlineError::Timeout)
        );
        assert_eq!(calls, 1);
    }
}
