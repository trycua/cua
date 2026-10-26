//! Whether an action's result waits for its post-action window-change report.
//!
//! After an input action the macOS adapter watches the window list for a short
//! time (300 ms for a direct call) so its result can name a menu, dialog, new
//! window or foreground change the action caused. For an agent that reads the
//! window state after every action anyway, that wait is most of a background
//! click's latency.
//!
//! The optional per-call argument [`POST_ACTION_WAIT_ARG`] lets a caller skip
//! it: with `"none"` the result returns as soon as the input is delivered and
//! says that window changes were not observed. The watch itself, and the
//! focus-steal suppression it holds, still run detached for the full deadline,
//! and a change it sees is reported on the next action's result. Omitted, or
//! `"default"`, keeps the current behavior.
//!
//! The dispatch boundary reads the argument and runs the tool inside [`scope`];
//! adapters read [`current`]. Only macOS implements the skip: the Linux X11
//! foreground transaction uses its window watch as delivery evidence, and the
//! Windows adapter does not watch for window changes after an action. On those
//! platforms the argument is not advertised, and the closed action schemas
//! refuse it as an unknown argument.

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

/// Whether this platform's adapter implements `"none"`.
pub const SKIP_SUPPORTED: bool = cfg!(target_os = "macos");

/// The caller's choice for the action being dispatched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PostActionWait {
    /// Wait for the window-change report as usual.
    #[default]
    Default,
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

/// Whether `tool` accepts [`POST_ACTION_WAIT_ARG`] on this platform. Only
/// macOS implements the skip; elsewhere the argument is not advertised, so the
/// closed action schemas refuse it as an unknown argument.
pub fn accepts(tool: &str) -> bool {
    SKIP_SUPPORTED && POST_ACTION_WAIT_TOOLS.contains(&tool)
}

/// The JSON schema of [`POST_ACTION_WAIT_ARG`].
pub fn schema() -> Value {
    json!({
        "type": "string",
        "enum": ["default", "none"],
        "description": "Whether the result waits for the post-action window-change report \
                        (default \"default\": up to 300 ms for a new window, menu, dialog or \
                        foreground change). \"none\" returns as soon as the input is delivered \
                        and says window changes were not observed; the watch keeps running in \
                        the background and a change it sees is reported on the next action's \
                        result."
    })
}

/// Read the caller's choice from a `tool` call's arguments. `Err` carries the
/// message and structured content of the `invalid_arguments` refusal.
pub fn from_args(tool: &str, args: &Value) -> Result<PostActionWait, (String, Value)> {
    match args.get(POST_ACTION_WAIT_ARG) {
        None => Ok(PostActionWait::Default),
        Some(Value::String(s)) if s == "default" => Ok(PostActionWait::Default),
        Some(Value::String(s)) if s == "none" => Ok(PostActionWait::None),
        Some(other) => {
            let detail = format!("post_action_wait must be \"default\" or \"none\", got {other}");
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
