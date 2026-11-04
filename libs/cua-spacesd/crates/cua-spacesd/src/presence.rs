// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Daemon-level multi-user state: who is connected, where each user's cursor
//! is, and the configured launchable app menu.
//!
//! Presence is a property of the daemon rather than of one session, so it
//! lives here instead of `cua-spacesd-session`. Client cursor positions fan out to
//! every other connection and drive the host desktop overlay through the
//! adapter's presenter, which keeps native window identifiers inside the
//! adapter.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_media_protocol::{AppEntry, ClientMessage, CursorState, PresenceUser, ServerMessage};
use cua_spacesd_desktop::{CuaAppLauncher, CuaPresenceOverlay};
use cua_spacesd_provider_api::{TargetProvider, TargetQuery};
use serde::Deserialize;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const CURSOR_COLORS: &[&str] = &[
    "#ff5f5f", "#4fc3f7", "#ffd54f", "#81c784", "#ba68c8", "#ff8a65", "#26c6da", "#f06292",
];

/// One launchable app configured for the daemon's app menu. The path never
/// crosses the wire; clients only see `app_id` and `name`.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub app_id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub args: Vec<String>,
}

pub fn load_apps(
    path: &std::path::Path,
) -> Result<Vec<AppConfig>, Box<dyn std::error::Error + Send + Sync>> {
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

struct Member {
    user: Option<PresenceUser>,
    sender: UnboundedSender<ServerMessage>,
}

pub struct PresenceHub {
    members: Mutex<HashMap<u64, Member>>,
    next_color: AtomicUsize,
    overlay: Arc<CuaPresenceOverlay>,
    launcher: Arc<CuaAppLauncher>,
    apps: Vec<AppConfig>,
}

impl PresenceHub {
    pub fn new(
        overlay: Arc<CuaPresenceOverlay>,
        launcher: Arc<CuaAppLauncher>,
        apps: Vec<AppConfig>,
    ) -> Self {
        overlay.activate();
        Self {
            members: Mutex::new(HashMap::new()),
            next_color: AtomicUsize::new(0),
            overlay,
            launcher,
            apps,
        }
    }

    pub fn register(&self, connection_id: u64) -> UnboundedReceiver<ServerMessage> {
        let (sender, receiver) = unbounded_channel();
        self.members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(connection_id, Member { user: None, sender });
        receiver
    }

    pub fn unregister(&self, connection_id: u64) {
        let removed = self
            .members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&connection_id);
        if let Some(member) = removed {
            if let Some(user) = member.user {
                self.overlay.remove_cursor(&user.user_id);
                self.broadcast(
                    None,
                    ServerMessage::Presence {
                        users: self.roster(),
                    },
                );
            }
        }
    }

    fn roster(&self) -> Vec<PresenceUser> {
        let mut users: Vec<PresenceUser> = self
            .members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter_map(|member| member.user.clone())
            .collect();
        // The person at the machine is always present: their physical cursor
        // is broadcast as the synthetic `host` user by the desktop watcher.
        users.push(PresenceUser {
            user_id: "host".into(),
            name: "host (desktop)".into(),
            color: "#e8e8e8".into(),
        });
        users
    }

    /// Send a message to every joined member except `skip`.
    fn broadcast(&self, skip: Option<u64>, message: ServerMessage) {
        let members = self
            .members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (id, member) in members.iter() {
            if Some(*id) == skip || member.user.is_none() {
                continue;
            }
            let _ = member.sender.send(message.clone());
        }
    }

    /// Send a message to every connection, joined or not. Window-list pushes
    /// and the host cursor matter to a client that has not joined presence.
    pub fn broadcast_all(&self, message: ServerMessage) {
        let members = self
            .members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for member in members.values() {
            let _ = member.sender.send(message.clone());
        }
    }

    /// Handle a daemon-level client message. Returns `None` when the message
    /// belongs to the per-connection session runtime instead.
    pub fn handle(
        self: &Arc<Self>,
        connection_id: u64,
        message: &ClientMessage,
    ) -> Option<Vec<ServerMessage>> {
        match message {
            ClientMessage::Join { name, color } => {
                let color = color.clone().unwrap_or_else(|| {
                    let index = self.next_color.fetch_add(1, Ordering::Relaxed);
                    CURSOR_COLORS[index % CURSOR_COLORS.len()].to_owned()
                });
                let user = PresenceUser {
                    user_id: format!("user-{connection_id}"),
                    name: name.clone(),
                    color,
                };
                if let Some(member) = self
                    .members
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_mut(&connection_id)
                {
                    member.user = Some(user.clone());
                }
                let users = self.roster();
                self.broadcast(
                    Some(connection_id),
                    ServerMessage::Presence {
                        users: users.clone(),
                    },
                );
                Some(vec![ServerMessage::Joined { user, users }])
            }
            ClientMessage::Cursor {
                window,
                x,
                y,
                visible,
                pressed,
            } => {
                let user = self
                    .members
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&connection_id)
                    .and_then(|member| member.user.clone());
                let Some(user) = user else {
                    // Cursor updates before `join` carry no identity; drop them.
                    return Some(Vec::new());
                };
                self.overlay.update_cursor(
                    &user.user_id,
                    window.as_ref(),
                    *x,
                    *y,
                    *visible,
                    *pressed,
                );
                self.broadcast(
                    Some(connection_id),
                    ServerMessage::RemoteCursor(CursorState {
                        user_id: user.user_id,
                        name: user.name,
                        color: user.color,
                        window: window.clone(),
                        x: *x,
                        y: *y,
                        visible: *visible,
                        pressed: *pressed,
                        // A remote participant does not own the origin
                        // desktop's one physical pointer, so no host can say
                        // what the OS is drawing under *their* point. Report
                        // Unknown rather than inventing an arrow; the viewer
                        // holds whatever shape it last had.
                        shape: cua_media_protocol::CursorShape::Unknown,
                    }),
                );
                Some(Vec::new())
            }
            ClientMessage::ListApps => Some(vec![ServerMessage::Apps {
                apps: self
                    .apps
                    .iter()
                    .map(|app| AppEntry {
                        app_id: app.app_id.clone(),
                        name: app.name.clone(),
                    })
                    .collect(),
            }]),
            ClientMessage::LaunchApp { app_id } => {
                let Some(app) = self.apps.iter().find(|app| &app.app_id == app_id) else {
                    return Some(vec![ServerMessage::Error {
                        code: cua_media_protocol::ServerErrorCode::Unsupported,
                        message: format!("unknown app_id {app_id}"),
                    }]);
                };
                // Launching can take seconds (window materialization retries),
                // so it must not stall this connection's frame pump. The result
                // is delivered through the presence channel.
                let hub = self.clone();
                let app = app.clone();
                tokio::spawn(async move {
                    let result = hub.launcher.launch(&app.path, &app.args).await;
                    let message = match result {
                        Ok(window) => ServerMessage::AppLaunched {
                            app_id: app.app_id.clone(),
                            window,
                        },
                        Err(error) => ServerMessage::Error {
                            code: cua_media_protocol::ServerErrorCode::Internal,
                            message: format!("launch failed: {error}"),
                        },
                    };
                    let members = hub
                        .members
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(member) = members.get(&connection_id) {
                        let _ = member.sender.send(message);
                    }
                });
                Some(Vec::new())
            }
            _ => None,
        }
    }
}

