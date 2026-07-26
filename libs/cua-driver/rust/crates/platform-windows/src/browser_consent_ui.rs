//! Exact Windows UIA handling for Chromium's browser-owned debugging consent.

use std::time::{Duration, Instant};

use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationInvokePattern, UIA_InvokePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use crate::uia::UiaNode;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn normalized_text(node: &UiaNode) -> String {
    [
        node.name.as_deref(),
        node.value.as_deref(),
        node.automation_id.as_deref(),
        node.help_text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .trim()
    .to_ascii_lowercase()
}

fn release_nodes(nodes: &[UiaNode]) {
    for node in nodes.iter().filter(|node| node.element_ptr != 0) {
        unsafe { drop(IUIAutomationElement::from_raw(node.element_ptr as *mut _)) };
    }
}

fn is_in_web_content(nodes: &[UiaNode], node: &UiaNode) -> bool {
    let mut parent = node.parent_element_index;
    for _ in 0..nodes.len() {
        let Some(parent_index) = parent else {
            return false;
        };
        let Some(parent_node) = nodes
            .iter()
            .find(|candidate| candidate.element_index == Some(parent_index))
        else {
            return false;
        };
        if parent_node.control_type.eq_ignore_ascii_case("Document") {
            return true;
        }
        parent = parent_node.parent_element_index;
    }
    true
}

fn trusted_prompt_nodes(nodes: &[UiaNode]) -> impl Iterator<Item = &UiaNode> {
    nodes.iter().filter(|node| {
        !node.in_web_content
            && !node.control_type.eq_ignore_ascii_case("Document")
            && !is_in_web_content(nodes, node)
    })
}

fn remote_debugging_prompt_present(nodes: &[UiaNode]) -> bool {
    let has_title =
        trusted_prompt_nodes(nodes).any(|node| normalized_text(node) == "allow remote debugging?");
    let body = trusted_prompt_nodes(nodes)
        .map(normalized_text)
        .collect::<Vec<_>>()
        .join(" ");
    has_title
        && body.contains("external app wants full control")
        && body.contains("saved data, cookies and site data")
        && body.contains("navigate to any url")
}

fn exact_allow_button(nodes: &[UiaNode]) -> Result<Option<usize>, BrowserRefusal> {
    if !remote_debugging_prompt_present(nodes) {
        return Ok(None);
    }
    let matches = trusted_prompt_nodes(nodes)
        .filter(|node| {
            node.control_type == "Button"
                && normalized_text(node) == "allow"
                && node.actions.iter().any(|action| action == "invoke")
                && node.element_ptr != 0
        })
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple exact Allow actions matched the browser consent prompt",
        )),
    }
}

unsafe fn invoke(element_ptr: usize) -> Result<(), BrowserRefusal> {
    let element = IUIAutomationElement::from_raw(element_ptr as *mut _);
    let result = element
        .GetCurrentPattern(UIA_InvokePatternId)
        .and_then(|pattern| pattern.cast::<IUIAutomationInvokePattern>())
        .and_then(|pattern| pattern.Invoke());
    std::mem::forget(element);
    result.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("the exact browser consent action failed: {error}"),
        )
    })
}

fn prove_window_owner(hwnd: u64, pid: u32) -> Result<(), BrowserRefusal> {
    let mut owner = 0u32;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut owner)) };
    if owner != pid {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            "the approved browser window changed ownership before consent",
        ));
    }
    Ok(())
}

pub async fn handle(
    request: BrowserConsentRequest,
) -> Result<BrowserConsentOutcome, BrowserRefusal> {
    let pid = u32::try_from(request.pid).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved browser pid is outside the Windows process-id range",
        )
    })?;
    prove_window_owner(request.window_id, pid)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut saw_prompt = false;
    loop {
        prove_window_owner(request.window_id, pid)?;
        let hwnd = request.window_id;
        let tree = tokio::task::spawn_blocking(move || crate::uia::walk_tree(hwnd, None))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not inspect the browser consent UI: {error}"),
                )
            })?;
        let prompt_present = remote_debugging_prompt_present(&tree.nodes);
        saw_prompt |= prompt_present;
        match exact_allow_button(&tree.nodes) {
            Ok(Some(element)) => {
                let invoked = unsafe { invoke(element) };
                release_nodes(&tree.nodes);
                invoked?;
                return Ok(BrowserConsentOutcome::Accepted);
            }
            Ok(None) => release_nodes(&tree.nodes),
            Err(error) => {
                release_nodes(&tree.nodes);
                return Err(error);
            }
        }
        if saw_prompt && !prompt_present {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRevoked,
                "the person dismissed the browser consent prompt",
            ));
        }
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "no exact Chromium remote-debugging consent prompt appeared for reconnect attempt {}",
                    request.attempt
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(control_type: &str, name: &str, actions: &[&str]) -> UiaNode {
        UiaNode {
            element_index: (!actions.is_empty()).then_some(0),
            control_type: control_type.to_owned(),
            name: Some(name.to_owned()),
            value: None,
            automation_id: None,
            help_text: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            element_ptr: 7,
            center_x: 0,
            center_y: 0,
            rect: None,
            msaa_role: None,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }

    fn prompt() -> Vec<UiaNode> {
        vec![
            node("Text", "Allow remote debugging?", &[]),
            node(
                "Text",
                "An external app wants full control. This includes access to your saved data, cookies and site data, and the ability to navigate to any URL.",
                &[],
            ),
            node("Button", "Cancel", &["invoke"]),
            node("Button", "Allow", &["invoke"]),
        ]
    }

    #[test]
    fn matcher_requires_exact_security_prompt_and_unique_allow_action() {
        assert_eq!(exact_allow_button(&prompt()).unwrap(), Some(7));
        assert_eq!(
            exact_allow_button(&[node("Button", "Allow", &["invoke"])]).unwrap(),
            None
        );
    }

    #[test]
    fn matcher_refuses_ambiguous_allow_actions() {
        let mut nodes = prompt();
        nodes.push(node("Button", "Allow", &["invoke"]));
        assert_eq!(
            exact_allow_button(&nodes).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matcher_ignores_a_spoofed_prompt_inside_web_content() {
        let mut nodes = prompt();
        nodes.insert(0, node("Document", "Example page", &[]));
        nodes[0].element_index = Some(42);
        for child in &mut nodes[1..] {
            child.parent_element_index = Some(42);
            child.in_web_content = true;
        }
        assert_eq!(exact_allow_button(&nodes).unwrap(), None);
    }
}
