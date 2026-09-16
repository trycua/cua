// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `suggest_action` — an optional policy head over the accessibility tree.
//!
//! The driver is mechanical by design: it snapshots a window and dispatches
//! clicks and keystrokes, and the caller decides what to do next. That
//! decision is usually the slowest thing in the loop. A general-purpose
//! model re-reads the whole tree and reasons in prose for seconds, then
//! emits one element index — which is all the loop needed.
//!
//! `suggest_action` offers a second path for that one decision: send the
//! goal plus a bounded projection of the window's interactive elements to a
//! small, fast classifier and get back a typed pick with calibrated
//! probabilities. Measured against a hand-driven loop on the same targets,
//! the decision cost drops from seconds to a couple hundred milliseconds.
//!
//! Three properties are deliberate:
//!
//! * **It never generates text.** The answer to "this is a text field" is
//!   `needs_text: true` plus the field to fill — what goes in it stays the
//!   caller's decision, because a policy head with a 120-label vocabulary
//!   has no business composing a message.
//! * **It never acts.** The response names an element; the caller still
//!   calls `click` / `type_text` and re-snapshots. Nothing here dispatches
//!   input, which is why the tool is `read_only`.
//! * **`deny` is enforced in code, twice.** Denied elements are removed
//!   before the model is asked, and the returned pick is re-checked against
//!   the list afterwards. A model that ignores an instruction cannot
//!   produce a denied pick, because the enforcement is not an instruction.
//!
//! The tool is registered only when a provider credential is configured
//! (see [`crate::jev::is_configured`]), so a default install advertises the
//! same tool list it advertised before this module existed.

use std::collections::BTreeSet;

use async_trait::async_trait;
use serde_json::{json, Value};

use cua_driver_core::{
    protocol::ToolResult,
    recording_tools::ReplayRegistrySlot,
    tool::{Tool, ToolDef, ToolRegistry},
};

use crate::jev;

/// Hard ceiling on how many elements are described to the policy head.
///
/// Two reasons, and the second is the binding one. Cost and latency scale
/// with the projection, and a real Electron or browser window produces
/// 10k+ nodes. More importantly, a `choice` question's confidence is a
/// distribution over its labels: past a few hundred it flattens into noise
/// and a confident answer stops being available at all. 120 is the largest
/// list that held useful confidence in the Wikipedia link-race runs
/// (433–1068 raw elements per page projected down to 120).
const MAX_ELEMENTS_CEILING: usize = 120;
const DEFAULT_MAX_ELEMENTS: usize = 120;

/// The label offered for "no element on this screen advances the goal".
/// Not an element index, so it can never collide with one.
const NONE_LABEL: &str = "__none__";

/// Roles never offered as the next action on a window.
///
/// Two groups, for two different reasons.
///
/// Containers carry structure rather than an action. The snapshot already
/// restricts `elements` to nodes that received an `element_index`, but
/// containers still earn indices on every platform, and describing a
/// scroll area to the policy head spends a label on something no caller
/// would ever click.
///
/// Menus are a different surface. On macOS the AX walk of any window
/// reaches the whole application menu bar, which on a real machine is
/// hundreds of rows deep and includes the Recent Items list — file names
/// from unrelated work. Left in, they crowd the window's own controls out
/// of a 120-element budget and send local file names to the provider for
/// nothing. A menu is driven by `invoke_menu` with a path, not by clicking
/// an element index, so a menu pick would not even be actionable. The cost
/// is real and stated in the docs: this tool cannot suggest a menu route.
///
/// Matched on the normalised role (lowercased, `AX`/`UIA` prefix stripped,
/// separators removed) so one list covers macOS `AXScrollArea`, Windows
/// `ScrollBar`, and AT-SPI `scroll pane`.
///
/// These are AX / UIA / AT-SPI role strings, and they live in this crate on
/// purpose: with one provider and no provider trait yet, a platform-owned
/// role table would be indirection with a single caller. The moment a second
/// provider arrives, this list and [`TEXT_ENTRY_ROLES`] are the two pieces
/// that belong behind that seam.
const EXCLUDED_ROLES: &[&str] = &[
    // containers and decoration
    "application",
    "document",
    "documentframe",
    "filler",
    "genericcontainer",
    "group",
    "heading",
    "image",
    "label",
    "layoutarea",
    "list",
    "outline",
    "pane",
    "panel",
    "scrollarea",
    "scrollbar",
    "scrollpane",
    "section",
    "separator",
    "splitgroup",
    "splitpane",
    "statictext",
    "table",
    "toolbar",
    "unknown",
    "window",
    // menus — a separate surface, reached with invoke_menu
    "menu",
    "menubar",
    "menubaritem",
    "menuitem",
    "popupmenu",
];

