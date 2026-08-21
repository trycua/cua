use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_contract::{
    GetMenuExtraStateInput, InvokeMenuExtraInput, MenuExtraNodeOutput, MenuExtraStateOutput,
    MenuExtraTarget, ToolOutput,
};
use cua_driver_core::{
    action_record::{
        ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
        EvidenceKind, RequestedDelivery,
    },
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::time::Duration;

use crate::ax::bindings::{
    copy_action_names, copy_bool_attr, copy_children_bounded, copy_element_attr_result,
    copy_string_attr, kAXErrorSuccess, perform_action, pid_of_element, AXIsProcessTrusted,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetMessagingTimeout,
};

const AX_MESSAGING_TIMEOUT_SECONDS: f32 = 2.0;
const DEFAULT_MAX_DEPTH: usize = 12;
const DEFAULT_MAX_ELEMENTS: usize = 500;
const MAX_DEPTH: u32 = 25;
const MAX_ELEMENTS: u32 = 2_000;
const MAX_SEMANTIC_CHILDREN: usize = 512;

pub struct GetMenuExtraStateTool;
pub struct InvokeMenuExtraTool;

#[derive(Debug)]
struct MenuExtraError {
    code: &'static str,
    message: String,
}

impl MenuExtraError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

struct OwnedAxElement(AXUIElementRef);

impl OwnedAxElement {
    unsafe fn new(element: AXUIElementRef) -> Self {
        Self(element)
    }

    fn as_ptr(&self) -> AXUIElementRef {
        self.0
    }
}

impl Drop for OwnedAxElement {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0 as CFTypeRef) };
    }
}

#[derive(Debug)]
struct ResolvedTarget {
    pid: i32,
    bundle_id: Option<String>,
}

#[derive(Debug)]
struct MenuExtraSnapshot {
    nodes: Vec<MenuExtraNodeOutput>,
    visited: usize,
    truncated: bool,
}

fn contract_def(name: &str) -> ToolDef {
    let contract = cua_driver_contract::tool_contract(name).expect("menu-extra contract exists");
    ToolDef::from_contract(&contract)
}

fn get_state_def() -> &'static ToolDef {
    static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
    DEF.get_or_init(|| contract_def("get_menu_extra_state"))
}

fn invoke_def() -> &'static ToolDef {
    static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
    DEF.get_or_init(|| contract_def("invoke_menu_extra"))
}

fn resolve_target(target: MenuExtraTarget) -> Result<ResolvedTarget, MenuExtraError> {
    match target {
        MenuExtraTarget::Pid { pid } => {
            let pid = i32::try_from(pid).map_err(|_| {
                MenuExtraError::new("target_unavailable", "pid is outside the macOS range")
            })?;
            let bundle_id = crate::apps::bundle_id_for_pid(pid).ok_or_else(|| {
                MenuExtraError::new(
                    "target_unavailable",
                    format!("pid {pid} is not a running bundled application"),
                )
            })?;
            Ok(ResolvedTarget {
                pid,
                bundle_id: Some(bundle_id),
            })
        }
        MenuExtraTarget::BundleId { bundle_id } => {
            let normalized = bundle_id.trim();
            if normalized.is_empty()
                || normalized != bundle_id
                || normalized.chars().count() > 255
                || normalized.chars().any(char::is_control)
            {
                return Err(MenuExtraError::new(
                    "invalid_target",
                    "bundle_id must be 1–255 non-control characters with no surrounding whitespace",
                ));
            }
            let pids = crate::apps::running_pids_for_bundle(normalized);
            match pids.as_slice() {
                [pid] => Ok(ResolvedTarget {
                    pid: *pid,
                    bundle_id: Some(bundle_id),
                }),
                [] => Err(MenuExtraError::new(
                    "target_unavailable",
                    format!("no running process has bundle_id {normalized}"),
                )),
                _ => Err(MenuExtraError::new(
                    "target_ambiguous",
                    format!(
                        "bundle_id {normalized} resolves to {} running processes",
                        pids.len()
                    ),
                )),
            }
        }
    }
}

fn normalize_path(path: Vec<String>) -> Result<Vec<String>, MenuExtraError> {
    if path.is_empty() || path.len() > 16 {
        return Err(MenuExtraError::new(
            "invalid_path",
            "path must contain between 1 and 16 segments",
        ));
    }
    path.into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let segment = segment.trim();
            if segment.is_empty()
                || segment.chars().count() > 200
                || segment.chars().any(char::is_control)
            {
                Err(MenuExtraError::new(
                    "invalid_path",
                    format!(
                        "path segment {index} must be 1–200 non-control characters after trimming"
                    ),
                ))
            } else {
                Ok(segment.to_owned())
            }
        })
        .collect()
}

