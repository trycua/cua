// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Recovery hints for refusals and unconfirmed actions.
//!
//! When an action is refused, or ran but could not be verified, an agent needs
//! to know what to try next. Without that, many agents stop at the first
//! refusal even when a working route exists. This module adds the same two
//! fields to every such result, whichever tool produced it:
//!
//! - `hint`: one short, specific sentence saying what to do next.
//! - `next_call`: `{tool, arguments}`, the exact call that moves the task
//!   forward. It is always safe to run as is: a read, or a call that keeps the
//!   user's window where it is. A foreground retry or anything that needs the
//!   user's consent is never a `next_call`; the `hint` names it with its
//!   condition ("only if the user allows the window to come forward").
//!
//! The browser tools already carry `detail.next_call` on a not-attached
//! refusal. That stays as it is, and the same call is repeated at the top
//! level so agents read one place.
//!
//! Everything lives in one table ([`RULES`]) plus the unverifiable-action
//! rules ([`effect_hint`]). [`annotate`] runs once, at the dispatch seam in
//! `ToolRegistry`, so every platform, every tool and every nested
//! `run_steps`/`run_script` call is covered. A producer that already set
//! `hint` or `next_call` keeps its own value.

use serde_json::{json, Map, Value};

use crate::protocol::{Content, ToolResult};

/// Longest `hint`, in characters. Hints stay short so they cost few tokens and
/// get read.
pub const MAX_HINT_CHARS: usize = 240;

/// A call the agent can make next. Serialized as `{"tool", "arguments"}`.
#[derive(Debug, Clone, PartialEq)]
pub struct NextCall {
    pub tool: &'static str,
    pub arguments: Map<String, Value>,
}

impl NextCall {
    fn to_value(&self) -> Value {
        json!({ "tool": self.tool, "arguments": Value::Object(self.arguments.clone()) })
    }
}

/// What to say and, when one exists, what to call.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryHint {
    pub hint: String,
    pub next_call: Option<NextCall>,
}

impl RecoveryHint {
    fn new(hint: impl Into<String>) -> Self {
        Self {
            hint: hint.into(),
            next_call: None,
        }
    }

    fn call(mut self, call: Option<NextCall>) -> Self {
        self.next_call = call;
        self
    }
}

/// The few request fields a hint needs. Taken from the call's public
/// arguments before the tool runs, so the dispatch seam keeps no copy of the
/// whole request.
#[derive(Debug, Clone, Default)]
pub struct Seed {
    pid: Option<i64>,
    window_id: Option<i64>,
    session: Option<String>,
    target_id: Option<String>,
    tab_id: Option<String>,
    desktop_target: bool,
}

impl Seed {
    /// Read the fields a hint can use from a call's public arguments.
    pub fn from_args(args: &Value) -> Self {
        let text = |value: &Value, key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_i64);
        let target = args.get("target");
        let window_target =
            target.filter(|t| t.get("kind").and_then(Value::as_str) == Some("window"));
        Self {
            pid: number(args, "pid").or_else(|| window_target.and_then(|t| number(t, "pid"))),
            window_id: number(args, "window_id")
                .or_else(|| window_target.and_then(|t| number(t, "window_id"))),
            session: text(args, "session"),
            target_id: text(args, "target_id"),
            tab_id: text(args, "tab_id"),
            desktop_target: target
                .is_some_and(|t| t.get("kind").and_then(Value::as_str) == Some("desktop"))
                || args.get("scope").and_then(Value::as_str) == Some("desktop"),
        }
    }
}

/// The structured fields a rule reads, resolved once per result.
struct Ctx<'a> {
    tool: &'a str,
    seed: &'a Seed,
    structured: &'a Map<String, Value>,
    /// The window to act on: the call's own, else the one the refusal names.
    pid: Option<i64>,
    window_id: Option<i64>,
}

impl<'a> Ctx<'a> {
    fn new(tool: &'a str, seed: &'a Seed, structured: &'a Map<String, Value>) -> Self {
        let named = |key: &str| {
            structured.get(key).and_then(Value::as_i64).or_else(|| {
                structured
                    .get("window")
                    .and_then(|window| window.get(key))
                    .and_then(Value::as_i64)
            })
        };
        Self {
            tool,
            seed,
            structured,
            pid: seed.pid.or_else(|| named("pid")),
            window_id: seed.window_id.or_else(|| named("window_id")),
        }
    }

