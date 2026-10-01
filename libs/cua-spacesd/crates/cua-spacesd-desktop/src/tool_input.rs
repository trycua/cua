// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Interactive input through the cua-driver tools.
//!
//! On Hyprland cua-driver has no stateful interactive session: its input
//! goes through the driver tools (`click`, `drag`, `scroll`, `press_key`,
//! `type_text`), which route the desktop through the virtual pointer and
//! keyboard and background window input through the cua-hyprland-plugin
//! seats. This lease folds each interactive batch into whole gestures with
//! the shared [`GestureFolder`] and invokes one tool per gesture, so a
//! streaming viewer's clicks, drags, scrolls and keys reach a Hyprland
//! desktop or window exactly as the same actions from an agent would. Nothing
//! here injects input.
//!
//! Addressing follows the one-shot action path: a window gets `pid`,
//! `window_id` and window-local capture pixels with the session's delivery
//! mode; a display gets `scope: desktop` and desktop pixels for the pointer,
//! and its keys and text go to the focused window in the foreground (the
//! gRPC backend's Linux keyboard rule; the desktop scope only when nothing is
//! focused). A display has no window to address background input to, so a
//! background session on one is refused with `WouldRequireActivation`, as on
//! X11 and macOS; so is a background session on a window the driver's
//! background route does not reach (the provider checks that when it
//! resolves the window).
//!
//! What a gesture tool cannot carry is not reported as delivered: hover moves
//! produce no tool call, and a key's down edge is its whole press.

// Used by the Linux (Hyprland) provider; compiled everywhere so its
// behavior is tested on every host.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::sync::{Arc, Mutex};
use std::time::Instant;

use cua_driver_core::interactive_input::gestures::{Gesture, GestureFolder};
use cua_driver_core::interactive_input::{
    validate_batch, InteractiveDeliveryMode, Modifier, PointerButton,
};
use cua_media_protocol::InteractiveInputBatch;
use cua_spacesd_provider_api::{
    InteractiveInputLease, InteractiveInputOutcome, ProviderDisplay, ProviderError,
    ProviderErrorCode, ProviderTargetId,
};
use serde_json::{json, Map, Value};

use crate::driver_input::{driver_batch, interactive_error};

/// Invokes one cua-driver tool and waits for its result.
pub(crate) trait GestureTools: Send + Sync + 'static {
    fn invoke(&self, tool: &str, arguments: Map<String, Value>) -> Result<(), ProviderError>;
}

/// Target pixel extent, read at every batch (a window can be resized).
pub(crate) type Extent = Box<dyn Fn() -> Option<(u32, u32)> + Send + Sync>;

/// The focused window's native `(pid, window_id)`, read at every key.
pub(crate) type Focused = Box<dyn Fn() -> Option<(i64, u64)> + Send + Sync>;

/// What a lease addresses.
pub(crate) enum ToolInputTarget {
    /// A catalogued window. Tool coordinates are window-local capture pixels.
    Window {
        pid: i64,
        window_id: u64,
        extent: Extent,
    },
    /// A whole display. Tool coordinates are desktop pixels.
    Display {
        origin_px: (f64, f64),
        extent: (u32, u32),
        focused: Focused,
    },
}

impl ToolInputTarget {
    /// The desktop-pixel frame of a display.
    pub(crate) fn display(display: &ProviderDisplay, focused: Focused) -> Self {
        let scale = if display.scale_factor > 0.0 {
            display.scale_factor
        } else {
            1.0
        };
        Self::Display {
            origin_px: (display.bounds.0 * scale, display.bounds.1 * scale),
            extent: (display.native_width_px, display.native_height_px),
            focused,
        }
    }
}