/// Roles that accept typed text. Membership decides `needs_text`.
///
/// Role-based on purpose. The obvious alternative — "it is a text field
/// and its value is empty" — is a web-forms assumption that breaks on
/// native controls: a macOS `AXSearchField` reports its *placeholder* as
/// its `AXValue`, so an empty search box looks filled and the caller is
/// told to press it instead of typing into it. The role is the stable
/// signal; the value is not.
///
/// Platform role strings, in this crate for the same reason as
/// [`EXCLUDED_ROLES`]: one provider, no trait, so no seam to put them
/// behind yet.
const TEXT_ENTRY_ROLES: &[&str] = &[
    "combobox",
    "edit",
    "entry",
    "passwordfield",
    "searchbox",
    "searchfield",
    "securetextfield",
    "textarea",
    "textbox",
    "textfield",
];

/// One element as offered to the policy head.
#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    element_index: u64,
    element_token: Option<String>,
    role: String,
    label: String,
    value: Option<String>,
    selected: Option<bool>,
}

impl Candidate {
    /// What the model is told about this element. Deliberately the same
    /// fields a person reading the row would use, and nothing more —
    /// frames and depths are not decision-relevant and cost tokens.
    fn criteria(&self) -> Value {
        let mut entry = json!({ "role": self.role, "name": self.label });
        if let Some(value) = &self.value {
            entry["value"] = Value::String(value.clone());
        }
        if let Some(selected) = self.selected {
            entry["selected"] = Value::Bool(selected);
        }
        entry
    }

    fn needs_text(&self) -> bool {
        TEXT_ENTRY_ROLES.contains(&normalise_role(&self.role).as_str())
    }
}

/// What the prefilter produced, plus the counts the caller needs to know
/// whether the decision saw the whole window.
#[derive(Debug, Clone, PartialEq)]
struct Projection {
    candidates: Vec<Candidate>,
    total: usize,
    denied: usize,
    truncated: bool,
}

/// Lowercase a role and strip the platform's decoration so one role list
/// covers `AXTextField`, `Edit`, and `entry`.
fn normalise_role(role: &str) -> String {
    let lowered = role.to_ascii_lowercase();
    let stripped = lowered
        .strip_prefix("ax")
        .or_else(|| lowered.strip_prefix("uia"))
        .unwrap_or(&lowered);
    stripped
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

/// True when `label` matches any deny term as a case-insensitive
/// substring. Substring rather than equality because a deny list is
/// written by a person naming a hazard ("Send", "Delete"), not
/// enumerating exact accessibility labels ("Send message to …").
fn is_denied(label: &str, deny: &[String]) -> Option<String> {
    let haystack = label.to_lowercase();
    deny.iter()
        .find(|term| {
            let needle = term.trim().to_lowercase();
            !needle.is_empty() && haystack.contains(&needle)
        })
        .cloned()
}

/// Roles whose label is worth lending to an unlabelled parent. These are
/// the leaf text nodes that make up a row's visible content.
const LABEL_LENDING_ROLES: &[&str] = &["heading", "label", "statictext", "textbox"];

/// Longest adopted label. A wide table row can have a dozen text cells;
/// the first few identify the row and the rest is padding the provider
/// pays for.
const ADOPTED_LABEL_MAX_CHARS: usize = 80;

/// Build a map from an element index to the visible text of its labelled
/// children, joined in tree order.
///
/// macOS puts a list row's text in a child `AXStaticText` and leaves the
/// `AXRow` itself unlabelled. A named-only filter therefore drops every
/// row in a sidebar, a table, or a search-result list — which is to say it
/// drops exactly the elements a navigation goal needs. Windows and AT-SPI
/// do the same with `DataItem` / `table row`.
///
/// The row is what the caller clicks (its frame is the whole row, and its
/// child text often advertises no press action at all), so the label is
/// lent upward and the row keeps its own `element_index`.
fn adopted_labels(elements: &[Value]) -> std::collections::HashMap<u64, String> {
    let mut adopted: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    for element in elements {
        let Some(parent) = element.get("parent_index").and_then(Value::as_u64) else {
            continue;
        };
        let role = element
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !LABEL_LENDING_ROLES.contains(&normalise_role(role).as_str()) {
            continue;
        }
        let Some(text) = element
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        let entry = adopted.entry(parent).or_default();
        if entry.chars().count() >= ADOPTED_LABEL_MAX_CHARS {
            continue;
        }
        if !entry.is_empty() {
            entry.push_str(" — ");
        }
        entry.push_str(text);
    }
    for label in adopted.values_mut() {
        if label.chars().count() > ADOPTED_LABEL_MAX_CHARS {
            *label = label.chars().take(ADOPTED_LABEL_MAX_CHARS).collect();
        }
    }
    adopted
}

/// Project a snapshot's `elements` array down to the candidates worth
/// deciding between.
///
/// Pure, so the whole filter — including deny enforcement — is testable
/// without a window, a registry, or a network.
fn project(elements: &[Value], deny: &[String], max_elements: usize) -> Projection {
    let mut candidates = Vec::new();
    let mut denied = 0usize;
    let mut total = 0usize;
    let adopted = adopted_labels(elements);

    for element in elements {
        let Some(element_index) = element.get("element_index").and_then(Value::as_u64) else {
            continue;
        };
        let role = element
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if EXCLUDED_ROLES.contains(&normalise_role(&role).as_str()) {
            continue;
        }
        // Named only. An unlabelled control cannot be described to the
        // policy head in a way that distinguishes it from its neighbours,
        // so offering it would be offering a coin flip. A row whose text
        // sits in child nodes borrows theirs rather than being dropped.
        let label = element
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_owned)
            .or_else(|| adopted.get(&element_index).cloned())
            .unwrap_or_default();
        if label.is_empty() {
            continue;
        }
        // A disabled control is never the next action. `enabled` is
        // absent on platforms that do not report it; absent means usable.
        if element.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        total += 1;
        // Enforcement pass one: a denied element is never described to
        // the model, so it cannot appear in the distribution at all.
        if is_denied(&label, deny).is_some() {
            denied += 1;
            continue;
        }
        candidates.push(Candidate {
            element_index,
            element_token: element
                .get("element_token")
                .and_then(Value::as_str)
                .map(str::to_owned),
            role,
            label,
            value: element
                .get("value")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            selected: element.get("selected").and_then(Value::as_bool),
        });
    }

    let truncated = candidates.len() > max_elements;
    candidates.truncate(max_elements);
    Projection {
        candidates,
        total,
        denied,
        truncated,
    }
}