    fn reason(&self) -> Option<&str> {
        self.structured.get("reason").and_then(Value::as_str)
    }

    /// `detail` of a browser refusal, when there is one.
    fn refusal_detail(&self) -> Option<&Value> {
        self.structured
            .get("refusal")
            .and_then(|refusal| refusal.get("detail"))
    }

    /// The route the refusing producer itself recommends.
    fn recommended(&self) -> Option<&str> {
        self.structured
            .get("escalation")
            .and_then(|escalation| escalation.get("recommended"))
            .and_then(Value::as_str)
    }

    fn window_args(&self) -> Option<Map<String, Value>> {
        let (pid, window_id) = (self.pid?, self.window_id?);
        let mut arguments = Map::new();
        arguments.insert("pid".into(), json!(pid));
        arguments.insert("window_id".into(), json!(window_id));
        if let Some(session) = &self.seed.session {
            arguments.insert("session".into(), json!(session));
        }
        Some(arguments)
    }

    /// `get_window_state` for a full read: fresh element tokens.
    fn read_window(&self) -> Option<NextCall> {
        Some(NextCall {
            tool: "get_window_state",
            arguments: self.window_args()?,
        })
    }

    /// `get_window_state` for what changed since the last read.
    fn read_changes(&self) -> Option<NextCall> {
        let mut arguments = self.window_args()?;
        arguments.insert("since".into(), json!("latest"));
        Some(NextCall {
            tool: "get_window_state",
            arguments,
        })
    }

    /// `get_window_state` for the screenshot only. The current snapshot's
    /// tokens stay valid.
    fn look_at_window(&self) -> Option<NextCall> {
        let mut arguments = self.window_args()?;
        arguments.insert("include_accessibility_tree".into(), json!(false));
        Some(NextCall {
            tool: "get_window_state",
            arguments,
        })
    }

    fn list_windows(&self) -> Option<NextCall> {
        let mut arguments = Map::new();
        arguments.insert("pid".into(), json!(self.pid?));
        Some(NextCall {
            tool: "list_windows",
            arguments,
        })
    }

    /// `get_browser_state`: the snapshot of a bound tab, else a fresh bind.
    fn read_page(&self) -> Option<NextCall> {
        let mut arguments = Map::new();
        if let (Some(target_id), Some(tab_id)) = (&self.seed.target_id, &self.seed.tab_id) {
            arguments.insert("target_id".into(), json!(target_id));
            arguments.insert("tab_id".into(), json!(tab_id));
        } else {
            return self.bind_page();
        }
        if let Some(session) = &self.seed.session {
            arguments.insert("session".into(), json!(session));
        }
        Some(NextCall {
            tool: "get_browser_state",
            arguments,
        })
    }

    /// `get_browser_state` in bind mode, which needs the native window.
    fn bind_page(&self) -> Option<NextCall> {
        Some(NextCall {
            tool: "get_browser_state",
            arguments: self.window_args()?,
        })
    }

    /// The `browser_prepare` call that attaches the user's own browser. It
    /// refuses with nothing changed when it cannot attach.
    fn attach_browser(&self) -> Option<NextCall> {
        let mut arguments = self.window_args()?;
        arguments.insert("strategy".into(), json!({ "kind": "existing_profile" }));
        Some(NextCall {
            tool: "browser_prepare",
            arguments,
        })
    }

    fn check_permissions() -> Option<NextCall> {
        let mut arguments = Map::new();
        arguments.insert("prompt".into(), json!(false));
        Some(NextCall {
            tool: "check_permissions",
            arguments,
        })
    }
}

/// One table row: every code in `codes` gets the hint `build` produces.
struct Rule {
    codes: &'static [&'static str],
    build: fn(&Ctx) -> RecoveryHint,
}

const FOREGROUND_CONDITION: &str = "only if the user allows the window to come forward";