/// Open a tool-backed input lease on `target`. A display target (see
/// [`crate::display_key`]) is looked up with `display`, never in the window
/// catalog, and its keys go to the window `focused` names; any other target
/// is resolved with `window`.
pub(crate) fn open(
    target: &ProviderTargetId,
    mode: InteractiveDeliveryMode,
    window: impl FnOnce(&ProviderTargetId) -> Result<ToolInputTarget, ProviderError>,
    display: impl FnOnce(&str) -> Result<ProviderDisplay, ProviderError>,
    focused: Focused,
    tools: Arc<dyn GestureTools>,
) -> Result<ToolInputLease, ProviderError> {
    let target = match crate::display_key(target) {
        Some(display_id) => {
            if mode == InteractiveDeliveryMode::Background {
                return Err(ProviderError::new(
                    ProviderErrorCode::WouldRequireActivation,
                    "a display has no window to address background input to",
                ));
            }
            ToolInputTarget::display(&display(&display_id)?, focused)
        }
        None => window(target)?,
    };
    Ok(ToolInputLease {
        target,
        mode,
        tools,
        folder: Mutex::new(GestureFolder::new()),
    })
}

pub(crate) struct ToolInputLease {
    target: ToolInputTarget,
    mode: InteractiveDeliveryMode,
    tools: Arc<dyn GestureTools>,
    folder: Mutex<GestureFolder>,
}

fn button_name(button: PointerButton) -> &'static str {
    match button {
        PointerButton::Left => "left",
        PointerButton::Right => "right",
        PointerButton::Middle => "middle",
    }
}

/// Driver modifier names (the X11 session's mapping: Command is Super).
fn modifier_names(modifiers: &[Modifier]) -> Vec<&'static str> {
    modifiers
        .iter()
        .filter_map(|modifier| match modifier {
            Modifier::Shift => Some("shift"),
            Modifier::Control => Some("ctrl"),
            Modifier::Option => Some("alt"),
            Modifier::Command => Some("super"),
            Modifier::Function => None,
        })
        .collect()
}

/// Driver key names for the viewers' key vocabulary.
fn key_name(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "arrowup" => "up".into(),
        "arrowdown" => "down".into(),
        "arrowleft" => "left".into(),
        "arrowright" => "right".into(),
        "command" | "cmd" => "super".into(),
        "option" => "alt".into(),
        _ => key.to_owned(),
    }
}

fn window_arguments(pid: i64, window_id: u64, mode: InteractiveDeliveryMode) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("pid".into(), pid.into());
    arguments.insert("window_id".into(), window_id.into());
    arguments.insert(
        "delivery_mode".into(),
        match mode {
            InteractiveDeliveryMode::Background => "background",
            InteractiveDeliveryMode::PersistentForeground => "foreground",
        }
        .into(),
    );
    // The stream already observes the target; skip the agent path's
    // post-action window-change poll.
    arguments.insert("_skip_window_change_detection".into(), true.into());
    arguments
}

fn desktop_arguments() -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("scope".into(), "desktop".into());
    arguments
}

