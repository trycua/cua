//! Exact macOS handling for Chrome's browser-owned remote-debugging consent.

use std::time::{Duration, Instant};
use std::{collections::HashSet, iter};

use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};

use crate::ax::bindings::{kAXErrorSuccess, perform_action, AXUIElementRef};
use crate::ax::tree::{walk_tree_bounded, AXNode, DEFAULT_MAX_DEPTH};

// Large Chromium pages can put the browser-owned consent sheet after the
// ordinary 2,000-node snapshot cap. Keep this privileged scan bounded while
// allowing enough headroom to inspect Chrome's top-level sheet on pages such
// as Gmail. The matcher below still requires one exact AXSheet and one exact
// semantic Allow action before it will press anything.
const CONSENT_MAX_ELEMENTS: usize = 5_000;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn normalized_text(node: &AXNode) -> String {
    let mut parts = Vec::new();
    for text in [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let text = text.trim().to_ascii_lowercase();
        if !text.is_empty() && !parts.contains(&text) {
            parts.push(text);
        }
    }
    parts.join(" ")
}

fn release_actionable_nodes(nodes: &[AXNode]) {
    for node in nodes.iter().filter(|node| node.element_index.is_some()) {
        unsafe { CFRelease(node.element_ptr as CFTypeRef) };
    }
}

fn consent_surface_ids(
    windows: impl IntoIterator<Item = crate::windows::WindowInfo>,
    pid: i32,
    approved_window_id: u32,
) -> Vec<u32> {
    let mut windows = windows
        .into_iter()
        .filter(|window| {
            window.pid == pid
                && window.title != "Allow remote debugging?"
                && !window.title.trim().is_empty()
                && window.bounds.width > 0.0
                && window.bounds.height > 0.0
        })
        .collect::<Vec<_>>();
    windows.sort_by_key(|window| std::cmp::Reverse(window.z_index));
    let mut seen = HashSet::new();
    iter::once(approved_window_id)
        .chain(windows.into_iter().map(|window| window.window_id))
        .filter(|window_id| seen.insert(*window_id))
        .collect()
}

fn is_pressable_button(node: &AXNode) -> bool {
    node.role == "AXButton" && node.actions.iter().any(|action| action == "AXPress")
}

/// Splits an accessibility identifier into lowercase terms.
///
/// Identifiers are matched term by term rather than by substring, so an
/// unrelated control cannot turn into consent evidence: `disallow` contains
/// "allow" and `wallow-confirm` contains "confirm", but neither names the
/// decision. Both separators and camel-case boundaries split terms, so
/// `remote-debugging-allow` and `allowRemoteDebugging` are read the same way.
fn identifier_terms(node: &AXNode) -> Vec<String> {
    let Some(identifier) = node.identifier.as_deref() else {
        return Vec::new();
    };
    let mut spaced = String::with_capacity(identifier.len());
    let mut previous: Option<char> = None;
    let mut characters = identifier.chars().peekable();
    while let Some(character) = characters.next() {
        if character.is_ascii_uppercase()
            && (previous.is_some_and(|value| value.is_ascii_lowercase() || value.is_ascii_digit())
                || (previous.is_some_and(|value| value.is_ascii_uppercase())
                    && characters
                        .peek()
                        .is_some_and(|value| value.is_ascii_lowercase())))
        {
            spaced.push(' ');
        }
        spaced.push(character);
        previous = Some(character);
    }
    spaced
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

/// The identifier must name both halves of the decision: `allow` on its own is
/// too common to identify a prompt, and `debug`/`confirm` is what ties the
/// signal to a consent sheet rather than to any sheet the browser renders.
fn identifier_allows_remote_debugging(node: &AXNode) -> bool {
    let terms = identifier_terms(node);
    let names = |needle: &str| terms.iter().any(|term| term == needle);
    names("allow") && (names("debug") || names("debugging") || names("confirm"))
}

fn identifier_is_cancel(node: &AXNode) -> bool {
    identifier_terms(node).iter().any(|term| term == "cancel")
}

fn semantic_allow(node: &AXNode) -> bool {
    if !is_pressable_button(node) {
        return false;
    }
    let label = normalized_text(node);
    matches!(label.as_str(), "allow" | "allow remote debugging")
        || identifier_allows_remote_debugging(node)
}

fn semantic_cancel(node: &AXNode) -> bool {
    is_pressable_button(node) && (normalized_text(node) == "cancel" || identifier_is_cancel(node))
}

fn validate_sheet_decisions(sheet: &[AXNode]) -> Result<(), BrowserRefusal> {
    if sheet
        .iter()
        .any(|node| semantic_allow(node) && semantic_cancel(node))
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "a browser consent action matched both allow and cancel decisions",
        ));
    }
    Ok(())
}

