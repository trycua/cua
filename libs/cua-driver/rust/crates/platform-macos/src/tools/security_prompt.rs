// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who owns the window an input action would reach, for
//! `cua_driver_core::security_prompt`: an action aimed at a system security
//! or consent prompt is refused before any event is sent.

use cua_driver_core::{protocol::ToolResult, security_prompt};

/// The executable name of `pid`, as `proc_name` reports it.
fn process_name(pid: i32) -> Option<String> {
    let mut buffer = [0u8; 256];
    let length = unsafe { libc::proc_name(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    let length = usize::try_from(length).ok().filter(|length| *length > 0)?;
    Some(String::from_utf8_lossy(&buffer[..length]).into_owned())
}

fn refusal_for_owner(bundle_id: Option<&str>, process_name: Option<&str>) -> Option<ToolResult> {
    security_prompt::is_macos_prompt_owner(bundle_id, process_name)
        .then(|| security_prompt::refusal(process_name.or(bundle_id).unwrap_or("unknown")))
}

/// The refusal for input addressed to a window or element of `pid`.
pub(super) fn refusal_for_pid(pid: i32) -> Option<ToolResult> {
    refusal_for_owner(
        crate::apps::bundle_id_for_pid(pid).as_deref(),
        process_name(pid).as_deref(),
    )
}

/// The refusal for input at screen point `(x, y)` (points) when a prompt's
/// window covers it. Every on-screen prompt window counts, on any layer and
/// in any order: prompts float above app windows, and a transparent overlay
/// in front of one must not hide it. Blocking: enumerates windows.
pub(super) fn refusal_at_point(x: f64, y: f64) -> Option<ToolResult> {
    crate::windows::on_screen_windows_any_layer()
        .into_iter()
        .filter(|window| {
            let b = &window.bounds;
            x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height
        })
        .find_map(|window| refusal_for_pid(window.pid))
}

/// The refusal for desktop-scope keys, which go to the frontmost app.
pub(super) fn frontmost_refusal() -> Option<ToolResult> {
    crate::apps::frontmost_pid().and_then(refusal_for_pid)
}

/// The refusal for a foreground pointer action at `points`: it goes to
/// whatever window is on top there, which may be a prompt over the target.
/// Background pointer events are posted to the target process and cannot
/// reach a prompt, so they are not checked.
pub(super) async fn foreground_refusal(
    foreground: bool,
    points: Vec<(f64, f64)>,
) -> Option<ToolResult> {
    if !foreground {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        points.into_iter().find_map(|(x, y)| refusal_at_point(x, y))
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_owner_is_refused_with_its_name() {
        let refusal = refusal_for_owner(
            Some("com.apple.UserNotificationCenter"),
            Some("UserNotificationCenter"),
        )
        .expect("the TCC alert owner is protected");
        let structured = refusal.structured_content.unwrap();
        assert_eq!(structured["code"], "security_prompt_protected");
        assert_eq!(structured["owner"], "UserNotificationCenter");
        assert!(refusal_for_owner(Some("com.apple.finder"), Some("Finder")).is_none());
    }

    #[test]
    fn this_process_is_not_a_prompt() {
        assert!(refusal_for_pid(std::process::id() as i32).is_none());
    }
}
