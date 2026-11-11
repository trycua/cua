// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Best-effort semantic cursor-event fanout.
//!
//! The dispatch boundary emits these events after authorization and scope
//! validation. Platform overlays may subscribe, but failures and missing
//! subscribers are intentionally invisible to the tool result.

use cua_driver_contract::{
    classify_cursor_semantics, CursorAction, CursorDelivery, CursorMotionSelection,
    CursorSemantics, CursorThemeSelection,
};
use serde_json::Value;
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorEventPhase {
    Begin,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorEvent {
    SetSessionLabel {
        session: String,
        label: String,
    },
    Action {
        session: String,
        phase: CursorEventPhase,
        semantics: CursorSemantics,
    },
    SelectTheme {
        session: String,
        selection: CursorThemeSelection,
    },
    /// Initial motion from `start_session.cursor_motion`, in the field shape
    /// of `set_agent_cursor_motion` without `session`.
    SelectMotion {
        session: String,
        motion: Value,
    },
    /// Keys a key action presses together, such as `["cmd", "c"]`. Emitted
    /// after the action's `Begin` so the badge can show the combo briefly.
    KeyCombo {
        session: String,
        keys: Vec<String>,
    },
}

pub trait CursorEventSink: Send + Sync + 'static {
    fn emit(&self, event: CursorEvent);
}

impl<F> CursorEventSink for F
where
    F: Fn(CursorEvent) + Send + Sync + 'static,
{
    fn emit(&self, event: CursorEvent) {
        self(event);
    }
}

fn sink_slot() -> &'static RwLock<Option<Arc<dyn CursorEventSink>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn CursorEventSink>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Install the process-owned overlay sink. Reinstalling replaces a stale sink
/// from an embedded runtime that has already shut down.
pub fn install_cursor_event_sink(sink: Arc<dyn CursorEventSink>) {
    *sink_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
}

