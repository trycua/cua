//! Background input synthesis for macOS.
//!
//! Two strategies:
//! 1. **AX action** (`element_index` path): `AXUIElementPerformAction` — pure
//!    RPC, works on hidden/backgrounded windows, no cursor move, no focus steal.
//! 2. **CGEvent / SkyLight** (`x, y` path): synthesize CGEvents and post them
//!    to the target pid. Prefers `SLEventPostToPid` (SkyLight SPI) over the
//!    public `CGEventPostToPid` to reach Catalyst/Chromium apps and trigger
//!    the activity-monitor tickle required for live-input detection.

pub mod ax_actions;
pub mod interactive;
pub mod keyboard;
pub mod mouse;
pub mod skylight;

pub use ax_actions::perform_ax_action;
pub use interactive::{
    GesturePhase, InteractiveDeliveryMode, InteractiveInputBatch, InteractiveInputConfig,
    InteractiveInputError, InteractiveInputEvent, InteractiveInputReceipt, InteractiveInputSession,
    KeyState, Modifier, PointerButton, PointerPhase,
};
pub use keyboard::{hotkey, press_key, type_text};
pub use mouse::click_at_xy;

pub(super) fn native_event_allowed(event: &core_graphics::event::CGEvent) -> bool {
    use core_graphics::event::CGEventType::*;
    use core_graphics::event::{CGEventFlags, EventField};
    let modifier_release = if matches!(event.get_type(), FlagsChanged) {
        let flag = match event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) {
            54 | 55 => Some(CGEventFlags::CGEventFlagCommand),
            56 | 60 => Some(CGEventFlags::CGEventFlagShift),
            58 | 61 => Some(CGEventFlags::CGEventFlagAlternate),
            59 | 62 => Some(CGEventFlags::CGEventFlagControl),
            63 => Some(CGEventFlags::CGEventFlagSecondaryFn),
            _ => None,
        };
        flag.is_some_and(|flag| !event.get_flags().contains(flag))
    } else {
        false
    };
    modifier_release
        || matches!(
            event.get_type(),
            KeyUp | LeftMouseUp | RightMouseUp | OtherMouseUp
        )
        || cua_driver_core::tool::native_dispatch_allowed()
}

pub(super) fn capture_native_pointer(
    event: &core_graphics::event::CGEvent,
    pid: Option<libc::pid_t>,
) {
    use core_graphics::event::CGEventType::*;
    if !matches!(
        event.get_type(),
        LeftMouseDown | RightMouseDown | OtherMouseDown
    ) {
        return;
    }
    if let Some((_, target_pid)) = cua_driver_core::recording::dispatch_click_target() {
        if pid.is_none_or(|pid| i64::from(pid) == target_pid) {
            let point = event.location();
            crate::recording_hooks::capture_desktop_click_point(point.x, point.y);
        }
    }
}

pub(super) fn post_native_event(
    event: &core_graphics::event::CGEvent,
    tap: core_graphics::event::CGEventTapLocation,
) {
    post_native_event_with(
        event,
        || capture_native_pointer(event, None),
        || event.post(tap),
    );
}

pub(super) fn post_native_event_to_pid(event: &core_graphics::event::CGEvent, pid: libc::pid_t) {
    post_native_event_with(
        event,
        || capture_native_pointer(event, Some(pid)),
        || event.post_to_pid(pid),
    );
}

pub(super) fn post_native_event_with(
    event: &core_graphics::event::CGEvent,
    prepare: impl FnOnce(),
    post: impl FnOnce(),
) {
    if native_event_allowed(event) {
        prepare();
        if native_event_allowed(event) {
            post();
        }
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::native_event_allowed;
    use core_graphics::{
        event::{CGEvent, CGEventFlags},
        event_source::{CGEventSource, CGEventSourceStateID},
    };

    #[tokio::test]
    async fn cancellation_during_capture_prevents_the_native_post() {
        let (entered, capturing) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let (send_worker, worker) = tokio::sync::oneshot::channel();
        let request = tokio::spawn(cua_driver_core::tool::scope_native_dispatch(
            None,
            async move {
                let worker = cua_driver_core::tool::spawn_native(move || {
                    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap();
                    let event = CGEvent::new_mouse_event(
                        source,
                        core_graphics::event::CGEventType::LeftMouseDown,
                        core_graphics::geometry::CGPoint::new(10.0, 10.0),
                        core_graphics::event::CGMouseButton::Left,
                    )
                    .unwrap();
                    let mut posted = false;
                    super::post_native_event_with(
                        &event,
                        || {
                            entered.send(()).unwrap();
                            wait.recv_timeout(std::time::Duration::from_secs(5))
                                .unwrap();
                        },
                        || posted = true,
                    );
                    posted
                });
                send_worker.send(worker).unwrap();
                std::future::pending::<()>().await;
            },
        ));
        let worker = worker.await.unwrap();
        capturing
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        assert!(!worker.await.unwrap());
    }

    #[tokio::test]
    async fn cancelled_worker_refuses_modifier_down_but_allows_release_with_other_modifiers_held() {
        let (send, receive) = std::sync::mpsc::channel();
        let worker = cua_driver_core::tool::scope_native_dispatch(None, async {
            cua_driver_core::tool::spawn_native(move || {
                receive
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap();
                let down = CGEvent::new_keyboard_event(source.clone(), 56, true).unwrap();
                down.set_flags(CGEventFlags::CGEventFlagShift | CGEventFlags::CGEventFlagAlternate);
                let up = CGEvent::new_keyboard_event(source, 56, false).unwrap();
                up.set_flags(CGEventFlags::CGEventFlagAlternate);
                assert!(matches!(
                    down.get_type(),
                    core_graphics::event::CGEventType::FlagsChanged
                ));
                assert!(matches!(
                    up.get_type(),
                    core_graphics::event::CGEventType::FlagsChanged
                ));
                (native_event_allowed(&down), native_event_allowed(&up))
            })
        })
        .await;
        send.send(()).unwrap();
        assert_eq!(worker.await.unwrap(), (false, true));
    }
}