/// Push daemon-observed desktop state to every connection:
///
/// - the window list, re-enumerated periodically and broadcast whenever it
///   changes, so clients stay in sync as windows open and close without
///   pressing refresh;
/// - the host desktop's physical cursor, sampled and broadcast as the
///   synthetic `host` user whenever it moves over a known window, so remote
///   clients see where the person at the machine is pointing.
pub fn spawn_desktop_watchers(
    hub: Arc<PresenceHub>,
    targets: Arc<dyn TargetProvider>,
    overlay: Arc<CuaPresenceOverlay>,
) {
    {
        let hub = hub.clone();
        tokio::spawn(async move {
            let mut fingerprint = String::new();
            loop {
                let windows = tokio::task::spawn_blocking({
                    let targets = targets.clone();
                    move || {
                        targets.enumerate(&TargetQuery {
                            on_screen_only: true,
                        })
                    }
                })
                .await;
                if let Ok(Ok(targets)) = windows {
                    let descriptors: Vec<_> = targets
                        .into_iter()
                        .map(|target| target.descriptor)
                        .collect();
                    let next_fingerprint = descriptors
                        .iter()
                        .map(|descriptor| {
                            format!(
                                "{}#{}#{}#{}x{}",
                                descriptor.window.0,
                                descriptor.target_epoch.0,
                                descriptor.title,
                                descriptor.geometry.width_px,
                                descriptor.geometry.height_px
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("|");
                    if next_fingerprint != fingerprint {
                        fingerprint = next_fingerprint;
                        hub.broadcast_all(ServerMessage::Windows {
                            windows: descriptors,
                        });
                    }
                }
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
        });
    }

    tokio::spawn(async move {
        let mut last: Option<(String, i64, i64, bool)> = None;
        loop {
            let cursor = overlay.host_cursor();
            let state = cursor
                .as_ref()
                .map(|(window, x, y, pressed)| (window.0.clone(), *x as i64, *y as i64, *pressed));
            if state != last {
                let message = match &cursor {
                    Some((window, x, y, pressed)) => ServerMessage::RemoteCursor(CursorState {
                        user_id: "host".into(),
                        name: "host (desktop)".into(),
                        color: "#e8e8e8".into(),
                        window: Some(window.clone()),
                        x: *x,
                        y: *y,
                        visible: true,
                        pressed: *pressed,
                        // The host cursor IS the physical pointer, so the
                        // system shape read now is the shape under it.
                        shape: cua_spacesd_desktop::host_cursor_shape(),
                    }),
                    None => ServerMessage::RemoteCursor(CursorState {
                        user_id: "host".into(),
                        name: "host (desktop)".into(),
                        color: "#e8e8e8".into(),
                        window: None,
                        x: 0.0,
                        y: 0.0,
                        visible: false,
                        shape: cua_media_protocol::CursorShape::Unknown,
                        pressed: false,
                    }),
                };
                hub.broadcast_all(message);
                last = state;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
}