/// The nodes of one `AXSheet` subtree, starting at the sheet itself.
fn sheet_subtree(nodes: &[AXNode], sheet_index: usize) -> &[AXNode] {
    let end = nodes
        .iter()
        .enumerate()
        .skip(sheet_index + 1)
        .find(|(_, node)| node.depth <= nodes[sheet_index].depth)
        .map_or(nodes.len(), |(index, _)| index);
    &nodes[sheet_index..end]
}

/// True when the sheet states the prompt in English. Kept as one predicate so
/// detection and the matchers cannot disagree about what an English host is.
fn sheet_prompt_is_english(sheet: &[AXNode]) -> bool {
    sheet.iter().any(|node| {
        let text = normalized_text(node);
        text.contains("remote debugging") || text.contains("remote-debugging")
    })
}

/// Recognizes Chrome's remote-debugging consent sheet without depending on the
/// host language.
///
/// Detection must not be gated on the prompt's visible text. That text is
/// localized, so an English-only gate rejects the sheet on a non-English host
/// before either button matcher can run, which leaves teardown unable to
/// dismiss the sheet it is looking for. The English text stays as a fast path
/// so English hosts behave exactly as before; it is no longer the only way in.
fn sheet_is_consent_prompt(sheet: &[AXNode]) -> bool {
    // The consent sheet is browser chrome. A dialog the page renders sits
    // under an `AXWebArea`, so web content is never the consent sheet — the
    // macOS counterpart of the trust gate the Windows matcher applies to
    // every node it considers.
    if sheet.iter().any(|node| node.in_web_content) {
        return false;
    }
    // Unchanged fast path: the browser UI is running in English.
    if sheet_prompt_is_english(sheet) {
        return true;
    }
    // Localized fast path: the accessibility identifier is not localized, so a
    // pressable button that names the allow decision identifies the sheet in
    // any language. Only the allow side is specific enough to stand alone
    // here — a bare "cancel" identifier is shared with Chrome's own save and
    // print sheets, which this code must never dismiss.
    sheet
        .iter()
        .any(|node| is_pressable_button(node) && identifier_allows_remote_debugging(node))
}

/// Every sheet in `nodes` that carries the remote-debugging consent decision.
///
/// Detection, allow matching and cancel matching all read this one view, so
/// they cannot disagree about which sheets are in scope.
fn consent_sheets(nodes: &[AXNode]) -> Vec<&[AXNode]> {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.role == "AXSheet")
        .map(|(index, _)| sheet_subtree(nodes, index))
        .filter(|sheet| sheet_is_consent_prompt(sheet))
        .collect()
}

fn remote_debugging_sheet_present(nodes: &[AXNode]) -> bool {
    !consent_sheets(nodes).is_empty()
}

fn exact_allow_button(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    let mut matches = Vec::new();
    for sheet in consent_sheets(nodes) {
        validate_sheet_decisions(sheet)?;
        for node in sheet.iter().filter(|node| semantic_allow(node)) {
            matches.push(node.element_ptr);
        }
    }
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple semantic allow actions matched the browser consent sheet",
        )),
    }
}

