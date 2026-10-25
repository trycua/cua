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
//! platforms `"none"` is refused with `unsupported_platform`.

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

/// Whether `tool` accepts [`POST_ACTION_WAIT_ARG`].
pub fn accepts(tool: &str) -> bool {
    POST_ACTION_WAIT_TOOLS.contains(&tool)
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
                        result. macOS only; refused with unsupported_platform elsewhere."
    })
}

/// Read the caller's choice from a `tool` call's arguments. `Err` carries the
/// message and structured content of the refusal for an invalid value or an
/// unsupported platform.
pub fn from_args(tool: &str, args: &Value) -> Result<PostActionWait, (String, Value)> {
    resolve(tool, args.get(POST_ACTION_WAIT_ARG), SKIP_SUPPORTED)
}

/// [`from_args`] with the platform support passed in. Pure.
fn resolve(
    tool: &str,
    value: Option<&Value>,
    skip_supported: bool,
) -> Result<PostActionWait, (String, Value)> {
    let refusal = |code: &str, detail: String| {
        (
            format!("{tool}: {detail}"),
            json!({ "code": code, "tool": tool, "detail": detail }),
        )
    };
    match value {
        None | Some(Value::Null) => Ok(PostActionWait::Default),
        Some(Value::String(s)) if s == "default" => Ok(PostActionWait::Default),
        Some(Value::String(s)) if s == "none" => {
            if skip_supported {
                Ok(PostActionWait::None)
            } else {
                Err(refusal(
                    "unsupported_platform",
                    "post_action_wait \"none\" is only implemented on macOS; omit it or pass \
                     \"default\""
                        .into(),
                ))
            }
        }
        Some(other) => Err(refusal(
            "invalid_arguments",
            format!("post_action_wait must be \"default\" or \"none\", got {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_null_and_default_keep_the_wait() {
        for value in [None, Some(Value::Null), Some(json!("default"))] {
            assert_eq!(
                resolve("click", value.as_ref(), true).unwrap(),
                PostActionWait::Default
            );
        }
    }

    #[test]
    fn none_skips_the_wait_where_supported() {
        assert_eq!(
            resolve("click", Some(&json!("none")), true).unwrap(),
            PostActionWait::None
        );
    }

    #[test]
    fn none_is_refused_where_unsupported() {
        let (_, structured) = resolve("click", Some(&json!("none")), false).unwrap_err();
        assert_eq!(structured["code"], "unsupported_platform");
    }

    #[test]
    fn other_values_are_invalid() {
        for value in [json!("off"), json!(0), json!(true)] {
            let (_, structured) = resolve("click", Some(&value), true).unwrap_err();
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
