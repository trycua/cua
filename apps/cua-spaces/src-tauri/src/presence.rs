// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Presence for the stream viewers: one `PresenceService` session per viewer
//! window, joined through the cua SDK (`cua_spaces::presence`) with cursor
//! shapes, roster heartbeats, cursor batches and the QUIC datagram channel
//! when the Space offers it.
//!
//! The webview draws; this module only moves data:
//! - `presence_join` joins and returns the caller, the roster and the render
//!   delay that matches the transport;
//! - every event is emitted to the joining webview as `presence-event`, in
//!   the TS SDK's `PresenceEvent` shape (camelCase), which
//!   `@trycua/cua/spaces/presence`'s `PresenceView` folds;
//! - `presence_publish` hands the local pointer to the SDK's `CursorSender`
//!   (at most every 33 ms, newest wins, show/hide and target changes at
//!   once);
//! - `presence_leave`, or the window closing, leaves.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_spaces::presence::{
    now_ms, Cursor, CursorSender, Identity, Participant, PresenceEvent, PresenceSession,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State, WebviewWindow};
use tokio::sync::oneshot;

use crate::commands::AppState;
use crate::core::CmdResult;

/// How long a join may take before the viewer gives up on presence.
const JOIN_TIMEOUT: Duration = Duration::from_secs(10);
/// One `next_event` wait; the loop also checks for a leave between waits.
const EVENT_WAIT: Duration = Duration::from_secs(1);
/// Event name emitted to the joining webview.
pub const PRESENCE_EVENT: &str = "presence-event";

struct Joined {
    window: String,
    sender: CursorSender,
    leave: Option<oneshot::Sender<()>>,
}

/// Live presence sessions by handle.
#[derive(Default)]
pub struct PresenceSessions {
    next: AtomicU64,
    live: Mutex<HashMap<String, Joined>>,
}

/// A participant, as the TS SDK's `PresenceParticipant`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebParticipant {
    pub participant_id: String,
    pub principal_id: String,
    pub display_name: String,
    pub color: String,
    pub kind: String,
}

impl From<&Participant> for WebParticipant {
    fn from(p: &Participant) -> Self {
        Self {
            participant_id: p.participant_id.clone(),
            principal_id: p.principal_id.clone(),
            display_name: p.display_name.clone(),
            color: p.color.clone(),
            kind: p.kind.clone(),
        }
    }
}

/// A cursor, as the TS SDK's `PresenceCursor`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebCursor {
    pub display_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub visible: bool,
    pub pressed: bool,
    pub shape: String,
    pub shape_source: String,
    pub at_ms: f64,
    pub received_ms: f64,
}

impl From<&Cursor> for WebCursor {
    fn from(c: &Cursor) -> Self {
        Self {
            display_id: c.display_id.clone(),
            window_id: c.window_id.clone(),
            x: c.x,
            y: c.y,
            visible: c.visible,
            pressed: c.pressed,
            shape: c.shape.as_str().into(),
            shape_source: c.shape_source.as_str().into(),
            at_ms: c.at_ms,
            received_ms: c.received_ms,
        }
    }
}

/// An event, as the TS SDK's `PresenceEvent`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebEvent {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant: Option<WebParticipant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<WebCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_ids: Option<Vec<String>>,
}

impl From<&PresenceEvent> for WebEvent {
    fn from(e: &PresenceEvent) -> Self {
        match e {
            PresenceEvent::Joined { participant } => WebEvent {
                kind: "joined".into(),
                participant: Some(participant.into()),
                ..Default::default()
            },
            PresenceEvent::Left {
                participant_id,
                reason,
            } => WebEvent {
                kind: "left".into(),
                participant_id: Some(participant_id.clone()),
                reason: Some(reason.clone()),
                ..Default::default()
            },
            PresenceEvent::CursorMoved {
                participant_id,
                cursor,
            } => WebEvent {
                kind: "cursor_moved".into(),
                participant_id: Some(participant_id.clone()),
                cursor: Some(cursor.into()),
                ..Default::default()
            },
            PresenceEvent::ShapeChanged {
                participant_id,
                shape,
                source,
            } => WebEvent {
                kind: "shape_changed".into(),
                participant_id: Some(participant_id.clone()),
                shape: Some(shape.as_str().into()),
                shape_source: Some(source.as_str().into()),
                ..Default::default()
            },
            PresenceEvent::Heartbeat { participant_ids } => WebEvent {
                kind: "heartbeat".into(),
                participant_ids: Some(participant_ids.clone()),
                ..Default::default()
            },
            PresenceEvent::KeepAlive => WebEvent {
                kind: "keep_alive".into(),
                ..Default::default()
            },
        }
    }
}