/// Every refusal code that carries a hint, with the hint. Add a row here to
/// cover a new code; the coverage test checks each row.
static RULES: &[Rule] = &[
    // The input route is closed in the background.
    Rule {
        codes: &["background_unavailable", "background_occluded"],
        build: background_unavailable,
    },
    Rule {
        codes: &["foreground_unavailable"],
        build: |c| {
            RecoveryHint::new(
                "The window could not be brought forward, so nothing was sent. List the app's windows, \
                 check the window still exists, then retry or act by element instead.",
            )
            .call(c.list_windows())
        },
    },
    Rule {
        codes: &["target_occluded"],
        build: |c| {
            RecoveryHint::new(format!(
                "Another window covers that point, so a background click would land on it. Act by element \
                 (get_window_state, then element_token). Bring the window forward {FOREGROUND_CONDITION}."
            ))
            .call(c.read_window())
        },
    },
    // Tokens, snapshots and geometry that went stale.
    Rule {
        codes: &["stale_element_token"],
        build: |c| {
            RecoveryHint::new(
                "That element_token comes from an older read. Read the window again for fresh tokens, \
                 then act with the new one.",
            )
            .call(c.read_window())
        },
    },
    Rule {
        codes: &["element_not_visible", "element_bounds_unavailable"],
        build: |c| {
            RecoveryHint::new(
                "The element has no on-screen bounds right now. Scroll it into view, or read the window \
                 again and pick a visible element. Use pixel x,y only if it is missing from the tree.",
            )
            .call(c.read_window())
        },
    },
    Rule {
        codes: &["target_changed", "stale_target", "stale_geometry"],
        build: |c| {
            RecoveryHint::new(
                "The window changed since your last read, so nothing was sent. Read it again and aim from \
                 the fresh state.",
            )
            .call(c.read_window())
        },
    },
    Rule {
        codes: &["desktop_changed"],
        build: |c| {
            RecoveryHint::new(
                "The screen changed since your last read, so nothing was sent. Read it again and aim from \
                 the fresh state.",
            )
            .call(c.read_window().or_else(|| {
                c.seed.desktop_target.then(|| NextCall {
                    tool: "get_desktop_state",
                    arguments: Map::new(),
                })
            }))
        },
    },
    Rule {
        codes: &["point_outside_window"],
        build: |c| {
            RecoveryHint::new(
                "x,y are pixels of that window's latest screenshot, and this point falls outside it. Look \
                 at the window to see its size, then aim inside it.",
            )
            .call(c.look_at_window())
        },
    },
    Rule {
        codes: &["px_frame_mismatch"],
        build: |c| {
            RecoveryHint::new(
                "The window's screenshot does not match its on-screen size, so no pixel was sent. Read the \
                 window again, or act by element_token instead of pixels.",
            )
            .call(c.read_window())
        },
    },
    Rule {
        codes: &["px_capture_unavailable"],
        build: |_| {
            RecoveryHint::new(
                "The window could not be captured. Check Screen Recording with check_permissions, and act \
                 by element_token while it is missing.",
            )
            .call(Ctx::check_permissions())
        },
    },
    // Which window.
    Rule {
        codes: &[
            "window_target_not_found",
            "window_id_not_found",
            "px_window_not_found",
        ],
        build: |c| {
            RecoveryHint::new(
                "No window matches that target now. List the app's windows and use a current window_id; \
                 launch or restore the app if it has none.",
            )
            .call(c.list_windows())
        },
    },
    Rule {
        codes: &["window_id_required"],
        build: |c| {
            RecoveryHint::new(
                "This action is addressed to one exact window. Take its window_id from list_windows for \
                 this pid and retry with it.",
            )
            .call(c.list_windows())
        },
    },
    Rule {
        codes: &["ambiguous_window_target"],
        build: |c| {
            RecoveryHint::new(
                "The app has several windows. Choose the window_id you mean from the candidates (or \
                 list_windows) and retry with it.",
            )
            .call(c.list_windows())
        },
    },
    Rule {
        codes: &["window_minimized"],
        build: |c| {
            RecoveryHint::new(format!(
                "The window is minimized, so input would miss. Restore it (bring_to_front {FOREGROUND_CONDITION}), \
                 then list the windows and retry."
            ))
            .call(c.list_windows())
        },
    },
    // Permissions and authorization.
    Rule {
        codes: &[
            "permission_denied",
            "authorization_required",
            "authorization_declined",
            "authorization_canceled",
            "authorization_expired",
            "authorization_scope_mismatch",
            "authorization_host_failed",
            "authorization_suspended",
            "authorization_revoked",
            "bounded_resource_outside_manifest",
            "foreign_process_termination_denied",
            "unbounded_operation_requires_unrestricted",
            "pending_operator_approval",
        ],
        build: |_| {
            RecoveryHint::new(
                "This session's permission rules, or the user, refused the call. Do not retry it or work \
                 around it. Tell the user what was refused and ask them to allow it if the task needs it.",
            )
        },
    },
    Rule {
        codes: &["os_permission_prompt_requires_trusted_host"],
        build: |_| {
            RecoveryHint::new(
                "Operating-system permission prompts are raised by the user or the host, not by tools. \
                 Check which grants are missing with check_permissions (prompt:false) and ask the user to \
                 grant them.",
            )
            .call(Ctx::check_permissions())
        },
    },
    Rule {
        codes: &["permission_required"],
        build: |_| {
            RecoveryHint::new(
                "A permission is still pending. Ask the user to approve it, check what is missing with \
                 check_permissions (prompt:false), then retry.",
            )
            .call(Ctx::check_permissions())
        },
    },
    // Sessions.
    Rule {
        codes: &["session_ended"],
        build: |c| {
            let mut arguments = Map::new();
            if let Some(session) = &c.seed.session {
                arguments.insert("session".into(), json!(session));
            }
            RecoveryHint::new(
                "That session has ended. Start it again with start_session (same label), or use a new \
                 session label.",
            )
            .call(Some(NextCall {
                tool: "start_session",
                arguments,
            }))
        },
    },
    Rule {
        codes: &["session_not_started", "session_required"],
        build: |c| {
            let mut arguments = Map::new();
            if let Some(session) = &c.seed.session {
                arguments.insert("session".into(), json!(session));
            }
            RecoveryHint::new(
                "This call needs a started session. Call start_session with a short label, then repeat \
                 that label on later calls.",
            )
            .call(Some(NextCall {
                tool: "start_session",
                arguments,
            }))
        },
    },
    // Menus.
    Rule {
        codes: &["menu_path_unavailable", "option_not_found"],
        build: |c| {
            RecoveryHint::new(
                "That menu path or option does not exist in the app right now. Read the window to see the \
                 exact menu titles, then retry with them.",
            )
            .call(c.read_window())
        },
    },
    // Typing and values.
    Rule {
        codes: &["type_text_incomplete"],
        build: |c| {
            let delivered = c
                .structured
                .get("delivered_chars")
                .and_then(Value::as_u64)
                .map(|n| format!("Only the first {n} characters landed. "))
                .unwrap_or_default();
            RecoveryHint::new(format!(
                "{delivered}Read the field to confirm, then type only the missing rest; do not resend it all."
            ))
            .call(c.read_changes())
        },
    },
    Rule {
        codes: &["write_not_applied", "set_value_unavailable"],
        build: |_| {
            RecoveryHint::new(format!(
                "The control did not take that value. Click it and use type_text, or use a keyboard \
                 shortcut. Bring the window forward {FOREGROUND_CONDITION}."
            ))
        },
    },
    Rule {
        codes: &["select_menu_would_raise_window"],
        build: |_| {
            RecoveryHint::new(format!(
                "Choose the option with set_value(element_token, option text); it never opens the menu. \
                 Open the menu with delivery_mode:\"foreground\" {FOREGROUND_CONDITION}."
            ))
        },
    },
    Rule {
        codes: &["paste_target_is_browser_chrome"],
        build: |_| {
            RecoveryHint::new(
                "The focused field is the browser's own address bar, not the page. Focus the page field \
                 first (click it by element_token), or use browser_type for page text.",
            )
        },
    },
    Rule {
        codes: &["popup_keyboard_grab"],
        build: |c| {
            RecoveryHint::new(
                "A popup holds the keyboard. Dismiss it (click an item in it, or outside it), then send \
                 the keys again.",
            )
            .call(c.read_window())
        },
    },
    Rule {
        codes: &["target_focus_unconfirmed"],
        build: |c| {
            RecoveryHint::new(
                "The window did not confirm keyboard focus, so no key was sent. Read the window, click a \
                 field in it by element_token, then send the keys.",
            )
            .call(c.read_window())
        },
    },
    Rule {
        codes: &["delivery_failed", "foreground_partial_unknown"],
        build: |c| {
            RecoveryHint::new(
                "The input may have partly landed. Read the window before repeating anything.",
            )
            .call(c.read_changes())
        },
    },
    Rule {
        codes: &[
            "typed_output_mismatch",
            "action_outcome_mismatch",
            "tool_output_invalid",
        ],
        build: |c| {
            RecoveryHint::new(
                "The driver could not confirm its own result, so the action may have run. Read the window \
                 before repeating it.",
            )
            .call(c.read_changes())
        },
    },
    Rule {
        codes: &["lane_busy", "primary_target_busy"],
        build: |_| {
            RecoveryHint::new(
                "Another action is using this window's input. Wait a moment and retry once; do not fire \
                 parallel actions at one window.",
            )
        },
    },
    Rule {
        codes: &["desktop_coordinate_scope_required"],
        build: |_| {
            RecoveryHint::new(
                "x,y with no pid or window_id mean screen pixels, which need scope:\"desktop\". To act in \
                 one window, pass its pid and window_id with that window's screenshot pixels.",
            )
        },
    },
    Rule {
        codes: &["invalid_arguments"],
        build: |_| {
            RecoveryHint::new(
                "An argument is missing, misspelled or the wrong type. Check the tool's input schema \
                 (tools/list, or `cua-driver describe <tool>`) and resend valid arguments.",
            )
        },
    },
    // Browser tools.
    Rule {
        codes: &["browser_consent_required"],
        build: browser_consent_required,
    },
    Rule {
        codes: &["browser_requires_setup"],
        build: |c| {
            RecoveryHint::new(
                "No browser endpoint is attached to this window yet. Call browser_prepare with this pid \
                 and window_id; it attaches, or refuses with nothing changed and says why. Do not switch \
                 to desktop pointer tools.",
            )
            .call(c.attach_browser())
        },
    },
    Rule {
        codes: &["browser_ref_stale"],
        build: |c| {
            RecoveryHint::new(
                "That page ref no longer matches the live page. Read the page again with get_browser_state \
                 for fresh refs, then act on a new one.",
            )
            .call(c.read_page())
        },
    },
    Rule {
        codes: &["browser_binding_stale"],
        build: |c| {
            RecoveryHint::new(
                "The browser window or process changed since you bound it. Bind again with \
                 get_browser_state (pid and window_id from list_windows) and use the new target_id and tab_id.",
            )
            .call(c.bind_page())
        },
    },
    Rule {
        codes: &["browser_binding_ambiguous"],
        build: |_| {
            RecoveryHint::new(
                "Several browser pages match this window, so none was chosen. Bind again with \
                 get_browser_state once the page you mean is the only match, or tell the user.",
            )
        },
    },
    Rule {
        codes: &["browser_tab_required", "browser_tab_not_found"],
        build: |c| {
            RecoveryHint::new(
                "This call needs a current tab_id. Bind the window with get_browser_state (pid and \
                 window_id); it returns target_id and tab_id.",
            )
            .call(c.bind_page())
        },
    },
    Rule {
        codes: &["browser_wrong_target_refused"],
        build: |c| {
            RecoveryHint::new(
                "The driver could not prove this page belongs to that window, so it did nothing. Bind \
                 again with get_browser_state, using the browser's main window and pid, not a helper.",
            )
            .call(c.bind_page())
        },
    },
    Rule {
        codes: &["browser_route_unavailable"],
        build: |c| {
            RecoveryHint::new(
                "This window has no browser-tool route. Act on it through the accessibility tree instead: \
                 get_window_state, then click or set_value by element_token.",
            )
            .call(c.read_window())
        },
    },
    Rule {
        codes: &["browser_input_trust_unavailable"],
        build: browser_input_trust_unavailable,
    },
    Rule {
        codes: &["browser_input_incomplete"],
        build: |c| {
            let counts = c.refusal_detail().and_then(|detail| {
                Some(format!(
                    "Typing stopped after {} of {} characters. ",
                    detail.get("delivered_chars")?.as_u64()?,
                    detail.get("requested_chars")?.as_u64()?
                ))
            });
            RecoveryHint::new(format!(
                "{}Read the field with get_browser_state, then type only what is missing.",
                counts.unwrap_or_default()
            ))
            .call(c.read_page())
        },
    },
    Rule {
        codes: &["browser_action_unavailable"],
        build: |c| {
            RecoveryHint::new(
                "That ref does not support this action. Read the page again with get_browser_state and \
                 pick a ref that lists it, or use another browser tool such as browser_pointer.",
            )
            .call(c.read_page())
        },
    },
    Rule {
        codes: &["browser_reconnect_exhausted"],
        build: |c| {
            RecoveryHint::new(
                "The browser connection dropped and did not come back. Bind again with get_browser_state; \
                 if it refuses, run browser_prepare from its next_call.",
            )
            .call(c.bind_page())
        },
    },
    Rule {
        codes: &["browser_endpoint_owner_mismatch"],
        build: |c| {
            RecoveryHint::new(
                "The DevTools endpoint found is not provably this browser's, so it was refused. Act \
                 through get_window_state and the accessibility tree instead.",
            )
            .call(c.read_window())
        },
    },
    Rule {
        codes: &["browser_consent_revoked", "browser_origin_outside_scope"],
        build: |_| {
            RecoveryHint::new(
                "Browser access was turned off or the page left the approved sites. Do not work around \
                 it. Tell the user, and continue without browser tools or ask them to allow it.",
            )
        },
    },
];