fn bounded_limit(
    value: Option<u32>,
    default: usize,
    maximum: u32,
    name: &str,
) -> Result<usize, MenuExtraError> {
    match value {
        Some(value) if (1..=maximum).contains(&value) => Ok(value as usize),
        Some(_) => Err(MenuExtraError::new(
            "invalid_limits",
            format!("{name} must be between 1 and {maximum}"),
        )),
        None => Ok(default),
    }
}

fn refusal(error: MenuExtraError) -> ToolResult {
    ToolResult::error(error.message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": { "code": error.code, "message": error.message }
    }))
}

fn blocking_failure(name: &str, error: tokio::task::JoinError) -> ToolResult {
    refusal(MenuExtraError::new(
        "internal_error",
        format!("{name}: blocking task failed: {error}"),
    ))
}

unsafe fn set_messaging_timeout(element: AXUIElementRef) {
    let _ = AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECONDS);
}

unsafe fn extras_menu_bar(pid: i32) -> Result<(OwnedAxElement, OwnedAxElement), MenuExtraError> {
    if !AXIsProcessTrusted() {
        return Err(MenuExtraError::new(
            "accessibility_permission_denied",
            "macOS Accessibility permission is required",
        ));
    }
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(MenuExtraError::new(
            "target_unavailable",
            "target application accessibility object is unavailable",
        ));
    }
    let app = OwnedAxElement::new(app);
    set_messaging_timeout(app.as_ptr());
    let menu_bar = copy_element_attr_result(app.as_ptr(), "AXExtrasMenuBar")
        .map_err(|error| {
            MenuExtraError::new(
                "menu_extra_query_failed",
                format!("reading AXExtrasMenuBar failed with AX error {error}"),
            )
        })?
        .ok_or_else(|| {
            MenuExtraError::new(
                "menu_extra_unavailable",
                "target application exposes no AXExtrasMenuBar",
            )
        })?;
    let menu_bar = OwnedAxElement::new(menu_bar);
    set_messaging_timeout(menu_bar.as_ptr());
    let owner_pid = pid_of_element(menu_bar.as_ptr()).map_err(|error| {
        MenuExtraError::new(
            "menu_extra_query_failed",
            format!("reading AXExtrasMenuBar owner failed with AX error {error}"),
        )
    })?;
    if owner_pid != pid {
        return Err(MenuExtraError::new(
            "menu_extra_foreign_owner",
            format!("AXExtrasMenuBar belongs to pid {owner_pid}, expected {pid}"),
        ));
    }
    Ok((app, menu_bar))
}