/// Build the three questions asked of the policy head.
///
/// `next` is a choice over element indices plus `__none__`; `done` and
/// `blocked` are independent probabilities, not branches of the choice,
/// so a caller can act on "the goal is already met" even when the model
/// also has an opinion about which button to press.
fn questions(projection: &Projection) -> Value {
    let mut criteria = serde_json::Map::new();
    criteria.insert(
        NONE_LABEL.to_owned(),
        json!("No element on this screen advances the goal; hand the step back to the caller"),
    );
    for candidate in &projection.candidates {
        criteria.insert(candidate.element_index.to_string(), candidate.criteria());
    }
    json!({
        "next": {
            "type": "choice",
            "instructions": "Which element should be acted on next to advance the goal",
            "criteria": Value::Object(criteria),
        },
        "done": {
            "type": "noul",
            "instructions": "The goal is already fully achieved on the current screen",
        },
        "blocked": {
            "type": "noul",
            "instructions": "Progress is blocked by a login wall, captcha, permission prompt, \
                             crash, or information that is not on this screen",
        },
    })
}

fn state(goal: &str, window: &Value, projection: &Projection, history: &[String]) -> Value {
    json!({
        "goal": goal,
        "window": window,
        "history": history,
        "elements": projection
            .candidates
            .iter()
            .map(|candidate| {
                let mut entry = candidate.criteria();
                entry["element_index"] = json!(candidate.element_index);
                entry
            })
            .collect::<Vec<_>>(),
    })
}

pub struct SuggestActionTool {
    registry: ReplayRegistrySlot,
}