/// Every refusal code that has a hint.
pub fn covered_codes() -> impl Iterator<Item = &'static str> {
    RULES.iter().flat_map(|rule| rule.codes.iter().copied())
}

fn background_unavailable(c: &Ctx) -> RecoveryHint {
    // A refusal with a working background route leads with it, then names the
    // foreground retry with its condition.
    let foreground = format!("Retry with delivery_mode:\"foreground\" {FOREGROUND_CONDITION}.");
    match (c.tool, c.reason()) {
        _ if c.structured.get("window").is_some() => RecoveryHint::new(
            "That desktop point is over a background app, so the action would use the user's pointer. \
             Target the window instead: target {kind:\"window\", pid, window_id} with pixels from its \
             own screenshot.",
        )
        .call(c.look_at_window()),
        (_, _) if c.seed.desktop_target => RecoveryHint::new(
            "Desktop input uses the real pointer, so it has no background mode. To stay in the background, \
             act inside the window: target {kind:\"window\", pid, window_id} with pixels from its \
             screenshot. Foreground only if the user allows the pointer to move.",
        )
        .call(c.look_at_window()),
        (_, Some("drag_and_drop_session")) => RecoveryHint::new(format!(
            "No drop landed: macOS drag-and-drop needs the real pointer. Try a route that is not a drag \
             first (Move button, menu item, shortcut). {foreground}"
        ))
        .call(c.read_window()),
        (_, Some("electron_main_process_unavailable")) => RecoveryHint::new(format!(
            "This Electron window cannot take a background drag. Use the app's own controls (button, \
             menu item or keyboard route). {foreground}"
        ))
        .call(c.read_window()),
        (_, Some("pointer_reading_toolkit")) => RecoveryHint::new(format!(
            "This app's toolkit reads the real pointer, so background pixel clicks miss. Act by element \
             (get_window_state, then element_token) or invoke_menu. {foreground}"
        ))
        .call(c.read_window()),
        (_, Some("windowserver_connection_invalid")) => RecoveryHint::new(
            "The daemon has no valid WindowServer connection. Run the app-bundle daemon (not a bare \
             `cua-driver serve`) and retry.",
        ),
        ("invoke_menu", _) => RecoveryHint::new(format!(
            "macOS keeps this menu item disabled while the app is behind the user's window. Use the same \
             control in the window (get_window_state, then click by element_token). bring_to_front first \
             {FOREGROUND_CONDITION}."
        ))
        .call(c.read_window()),
        ("browser_prepare", _) => RecoveryHint::new(format!(
            "Not blocked: read the page through the window's accessibility tree with get_window_state and \
             act by element_token. Use foreground delivery {FOREGROUND_CONDITION}."
        ))
        .call(c.read_window()),
        ("scroll", _) => RecoveryHint::new(format!(
            "This window drops background wheel events. Find the item with a get_window_state query and \
             act on it by element_token, with no scroll. {foreground}"
        ))
        .call(c.read_window()),
        _ if c.recommended() == Some("page") => RecoveryHint::new(
            "This is a web page: use the browser tools, which leave the user's pointer alone. Read it \
             with get_browser_state, then browser_click, browser_type or browser_pointer.",
        )
        .call(c.bind_page()),
        _ if c.recommended() == Some("px") => RecoveryHint::new(
            "Use the pixel form instead: x,y from a fresh screenshot of this window (get_window_state).",
        )
        .call(c.look_at_window()),
        _ => RecoveryHint::new(format!(
            "No background route for this action. Try an element action, invoke_menu, a shortcut or the \
             browser tools first. {foreground} Otherwise report the step as blocked."
        ))
        .call(c.read_window()),
    }
}