pub fn clear_cursor_event_sink() {
    *sink_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn explicit_session(args: &Value) -> Option<String> {
    args.get("session")
        .or_else(|| args.get("_session_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|session| !session.is_empty())
        .map(ToOwned::to_owned)
}

pub fn begin_tool(name: &str, args: &Value) -> Option<(String, CursorAction)> {
    let session = explicit_session(args)?;
    if let Some(label) = args
        .get("_public_session_label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        emit(CursorEvent::SetSessionLabel {
            session: session.clone(),
            label: label.to_owned(),
        });
    }
    if name == "start_session" {
        if let Some(selection) = args
            .get("cursor_theme")
            .cloned()
            .and_then(|value| serde_json::from_value::<CursorThemeSelection>(value).ok())
        {
            emit(CursorEvent::SelectTheme {
                session: session.clone(),
                selection,
            });
        }
        if let Some(motion) = args.get("cursor_motion").filter(|value| value.is_object()) {
            if serde_json::from_value::<CursorMotionSelection>(motion.clone()).is_ok() {
                emit(CursorEvent::SelectMotion {
                    session: session.clone(),
                    motion: motion.clone(),
                });
            }
        }
    }
    let mut semantics = classify_cursor_semantics(name, args)?;
    if semantics.delivery.is_none() && brings_app_to_front(name) {
        semantics.delivery = Some(CursorDelivery::Foreground);
    }
    emit(CursorEvent::Action {
        session: session.clone(),
        phase: CursorEventPhase::Begin,
        semantics,
    });
    if semantics.action == CursorAction::Key {
        let keys = pressed_keys(args);
        if keys.len() > 1 {
            emit(CursorEvent::KeyCombo {
                session: session.clone(),
                keys,
            });
        }
    }
    Some((session, semantics.action))
}

/// Tools whose whole job is to bring an app or window to the foreground.
fn brings_app_to_front(name: &str) -> bool {
    matches!(name, "bring_to_front" | "activate_app" | "browser_activate")
}

/// The keys a key tool presses together: `hotkey`'s `keys` (an array, or a
/// `cmd+c` string), or `press_key`'s `modifiers` plus `key`.
fn pressed_keys(args: &Value) -> Vec<String> {
    let strings = |value: Option<&Value>| -> Vec<String> {
        match value {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            Some(Value::String(text)) => text
                .split('+')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    };
    let keys = strings(args.get("keys"));
    if !keys.is_empty() {
        return keys;
    }
    let mut keys = strings(args.get("modifiers"));
    keys.extend(strings(args.get("key")));
    keys
}

pub fn end_tool(active: Option<(String, CursorAction)>) {
    let Some((session, action)) = active else {
        return;
    };
    emit(CursorEvent::Action {
        session,
        phase: CursorEventPhase::End,
        semantics: CursorSemantics::new(action),
    });
}

fn emit(event: CursorEvent) {
    let sink = sink_slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(sink) = sink {
        sink.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    fn test_sink_guard() -> std::sync::MutexGuard<'static, ()> {
        static TEST_SINK: Mutex<()> = Mutex::new(());
        TEST_SINK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn emits_only_for_explicit_session_and_known_tool() {
        let _guard = test_sink_guard();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        install_cursor_event_sink(Arc::new(move |event| {
            captured
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        }));

        assert!(begin_tool("click", &json!({"x":1,"y":2})).is_none());
        assert!(begin_tool("unknown", &json!({"session":"run"})).is_none());
        let active = begin_tool(
            "click",
            &json!({"session":"run","x":1,"y":2,"delivery_mode":"background"}),
        );
        end_tool(active);

        clear_cursor_event_sink();
        let events = events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            CursorEvent::Action {
                phase: CursorEventPhase::Begin,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            CursorEvent::Action {
                phase: CursorEventPhase::End,
                ..
            }
        ));
    }

    fn capture(run: impl FnOnce()) -> Vec<CursorEvent> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        install_cursor_event_sink(Arc::new(move |event| {
            captured
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        }));
        run();
        clear_cursor_event_sink();
        let events = events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        events.clone()
    }

    #[test]
    fn key_combos_follow_their_action_and_bare_keys_emit_none() {
        let _guard = test_sink_guard();
        let events = capture(|| {
            begin_tool("hotkey", &json!({"session":"run","keys":["cmd","c"]}));
            begin_tool(
                "press_key",
                &json!({"session":"run","key":"v","modifiers":["cmd"]}),
            );
            begin_tool("press_key", &json!({"session":"run","key":"return"}));
            begin_tool("type_text", &json!({"session":"run","text":"cmd+c"}));
        });
        let combos: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                CursorEvent::KeyCombo { keys, .. } => Some(keys.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            combos,
            vec![
                vec!["cmd".to_owned(), "c".to_owned()],
                vec!["cmd".to_owned(), "v".to_owned()],
            ]
        );
        assert!(matches!(
            events[0],
            CursorEvent::Action {
                phase: CursorEventPhase::Begin,
                ..
            }
        ));
        assert!(matches!(events[1], CursorEvent::KeyCombo { .. }));
    }

    #[test]
    fn bringing_an_app_forward_is_a_foreground_moment() {
        let _guard = test_sink_guard();
        let events = capture(|| {
            begin_tool("bring_to_front", &json!({"session":"run","pid":1}));
            begin_tool("launch_app", &json!({"session":"run","bundle_id":"x"}));
        });
        let deliveries: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                CursorEvent::Action { semantics, .. } => Some(semantics.delivery),
                _ => None,
            })
            .collect();
        assert_eq!(deliveries, vec![Some(CursorDelivery::Foreground), None]);
    }

    #[test]
    fn start_session_cursor_motion_is_emitted_before_the_cursor_is_shown() {
        let _guard = test_sink_guard();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        install_cursor_event_sink(Arc::new(move |event| {
            captured
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        }));

        begin_tool(
            "start_session",
            &json!({"session":"run","cursor_motion":{"style":"spring_settle"}}),
        );
        // An invalid selection never reaches the overlay.
        begin_tool(
            "start_session",
            &json!({"session":"run","cursor_motion":{"style":7}}),
        );

        clear_cursor_event_sink();
        let events = events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let selected: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, CursorEvent::SelectMotion { .. }))
            .collect();
        assert_eq!(
            selected,
            vec![&CursorEvent::SelectMotion {
                session: "run".into(),
                motion: json!({"style":"spring_settle"}),
            }]
        );
        // The motion is queued ahead of the action that first shows the cursor.
        assert!(matches!(events[0], CursorEvent::SelectMotion { .. }));
    }

    #[test]
    fn public_label_metadata_is_emitted_separately_from_private_cursor_key() {
        let _guard = test_sink_guard();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        install_cursor_event_sink(Arc::new(move |event| {
            captured
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        }));

        let active = begin_tool(
            "click",
            &json!({
                "session":"__cua_runtime_private:research",
                "_public_session_label":"Research run",
                "x":1,
                "y":2
            }),
        );
        end_tool(active);

        clear_cursor_event_sink();
        let events = events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(matches!(
            &events[0],
            CursorEvent::SetSessionLabel { session, label }
                if session == "__cua_runtime_private:research" && label == "Research run"
        ));
        assert!(matches!(
            &events[1],
            CursorEvent::Action { session, .. }
                if session == "__cua_runtime_private:research"
        ));
    }
}