impl SuggestActionTool {
    pub fn new(registry: ReplayRegistrySlot) -> Self {
        Self { registry }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "suggest_action".into(),
        description: "Ask an optional fast policy head which element to act on next, for one \
             step of a goal in one window. Snapshots the window itself (no screenshot), \
             projects the accessibility tree down to named, enabled, interactive elements \
             (cap 120), and returns a typed pick: `element_index` + `element_token` to pass \
             straight to click / type_text, `kind` (\"press\" | \"type\" | \"none\"), \
             `confidence`, the full `probabilities` distribution, and independent `done` / \
             `blocked` readings.\n\n\
             ADVISORY, NOT AN ACTUATOR. It never dispatches input and never generates text. \
             A text field comes back as `needs_text: true` naming the field — what to type \
             stays your decision. After acting, re-snapshot: element indices are replaced by \
             the next get_window_state, exactly as for any other element-indexed action.\n\n\
             `deny` is enforced in code, not by prompt. Elements whose label matches a deny \
             term (case-insensitive substring) are removed before the model is asked, and the \
             returned pick is re-checked against the list. Name the hazards on the screen \
             (\"Send\", \"Delete\", \"compose\") and a denied element cannot come back.\n\n\
             Use `confidence` to route: below ~0.5 the head is guessing — read the tree \
             yourself. The tool is an accelerator for the one bounded question \"which of \
             these elements is next\", not a planner: goals spanning several windows or apps \
             stay yours to decompose. If the provider is unreachable, unauthorized, unpaid, \
             or rate limited, the call returns an error saying so and you decide the step \
             yourself; nothing about the window changes."
            .into(),
        input_schema: json!({
            "type": "object",
            "required": ["goal", "pid", "window_id"],
            "properties": {
                "session": cua_driver_core::tool_schema::session_schema_with(
                    "Passed through to the get_window_state snapshot this tool takes."
                ),
                "goal": {
                    "type": "string",
                    "description": "What the caller is trying to achieve, in one sentence \
                        (\"open the search field and search for the word lunch\"). Describe \
                        the outcome, not the click — naming the element defeats the point."
                },
                "pid": { "type": "integer", "description": "Target process ID." },
                "window_id": {
                    "type": "integer",
                    "description": "Target window ID from list_windows."
                },
                "deny": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Element labels never to suggest, as case-insensitive \
                        substrings. Enforced in code both before and after the model is \
                        asked. Name the irreversible or off-limits controls on the screen \
                        (\"Send\", \"Delete\", \"compose\")."
                },
                "history": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Actions already taken this run, oldest first \
                        (\"pressed Search field\"). Supplying it is what stops the head \
                        re-picking the element it just picked on an unchanged screen."
                },
                "max_elements": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 120,
                    "description": "Cap on elements described to the policy head \
                        (default 120, also the ceiling). Lower it on dense windows to cut \
                        cost; `truncated` reports whether the cap bit."
                }
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        // Same window, same goal, same history yields the same pick — but
        // the window is live and the provider is remote, so two calls are
        // not guaranteed identical.
        idempotent: false,
        // Calls a third-party service.
        open_world: true,
    })
}

#[async_trait]
impl Tool for SuggestActionTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let goal = match args.get("goal").and_then(Value::as_str).map(str::trim) {
            Some(goal) if !goal.is_empty() => goal.to_owned(),
            _ => return ToolResult::error("`goal` is required and must be a non-empty string."),
        };
        let (Some(pid), Some(window_id)) = (
            args.get("pid").and_then(Value::as_u64),
            args.get("window_id").and_then(Value::as_u64),
        ) else {
            return ToolResult::error(
                "`pid` and `window_id` are required. Call list_windows to find them.",
            );
        };
        let deny = string_array(args.get("deny"));
        let history = string_array(args.get("history"));
        let max_elements = args
            .get("max_elements")
            .and_then(Value::as_u64)
            .map(|value| (value as usize).clamp(1, MAX_ELEMENTS_CEILING))
            .unwrap_or(DEFAULT_MAX_ELEMENTS);

        let Some(registry) = self.registry.lock().ok().and_then(|slot| slot.upgrade()) else {
            return ToolResult::error(
                "suggest_action is unavailable: the tool registry is not initialised yet.",
            );
        };

        // Snapshot through the canonical registry chokepoint so the AX read
        // is authorized, recorded, and scoped exactly as a direct
        // get_window_state call would be. No screenshot: the policy head
        // reads structure, not pixels, and the grab is the expensive half.
        let mut snapshot_args = json!({
            "pid": pid,
            "window_id": window_id,
            "include_screenshot": false,
        });
        if let Some(session) = args.get("session").and_then(Value::as_str) {
            snapshot_args["session"] = Value::String(session.to_owned());
        }
        let snapshot_started = std::time::Instant::now();
        let snapshot = registry.invoke("get_window_state", snapshot_args).await;
        let snapshot_ms = snapshot_started.elapsed().as_millis() as u64;