fn browser_consent_required(c: &Ctx) -> RecoveryHint {
    if c.tool == "browser_prepare" {
        return RecoveryHint::new(
            "Attaching to this browser needs authorization from the user or the host. Retrying will not \
             change that. Read the page through get_window_state and the accessibility tree instead, or \
             ask the user to authorize it.",
        )
        .call(c.read_window());
    }
    // A producer that already names the attach call keeps its exact arguments.
    let named = c
        .refusal_detail()
        .and_then(|detail| detail.get("next_call"))
        .and_then(|call| {
            let arguments = call.get("arguments")?.as_object()?.clone();
            (call.get("tool")?.as_str()? == "browser_prepare").then_some(NextCall {
                tool: "browser_prepare",
                arguments,
            })
        });
    RecoveryHint::new(
        "Attach to this browser window first: make the browser_prepare call in next_call. It attaches, \
         or refuses with nothing changed and says why. Do not switch to desktop pointer tools; they move \
         the user's pointer.",
    )
    .call(named.or_else(|| c.attach_browser()))
}

fn browser_input_trust_unavailable(c: &Ctx) -> RecoveryHint {
    let alternative = c
        .refusal_detail()
        .and_then(|detail| detail.get("alternative_route"))
        .and_then(Value::as_str);
    match alternative {
        Some(route) => RecoveryHint::new(format!(
            "Trusted input is not available in the background here. For a ref, retry with \
             input_route:\"{route}\" (a synthetic click). Foreground delivery {FOREGROUND_CONDITION}."
        )),
        None => RecoveryHint::new(format!(
            "Trusted browser input is not available right now. Read the page again with get_browser_state \
             and retry once, or act through the accessibility tree. Foreground delivery {FOREGROUND_CONDITION}."
        ))
        .call(c.read_page()),
    }
}