/// One roster member at join.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebMember {
    pub participant: WebParticipant,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<WebCursor>,
}

/// What `presence_join` returns.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinInfo {
    pub handle: String,
    pub me: WebParticipant,
    pub members: Vec<WebMember>,
    /// Render delay for remote cursors on this transport (ms).
    pub delay_ms: f64,
    pub datagrams: bool,
}

/// What `presence-event` carries.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    handle: String,
    event: WebEvent,
}

/// Who this app joins as, decided by the app core as in the SwiftUI app: the
/// signed-in account (`None` when signed out) by its name, else its email's
/// local part, else its username; else this computer's user.
fn identity(account: Option<&cua_auth::Identity>, os_user: Option<&str>) -> Identity {
    use cua_spaces_app_core::presence::{presence_name, presence_principal_id};
    let claim = |pick: fn(&cua_auth::Identity) -> &Option<String>| {
        account.and_then(|account| pick(account).as_deref())
    };
    Identity {
        id: presence_principal_id(
            claim(|a| &a.email),
            claim(|a| &a.subject),
            claim(|a| &a.username),
            os_user,
        ),
        display_name: presence_name(
            claim(|a| &a.name),
            claim(|a| &a.email),
            claim(|a| &a.username),
            None,
            os_user,
        ),
        color: String::new(),
        agent: false,
    }
}

/// Joins presence for the calling webview window.
#[tauri::command]
pub async fn presence_join(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    sessions: State<'_, Arc<PresenceSessions>>,
    space_id: String,
) -> CmdResult<JoinInfo> {
    // The account's claims only while signed in as a user (not an API key).
    let signed_in = state.0.fleet_status(false).await.identity.is_some();
    let account = signed_in.then(|| state.0.session().profile()).flatten();
    let os_user = std::env::var("USER").ok();
    let space = state
        .0
        .spaces()
        .space(&space_id)
        .await
        .map_err(|e| e.to_string())?;
    let session = space
        .join_presence(identity(account.as_ref(), os_user.as_deref()), JOIN_TIMEOUT)
        .await
        .map_err(|e| e.to_string())?;
    let handle = format!("p{}", sessions.next.fetch_add(1, Ordering::Relaxed) + 1);
    let info = JoinInfo {
        handle: handle.clone(),
        me: session.me().into(),
        members: session
            .roster()
            .iter()
            .map(|(p, c)| WebMember {
                participant: p.into(),
                cursor: c.as_ref().map(Into::into),
            })
            .collect(),
        delay_ms: if session.uses_datagrams() {
            66.0
        } else {
            100.0
        },
        datagrams: session.uses_datagrams(),
    };
    let (leave_tx, leave_rx) = oneshot::channel();
    sessions.live.lock().unwrap().insert(
        handle.clone(),
        Joined {
            window: window.label().to_string(),
            sender: session.sender(),
            leave: Some(leave_tx),
        },
    );
    let label = window.label().to_string();
    let registry = sessions.inner().clone();
    tauri::async_runtime::spawn(pump(app, label, handle, session, leave_rx, registry));
    Ok(info)
}

/// Forwards events to the webview until a leave or the stream ends.
async fn pump(
    app: AppHandle,
    label: String,
    handle: String,
    mut session: PresenceSession,
    mut leave: oneshot::Receiver<()>,
    registry: Arc<PresenceSessions>,
) {
    loop {
        if leave.try_recv().is_ok() {
            break;
        }
        match session.next_event(EVENT_WAIT).await {
            Ok(Some(event)) => {
                let _ = app.emit_to(
                    label.as_str(),
                    PRESENCE_EVENT,
                    Envelope {
                        handle: handle.clone(),
                        event: (&event).into(),
                    },
                );
            }
            Ok(None) => break,
            // A quiet second: loop to check for a leave.
            Err(cua_spaces::Error::Timeout(_)) => continue,
            Err(_) => break,
        }
    }
    registry.live.lock().unwrap().remove(&handle);
    let _ = session.leave().await;
}

/// Publishes the local pointer (normalized to the streamed surface).
#[tauri::command]
pub async fn presence_publish(
    sessions: State<'_, Arc<PresenceSessions>>,
    handle: String,
    x: f64,
    y: f64,
    visible: bool,
    window_id: Option<String>,
    display_id: Option<String>,
) -> CmdResult<()> {
    let sender = sessions
        .live
        .lock()
        .unwrap()
        .get(&handle)
        .map(|j| j.sender.clone());
    let Some(sender) = sender else {
        return Err(format!("unknown presence session {handle}"));
    };
    let mut cursor = Cursor::at(x, y);
    cursor.visible = visible;
    cursor.window_id = window_id.filter(|w| !w.is_empty());
    cursor.display_id = display_id.unwrap_or_default();
    cursor.received_ms = now_ms();
    sender.update(&cursor).await.map_err(|e| e.to_string())
}