impl ToolInputLease {
    fn extent(&self) -> Result<(u32, u32), ProviderError> {
        let extent = match &self.target {
            ToolInputTarget::Window { extent, .. } => extent(),
            ToolInputTarget::Display { extent, .. } => Some(*extent),
        };
        extent
            .filter(|(width, height)| *width > 0 && *height > 0)
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorCode::TargetUnavailable,
                    "the target's pixel geometry is unavailable",
                )
            })
    }

    /// Base arguments addressing the target's pointer, without a point.
    fn base(&self) -> Map<String, Value> {
        match &self.target {
            ToolInputTarget::Window { pid, window_id, .. } => {
                window_arguments(*pid, *window_id, self.mode)
            }
            ToolInputTarget::Display { .. } => desktop_arguments(),
        }
    }

    /// Base arguments for a key or text: a display's keys go to the focused
    /// window in the foreground, as a physical keyboard's would.
    fn keyboard_base(&self) -> Map<String, Value> {
        match &self.target {
            ToolInputTarget::Display { focused, .. } => match focused() {
                Some((pid, window_id)) => window_arguments(
                    pid,
                    window_id,
                    InteractiveDeliveryMode::PersistentForeground,
                ),
                None => desktop_arguments(),
            },
            ToolInputTarget::Window { .. } => self.base(),
        }
    }

    /// A normalized point in the tool's coordinate frame.
    fn point(&self, x: f64, y: f64, extent: (u32, u32)) -> (f64, f64) {
        let local = (
            (x * f64::from(extent.0.saturating_sub(1))).round(),
            (y * f64::from(extent.1.saturating_sub(1))).round(),
        );
        match &self.target {
            ToolInputTarget::Window { .. } => local,
            ToolInputTarget::Display { origin_px, .. } => {
                (origin_px.0 + local.0, origin_px.1 + local.1)
            }
        }
    }

    /// The tool call for one gesture.
    fn call(
        &self,
        gesture: Gesture,
        extent: (u32, u32),
    ) -> Vec<(&'static str, Map<String, Value>)> {
        let pointer = || self.base();
        match gesture {
            Gesture::Click {
                x,
                y,
                button,
                modifiers,
            } => {
                let mut arguments = pointer();
                let (x, y) = self.point(x, y, extent);
                arguments.insert("x".into(), x.into());
                arguments.insert("y".into(), y.into());
                arguments.insert("button".into(), button_name(button).into());
                let modifiers = modifier_names(&modifiers);
                if !modifiers.is_empty() {
                    arguments.insert("modifier".into(), json!(modifiers));
                }
                vec![("click", arguments)]
            }
            Gesture::Drag {
                from,
                to,
                button,
                modifiers,
            } => {
                let mut arguments = pointer();
                let from = self.point(from.0, from.1, extent);
                let to = self.point(to.0, to.1, extent);
                arguments.insert("from_x".into(), from.0.into());
                arguments.insert("from_y".into(), from.1.into());
                arguments.insert("to_x".into(), to.0.into());
                arguments.insert("to_y".into(), to.1.into());
                arguments.insert("button".into(), button_name(button).into());
                let modifiers = modifier_names(&modifiers);
                if !modifiers.is_empty() {
                    arguments.insert("modifier".into(), json!(modifiers));
                }
                vec![("drag", arguments)]
            }
            Gesture::Scroll { x, y, dx, dy } => {
                let mut arguments = pointer();
                let (x, y) = self.point(x, y, extent);
                arguments.insert("x".into(), x.into());
                arguments.insert("y".into(), y.into());
                let mut calls = Vec::new();
                for (amount, negative, positive) in [(dy, "up", "down"), (dx, "left", "right")] {
                    if amount == 0 {
                        continue;
                    }
                    let mut arguments = arguments.clone();
                    arguments.insert(
                        "direction".into(),
                        if amount < 0 { negative } else { positive }.into(),
                    );
                    // The tool takes at most 50 steps per call.
                    arguments.insert("amount".into(), amount.unsigned_abs().min(50).into());
                    calls.push(("scroll", arguments));
                }
                calls
            }
            Gesture::Key { key, modifiers } => {
                let mut arguments = self.keyboard_base();
                arguments.insert("key".into(), key_name(&key).into());
                let modifiers = modifier_names(&modifiers);
                if !modifiers.is_empty() {
                    arguments.insert("modifiers".into(), json!(modifiers));
                }
                vec![("press_key", arguments)]
            }
            Gesture::Text { text } => {
                let mut arguments = self.keyboard_base();
                arguments.insert("text".into(), text.into());
                vec![("type_text", arguments)]
            }
        }
    }
}

impl InteractiveInputLease for ToolInputLease {
    fn dispatch(
        &self,
        batch: &InteractiveInputBatch,
    ) -> Result<InteractiveInputOutcome, ProviderError> {
        let started = Instant::now();
        let batch = driver_batch(batch)?;
        let through = validate_batch(&batch).map_err(interactive_error)?;
        let extent = self.extent()?;
        let gestures = self
            .folder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fold(&batch.events, extent);
        for gesture in gestures {
            for (tool, arguments) in self.call(gesture, extent) {
                self.tools.invoke(tool, arguments)?;
            }
        }
        Ok(InteractiveInputOutcome {
            through_sequence: through,
            event_count: batch.events.len(),
            dispatch_micros: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
        })
    }