        if snapshot.is_error == Some(true) {
            let detail = first_text(&snapshot);
            return ToolResult::error(format!(
                "suggest_action could not snapshot pid {pid} window {window_id}: {detail}"
            ));
        }
        let Some(structured) = snapshot.structured_content.as_ref() else {
            return ToolResult::error(
                "suggest_action could not read the window: get_window_state returned no \
                 structured content.",
            );
        };
        // A refused snapshot carries a `code` and no elements. Without this,
        // `window_id_not_found` and an off-Space window would both be reported
        // below as "this window has no interactive elements", which is a
        // confident answer about a window that was never read.
        if let Some(code) = structured.get("code").and_then(Value::as_str) {
            let detail = structured
                .get("reason")
                .or_else(|| structured.get("suggestion"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| first_text(&snapshot));
            return ToolResult::error(format!(
                "suggest_action could not snapshot pid {pid} window {window_id} \
                 ({code}): {detail}"
            ))
            .with_structured(json!({
                "error": "snapshot_refused",
                "snapshot_code": code,
                "pid": pid,
                "window_id": window_id,
                "snapshot_ms": snapshot_ms,
            }));
        }
        let elements = structured
            .get("elements")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let projection = project(&elements, &deny, max_elements);
        if projection.candidates.is_empty() {
            // An empty projection is a real answer about the window, not a
            // failure: on macOS it usually means the AX tree came back
            // empty (TCC, or an unresolved window surface) and the caller
            // should act by pixel off a screenshot instead.
            return ToolResult::text(format!(
                "No named, enabled, interactive elements to choose between in pid {pid} \
                 window {window_id} ({} element(s) in the snapshot, {} removed by `deny`). \
                 Take a get_window_state screenshot and act by pixel.",
                elements.len(),
                projection.denied
            ))
            .with_structured(json!({
                "goal": goal,
                "pid": pid,
                "window_id": window_id,
                "element_index": Value::Null,
                "kind": "none",
                "needs_text": false,
                "done": false,
                "blocked": true,
                "elements_total": projection.total,
                "elements_considered": 0,
                "elements_denied": projection.denied,
                "snapshot_ms": snapshot_ms,
            }));
        }

        // Deliberately no window title. The caller already named the exact
        // window, so a title disambiguates nothing the elements do not
        // already carry, and it is the single most likely string in a window
        // to be somebody's document name, message subject, or chat name.
        // An R3 tool should send the least that answers the question.
        let window = json!({
            "pid": pid,
            "window_id": window_id,
            "app_name": structured.get("app_name").cloned().unwrap_or(Value::Null),
        });
        let state = state(&goal, &window, &projection, &history);
        let questions = questions(&projection);

        // Blocking HTTP on a blocking pool, so the MCP server keeps
        // multiplexing other tool calls during the round-trip.
        let decide_started = std::time::Instant::now();
        let answered = tokio::task::spawn_blocking(move || jev::ask(&state, &questions)).await;
        let decide_ms = decide_started.elapsed().as_millis() as u64;

        let response = match answered {
            Err(_) => {
                return ToolResult::error(
                    "suggest_action failed: the policy-head request panicked. Decide this \
                     step yourself.",
                )
            }
            Ok(Err(error)) => {
                return ToolResult::error(error.message()).with_structured(json!({
                    "error": error.code(),
                    "status": error.status(),
                    "fallback": "decide_yourself",
                    "snapshot_ms": snapshot_ms,
                    "decide_ms": decide_ms,
                }))
            }
            Ok(Ok(response)) => response,
        };

        let next = match response.choice("next") {
            Ok(next) => next,
            Err(error) => return ToolResult::error(error.message()),
        };
        let done_p = response.noul("done").unwrap_or(0.0);
        let blocked_p = response.noul("blocked").unwrap_or(0.0);

        // Every label offered was either an element index or `__none__`, so
        // anything else is an answer outside the question. Reporting that as
        // "no element advances the goal" would turn a provider fault into a
        // confident statement about the screen.
        let picked = if next.choice == NONE_LABEL {
            None
        } else {
            match next.choice.parse::<u64>().ok().and_then(|index| {
                projection
                    .candidates
                    .iter()
                    .find(|candidate| candidate.element_index == index)
            }) {
                Some(candidate) => Some(candidate),
                None => {
                    return ToolResult::error(format!(
                        "suggest_action received an answer outside the offered choices \
                         ({:?}); it names no element on this screen. Decide this step \
                         yourself.",
                        next.choice
                    ))
                    .with_structured(json!({
                        "error": "jev_malformed_response",
                        "fallback": "decide_yourself",
                        "returned_choice": next.choice,
                    }))
                }
            }
        };

        // Enforcement pass two. A denied label cannot reach here — the
        // projection removed it — but the check is cheap and this is the
        // property the tool advertises, so it is proved at the boundary
        // rather than assumed from the filter upstream.
        if let Some(candidate) = picked {
            if let Some(term) = is_denied(&candidate.label, &deny) {
                return ToolResult::error(format!(
                    "suggest_action refused its own suggestion: the chosen element matches \
                     the deny term {term:?}. Nothing was acted on. Decide this step yourself."
                ))
                .with_structured(json!({
                    "error": "deny_list_violation",
                    "deny_hit": term,
                    "element_index": Value::Null,
                    "kind": "none",
                    "blocked": true,
                }));
            }
        }

        let done = done_p > 0.5;
        let blocked = blocked_p > 0.5;
        let needs_text = picked.is_some_and(Candidate::needs_text);
        let kind = match picked {
            None => "none",
            Some(_) if needs_text => "type",
            Some(_) => "press",
        };

        let summary = match picked {
            Some(candidate) => format!(
                "{} {} [{}] \"{}\" (confidence {:.2}){}",
                if needs_text { "Type into" } else { "Press" },
                candidate.role,
                candidate.element_index,
                candidate.label,
                next.confidence,
                if needs_text {
                    " — you choose the text."
                } else {
                    ""
                }
            ),
            None if done => "The goal already looks achieved on this screen.".to_owned(),
            None if blocked => {
                "No element advances the goal and the screen looks blocked.".to_owned()
            }
            None => "No element on this screen advances the goal.".to_owned(),
        };

        let structured_out = json!({
            "goal": goal,
            "pid": pid,
            "window_id": window_id,
            "element_index": picked.map(|candidate| candidate.element_index),
            "element_token": picked.and_then(|candidate| candidate.element_token.clone()),
            "role": picked.map(|candidate| candidate.role.clone()),
            "label": picked.map(|candidate| candidate.label.clone()),
            "kind": kind,
            "needs_text": needs_text,
            "confidence": next.confidence,
            "probabilities": next.probabilities,
            "done": done,
            "done_p": done_p,
            "blocked": blocked,
            "blocked_p": blocked_p,
            "elements_total": projection.total,
            "elements_considered": projection.candidates.len(),
            "elements_denied": projection.denied,
            "truncated": projection.truncated,
            "deny": deny,
            "snapshot_ms": snapshot_ms,
            "decide_ms": decide_ms,
            "model": response.model,
            "input_tokens": response.input_tokens,
        });
        ToolResult::text(summary).with_structured(structured_out)
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                // De-duplicate while keeping the caller's order stable.
                .fold(
                    (BTreeSet::new(), Vec::new()),
                    |(mut seen, mut out), item| {
                        if seen.insert(item.to_lowercase()) {
                            out.push(item);
                        }
                        (seen, out)
                    },
                )
                .1
        })
        .unwrap_or_default()
}