fn nonempty_attr(element: AXUIElementRef, attribute: &str) -> Option<String> {
    unsafe { copy_string_attr(element, attribute) }
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn semantic_label(element: AXUIElementRef) -> Option<String> {
    nonempty_attr(element, "AXTitle").or_else(|| nonempty_attr(element, "AXDescription"))
}

unsafe fn walk_element(
    element: AXUIElementRef,
    expected_pid: i32,
    depth: usize,
    max_depth: usize,
    max_elements: usize,
    snapshot: &mut MenuExtraSnapshot,
) -> Result<(), MenuExtraError> {
    if snapshot.visited >= max_elements {
        snapshot.truncated = true;
        return Ok(());
    }
    snapshot.visited += 1;

    set_messaging_timeout(element);
    let owner_pid = pid_of_element(element).map_err(|error| {
        MenuExtraError::new(
            "menu_extra_query_failed",
            format!("reading element owner failed with AX error {error}"),
        )
    })?;
    if owner_pid != expected_pid {
        return Err(MenuExtraError::new(
            "menu_extra_foreign_owner",
            format!("menu-extra element belongs to pid {owner_pid}, expected {expected_pid}"),
        ));
    }
    let role = copy_string_attr(element, "AXRole").unwrap_or_else(|| "AXUnknown".into());
    let transparent_menu = role == "AXMenu";
    let title = nonempty_attr(element, "AXTitle");
    let description = nonempty_attr(element, "AXDescription");
    let label = title.clone().or_else(|| description.clone());

    if !transparent_menu {
        snapshot.nodes.push(MenuExtraNodeOutput {
            depth: u32::try_from(depth).unwrap_or(u32::MAX),
            role,
            label,
            title,
            description,
            enabled: copy_bool_attr(element, "AXEnabled"),
            actions: copy_action_names(element),
        });
    }

    let child_depth = if transparent_menu { depth } else { depth + 1 };
    let can_descend = child_depth <= max_depth && snapshot.visited < max_elements;
    let remaining = max_elements.saturating_sub(snapshot.visited);
    let query_limit = if can_descend { remaining.max(1) } else { 1 };
    let (children, total_count) = copy_children_bounded(element, query_limit).map_err(|error| {
        MenuExtraError::new(
            "menu_extra_query_failed",
            format!("reading AXChildren failed with AX error {error}"),
        )
    })?;

    if total_count > children.len() || (!can_descend && total_count > 0) {
        snapshot.truncated = true;
    }
    if !can_descend {
        for child in children {
            drop(OwnedAxElement::new(child));
        }
        return Ok(());
    }

    for child in children {
        let child = OwnedAxElement::new(child);
        walk_element(
            child.as_ptr(),
            expected_pid,
            child_depth,
            max_depth,
            max_elements,
            snapshot,
        )?;
    }
    Ok(())
}

fn snapshot_menu_extra(
    target: ResolvedTarget,
    max_depth: usize,
    max_elements: usize,
) -> Result<MenuExtraStateOutput, MenuExtraError> {
    unsafe {
        let (_app, menu_bar) = extras_menu_bar(target.pid)?;
        let mut snapshot = MenuExtraSnapshot {
            nodes: Vec::new(),
            visited: 0,
            truncated: false,
        };
        let (children, total_count) = copy_children_bounded(menu_bar.as_ptr(), max_elements)
            .map_err(|error| {
                MenuExtraError::new(
                    "menu_extra_query_failed",
                    format!("reading AXExtrasMenuBar children failed with AX error {error}"),
                )
            })?;
        if total_count > children.len() {
            snapshot.truncated = true;
        }
        for child in children {
            let child = OwnedAxElement::new(child);
            walk_element(
                child.as_ptr(),
                target.pid,
                0,
                max_depth,
                max_elements,
                &mut snapshot,
            )?;
        }
        let node_count = u32::try_from(snapshot.nodes.len()).unwrap_or(u32::MAX);
        Ok(MenuExtraStateOutput {
            pid: u32::try_from(target.pid).unwrap_or_default(),
            bundle_id: target.bundle_id,
            nodes: snapshot.nodes,
            node_count,
            truncated: snapshot.truncated,
            privacy_sensitive: true,
            content_redacted_from_telemetry: true,
        })
    }
}

unsafe fn semantic_children(
    parent: AXUIElementRef,
    expected_pid: i32,
) -> Result<Vec<OwnedAxElement>, MenuExtraError> {
    let (children, total_count) = copy_children_bounded(parent, MAX_SEMANTIC_CHILDREN + 1)
        .map_err(|error| {
            MenuExtraError::new(
                "menu_extra_query_failed",
                format!("reading menu children failed with AX error {error}"),
            )
        })?;
    if total_count > MAX_SEMANTIC_CHILDREN {
        for child in children {
            drop(OwnedAxElement::new(child));
        }
        return Err(MenuExtraError::new(
            "menu_extra_query_limit",
            format!("menu level exceeds {MAX_SEMANTIC_CHILDREN} native children"),
        ));
    }

    let mut semantic = Vec::new();
    for child in children {
        let child = OwnedAxElement::new(child);
        let owner_pid = pid_of_element(child.as_ptr()).map_err(|error| {
            MenuExtraError::new(
                "menu_extra_query_failed",
                format!("reading menu child owner failed with AX error {error}"),
            )
        })?;
        if owner_pid != expected_pid {
            return Err(MenuExtraError::new(
                "menu_extra_foreign_owner",
                format!("menu child belongs to pid {owner_pid}, expected {expected_pid}"),
            ));
        }
        if copy_string_attr(child.as_ptr(), "AXRole").as_deref() == Some("AXMenu") {
            let (menu_children, menu_total) =
                copy_children_bounded(child.as_ptr(), MAX_SEMANTIC_CHILDREN + 1).map_err(
                    |error| {
                        MenuExtraError::new(
                            "menu_extra_query_failed",
                            format!("reading menu container failed with AX error {error}"),
                        )
                    },
                )?;
            if semantic.len() + menu_total > MAX_SEMANTIC_CHILDREN {
                for menu_child in menu_children {
                    drop(OwnedAxElement::new(menu_child));
                }
                return Err(MenuExtraError::new(
                    "menu_extra_query_limit",
                    format!("semantic menu level exceeds {MAX_SEMANTIC_CHILDREN} children"),
                ));
            }
            for menu_child in menu_children {
                let menu_child = OwnedAxElement::new(menu_child);
                let owner_pid = pid_of_element(menu_child.as_ptr()).map_err(|error| {
                    MenuExtraError::new(
                        "menu_extra_query_failed",
                        format!("reading semantic child owner failed with AX error {error}"),
                    )
                })?;
                if owner_pid != expected_pid {
                    return Err(MenuExtraError::new(
                        "menu_extra_foreign_owner",
                        format!(
                            "semantic menu child belongs to pid {owner_pid}, expected {expected_pid}"
                        ),
                    ));
                }
                semantic.push(menu_child);
            }
        } else {
            semantic.push(child);
        }
    }
    Ok(semantic)
}

unsafe fn resolve_exact_prefix(
    menu_bar: AXUIElementRef,
    expected_pid: i32,
    prefix: &[String],
) -> Result<OwnedAxElement, MenuExtraError> {
    let mut current = None;
    let mut parent = menu_bar;

    for (depth, segment) in prefix.iter().enumerate() {
        let children = semantic_children(parent, expected_pid)?;
        let mut matches = children
            .into_iter()
            .filter(|child| semantic_label(child.as_ptr()).as_deref() == Some(segment.as_str()))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            let match_count = matches.len();
            return Err(MenuExtraError::new(
                if match_count == 0 {
                    "menu_path_not_found"
                } else {
                    "menu_path_ambiguous"
                },
                if match_count == 0 {
                    format!("path segment {depth} was not found")
                } else {
                    format!("path segment {depth} matched {match_count} elements")
                },
            ));
        }
        current = matches.pop();
        parent = current.as_ref().expect("one exact match").as_ptr();
    }

    current.ok_or_else(|| MenuExtraError::new("invalid_path", "path is empty"))
}