fn exact_cancel_button(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    let mut matches = Vec::new();
    for sheet in consent_sheets(nodes) {
        validate_sheet_decisions(sheet)?;
        // Language-independent structural signal: a consent sheet that exposes
        // exactly two pressable buttons presents a decision pair, so once the
        // allow side is identified the remaining button is the cancel side in
        // every locale. The allow identifier proves the localized sheet;
        // the cancel button does not need its own identifier.
        //
        // It only runs for sheets that English text could not prove. On an
        // English host the label path below already decides, and "not allow"
        // is not the same thing as "cancel" — a sheet pairing `Allow` with
        // `Deny` or `Block` must keep refusing instead of pressing the other
        // button, which is what teardown has always done.
        if !sheet_prompt_is_english(sheet) {
            let pressable = sheet
                .iter()
                .filter(|node| is_pressable_button(node))
                .collect::<Vec<_>>();
            if pressable.len() == 2 {
                let allow = pressable
                    .iter()
                    .filter(|node| semantic_allow(node))
                    .collect::<Vec<_>>();
                let [allow] = allow.as_slice() else {
                    return Err(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        "the localized browser consent pair exposed no unique semantic allow action",
                    ));
                };
                let Some(cancel) = pressable
                    .iter()
                    .find(|node| node.element_ptr != allow.element_ptr)
                else {
                    return Err(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        "the localized browser consent pair exposed no distinct cancel action",
                    ));
                };
                matches.push(cancel.element_ptr);
                continue;
            }
        }
        for node in sheet {
            // The button label is localized, so a label equality check only
            // matches English hosts. The accessibility identifier is not
            // localized; accept it as an equivalent signal, mirroring how
            // `semantic_allow` already considers the identifier alongside the
            // label. The label check is kept so English hosts are unaffected.
            // Matching a bare "cancel" term is safe here only because the
            // sheet has already been proven to be the consent sheet — the
            // sheet, not the button, carries the meaning — and ambiguity still
            // refuses below.
            if semantic_cancel(node) {
                matches.push(node.element_ptr);
            }
        }
    }
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple semantic cancel actions matched the browser consent sheet",
        )),
    }
}