/// Tools whose successful result is an action result.
const ACTION_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "scroll",
    "drag",
    "mouse_drag",
    "parallel_mouse_drag",
    "move_cursor",
    "mouse_button_down",
    "mouse_button_up",
    "type_text",
    "type_text_chars",
    "press_key",
    "hotkey",
    "set_value",
    "set_window_frame",
    "invoke_menu",
    "browser_click",
    "browser_pointer",
    "browser_type",
    "browser_select",
    "browser_set_input_files",
];

/// The hint for an action that ran but was not confirmed. `effect` is
/// `unverifiable` (nothing could prove it) or `suspected_noop` (the evidence
/// says nothing changed).
fn effect_hint(c: &Ctx, effect: &str) -> Option<RecoveryHint> {
    if !ACTION_TOOLS.contains(&c.tool) {
        return None;
    }
    let noop = effect == "suspected_noop";
    let browser = c.tool.starts_with("browser_");
    if browser {
        let text = match (c.tool, noop) {
            ("browser_type", false) => {
                "The text was typed but not read back. Read the field with get_browser_state before typing again."
            }
            (_, true) => {
                "The page probably did not react. Read it again with get_browser_state and pick a fresh \
                 ref before retrying."
            }
            _ => {
                "The action was sent but nothing confirmed it. Read the page again with get_browser_state \
                 and check the change before repeating."
            }
        };
        return Some(RecoveryHint::new(text).call(c.read_page()));
    }
    let text = match (c.tool, noop) {
        ("click" | "double_click" | "right_click", false) => {
            "The click was sent but nothing confirmed it. Read what changed before clicking again; a \
             repeat can toggle or double-fire."
        }
        ("click" | "double_click" | "right_click", true) => {
            "The click probably did nothing. Read the window, then aim again from the fresh screenshot or \
             use the element's own action (element_token)."
        }
        ("drag" | "mouse_drag" | "parallel_mouse_drag", _) => {
            "The drag was posted but no drop was confirmed. Read what changed before repeating. If nothing \
             moved, start the press inside the item (pixels of the latest screenshot) or use a Move \
             control or menu instead."
        }
        ("type_text" | "type_text_chars", _) => {
            "The text was delivered but not read back. Read the field before typing again, or it may be \
             duplicated."
        }
        ("press_key" | "hotkey", _) => {
            "The key was sent but its effect is not confirmed. Read what changed before sending it again."
        }
        ("scroll", _) => {
            "The scroll was sent but not confirmed. Check that the content moved before scrolling more."
        }
        ("set_value", _) => {
            "The value was written but not read back. Read the element again to confirm it before relying \
             on it."
        }
        ("invoke_menu", _) => {
            "The menu item was invoked but its effect is not confirmed. Read the window for a new dialog \
             or change before invoking it again."
        }
        _ => {
            "The action was sent but not confirmed. Read what changed, or check a postcondition with \
             verify_state, before repeating it."
        }
    };
    Some(RecoveryHint::new(text).call(c.read_changes()))
}