fn choose_action(actions: &[String], final_segment: bool) -> Option<&'static str> {
    let supports = |name: &str| actions.iter().any(|action| action == name);
    let order: &[&str] = if final_segment {
        &["AXPress", "AXPick", "AXConfirm"]
    } else {
        &["AXPress", "AXPick", "AXShowMenu", "AXOpen"]
    };
    order.iter().copied().find(|action| supports(action))
}

fn invoke_path(target: ResolvedTarget, path: &[String]) -> Result<bool, MenuExtraError> {
    let prior_frontmost = crate::apps::frontmost_pid();
    let prior_focused_window =
        prior_frontmost.and_then(crate::ax::bindings::focused_window_id_of_pid);
    unsafe {
        for depth in 0..path.len() {
            let (_app, menu_bar) = extras_menu_bar(target.pid)?;
            let element = resolve_exact_prefix(menu_bar.as_ptr(), target.pid, &path[..=depth])?;
            if copy_bool_attr(element.as_ptr(), "AXEnabled") == Some(false) {
                return Err(MenuExtraError::new(
                    "menu_path_disabled",
                    format!("path segment {depth} is disabled"),
                ));
            }
            let actions = copy_action_names(element.as_ptr());
            let action = choose_action(&actions, depth + 1 == path.len()).ok_or_else(|| {
                MenuExtraError::new(
                    "menu_action_unavailable",
                    format!("path segment {depth} has no usable native accessibility action"),
                )
            })?;
            let error = perform_action(element.as_ptr(), action);
            if error != kAXErrorSuccess {
                return Err(MenuExtraError::new(
                    "menu_action_failed",
                    format!("path segment {depth} action failed with AX error {error}"),
                ));
            }
            if depth + 1 != path.len() {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    let current_frontmost = crate::apps::frontmost_pid();
    let current_focused_window =
        current_frontmost.and_then(crate::ax::bindings::focused_window_id_of_pid);
    Ok(current_frontmost == prior_frontmost && current_focused_window == prior_focused_window)
}

#[async_trait]
impl Tool for GetMenuExtraStateTool {
    fn def(&self) -> &ToolDef {
        get_state_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input: GetMenuExtraStateInput =
            match cua_driver_core::tool_args::parse_typed_input("get_menu_extra_state", args) {
                Ok(input) => input,
                Err(result) => return result,
            };
        let max_depth =
            match bounded_limit(input.max_depth, DEFAULT_MAX_DEPTH, MAX_DEPTH, "max_depth") {
                Ok(value) => value,
                Err(error) => return refusal(error),
            };
        let max_elements = match bounded_limit(
            input.max_elements,
            DEFAULT_MAX_ELEMENTS,
            MAX_ELEMENTS,
            "max_elements",
        ) {
            Ok(value) => value,
            Err(error) => return refusal(error),
        };
        let outcome = tokio::task::spawn_blocking(move || {
            let target = resolve_target(input.application)?;
            snapshot_menu_extra(target, max_depth, max_elements)
        })
        .await;

        match outcome {
            Ok(Ok(output)) => {
                let node_count = output.node_count;
                let truncated = output.truncated;
                let structured =
                    serde_json::to_value(&output).expect("MenuExtraStateOutput always serializes");
                debug_assert!(output.validate().is_ok());
                ToolResult::text(format!(
                    "Observed {node_count} menu-extra accessibility node(s){}.",
                    if truncated { " (truncated)" } else { "" }
                ))
                .with_structured(structured)
            }
            Ok(Err(error)) => refusal(error),
            Err(error) => blocking_failure("get_menu_extra_state", error),
        }
    }
}

#[async_trait]
impl Tool for InvokeMenuExtraTool {
    fn def(&self) -> &ToolDef {
        invoke_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input: InvokeMenuExtraInput =
            match cua_driver_core::tool_args::parse_typed_input("invoke_menu_extra", args) {
                Ok(input) => input,
                Err(result) => return result,
            };
        let path = match normalize_path(input.path) {
            Ok(path) => path,
            Err(error) => return refusal(error),
        };
        let outcome = tokio::task::spawn_blocking(move || {
            let target = resolve_target(input.application)?;
            invoke_path(target, &path)
        })
        .await;

        match outcome {
            Ok(Ok(focus_preserved)) => {
                let (effect, actual_delivery, detail) = if focus_preserved {
                    (
                        ActionEffect::Unverifiable,
                        ActualDelivery::NotApplicable,
                        "Every path hop resolved uniquely, AX accepted the final action, and the frontmost application and focused window were preserved",
                    )
                } else {
                    (
                        ActionEffect::Partial,
                        ActualDelivery::Unknown,
                        "AX accepted the final action, but the frontmost application or focused window changed",
                    )
                };
                ToolResult::text(
                    "Invoked the exact live menu-extra path through macOS Accessibility; verify its semantic effect from fresh state.",
                )
                .with_action_record(
                    ActionExecutionRecord::builder(
                        effect,
                        ActionTransport::MacosAxAction,
                        RequestedDelivery::NotApplicable,
                    )
                    .actual_delivery(actual_delivery)
                    .evidence(ActionEvidence {
                        kind: EvidenceKind::NativeApiResult,
                        detail: detail.into(),
                    })
                    .build()
                    .expect("invoke_menu_extra record is valid"),
                )
            }
            Ok(Err(error)) => refusal(error),
            Err(error) => blocking_failure("invoke_menu_extra", error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_normalization_is_exact_and_bounded() {
        assert_eq!(
            normalize_path(vec![" User ".into(), " Switch Test ".into()])
                .unwrap()
                .as_slice(),
            ["User", "Switch Test"]
        );
        assert!(normalize_path(Vec::new()).is_err());
        assert!(normalize_path(vec![" ".into()]).is_err());
        assert!(normalize_path(vec!["x".into(); 17]).is_err());
    }

    #[test]
    fn action_priority_distinguishes_intermediate_and_final_segments() {
        let actions = vec!["AXOpen".into(), "AXPress".into(), "AXPick".into()];
        assert_eq!(choose_action(&actions, false), Some("AXPress"));
        assert_eq!(choose_action(&actions, true), Some("AXPress"));
        assert_eq!(
            choose_action(&["AXShowMenu".into()], false),
            Some("AXShowMenu")
        );
        assert_eq!(choose_action(&["AXShowMenu".into()], true), None);
    }

    #[test]
    fn native_walk_limits_are_enforced_after_deserialization() {
        assert_eq!(bounded_limit(None, 12, 25, "max_depth").unwrap(), 12);
        assert_eq!(bounded_limit(Some(25), 12, 25, "max_depth").unwrap(), 25);
        assert!(bounded_limit(Some(0), 12, 25, "max_depth").is_err());
        assert!(bounded_limit(Some(26), 12, 25, "max_depth").is_err());
    }
}