/// Dismiss any exact Chrome-owned remote-debugging sheet before teardown.
/// Turning off the setting does not reliably close a sheet that an existing
/// WebSocket connection already presented.
pub fn dismiss(pid: i32, approved_window_id: u32) -> Result<bool, BrowserRefusal> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut dismissed = false;
    loop {
        let trees = consent_surface_ids(crate::windows::all_windows(), pid, approved_window_id)
            .into_iter()
            .map(|candidate_window_id| {
                walk_tree_bounded(
                    pid,
                    Some(candidate_window_id),
                    None,
                    CONSENT_MAX_ELEMENTS,
                    DEFAULT_MAX_DEPTH,
                )
                .nodes
            })
            .collect::<Vec<_>>();
        let prompt_present = trees
            .iter()
            .any(|nodes| remote_debugging_sheet_present(nodes));
        if !prompt_present {
            for nodes in &trees {
                release_actionable_nodes(nodes);
            }
            return Ok(dismissed);
        }

        let mut candidates = Vec::new();
        let mut matcher_error = None;
        for nodes in &trees {
            match exact_cancel_button(nodes) {
                Ok(Some(element)) => candidates.push(element),
                Ok(None) => {}
                Err(error) => {
                    matcher_error = Some(error);
                    break;
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        if let Some(error) = matcher_error {
            for nodes in &trees {
                release_actionable_nodes(nodes);
            }
            return Err(error);
        }
        let pressed = match candidates.as_slice() {
            [element] => unsafe { perform_action(*element as AXUIElementRef, "AXPress") },
            [] => {
                for nodes in &trees {
                    release_actionable_nodes(nodes);
                }
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the exact remote-debugging consent sheet exposed no semantic cancel action",
                ));
            }
            _ => {
                for nodes in &trees {
                    release_actionable_nodes(nodes);
                }
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "multiple Chrome-owned remote-debugging consent sheets exposed semantic cancel actions",
                ));
            }
        };
        for nodes in &trees {
            release_actionable_nodes(nodes);
        }
        if pressed != kAXErrorSuccess {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the exact browser consent cancel action became stale before AXPress",
            ));
        }
        dismissed = true;
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the remote-debugging consent sheet remained after its exact cancel action",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub async fn handle(
    request: BrowserConsentRequest,
) -> Result<BrowserConsentOutcome, BrowserRefusal> {
    let pid = i32::try_from(request.pid).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved browser pid is outside the macOS process-id range",
        )
    })?;
    let window_id = u32::try_from(request.window_id).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved browser window is outside the macOS window-id range",
        )
    })?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut saw_prompt = false;
    let mut accepted_prompt = false;
    loop {
        let trees = tokio::task::spawn_blocking(move || {
            consent_surface_ids(crate::windows::all_windows(), pid, window_id)
                .into_iter()
                .map(|candidate_window_id| {
                    walk_tree_bounded(
                        pid,
                        Some(candidate_window_id),
                        None,
                        CONSENT_MAX_ELEMENTS,
                        DEFAULT_MAX_DEPTH,
                    )
                    .nodes
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not inspect the browser consent UI: {error}"),
            )
        })?;
        let prompt_present = trees
            .iter()
            .any(|nodes| remote_debugging_sheet_present(nodes));
        saw_prompt |= prompt_present;
        let mut candidates = Vec::new();
        let mut matcher_error = None;
        for nodes in &trees {
            match exact_allow_button(nodes) {
                Ok(Some(element)) => candidates.push(element),
                Ok(None) => {}
                Err(error) => {
                    matcher_error = Some(error);
                    break;
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        if let Some(error) = matcher_error {
            for nodes in &trees {
                release_actionable_nodes(nodes);
            }
            return Err(error);
        }
        if let [element] = candidates.as_slice() {
            let pressed = unsafe { perform_action(*element as AXUIElementRef, "AXPress") };
            for nodes in &trees {
                release_actionable_nodes(nodes);
            }
            if pressed != kAXErrorSuccess {
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the exact browser consent action became stale before AXPress",
                ));
            }
            // Chrome can queue more than one browser-owned consent sheet when
            // an earlier connection attempt was interrupted. Do not report
            // acceptance merely because AXPress returned success: keep
            // inspecting the exact approved process until every matching
            // sheet is gone, or the bounded deadline/ambiguity checks refuse.
            accepted_prompt = true;
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        for nodes in &trees {
            release_actionable_nodes(nodes);
        }
        if candidates.len() > 1 {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "multiple Chrome-owned remote-debugging consent sheets exposed semantic allow actions",
            ));
        }
        if accepted_prompt && !prompt_present {
            return Ok(BrowserConsentOutcome::Accepted);
        }
        if saw_prompt && !prompt_present {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRevoked,
                "the person dismissed the browser consent sheet",
            ));
        }
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "no exact Chrome remote-debugging consent sheet appeared for reconnect attempt {}",
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

    fn node_with_ptr(
        role: &str,
        depth: usize,
        title: Option<&str>,
        actions: &[&str],
        element_ptr: usize,
    ) -> AXNode {
        AXNode {
            element_index: (!actions.is_empty()).then_some(0),
            role: role.to_owned(),
            title: title.map(str::to_owned),
            value: None,
            description: None,
            identifier: None,
            help: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            element_ptr,
            depth,
            parent_element_index: None,
            frame: None,
            value_state: None,
            value_description: None,
            min_value: None,
            max_value: None,
            enabled: None,
            selected: None,
            in_web_content: false,
        }
    }

    fn node(role: &str, depth: usize, title: Option<&str>, actions: &[&str]) -> AXNode {
        node_with_ptr(role, depth, title, actions, 7)
    }

    /// A consent sheet as a non-English host renders it: localized labels and
    /// no English "remote debugging" text anywhere in the subtree.
    fn localized_sheet(button_identifiers: [Option<&str>; 3]) -> Vec<AXNode> {
        let sheet = node("AXSheet", 1, Some("Debuggen aus der Ferne erlauben?"), &[]);
        let mut allow = node_with_ptr("AXButton", 2, Some("Zulassen"), &["AXPress"], 11);
        allow.identifier = button_identifiers[0].map(str::to_owned);
        let mut cancel = node_with_ptr("AXButton", 2, Some("Abbrechen"), &["AXPress"], 12);
        cancel.identifier = button_identifiers[1].map(str::to_owned);
        // A third pressable button so the two-button structural complement
        // cannot answer for the identifier path.
        let mut extra = node_with_ptr("AXButton", 2, Some("Einstellungen"), &["AXPress"], 13);
        extra.identifier = button_identifiers[2].map(str::to_owned);
        vec![
            node("AXWindow", 0, Some("Chrome"), &[]),
            sheet,
            allow,
            cancel,
            extra,
        ]
    }

    fn localized_decision_pair(button_identifiers: [Option<&str>; 2]) -> Vec<AXNode> {
        let mut nodes = localized_sheet([button_identifiers[0], button_identifiers[1], None]);
        nodes.pop();
        nodes
    }

    #[test]
    fn matcher_requires_sheet_prompt_and_unique_press_action() {
        let nodes = vec![
            node("AXWindow", 0, Some("Chrome"), &[]),
            node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
            node("AXButton", 2, Some("Cancel"), &["AXPress"]),
            node("AXButton", 2, Some("Allow"), &["AXPress"]),
        ];
        assert_eq!(exact_allow_button(&nodes).unwrap(), Some(7));

        let no_sheet = vec![node("AXButton", 1, Some("Allow"), &["AXPress"])];
        assert_eq!(exact_allow_button(&no_sheet).unwrap(), None);
    }

    #[test]
    fn matcher_refuses_ambiguous_allow_actions() {
        let nodes = vec![
            node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
            node("AXButton", 2, Some("Allow"), &["AXPress"]),
            node("AXButton", 2, Some("Allow"), &["AXPress"]),
        ];
        assert_eq!(
            exact_allow_button(&nodes).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn cancel_matcher_requires_remote_debugging_sheet() {
        let nodes = vec![
            node("AXWindow", 0, Some("Chrome"), &[]),
            node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
            node("AXButton", 2, Some("Cancel"), &["AXPress"]),
            node("AXButton", 2, Some("Allow"), &["AXPress"]),
        ];
        assert_eq!(exact_cancel_button(&nodes).unwrap(), Some(7));

        let unrelated = vec![
            node("AXSheet", 1, Some("Save changes?"), &[]),
            node("AXButton", 2, Some("Cancel"), &["AXPress"]),
        ];
        assert_eq!(exact_cancel_button(&unrelated).unwrap(), None);
    }

    #[test]
    fn localized_consent_sheet_is_detected_without_english_text() {
        let nodes = localized_sheet([
            Some("remote-debugging-allow"),
            Some("remote-debugging-cancel"),
            Some("settings"),
        ]);
        assert!(
            remote_debugging_sheet_present(&nodes),
            "a localized consent sheet must be recognized without English prompt text"
        );
        assert_eq!(exact_allow_button(&nodes).unwrap(), Some(11));
        assert_eq!(exact_cancel_button(&nodes).unwrap(), Some(12));
    }

    #[test]
    fn cancel_matcher_uses_the_identified_allow_pair_without_a_cancel_identifier() {
        // The allow identifier proves the localized sheet. With exactly two
        // pressable buttons, its complement needs no cancel identifier.
        let nodes = localized_decision_pair([Some("remote-debugging-allow"), None]);
        assert_eq!(exact_cancel_button(&nodes).unwrap(), Some(12));
    }

    #[test]
    fn localized_decision_pair_refuses_multiple_allow_actions() {
        let nodes =
            localized_decision_pair([Some("remote-debugging-allow"), Some("allow-confirm")]);
        assert_eq!(
            exact_cancel_button(&nodes).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(
            exact_allow_button(&nodes).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn localized_cancel_matcher_refuses_duplicate_cancel_identifiers() {
        let nodes = localized_sheet([
            Some("remote-debugging-allow"),
            Some("remote-debugging-cancel"),
            Some("cancel"),
        ]);
        assert_eq!(
            exact_cancel_button(&nodes).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matchers_refuse_conflicting_allow_and_cancel_decisions() {
        for identifier in ["remote-debugging-allow-cancel", "allow-confirm-cancel"] {
            let nodes = localized_decision_pair([Some(identifier), None]);
            assert_eq!(
                exact_allow_button(&nodes).unwrap_err().code,
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "{identifier} must not select allow"
            );
            assert_eq!(
                exact_cancel_button(&nodes).unwrap_err().code,
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "{identifier} must not select cancel"
            );
        }
    }

    #[test]
    fn english_sheet_pairing_allow_with_a_second_decision_still_refuses() {
        // Regression: the decision-pair shortcut must not turn "not allow"
        // into "cancel" on an English host. `Deny` carries a different
        // decision, so teardown has to keep refusing rather than press it.
        for label in ["Deny", "Not now", "Block"] {
            let nodes = vec![
                node("AXWindow", 0, Some("Chrome"), &[]),
                node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
                node_with_ptr("AXButton", 2, Some("Allow"), &["AXPress"], 11),
                node_with_ptr("AXButton", 2, Some(label), &["AXPress"], 12),
            ];
            assert_eq!(
                exact_cancel_button(&nodes).unwrap(),
                None,
                "{label} must not be treated as the cancel control"
            );
        }
    }

    #[test]
    fn identifier_terms_are_matched_whole_not_as_substrings() {
        // `disallow` contains "allow" and `wallow-confirm` contains
        // "confirm". Promoting the identifier to sheet-level evidence means a
        // substring match here would make an unrelated control prove consent.
        for identifier in [
            "disallow-debug",
            "wallow-confirm",
            "allowance-debug",
            "allow-confirmation",
            "allow-debugger",
            "allow-debuggings",
        ] {
            let mut allow = node_with_ptr("AXButton", 2, Some("Zulassen"), &["AXPress"], 11);
            allow.identifier = Some(identifier.to_owned());
            let nodes = vec![
                node("AXWindow", 0, Some("Chrome"), &[]),
                node("AXSheet", 1, Some("Debuggen aus der Ferne erlauben?"), &[]),
                allow,
                node_with_ptr("AXButton", 2, Some("Abbrechen"), &["AXPress"], 12),
            ];
            assert!(
                !remote_debugging_sheet_present(&nodes),
                "{identifier} must not prove a consent sheet"
            );
            assert_eq!(exact_allow_button(&nodes).unwrap(), None);
            assert_eq!(exact_cancel_button(&nodes).unwrap(), None);
        }
    }

    #[test]
    fn identifier_terms_split_on_separators_and_camel_case() {
        // Separator, camel-case and acronym forms name the same decisions.
        for identifier in [
            "allow-debug",
            "remote-debugging-allow",
            "remote_debugging_allow",
            "allowRemoteDebugging",
            "AllowRemoteDebugging",
            "ALLOW_REMOTE_DEBUGGING",
            "remoteDebuggingALLOW",
            "ALLOWRemoteDebugging",
            "allow-confirm",
        ] {
            let mut allow = node_with_ptr("AXButton", 2, Some("Zulassen"), &["AXPress"], 11);
            allow.identifier = Some(identifier.to_owned());
            let nodes = vec![
                node("AXWindow", 0, Some("Chrome"), &[]),
                node("AXSheet", 1, Some("Debuggen aus der Ferne erlauben?"), &[]),
                allow,
                node_with_ptr("AXButton", 2, Some("Abbrechen"), &["AXPress"], 12),
            ];
            assert_eq!(
                exact_allow_button(&nodes).unwrap(),
                Some(11),
                "{identifier} must name the allow decision"
            );
        }
    }

    #[test]
    fn localized_sheet_without_identifiers_stays_unmatched() {
        // Documented limitation: with no English text and no identifier the
        // sheet cannot be proven to be the consent sheet, so it must not be
        // touched rather than guessed at.
        for nodes in [
            localized_sheet([None, None, None]),
            localized_decision_pair([None, None]),
        ] {
            assert!(!remote_debugging_sheet_present(&nodes));
            assert_eq!(exact_cancel_button(&nodes).unwrap(), None);
            assert_eq!(exact_allow_button(&nodes).unwrap(), None);
        }
    }

    #[test]
    fn web_content_sheet_is_never_the_consent_sheet() {
        // A page can render a look-alike dialog carrying the English text.
        // Web content is not browser chrome, so it must not be trusted.
        let mut nodes = vec![
            node("AXWindow", 0, Some("Chrome"), &[]),
            node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
            node_with_ptr("AXButton", 2, Some("Cancel"), &["AXPress"], 12),
        ];
        for node in &mut nodes {
            node.in_web_content = true;
        }
        assert!(!remote_debugging_sheet_present(&nodes));
        assert_eq!(exact_cancel_button(&nodes).unwrap(), None);
    }

    #[test]
    fn consent_surfaces_keep_approved_window_then_frontmost_normal_windows() {
        let window = |window_id, title: &str, z_index| crate::windows::WindowInfo {
            window_id,
            pid: 42,
            app_name: "Google Chrome".to_owned(),
            title: title.to_owned(),
            bounds: crate::windows::WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 1200.0,
                height: 800.0,
            },
            layer: 0,
            z_index,
            is_on_screen: true,
            current_space_id: None,
            on_current_space: Some(true),
            space_ids: None,
        };
        assert_eq!(
            consent_surface_ids(
                [
                    window(7, "Approved", 10),
                    window(8, "Frontmost", 30),
                    window(9, "Allow remote debugging?", 40),
                ],
                42,
                7,
            ),
            vec![7, 8]
        );
    }
}
