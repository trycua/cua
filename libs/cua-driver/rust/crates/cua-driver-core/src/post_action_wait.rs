// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Whether an action's result waits for its post-action window-change report.
//!
//! After an input action the macOS adapter and the Linux X11 foreground
//! transaction watch the window list so a result can name a menu, dialog, new
//! window or foreground change the action caused. This choice sets how long
//! the result waits for a change. On macOS the watch then keeps running for
//! its whole deadline, and what it sees after the result returned is reported
//! on the next action's result; the Linux watch ends with the wait.
//!
//! The optional per-call argument [`POST_ACTION_WAIT_ARG`]:
//!
//! - omitted, or `"default"`: macOS returns as soon as the input is delivered;
//!   Linux X11 waits [`LINUX_DEFAULT_REPORT_WINDOW`], which covers menus and
//!   GTK/VCL dialogs, and reports `unverifiable` when nothing opened by then.
//! - `"report"`: wait for a change for the adapter's full report window (300 ms
//!   on macOS, the whole 800 ms deadline on Linux X11, where a slow Qt dialog
//!   then still confirms the action).
//! - `"none"`: return at delivery and say that window changes were not
//!   observed.
//!
//! The dispatch boundary reads the argument and runs the tool inside [`scope`];
//! adapters read [`current`]. The Windows adapter does not watch for window
//! changes after an action, so the argument is not advertised there, and the
//! closed action schemas refuse it as an unknown argument.

use std::future::Future;

use serde_json::{json, Value};

/// The per-call argument name.
pub const POST_ACTION_WAIT_ARG: &str = "post_action_wait";

/// Tools whose result waits for the post-action window-change report, and
/// therefore accept [`POST_ACTION_WAIT_ARG`].
pub const POST_ACTION_WAIT_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "drag",
    "scroll",
    "type_text",
    "press_key",
    "hotkey",
    "set_value",
];

/// Whether this platform's adapter watches for window changes after an action.
pub const SKIP_SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// How long a direct Linux X11 foreground action's result waits for a window
/// change when the caller did not ask for `"report"`. GTK and VCL menus and
/// dialogs map within about 100 ms of the input.
pub const LINUX_DEFAULT_REPORT_WINDOW: std::time::Duration = std::time::Duration::from_millis(150);

/// The caller's choice for the action being dispatched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PostActionWait {
    /// The adapter's default wait (see the module docs).
    #[default]
    Default,
    /// Wait for a change for the adapter's full report window.
    Report,
    /// Return without waiting; the result says nothing was observed.
    None,
}

tokio::task_local! {
    static CURRENT: PostActionWait;
}

/// Run `future` with `wait` as the choice of every action it dispatches.
pub async fn scope<F: Future>(wait: PostActionWait, future: F) -> F::Output {
    CURRENT.scope(wait, future).await
}

/// The choice for the action being dispatched; [`PostActionWait::Default`]
/// outside a [`scope`].
pub fn current() -> PostActionWait {
    CURRENT.try_with(|wait| *wait).unwrap_or_default()
}

/// Whether `tool` accepts [`POST_ACTION_WAIT_ARG`] on this platform. Windows
/// does not advertise the argument, so its closed action schemas refuse it as
/// an unknown argument.
pub fn accepts(tool: &str) -> bool {
    SKIP_SUPPORTED && POST_ACTION_WAIT_TOOLS.contains(&tool)
}

/// The JSON schema of [`POST_ACTION_WAIT_ARG`].
pub fn schema() -> Value {
    json!({
        "type": "string",
        "enum": ["default", "report", "none"],
        "description": "Wait for a window change this action caused: \"default\" (macOS \
                        none, Linux X11 150 ms), \"report\" (300 ms, Linux X11 800 ms) or \
                        \"none\". See EMBEDDING.md."
    })
}

/// Read the caller's choice from a `tool` call's arguments. `Err` carries the
/// message and structured content of the `invalid_arguments` refusal.
pub fn from_args(tool: &str, args: &Value) -> Result<PostActionWait, (String, Value)> {
    match args.get(POST_ACTION_WAIT_ARG) {
        None => Ok(PostActionWait::Default),
        Some(Value::String(s)) if s == "default" => Ok(PostActionWait::Default),
        Some(Value::String(s)) if s == "report" => Ok(PostActionWait::Report),
        Some(Value::String(s)) if s == "none" => Ok(PostActionWait::None),
        Some(other) => {
            let detail = format!(
                "post_action_wait must be \"default\", \"report\" or \"none\", got {other}"
            );
            Err((
                format!("{tool}: {detail}"),
                json!({ "code": "invalid_arguments", "tool": tool, "detail": detail }),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_and_default_keep_the_wait() {
        for args in [json!({}), json!({"post_action_wait": "default"})] {
            assert_eq!(from_args("click", &args).unwrap(), PostActionWait::Default);
        }
    }

    #[test]
    fn none_skips_the_wait() {
        assert_eq!(
            from_args("click", &json!({"post_action_wait": "none"})).unwrap(),
            PostActionWait::None
        );
    }

    #[test]
    fn report_asks_for_the_full_wait() {
        assert_eq!(
            from_args("click", &json!({"post_action_wait": "report"})).unwrap(),
            PostActionWait::Report
        );
    }

    #[test]
    fn other_values_are_invalid() {
        for value in [json!(null), json!("off"), json!(0), json!(true)] {
            let (_, structured) =
                from_args("click", &json!({"post_action_wait": value})).unwrap_err();
            assert_eq!(structured["code"], "invalid_arguments", "{value}");
        }
    }

    #[tokio::test]
    async fn the_choice_is_scoped_to_the_call() {
        assert_eq!(current(), PostActionWait::Default);
        scope(PostActionWait::None, async {
            assert_eq!(current(), PostActionWait::None);
        })
        .await;
        assert_eq!(current(), PostActionWait::Default);
    }
}
