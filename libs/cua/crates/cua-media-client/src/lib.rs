// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Transport-independent RCDP client state.
//!
//! A Unix socket, named pipe, WebSocket, or future transport sends the
//! `ClientMessage` values returned here and feeds received `ServerMessage`
//! values back through `apply`. This crate never links a server runtime or an
//! operating-system provider.

#[cfg(feature = "media-decode")]
pub mod decode;
pub mod v2;

use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;

use cua_media_protocol::{
    ActionBasis, ActionRequest, ClientMessage, Hello, OpenSession, ServerMessage, SessionOpened,
    SessionPolicy, TargetEpoch, TargetGrant, TargetHandle, Value, VideoCodec, WindowDescriptor,
    WindowGeometryControl, WindowSessionId, PROTOCOL_NAME, PROTOCOL_VERSION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientPhase {
    New,
    Authenticated,
    Negotiated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    AuthenticationRequired,
    NegotiationRequired,
    ProtocolMismatch,
    UnknownTarget,
    StaleTarget {
        expected: TargetEpoch,
        actual: TargetEpoch,
    },
    UnknownSession,
    UnexpectedResponse(&'static str),
    Server {
        code: String,
        message: String,
    },
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthenticationRequired => formatter.write_str("authentication is required"),
            Self::NegotiationRequired => formatter.write_str("protocol negotiation is required"),
            Self::ProtocolMismatch => {
                formatter.write_str("server selected an incompatible protocol")
            }
            Self::UnknownTarget => formatter.write_str("target is unknown"),
            Self::StaleTarget { expected, actual } => write!(
                formatter,
                "target epoch is stale: expected {}, got {}",
                expected.0, actual.0
            ),
            Self::UnknownSession => formatter.write_str("session is unknown"),
            Self::UnexpectedResponse(message) => formatter.write_str(message),
            Self::Server { code, message } => write!(formatter, "server error {code}: {message}"),
        }
    }
}

impl Error for ClientError {}

#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    pub authentication_required: bool,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub accepted_codecs: Vec<VideoCodec>,
    pub max_fps: u16,
    pub max_dimension: u32,
    pub target_bitrate_kbps: Option<u32>,
    pub policy: SessionPolicy,
    pub geometry_control: WindowGeometryControl,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            accepted_codecs: vec![VideoCodec::H264, VideoCodec::Png, VideoCodec::Bgra],
            max_fps: 15,
            max_dimension: 1280,
            target_bitrate_kbps: None,
            policy: SessionPolicy::BackgroundOnly,
            geometry_control: WindowGeometryControl::ObserveOnly,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientSession {
    pub target: TargetHandle,
    pub opened: SessionOpened,
}

