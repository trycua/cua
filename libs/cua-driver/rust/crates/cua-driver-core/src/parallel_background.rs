//! Opt-in parallel background input lanes (local build).
//!
//! By default every physical action (click, type_text, …) is admitted one at
//! a time through the process-wide desktop-action coordinator in
//! [`crate::tool`]. That is the only safe posture for routes that touch the
//! shared primary input lane: the real pointer, the global HID queue, or the
//! frontmost/active application.
//!
//! Several macOS background routes never touch that lane. They deliver
//! through AX RPC or PID-routed SkyLight/CGEvent posting to one exact process,
//! and the macOS adapter already serializes every mutation of one process
//! through its per-pid background-mutation lease. When the daemon is started
//! with `CUA_DRIVER_PARALLEL_BACKGROUND=1`, those routes attest an
//! independent input lane so agents acting in *different* apps can run
//! concurrently. Actions against the *same* app keep serializing on the
//! per-pid lease (and the per-pid text-input guard).
//!
//! Independent lanes still take the coordinator in shared mode: they may run
//! alongside each other, but never alongside a coordinated (foreground,
//! desktop, or global-HID) action. That keeps each background action's
//! focus-steal suppression lease from fighting an intentional foreground
//! activation made by another agent.
//!
//! The switch is read once per process. With it unset (the default) the
//! behaviour is byte-for-byte the upstream one.

use serde_json::Value;
use std::sync::OnceLock;

/// Daemon environment variable that enables parallel background lanes.
pub const PARALLEL_BACKGROUND_ENV: &str = "CUA_DRIVER_PARALLEL_BACKGROUND";

/// Parse the switch value. Accepts `1`, `true`, `yes`, `on` (case-insensitive).
pub fn flag_value_enabled(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Whether this process was started with parallel background lanes enabled.
/// Read once; later environment changes have no effect.
pub fn parallel_background_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let enabled = flag_value_enabled(std::env::var(PARALLEL_BACKGROUND_ENV).ok().as_deref());
        if enabled {
            tracing::info!(
                "{PARALLEL_BACKGROUND_ENV} is on: exact-window background click/double_click/\
                 right_click/scroll/type_text/press_key/set_value run in parallel across apps"
            );
        }
        enabled
    })
}

/// The call names one exact process window and asks for neither desktop
/// scope nor foreground delivery. Shared with the coordinator predicate in
/// [`crate::tool`] so both sides agree on what "exact background" means.
pub fn is_exact_background_window(args: &Value) -> bool {
    args["pid"].as_u64().is_some_and(|pid| pid > 0)
        && args["window_id"].as_u64().is_some_and(|id| id > 0)
        && args["scope"] != "desktop"
        && args.pointer("/target/kind").and_then(Value::as_str) != Some("desktop")
        && args["delivery_mode"] != "foreground"
        && args["dispatch"] != "foreground"
}

fn has_pixel_point(args: &Value) -> bool {
    ["x", "y"]
        .iter()
        .any(|key| args.get(*key).is_some_and(|value| !value.is_null()))
}

fn is_left_button(args: &Value) -> bool {
    match args.get("button").and_then(Value::as_str) {
        None => true,
        Some(button) => {
            let button = button.trim();
            button.is_empty() || button.eq_ignore_ascii_case("left")
        }
    }
}

/// Route-shape predicate for the macOS adapter: true only when every
/// background route the tool can take for these arguments stays on AX RPC or
/// PID-routed event posting to the target process, with no escalation to the
/// real pointer, the global HID queue, or app activation.
///
/// * `click` — excluded for a left-button pixel click (`x`/`y` with button
///   left/absent): that route makes the target AppKit-active without raising
///   it (`prepare_background_pixel_click`) and may re-activate the prior
///   frontmost app afterwards, which is process-global focus state. Element
///   clicks and right/middle pixel clicks are PID-routed only.
/// * `double_click`, `right_click`, `scroll`, `type_text`, `press_key`,
///   `set_value` — every background route is AX or PID-routed. Their
///   global-HID / activation rungs are reachable only with
///   `delivery_mode:"foreground"` or `scope:"desktop"`, which
///   [`is_exact_background_window`] already rejects.
/// * Everything else (hotkey, drag, mouse_*, bring_to_front,
///   set_window_frame, move_cursor, …) — never attested here.
pub fn background_route_is_pid_scoped(tool: &str, args: &Value) -> bool {
    if !is_exact_background_window(args) {
        return false;
    }
    match tool {
        "double_click" | "right_click" | "scroll" | "type_text" | "press_key" | "set_value" => true,
        "click" => !(has_pixel_point(args) && is_left_button(args)),
        _ => false,
    }
}

/// Pure form of [`macos_independent_lane`] with the switch injected.
pub fn independent_lane_with(enabled: bool, tool: &str, args: &Value) -> bool {
    enabled && background_route_is_pid_scoped(tool, args)
}

/// What a macOS tool returns from `Tool::has_independent_input_lane`.
pub fn macos_independent_lane(tool: &str, args: &Value) -> bool {
    independent_lane_with(parallel_background_enabled(), tool, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flag_parsing_is_strict_opt_in() {
        for on in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(flag_value_enabled(Some(on)), "{on}");
        }
        for off in ["", "0", "false", "off", "no", "2", "enabled"] {
            assert!(!flag_value_enabled(Some(off)), "{off}");
        }
        assert!(!flag_value_enabled(None));
    }

    #[test]
    fn click_pixel_left_is_never_independent() {
        let base = json!({"pid": 10, "window_id": 20});
        for extra in [
            json!({"x": 5, "y": 6}),
            json!({"x": 5, "y": 6, "button": "left"}),
            json!({"x": 5, "y": 6, "button": ""}),
            json!({"x": 5, "y": 6, "count": 2}),
            json!({"x": 5, "y": 6, "element_index": 3}),
        ] {
            let mut args = base.clone();
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(!independent_lane_with(true, "click", &args), "{args}");
        }
        for extra in [
            json!({"element_index": 3}),
            json!({"element_token": "tok"}),
            json!({"x": 5, "y": 6, "button": "right"}),
            json!({"x": 5, "y": 6, "button": "middle"}),
        ] {
            let mut args = base.clone();
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(independent_lane_with(true, "click", &args), "{args}");
        }
    }

    #[test]
    fn only_listed_tools_attest() {
        let args = json!({"pid": 10, "window_id": 20, "element_index": 1});
        for tool in [
            "hotkey",
            "drag",
            "mouse_drag",
            "parallel_mouse_drag",
            "mouse_button_down",
            "mouse_button_up",
            "bring_to_front",
            "set_window_frame",
            "move_cursor",
            "type_text_chars",
            "launch_app",
        ] {
            assert!(!independent_lane_with(true, tool, &args), "{tool}");
        }
    }
}