fn first_text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|content| match content {
            cua_driver_core::protocol::Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "no detail".to_owned())
}

/// Register `suggest_action`, but only when a provider credential is
/// configured. Without one the driver's tool list is unchanged, so an
/// install that never sets `TYPESAFE_API_KEY` cannot tell this module
/// exists.
pub fn register_into(registry: &mut ToolRegistry) {
    if !jev::is_configured() {
        return;
    }
    let slot = registry.self_registry_slot();
    registry.register(Box::new(SuggestActionTool::new(slot)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(index: u64, role: &str, label: &str) -> Value {
        json!({ "element_index": index, "role": role, "label": label })
    }

    #[test]
    fn roles_normalise_across_platform_spellings() {
        assert_eq!(normalise_role("AXTextField"), "textfield");
        assert_eq!(normalise_role("Edit"), "edit");
        assert_eq!(normalise_role("scroll pane"), "scrollpane");
        assert_eq!(normalise_role("AXScrollArea"), "scrollarea");
    }

    #[test]
    fn the_projection_keeps_named_enabled_interactive_elements() {
        let elements = vec![
            element(1, "AXButton", "Send"),
            element(2, "AXTextField", "Search"),
            element(3, "AXGroup", "container"),
            element(4, "AXButton", "   "),
            json!({
                "element_index": 5, "role": "AXButton", "label": "Create account",
                "enabled": false
            }),
            json!({ "role": "AXButton", "label": "no index" }),
        ];
        let projection = project(&elements, &[], DEFAULT_MAX_ELEMENTS);
        let kept: Vec<u64> = projection
            .candidates
            .iter()
            .map(|candidate| candidate.element_index)
            .collect();
        assert_eq!(
            kept,
            vec![1, 2],
            "structural, unnamed, and disabled are cut"
        );
        assert_eq!(projection.total, 2);
        assert!(!projection.truncated);
    }

    #[test]
    fn an_unlabelled_row_borrows_its_child_text() {
        // The exact shape macOS System Settings returns for a search
        // result: an unlabelled AXRow whose text sits in a child
        // AXStaticText that advertises no press action. Named-only dropped
        // every one of these, so a "navigate to X" goal had nothing to
        // pick and the head chose the least wrong button instead.
        let elements = vec![
            json!({ "element_index": 2, "role": "AXRow" }),
            json!({
                "element_index": 3, "role": "AXStaticText",
                "label": "Accessibility", "parent_index": 2
            }),
            json!({ "element_index": 4, "role": "AXRow" }),
            json!({
                "element_index": 5, "role": "AXStaticText",
                "label": "Accessibility Keyboard", "parent_index": 4
            }),
        ];
        let projection = project(&elements, &[], DEFAULT_MAX_ELEMENTS);
        let kept: Vec<(u64, &str)> = projection
            .candidates
            .iter()
            .map(|candidate| (candidate.element_index, candidate.label.as_str()))
            .collect();
        assert_eq!(
            kept,
            vec![(2, "Accessibility"), (4, "Accessibility Keyboard")],
            "the row is the click target and keeps its own index"
        );
        // Still a press, not a type: the borrowed label must not change
        // how the row is acted on.
        assert!(!projection.candidates[0].needs_text());
    }

    #[test]
    fn a_wide_row_joins_its_cells_and_stops_at_the_cap() {
        let mut elements = vec![json!({ "element_index": 1, "role": "AXRow" })];
        for cell in 0..20 {
            elements.push(json!({
                "element_index": 100 + cell,
                "role": "AXStaticText",
                "label": format!("cell number {cell}"),
                "parent_index": 1
            }));
        }
        let projection = project(&elements, &[], DEFAULT_MAX_ELEMENTS);
        let label = &projection.candidates[0].label;
        assert!(
            label.starts_with("cell number 0 — cell number 1"),
            "{label}"
        );
        assert!(
            label.chars().count() <= ADOPTED_LABEL_MAX_CHARS,
            "a wide row must not spend the whole budget on one label: {label}"
        );
    }

    #[test]
    fn an_elements_own_label_wins_over_its_children() {
        let elements = vec![
            json!({ "element_index": 1, "role": "AXButton", "label": "Save" }),
            json!({
                "element_index": 2, "role": "AXStaticText",
                "label": "Save changes to disk", "parent_index": 1
            }),
        ];
        let projection = project(&elements, &[], DEFAULT_MAX_ELEMENTS);
        assert_eq!(projection.candidates[0].label, "Save");
    }

    #[test]
    fn a_borrowed_label_is_still_subject_to_deny() {
        let elements = vec![
            json!({ "element_index": 1, "role": "AXRow" }),
            json!({
                "element_index": 2, "role": "AXStaticText",
                "label": "Transfer or Reset", "parent_index": 1
            }),
        ];
        let projection = project(&elements, &["reset".to_owned()], DEFAULT_MAX_ELEMENTS);
        assert!(projection.candidates.is_empty());
        assert_eq!(projection.denied, 1);
    }

    #[test]
    fn menu_rows_never_reach_the_policy_head() {
        // On macOS the AX walk of any window reaches the application menu
        // bar, and Recent Items carries file names from unrelated work.
        // Those must not be spent from the element budget and must not be
        // sent to the provider.
        let elements = vec![
            element(1, "AXButton", "Search"),
            element(2, "AXMenuBarItem", "File"),
            element(3, "AXMenuItem", "quarterly-forecast.xlsx"),
            element(4, "AXMenu", "Recent Items"),
            element(5, "AXTextField", "Search"),
        ];
        let projection = project(&elements, &[], DEFAULT_MAX_ELEMENTS);
        let kept: Vec<u64> = projection
            .candidates
            .iter()
            .map(|candidate| candidate.element_index)
            .collect();
        assert_eq!(kept, vec![1, 5]);
        let rendered = questions(&projection).to_string();
        assert!(
            !rendered.contains("quarterly-forecast"),
            "a recent-items file name must never be described to the provider: {rendered}"
        );
    }

    #[test]
    fn a_denied_element_is_removed_before_the_model_is_asked() {
        let elements = vec![
            element(1, "AXButton", "Send"),
            element(2, "AXTextField", "Search"),
            element(3, "AXTextArea", "Compose message"),
        ];
        let deny = vec!["Send".to_owned(), "compose".to_owned()];
        let projection = project(&elements, &deny, DEFAULT_MAX_ELEMENTS);

        let kept: Vec<u64> = projection
            .candidates
            .iter()
            .map(|candidate| candidate.element_index)
            .collect();
        assert_eq!(kept, vec![2]);
        assert_eq!(projection.denied, 2);
        assert_eq!(
            projection.total, 3,
            "denied elements still count toward what the window held"
        );

        // The decisive property: a denied label is not merely disfavoured,
        // it is absent from the question, so no answer can name it.
        let asked = questions(&projection);
        let criteria = asked
            .pointer("/next/criteria")
            .and_then(Value::as_object)
            .expect("criteria object");
        assert!(criteria.contains_key("2"));
        assert!(criteria.contains_key(NONE_LABEL));
        assert_eq!(
            criteria.len(),
            2,
            "only the surviving element and __none__ are offered: {criteria:?}"
        );
        let rendered = asked.to_string().to_lowercase();
        assert!(!rendered.contains("send"));
        assert!(!rendered.contains("compose"));
    }

    #[test]
    fn deny_terms_match_case_insensitively_inside_longer_labels() {
        assert_eq!(
            is_denied("Send Message to Recipient", &["send".to_owned()]),
            Some("send".to_owned())
        );
        assert_eq!(
            is_denied("Resend", &["send".to_owned()]),
            Some("send".to_owned())
        );
        assert_eq!(is_denied("Search", &["send".to_owned()]), None);
        assert_eq!(is_denied("anything", &["   ".to_owned()]), None);
    }

    #[test]
    fn the_projection_is_capped_and_reports_the_cap() {
        let elements: Vec<Value> = (0..200)
            .map(|index| element(index, "AXButton", &format!("Button {index}")))
            .collect();
        let projection = project(&elements, &[], DEFAULT_MAX_ELEMENTS);
        assert_eq!(projection.candidates.len(), MAX_ELEMENTS_CEILING);
        assert_eq!(projection.total, 200);
        assert!(projection.truncated);
    }

    #[test]
    fn a_text_field_needs_text_even_when_its_value_mirrors_a_placeholder() {
        // The macOS Messages search field reports its placeholder as its
        // AXValue. A value-based heuristic reads that as "already filled"
        // and tells the caller to press it; the role does not lie.
        let elements = vec![json!({
            "element_index": 7,
            "role": "AXSearchField",
            "label": "Search",
            "value": "Search"
        })];
        let projection = project(&elements, &[], DEFAULT_MAX_ELEMENTS);
        let candidate = &projection.candidates[0];
        assert!(candidate.needs_text());
        assert_eq!(candidate.value.as_deref(), Some("Search"));

        let button = project(
            &[element(8, "AXButton", "Search")],
            &[],
            DEFAULT_MAX_ELEMENTS,
        );
        assert!(!button.candidates[0].needs_text());
    }

    #[test]
    fn the_question_offers_none_alongside_every_candidate() {
        let projection = project(
            &[
                element(3, "AXButton", "OK"),
                element(4, "AXButton", "Cancel"),
            ],
            &[],
            DEFAULT_MAX_ELEMENTS,
        );
        let criteria = questions(&projection)
            .pointer("/next/criteria")
            .and_then(Value::as_object)
            .expect("criteria object")
            .clone();
        assert_eq!(criteria.len(), 3);
        assert_eq!(criteria["3"]["name"], json!("OK"));
        assert_eq!(criteria["3"]["role"], json!("AXButton"));
    }

    #[test]
    fn deny_and_history_arrays_are_trimmed_and_deduplicated() {
        let parsed = string_array(Some(&json!(["Send", " send ", "", "  ", "Delete", 7])));
        assert_eq!(parsed, vec!["Send".to_owned(), "Delete".to_owned()]);
        assert!(string_array(None).is_empty());
    }

    #[test]
    fn max_elements_is_clamped_to_the_ceiling() {
        let elements: Vec<Value> = (0..200)
            .map(|index| element(index, "AXButton", &format!("Button {index}")))
            .collect();
        // The schema caps it, but the tool clamps too — a client that
        // ignores the schema still cannot ask for a 10 000-label choice.
        let requested = 10_000usize.clamp(1, MAX_ELEMENTS_CEILING);
        assert_eq!(project(&elements, &[], requested).candidates.len(), 120);
        assert_eq!(project(&elements, &[], 5).candidates.len(), 5);
    }

    #[test]
    fn the_state_carries_the_goal_history_and_only_surviving_elements() {
        let projection = project(
            &[
                element(1, "AXButton", "Send"),
                element(2, "AXButton", "Search"),
            ],
            &["Send".to_owned()],
            DEFAULT_MAX_ELEMENTS,
        );
        let state = state(
            "find a message",
            &json!({ "pid": 42 }),
            &projection,
            &["pressed Search".to_owned()],
        );
        assert_eq!(state["goal"], json!("find a message"));
        assert_eq!(state["history"][0], json!("pressed Search"));
        let elements = state["elements"].as_array().expect("elements array");
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0]["element_index"], json!(2));
    }

    #[test]
    fn the_tool_is_advertised_as_read_only_and_open_world() {
        let definition = def();
        assert_eq!(definition.name, "suggest_action");
        assert!(definition.read_only, "it never dispatches input");
        assert!(!definition.destructive);
        assert!(definition.open_world, "it calls a third-party service");
        assert_eq!(
            definition.input_schema["required"],
            json!(["goal", "pid", "window_id"])
        );
        assert_eq!(
            definition.input_schema["properties"]["max_elements"]["maximum"],
            json!(120)
        );
    }

    #[test]
    fn the_tool_carries_a_reviewed_risk_class_and_a_capability() {
        // Registry dispatch refuses any tool without a reviewed class, so
        // a missing entry would make every call fail closed at runtime
        // rather than at compile time.
        assert_ne!(
            cua_driver_core::authorization::advertised_risk_for("suggest_action").class,
            cua_driver_core::authorization::RiskClass::Unclassified
        );
        assert!(
            cua_driver_core::tool::default_capabilities_for("suggest_action")
                .contains(&"policy.suggest_action".to_owned())
        );
    }
}
