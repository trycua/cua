// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Best-effort semantic cursor-event fanout.
//!
//! The dispatch boundary emits these events after authorization and scope
//! validation. Platform overlays may subscribe, but failures and missing
//! subscribers are intentionally invisible to the tool result.

use cua_driver_contract::{
    classify_cursor_semantics, CursorAction, CursorSemantics, CursorThemeSelection,
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
    Action {
        session: String,
        phase: CursorEventPhase,
        semantics: CursorSemantics,
    },
    SelectTheme {
        session: String,
        selection: CursorThemeSelection,
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
    }
    let semantics = classify_cursor_semantics(name, args)?;
    emit(CursorEvent::Action {
        session: session.clone(),
        phase: CursorEventPhase::Begin,
        semantics,
    });
    Some((session, semantics.action))
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

    #[test]
    fn emits_only_for_explicit_session_and_known_tool() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        install_cursor_event_sink(Arc::new(move |event| {
            captured.lock().unwrap().push(event);
        }));

        assert!(begin_tool("click", &json!({"x":1,"y":2})).is_none());
        assert!(begin_tool("unknown", &json!({"session":"run"})).is_none());
        let active = begin_tool(
            "click",
            &json!({"session":"run","x":1,"y":2,"delivery_mode":"background"}),
        );
        end_tool(active);

        let events = events.lock().unwrap();
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
        clear_cursor_event_sink();
    }
}