/// Leaves one session.
#[tauri::command]
pub fn presence_leave(sessions: State<'_, Arc<PresenceSessions>>, handle: String) {
    sessions.leave(&handle);
}

impl PresenceSessions {
    /// Signals the session's pump to leave.
    pub fn leave(&self, handle: &str) {
        if let Some(mut j) = self.live.lock().unwrap().remove(handle) {
            if let Some(tx) = j.leave.take() {
                let _ = tx.send(());
            }
        }
    }

    /// Leaves every session a closed window joined.
    pub fn window_closed(&self, label: &str) {
        let handles: Vec<String> = self
            .live
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, j)| j.window == label)
            .map(|(h, _)| h.clone())
            .collect();
        for h in handles {
            self.leave(&h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_spaces::presence::{CursorShape, ShapeSource};
    use serde_json::json;

    fn participant() -> Participant {
        Participant {
            participant_id: "p1".into(),
            principal_id: "user:a".into(),
            display_name: "A".into(),
            color: "#e6194b".into(),
            kind: "human".into(),
        }
    }

    #[test]
    fn events_use_the_ts_sdk_shape() {
        let mut c = Cursor::at(0.25, 0.5);
        c.shape = CursorShape::Text;
        c.shape_source = ShapeSource::Probe;
        c.at_ms = 10.0;
        c.received_ms = 20.0;
        let moved = WebEvent::from(&PresenceEvent::CursorMoved {
            participant_id: "p1".into(),
            cursor: c,
        });
        assert_eq!(
            serde_json::to_value(&moved).unwrap(),
            json!({"kind": "cursor_moved", "participantId": "p1", "cursor": {
                "displayId": "", "x": 0.25, "y": 0.5, "visible": true, "pressed": false,
                "shape": "text", "shapeSource": "probe", "atMs": 10.0, "receivedMs": 20.0}})
        );
        let joined = WebEvent::from(&PresenceEvent::Joined {
            participant: participant(),
        });
        assert_eq!(
            serde_json::to_value(&joined).unwrap(),
            json!({"kind": "joined", "participant": {"participantId": "p1",
                "principalId": "user:a", "displayName": "A", "color": "#e6194b", "kind": "human"}})
        );
        let left = WebEvent::from(&PresenceEvent::Left {
            participant_id: "p1".into(),
            reason: "run_ended".into(),
        });
        assert_eq!(
            serde_json::to_value(&left).unwrap(),
            json!({"kind": "left", "participantId": "p1", "reason": "run_ended"})
        );
        let shape = WebEvent::from(&PresenceEvent::ShapeChanged {
            participant_id: "p1".into(),
            shape: CursorShape::Pointer,
            source: ShapeSource::HitTest,
        });
        assert_eq!(
            serde_json::to_value(&shape).unwrap(),
            json!({"kind": "shape_changed", "participantId": "p1", "shape": "pointer",
                "shapeSource": "hit_test"})
        );
        let beat = WebEvent::from(&PresenceEvent::Heartbeat {
            participant_ids: vec!["p1".into()],
        });
        assert_eq!(
            serde_json::to_value(&beat).unwrap(),
            json!({"kind": "heartbeat", "participantIds": ["p1"]})
        );
    }

    #[test]
    fn identity_prefers_the_account() {
        let account = cua_auth::Identity {
            username: Some("dana42".into()),
            email: Some("dana@example.com".into()),
            name: Some("Dana Scully".into()),
            subject: Some("sub-1".into()),
        };
        let id = identity(Some(&account), Some("d"));
        assert_eq!(id.id, "user:dana@example.com");
        assert_eq!(id.display_name, "Dana Scully");
        assert!(!id.agent);
        let email_only = cua_auth::Identity {
            email: Some("dana@example.com".into()),
            ..Default::default()
        };
        assert_eq!(identity(Some(&email_only), None).display_name, "dana");
        let local = identity(None, Some("dillon"));
        assert_eq!(local.id, "user:local:dillon");
        assert_eq!(local.display_name, "dillon");
        // Never an agent's name, "You" or empty.
        let you = cua_auth::Identity {
            name: Some("You".into()),
            ..Default::default()
        };
        assert_eq!(identity(Some(&you), Some("sam")).display_name, "sam");
        assert!(!identity(None, None).display_name.is_empty());
    }
}