/// The refusal code in a structured payload: the top-level `code`, the
/// browser refusal's `refusal.code`, or a refused action's `error.code`.
fn code_of(structured: &Map<String, Value>) -> Option<&str> {
    structured
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| {
            structured
                .get("refusal")
                .and_then(|refusal| refusal.get("code"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            structured
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str)
        })
}

/// The hint for a structured payload, or `None` when the payload is not a
/// refusal or an unconfirmed action, or no rule covers its code.
pub fn recovery_for(
    tool: &str,
    seed: &Seed,
    is_error: bool,
    structured: &Map<String, Value>,
) -> Option<RecoveryHint> {
    let ctx = Ctx::new(tool, seed, structured);
    let effect = structured.get("effect").and_then(Value::as_str);
    let refused = is_error
        || effect == Some("refused")
        || structured.get("status").and_then(Value::as_str) == Some("refused");
    if refused {
        let code = code_of(structured)?;
        let rule = RULES.iter().find(|rule| rule.codes.contains(&code))?;
        return Some((rule.build)(&ctx));
    }
    match effect {
        Some(effect @ ("unverifiable" | "suspected_noop")) => effect_hint(&ctx, effect),
        _ => None,
    }
}

/// The hint for an error with no structured payload, recognised from its
/// message. Only unmistakable cases: everything else stays as it is.
fn hint_for_plain_error(message: &str) -> Option<RecoveryHint> {
    let lowered = message.to_ascii_lowercase();
    if lowered.starts_with("missing required parameter") || lowered.contains(": missing required") {
        return Some(RecoveryHint::new(
            "A required argument is missing. Check the tool's input schema (tools/list, or `cua-driver \
             describe <tool>`) and resend with every required argument.",
        ));
    }
    None
}