pub struct Client {
    phase: ClientPhase,
    config: ClientConfig,
    targets: HashMap<TargetHandle, WindowDescriptor>,
    pending_opens: VecDeque<(TargetHandle, TargetEpoch)>,
    sessions: HashMap<WindowSessionId, ClientSession>,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        Self {
            phase: ClientPhase::New,
            config,
            targets: HashMap::new(),
            pending_opens: VecDeque::new(),
            sessions: HashMap::new(),
        }
    }

    pub fn phase(&self) -> ClientPhase {
        self.phase
    }

    pub fn authenticate(&self, token: impl Into<String>) -> Result<ClientMessage, ClientError> {
        if !self.config.authentication_required {
            return Err(ClientError::UnexpectedResponse(
                "this transport does not require protocol authentication",
            ));
        }
        if self.phase != ClientPhase::New {
            return Err(ClientError::UnexpectedResponse(
                "authentication has already completed",
            ));
        }
        Ok(ClientMessage::Authenticate {
            token: token.into(),
        })
    }

    pub fn hello(&self) -> Result<ClientMessage, ClientError> {
        if self.config.authentication_required && self.phase == ClientPhase::New {
            return Err(ClientError::AuthenticationRequired);
        }
        Ok(ClientMessage::Hello(Hello {
            protocol_name: PROTOCOL_NAME.into(),
            protocol_versions: vec![PROTOCOL_VERSION],
            capabilities: self.config.capabilities.clone(),
            build_revision: Some(cua_media_protocol::build_revision().into()),
        }))
    }

    pub fn list_targets(&self, on_screen_only: bool) -> Result<ClientMessage, ClientError> {
        self.require_negotiated()?;
        Ok(ClientMessage::ListWindows { on_screen_only })
    }

    pub fn pick_target(&self, prompt: Option<String>) -> Result<ClientMessage, ClientError> {
        self.require_negotiated()?;
        Ok(ClientMessage::PickWindow { prompt })
    }

    pub fn restore_target(&self, grant: TargetGrant) -> Result<ClientMessage, ClientError> {
        self.require_negotiated()?;
        Ok(ClientMessage::RestoreWindow { grant })
    }

    pub fn app_icon(
        &self,
        target: &TargetHandle,
        epoch: TargetEpoch,
    ) -> Result<ClientMessage, ClientError> {
        self.require_negotiated()?;
        let descriptor = self.targets.get(target).ok_or(ClientError::UnknownTarget)?;
        if descriptor.target_epoch != epoch {
            return Err(ClientError::StaleTarget {
                expected: descriptor.target_epoch,
                actual: epoch,
            });
        }
        Ok(ClientMessage::GetAppIcon {
            window: target.clone(),
            target_epoch: epoch,
        })
    }

    pub fn open_target(
        &mut self,
        target: &TargetHandle,
        epoch: TargetEpoch,
        options: OpenOptions,
    ) -> Result<ClientMessage, ClientError> {
        self.require_negotiated()?;
        let descriptor = self.targets.get(target).ok_or(ClientError::UnknownTarget)?;
        if descriptor.target_epoch != epoch {
            return Err(ClientError::StaleTarget {
                expected: descriptor.target_epoch,
                actual: epoch,
            });
        }
        self.pending_opens.push_back((target.clone(), epoch));
        Ok(ClientMessage::OpenSession(OpenSession {
            window: target.clone(),
            target_epoch: epoch,
            accepted_codecs: options.accepted_codecs,
            max_fps: options.max_fps,
            max_dimension: options.max_dimension,
            target_bitrate_kbps: options.target_bitrate_kbps,
            policy: options.policy,
            geometry_control: options.geometry_control,
        }))
    }

    pub fn action(
        &self,
        action_id: impl Into<String>,
        session_id: &WindowSessionId,
        action: impl Into<String>,
        arguments: Value,
        basis: ActionBasis,
    ) -> Result<ClientMessage, ClientError> {
        self.require_negotiated()?;
        if !self.sessions.contains_key(session_id) {
            return Err(ClientError::UnknownSession);
        }
        Ok(ClientMessage::Action(ActionRequest {
            action_id: action_id.into(),
            session_id: session_id.clone(),
            tool: action.into(),
            arguments,
            basis,
        }))
    }

    pub fn window_state(&self, session_id: &WindowSessionId) -> Result<ClientMessage, ClientError> {
        self.require_negotiated()?;
        if !self.sessions.contains_key(session_id) {
            return Err(ClientError::UnknownSession);
        }
        Ok(ClientMessage::GetWindowState {
            session_id: session_id.clone(),
        })
    }

    pub fn close_session(
        &self,
        session_id: &WindowSessionId,
    ) -> Result<ClientMessage, ClientError> {
        self.require_negotiated()?;
        if !self.sessions.contains_key(session_id) {
            return Err(ClientError::UnknownSession);
        }
        Ok(ClientMessage::CloseSession {
            session_id: session_id.clone(),
        })
    }

    pub fn targets(&self) -> impl Iterator<Item = &WindowDescriptor> {
        self.targets.values()
    }

    pub fn session(&self, id: &WindowSessionId) -> Option<&ClientSession> {
        self.sessions.get(id)
    }

    pub fn apply(&mut self, message: ServerMessage) -> Result<(), ClientError> {
        match message {
            ServerMessage::Authenticated => {
                if !self.config.authentication_required || self.phase != ClientPhase::New {
                    return Err(ClientError::UnexpectedResponse(
                        "unexpected authentication response",
                    ));
                }
                self.phase = ClientPhase::Authenticated;
            }
            ServerMessage::Hello(hello) => {
                if hello.protocol_name != PROTOCOL_NAME
                    || hello.protocol_versions.as_slice() != [PROTOCOL_VERSION]
                {
                    return Err(ClientError::ProtocolMismatch);
                }
                if self.config.authentication_required && self.phase != ClientPhase::Authenticated {
                    return Err(ClientError::AuthenticationRequired);
                }
                self.phase = ClientPhase::Negotiated;
            }
            ServerMessage::Windows { windows } => {
                self.require_negotiated()?;
                self.targets = windows
                    .into_iter()
                    .map(|descriptor| (descriptor.window.clone(), descriptor))
                    .collect();
            }
            ServerMessage::WindowSelected { window, .. } => {
                self.require_negotiated()?;
                self.targets.insert(window.window.clone(), window);
            }
            ServerMessage::SessionOpened(opened) => {
                self.require_negotiated()?;
                let (target, requested_epoch) =
                    self.pending_opens
                        .pop_front()
                        .ok_or(ClientError::UnexpectedResponse(
                            "session opened without a pending request",
                        ))?;
                if opened.target_epoch != requested_epoch {
                    return Err(ClientError::StaleTarget {
                        expected: requested_epoch,
                        actual: opened.target_epoch,
                    });
                }
                self.sessions
                    .insert(opened.session_id.clone(), ClientSession { target, opened });
            }
            ServerMessage::SessionClosed { session_id } => {
                self.sessions.remove(&session_id);
            }
            ServerMessage::Error { code, message } => {
                return Err(ClientError::Server {
                    code: format!("{code:?}"),
                    message,
                });
            }
            ServerMessage::Unsupported => {}
            _ => {
                self.require_negotiated()?;
            }
        }
        Ok(())
    }

    fn require_negotiated(&self) -> Result<(), ClientError> {
        if self.phase == ClientPhase::Negotiated {
            Ok(())
        } else {
            Err(ClientError::NegotiationRequired)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_media_protocol::{
        ActionCapability, ActionDeliveryGuarantee, CodecEpoch, GeometryEpoch, SurfaceGeometry,
    };

    fn descriptor(epoch: u64) -> WindowDescriptor {
        WindowDescriptor {
            window: TargetHandle("target-opaque".into()),
            target_epoch: TargetEpoch(epoch),
            app_name: "Notes".into(),
            title: "Draft".into(),
            geometry: SurfaceGeometry {
                width_px: 800,
                height_px: 600,
                scale_factor: 2.0,
            },
            visible: true,
        }
    }

    fn negotiated_client() -> Client {
        let mut client = Client::new(ClientConfig::default());
        client
            .apply(ServerMessage::Hello(Hello::default()))
            .unwrap();
        client
    }

    #[test]
    fn local_client_negotiates_before_discovery() {
        let client = Client::new(ClientConfig::default());
        assert_eq!(
            client.list_targets(true).unwrap_err(),
            ClientError::NegotiationRequired
        );
        assert!(matches!(client.hello().unwrap(), ClientMessage::Hello(_)));
    }

    #[test]
    fn open_rejects_stale_target_before_transport() {
        let mut client = negotiated_client();
        client
            .apply(ServerMessage::Windows {
                windows: vec![descriptor(7)],
            })
            .unwrap();
        let error = client
            .open_target(
                &TargetHandle("target-opaque".into()),
                TargetEpoch(6),
                OpenOptions::default(),
            )
            .unwrap_err();
        assert_eq!(
            error,
            ClientError::StaleTarget {
                expected: TargetEpoch(7),
                actual: TargetEpoch(6),
            }
        );
    }

    #[test]
    fn app_icon_request_reuses_target_validation() {
        let mut client = negotiated_client();
        let target = descriptor(7);
        client
            .apply(ServerMessage::Windows {
                windows: vec![target.clone()],
            })
            .unwrap();
        assert_eq!(
            client.app_icon(&target.window, TargetEpoch(6)).unwrap_err(),
            ClientError::StaleTarget {
                expected: TargetEpoch(7),
                actual: TargetEpoch(6),
            }
        );
        assert_eq!(
            client
                .app_icon(&target.window, target.target_epoch)
                .unwrap(),
            ClientMessage::GetAppIcon {
                window: target.window,
                target_epoch: TargetEpoch(7),
            }
        );
    }

    #[test]
    fn session_tracks_opaque_target_and_policy() {
        let mut client = negotiated_client();
        let target = descriptor(7);
        client
            .apply(ServerMessage::Windows {
                windows: vec![target.clone()],
            })
            .unwrap();
        client
            .open_target(&target.window, target.target_epoch, OpenOptions::default())
            .unwrap();

        let session_id = WindowSessionId("session-opaque".into());
        client
            .apply(ServerMessage::SessionOpened(SessionOpened {
                session_id: session_id.clone(),
                target_epoch: target.target_epoch,
                geometry_epoch: GeometryEpoch(1),
                codec_epoch: CodecEpoch(1),
                geometry: target.geometry,
                codec: VideoCodec::Png,
                max_fps: 15,
                max_dimension: 1280,
                target_bitrate_kbps: None,
                capabilities: vec!["video.png".into()],
                action_capabilities: vec![ActionCapability {
                    action: "type_text".into(),
                    guarantee: ActionDeliveryGuarantee::Background,
                }],
                policy: SessionPolicy::BackgroundOnly,
                geometry_control: WindowGeometryControl::ObserveOnly,
            }))
            .unwrap();

        let session = client.session(&session_id).unwrap();
        assert_eq!(session.target, TargetHandle("target-opaque".into()));
        assert_eq!(session.opened.policy, SessionPolicy::BackgroundOnly);
    }

    #[test]
    fn websocket_client_requires_authentication_before_hello() {
        let mut client = Client::new(ClientConfig {
            authentication_required: true,
            capabilities: Vec::new(),
        });
        assert_eq!(
            client.hello().unwrap_err(),
            ClientError::AuthenticationRequired
        );
        client.apply(ServerMessage::Authenticated).unwrap();
        assert!(matches!(client.hello().unwrap(), ClientMessage::Hello(_)));
    }
}