    /// Gestures are delivered whole, so nothing is held down; forget a
    /// half-finished press.
    fn release_all(&self) {
        self.folder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_media_protocol::{
        InputGesturePhase, InputKeyState, InputPointerButton, InputPointerPhase,
        InteractiveInputEvent, TargetEpoch, WindowSessionId,
    };
    use cua_spacesd_provider_api::BackendTargetKey;

    #[derive(Default)]
    struct Recorded(Mutex<Vec<(String, Value)>>);

    impl GestureTools for Recorded {
        fn invoke(&self, tool: &str, arguments: Map<String, Value>) -> Result<(), ProviderError> {
            self.0
                .lock()
                .unwrap()
                .push((tool.to_owned(), Value::Object(arguments)));
            Ok(())
        }
    }

    fn display() -> ProviderDisplay {
        ProviderDisplay {
            id: "Virtual-1".into(),
            name: "Virtual-1".into(),
            primary: true,
            bounds: (0.0, 0.0, 1280.0, 800.0),
            native_width_px: 1280,
            native_height_px: 800,
            scale_factor: 1.0,
            refresh_rate_hz: 60,
        }
    }

    fn display_target() -> ProviderTargetId {
        ProviderTargetId {
            key: BackendTargetKey::new("display:Virtual-1"),
            epoch: TargetEpoch(1),
        }
    }

    fn window_target() -> ProviderTargetId {
        ProviderTargetId {
            key: BackendTargetKey::new("macos:4242:77"),
            epoch: TargetEpoch(1),
        }
    }

    fn batch(first_sequence: u64, events: Vec<InteractiveInputEvent>) -> InteractiveInputBatch {
        InteractiveInputBatch {
            session_id: WindowSessionId("media-1".into()),
            first_sequence,
            events,
        }
    }

    fn pointer(phase: InputPointerPhase, x: f64, y: f64) -> InteractiveInputEvent {
        InteractiveInputEvent::Pointer {
            phase,
            button: Some(InputPointerButton::Left),
            x_normalized: x,
            y_normalized: y,
            modifiers: Vec::new(),
        }
    }

    fn key(name: &str, state: InputKeyState) -> InteractiveInputEvent {
        InteractiveInputEvent::Key {
            key: name.into(),
            state,
            modifiers: Vec::new(),
            repeat: false,
        }
    }

    /// The provider's window catalog, which never enumerates a display.
    fn unknown_window(target: &ProviderTargetId) -> Result<ToolInputTarget, ProviderError> {
        let native = crate::TargetCatalog::default().native(target)?;
        panic!("a display resolved to window {native:?}")
    }

    /// Regression: a Hyprland desktop stream's input was resolved through
    /// the window catalog, which never holds display targets, and every
    /// click failed with "provider target is unknown".
    #[test]
    fn desktop_stream_input_reaches_the_desktop_scope_tools() {
        let tools = Arc::new(Recorded::default());
        let lease = open(
            &display_target(),
            InteractiveDeliveryMode::PersistentForeground,
            unknown_window,
            |id| {
                assert_eq!(id, "Virtual-1");
                Ok(display())
            },
            Box::new(|| Some((4242, 77))),
            tools.clone(),
        )
        .expect("a desktop stream opens an input lease");
        let outcome = lease
            .dispatch(&batch(
                7,
                vec![
                    pointer(InputPointerPhase::Move, 0.5, 0.5),
                    pointer(InputPointerPhase::Down, 0.5, 0.5),
                    pointer(InputPointerPhase::Up, 0.5, 0.5),
                    InteractiveInputEvent::TextCommit { text: "hi".into() },
                    key("enter", InputKeyState::Down),
                    key("enter", InputKeyState::Up),
                    InteractiveInputEvent::Scroll {
                        x_normalized: 0.25,
                        y_normalized: 0.25,
                        delta_x: 0.0,
                        delta_y: 2.0,
                        phase: InputGesturePhase::None,
                        momentum_phase: InputGesturePhase::None,
                        precise: false,
                    },
                ],
            ))
            .expect("delivered");
        assert_eq!(outcome.through_sequence, 13);
        assert_eq!(outcome.event_count, 7);
        let calls = tools.0.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                (
                    "click".into(),
                    json!({"scope": "desktop", "x": 640.0, "y": 400.0, "button": "left"})
                ),
                // Keys and text go to the focused window, in the foreground.
                (
                    "type_text".into(),
                    json!({
                        "pid": 4242, "window_id": 77, "delivery_mode": "foreground",
                        "_skip_window_change_detection": true, "text": "hi",
                    })
                ),
                (
                    "press_key".into(),
                    json!({
                        "pid": 4242, "window_id": 77, "delivery_mode": "foreground",
                        "_skip_window_change_detection": true, "key": "enter",
                    })
                ),
                (
                    "scroll".into(),
                    json!({"scope": "desktop", "x": 320.0, "y": 200.0, "direction": "down", "amount": 2})
                ),
            ]
        );
    }

    #[test]
    fn desktop_keys_without_a_focused_window_use_the_desktop_scope() {
        let tools = Arc::new(Recorded::default());
        let lease = open(
            &display_target(),
            InteractiveDeliveryMode::PersistentForeground,
            unknown_window,
            |_| Ok(display()),
            Box::new(|| None),
            tools.clone(),
        )
        .unwrap();
        lease
            .dispatch(&batch(
                0,
                vec![
                    key("escape", InputKeyState::Down),
                    InteractiveInputEvent::TextCommit { text: "x".into() },
                ],
            ))
            .unwrap();
        assert_eq!(
            tools.0.lock().unwrap().clone(),
            vec![
                (
                    "press_key".into(),
                    json!({"scope": "desktop", "key": "escape"})
                ),
                ("type_text".into(), json!({"scope": "desktop", "text": "x"})),
            ]
        );
    }

    #[test]
    fn desktop_background_input_is_refused_like_x11_and_macos() {
        let error = open(
            &display_target(),
            InteractiveDeliveryMode::Background,
            unknown_window,
            |_| Ok(display()),
            Box::new(|| None),
            Arc::new(Recorded::default()),
        )
        .err()
        .expect("refused");
        assert_eq!(error.code, ProviderErrorCode::WouldRequireActivation);
    }

    #[test]
    fn window_stream_input_addresses_the_window_in_background() {
        let tools = Arc::new(Recorded::default());
        let lease = open(
            &window_target(),
            InteractiveDeliveryMode::Background,
            |_| {
                Ok(ToolInputTarget::Window {
                    pid: 4242,
                    window_id: 77,
                    extent: Box::new(|| Some((801, 601))),
                })
            },
            |_| panic!("a window target is not a display"),
            Box::new(|| panic!("a window's keys go to the window")),
            tools.clone(),
        )
        .expect("a window stream opens an input lease");
        lease
            .dispatch(&batch(
                0,
                vec![
                    pointer(InputPointerPhase::Down, 0.25, 0.5),
                    pointer(InputPointerPhase::Move, 0.5, 0.5),
                ],
            ))
            .unwrap();
        // The drag completes in a later batch.
        lease
            .dispatch(&batch(2, vec![pointer(InputPointerPhase::Up, 0.75, 0.5)]))
            .unwrap();
        lease
            .dispatch(&batch(
                3,
                vec![InteractiveInputEvent::Key {
                    key: "c".into(),
                    state: InputKeyState::Down,
                    modifiers: vec![cua_media_protocol::InputModifier::Control],
                    repeat: false,
                }],
            ))
            .unwrap();
        let calls = tools.0.lock().unwrap().clone();
        let window = |extra: Value| {
            let mut arguments = json!({
                "pid": 4242,
                "window_id": 77,
                "delivery_mode": "background",
                "_skip_window_change_detection": true,
            });
            arguments
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            arguments
        };
        assert_eq!(
            calls,
            vec![
                (
                    "drag".into(),
                    window(json!({
                        "from_x": 200.0, "from_y": 300.0, "to_x": 600.0, "to_y": 300.0,
                        "button": "left",
                    }))
                ),
                (
                    "press_key".into(),
                    window(json!({"key": "c", "modifiers": ["ctrl"]}))
                ),
            ]
        );
    }

    #[test]
    fn a_window_without_pixel_geometry_is_unavailable_not_misaddressed() {
        let tools = Arc::new(Recorded::default());
        let lease = open(
            &window_target(),
            InteractiveDeliveryMode::Background,
            |_| {
                Ok(ToolInputTarget::Window {
                    pid: 1,
                    window_id: 2,
                    extent: Box::new(|| None),
                })
            },
            |_| unreachable!(),
            Box::new(|| None),
            tools.clone(),
        )
        .unwrap();
        let error = lease
            .dispatch(&batch(0, vec![pointer(InputPointerPhase::Down, 0.5, 0.5)]))
            .expect_err("no geometry");
        assert_eq!(error.code, ProviderErrorCode::TargetUnavailable);
        assert!(tools.0.lock().unwrap().is_empty());
    }
}