fn first_text(result: &ToolResult) -> Option<&str> {
    result.content.iter().find_map(|content| match content {
        Content::Text { text, .. } => Some(text.as_str()),
        _ => None,
    })
}

/// Add `hint` and `next_call` to a refusal or an unconfirmed action. A field
/// the producer already set stays as it is. Called once per tool call at the
/// dispatch seam, with the arguments the caller sent.
pub fn annotate(tool: &str, seed: &Seed, result: &mut ToolResult) {
    let is_error = result.is_error == Some(true);
    let recovery = match result.structured_content.as_ref() {
        Some(Value::Object(structured)) => recovery_for(tool, seed, is_error, structured),
        Some(_) => None,
        None if is_error => {
            let hint = first_text(result).and_then(hint_for_plain_error);
            if hint.is_some() {
                // A plain error gains a code so the hint has a payload to ride on.
                result.structured_content = Some(json!({ "code": "invalid_arguments" }));
            }
            hint
        }
        None => None,
    };
    let Some(recovery) = recovery else {
        return;
    };
    let Some(Value::Object(structured)) = result.structured_content.as_mut() else {
        return;
    };
    structured
        .entry("hint")
        .or_insert_with(|| Value::String(recovery.hint));
    if let Some(call) = recovery.next_call {
        structured
            .entry("next_call")
            .or_insert_with(|| call.to_value());
    }
}

#[cfg(test)]
mod tests;
