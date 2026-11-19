// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Dependency-light provider contracts for RCDP servers.
//!
//! Providers own native window identifiers, permission objects, and operating
//! system resources. RCDP clients see only server-issued handles and epochs.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use cua_media_protocol::{
    AccessibilitySnapshotId, ActionBasis, ActionCapability, ActionDeliveryGuarantee,
    InteractiveInputBatch, SessionPolicy, SurfaceGeometry, TargetEpoch, TargetGrant, TargetHandle,
    WindowDescriptor,
};
use serde_json::Value;
use thiserror::Error;

pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Provider-owned identity. Its value is never serialized or accepted from a
/// client. Adapters may encode native identifiers inside it, but must not log
/// the value as a public RCDP target handle.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BackendTargetKey(Arc<str>);

impl BackendTargetKey {
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }

    /// The provider-private value. Only the provider that minted the key
    /// may interpret it; never log or serialize it.
    pub fn provider_value(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BackendTargetKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BackendTargetKey([redacted])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderTargetId {
    pub key: BackendTargetKey,
    pub epoch: TargetEpoch,
}

#[derive(Debug, Clone)]
pub struct ProviderTarget {
    pub id: ProviderTargetId,
    pub descriptor: WindowDescriptor,
    /// Server-owned restoration capability. This is not a native portal token.
    pub grant: Option<TargetGrant>,
}

#[derive(Debug, Clone)]
pub struct ProviderAppIcon {
    pub media_type: String,
    pub bytes: Arc<[u8]>,
}

#[derive(Debug, Clone, Default)]
pub struct TargetQuery {
    pub on_screen_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PickTargetRequest {
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorCode {
    StaleTarget,
    PermissionDenied,
    ConsentRequired,
    TargetUnavailable,
    Unsupported,
    ViewOnly,
    WouldRequireActivation,
    DeliveryFailed,
    CaptureFailed,
    Internal,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{code:?}: {message}")]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    pub message: String,
}

impl ProviderError {
    pub fn new(code: ProviderErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub trait TargetProvider: Send + Sync + 'static {
    fn enumerate(&self, query: &TargetQuery) -> Result<Vec<ProviderTarget>, ProviderError>;

    fn pick(&self, request: &PickTargetRequest) -> Result<ProviderTarget, ProviderError>;

    fn restore(&self, grant: &TargetGrant) -> Result<ProviderTarget, ProviderError>;

    fn resolve(
        &self,
        handle: &TargetHandle,
        epoch: TargetEpoch,
    ) -> Result<ProviderTarget, ProviderError>;

    /// Return an application icon for a validated target. Providers that do
    /// not project native app identity may leave this unsupported.
    fn app_icon(
        &self,
        handle: &TargetHandle,
        epoch: TargetEpoch,
    ) -> Result<Option<ProviderAppIcon>, ProviderError> {
        self.resolve(handle, epoch)?;
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra8,
    Png,
    H264AnnexB,
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub max_fps: u16,
    pub max_dimension: u32,
    pub target_bitrate_kbps: Option<u32>,
    pub accepted_formats: Vec<PixelFormat>,
}

#[derive(Debug, Clone)]
pub struct OwnedFrame {
    pub bytes: Arc<[u8]>,
    pub format: PixelFormat,
    pub width_px: u32,
    pub height_px: u32,
    /// Packed formats set this explicitly. Encoded formats use `None`.
    pub bytes_per_row: Option<u32>,
    pub capture_timestamp_us: u64,
    /// Time from encoder submission until encoded output. Raw frames and
    /// providers without timing support use `None`.
    pub encode_duration_us: Option<u32>,
    /// Starts at one and changes whenever decoder state is replaced. Raw and
    /// independently decodable image formats use epoch one.
    pub codec_epoch: u64,
    pub keyframe: bool,
}

impl OwnedFrame {
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.width_px == 0 || self.height_px == 0 {
            return Err(ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                "frame dimensions must be non-zero",
            ));
        }
        if self.codec_epoch == 0 {
            return Err(ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                "codec epoch must be non-zero",
            ));
        }
        if self.format == PixelFormat::Bgra8 {
            let expected_stride = self.width_px.checked_mul(4).ok_or_else(|| {
                ProviderError::new(ProviderErrorCode::CaptureFailed, "BGRA stride overflow")
            })?;
            if self.bytes_per_row != Some(expected_stride) {
                return Err(ProviderError::new(
                    ProviderErrorCode::CaptureFailed,
                    "v1 BGRA frames must be tightly packed",
                ));
            }
            let expected_len = usize::try_from(expected_stride)
                .ok()
                .and_then(|stride| stride.checked_mul(self.height_px as usize))
                .ok_or_else(|| {
                    ProviderError::new(ProviderErrorCode::CaptureFailed, "BGRA size overflow")
                })?;
            if self.bytes.len() != expected_len {
                return Err(ProviderError::new(
                    ProviderErrorCode::CaptureFailed,
                    "BGRA payload length does not match dimensions",
                ));
            }
        }
        if self.format == PixelFormat::H264AnnexB {
            if self.bytes_per_row.is_some() {
                return Err(ProviderError::new(
                    ProviderErrorCode::CaptureFailed,
                    "encoded H.264 frames must not declare a row stride",
                ));
            }
            if self.bytes.is_empty()
                || !(self.bytes.starts_with(&[0, 0, 0, 1]) || self.bytes.starts_with(&[0, 0, 1]))
            {
                return Err(ProviderError::new(
                    ProviderErrorCode::CaptureFailed,
                    "H.264 frames must contain Annex B NAL units",
                ));
            }
            if self.keyframe {
                let nal_types = annex_b_nal_types(&self.bytes);
                if !nal_types.contains(&7) || !nal_types.contains(&8) || !nal_types.contains(&5) {
                    return Err(ProviderError::new(
                        ProviderErrorCode::CaptureFailed,
                        "H.264 keyframes must contain SPS, PPS, and IDR NAL units",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn annex_b_nal_types(bytes: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut offset = 0usize;
    while offset + 3 < bytes.len() {
        let header = if bytes[offset..].starts_with(&[0, 0, 0, 1]) {
            Some(offset + 4)
        } else if bytes[offset..].starts_with(&[0, 0, 1]) {
            Some(offset + 3)
        } else {
            None
        };
        if let Some(header) = header {
            if let Some(byte) = bytes.get(header) {
                types.push(byte & 0x1f);
            }
        }
        offset += 1;
    }
    types
}

#[derive(Debug, Clone)]
pub enum CaptureEvent {
    Frame(OwnedFrame),
    GeometryChanged(SurfaceGeometry),
    TitleChanged(String),
    Suspended(String),
    Resumed,
    Closed,
}

pub trait CaptureSink: Send + Sync + 'static {
    fn on_event(&self, event: CaptureEvent);
}

pub trait CaptureLease: Send + Sync + 'static {
    fn request_keyframe(&self) {}
    fn stop(&self);

    /// Rate control hook: change the encoder's target bitrate without a
    /// restart. Providers without a bitrate-controlled encoder ignore it.
    fn set_target_bitrate_kbps(&self, _kbps: u32) {}

    /// Rate control hook: change the capture frame-rate cap without a
    /// restart. Providers that cannot change it live ignore it.
    fn set_max_fps(&self, _fps: u16) {}

    /// Pause capture and encoding while no socket is attached (a closed
    /// browser tab keeps the session until its ticket expires), and resume
    /// on the next attach. Providers that cannot pause ignore it.
    fn set_paused(&self, _paused: bool) {}

    /// Name of the encoder producing this lease's frames (for example
    /// `openh264` or `videotoolbox`), when the provider encodes.
    fn encoder_name(&self) -> Option<String> {
        None
    }
}

/// One display (monitor) of the guest desktop. Display targets stream the
/// whole framebuffer; they are captured through the same `CaptureProvider`
/// with the `ProviderTargetId` returned by [`DisplayProvider::display_target`].
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderDisplay {
    /// Opaque, stable display id. "primary" is accepted as an alias.
    pub id: String,
    pub name: String,
    pub primary: bool,
    /// Global logical-point bounds: x, y, width, height.
    pub bounds: (f64, f64, f64, f64),
    pub native_width_px: u32,
    pub native_height_px: u32,
    pub scale_factor: f64,
    pub refresh_rate_hz: u32,
}

pub trait DisplayProvider: Send + Sync + 'static {
    fn displays(&self) -> Result<Vec<ProviderDisplay>, ProviderError>;

    /// Resolve a display id (or "primary") to a capture target.
    fn display_target(&self, display_id: &str) -> Result<ProviderTarget, ProviderError>;
}

pub trait CaptureProvider: Send + Sync + 'static {
    fn formats(&self, target: &ProviderTargetId) -> Result<Vec<PixelFormat>, ProviderError>;

    fn start(
        &self,
        target: &ProviderTargetId,
        config: &CaptureConfig,
        sink: Arc<dyn CaptureSink>,
    ) -> Result<Arc<dyn CaptureLease>, ProviderError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedWindowGeometry {
    pub width_points: u32,
    pub height_points: u32,
}

/// Background-safe host-window geometry control. Implementations must not
/// activate the target or disturb the host user's foreground application.
pub trait WindowGeometryProvider: Send + Sync + 'static {
    fn supports(&self, target: &ProviderTargetId) -> Result<bool, ProviderError>;

    fn resize<'a>(
        &'a self,
        target: &'a ProviderTargetId,
        width_points: u32,
        height_points: u32,
    ) -> ProviderFuture<'a, Result<AppliedWindowGeometry, ProviderError>>;
}

#[derive(Debug, Clone)]
pub struct ActionInvocation {
    pub action_id: String,
    pub action: String,
    pub arguments: Value,
    pub basis: ActionBasis,
    /// Video-frame coordinate space used by any pixel arguments in this
    /// invocation. Providers translate this canonical RCDP space to their
    /// native input space; clients never send platform-native coordinates.
    pub coordinate_space: Option<SurfaceGeometry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionOutcome {
    pub delivered: bool,
    pub detail: Option<Value>,
}

pub trait ActionProvider: Send + Sync + 'static {
    fn capabilities(
        &self,
        target: &ProviderTargetId,
    ) -> Result<Vec<ActionCapability>, ProviderError>;

    fn perform<'a>(
        &'a self,
        target: &'a ProviderTargetId,
        action: ActionInvocation,
        policy: SessionPolicy,
    ) -> ProviderFuture<'a, Result<ActionOutcome, ProviderError>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractiveInputOutcome {
    pub through_sequence: u64,
    pub event_count: usize,
    pub dispatch_micros: u64,
}

/// One stateful native input session. Implementations preserve event ordering
/// and keep any native source/button/gesture state alive between batches.
pub trait InteractiveInputLease: Send + Sync + 'static {
    fn dispatch(
        &self,
        batch: &InteractiveInputBatch,
    ) -> Result<InteractiveInputOutcome, ProviderError>;

    /// Release every key and button this lease still holds down. Called when
    /// input ownership of the target moves to another principal, so a
    /// change of owner never leaves stuck modifiers.
    fn release_all(&self) {}
}

/// Who drives an interactive input session: the media session's principal
/// (its presence identity when the viewer named one).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputOwner {
    /// `Principal.id`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// An agent rather than a human viewer.
    pub agent: bool,
}

/// Optional high-frequency input path, separate from one-shot automation.
pub trait InteractiveInputProvider: Send + Sync + 'static {
    fn open(
        &self,
        target: &ProviderTargetId,
        policy: SessionPolicy,
    ) -> Result<Option<Arc<dyn InteractiveInputLease>>, ProviderError>;

    /// [`Self::open`] on behalf of `owner`. A provider whose input can draw
    /// an agent cursor or reach presence (cua-driver's tools) overrides this
    /// so a human's input is never shown as an agent's; the default ignores
    /// the owner.
    fn open_as(
        &self,
        target: &ProviderTargetId,
        policy: SessionPolicy,
        owner: &InputOwner,
    ) -> Result<Option<Arc<dyn InteractiveInputLease>>, ProviderError> {
        let _ = owner;
        self.open(target, policy)
    }
}

/// Compatibility provider used by runtimes assembled without an interactive
/// input adapter.
#[derive(Debug, Default)]
pub struct UnsupportedInteractiveInputProvider;

impl InteractiveInputProvider for UnsupportedInteractiveInputProvider {
    fn open(
        &self,
        _target: &ProviderTargetId,
        _policy: SessionPolicy,
    ) -> Result<Option<Arc<dyn InteractiveInputLease>>, ProviderError> {
        Ok(None)
    }
}

pub fn enforce_action_policy(
    policy: SessionPolicy,
    capability: &ActionCapability,
) -> Result<(), ProviderError> {
    if capability.guarantee == ActionDeliveryGuarantee::Unsupported {
        return Err(ProviderError::new(
            ProviderErrorCode::Unsupported,
            format!("action {} is unsupported", capability.action),
        ));
    }
    match policy {
        SessionPolicy::ViewOnly => Err(ProviderError::new(
            ProviderErrorCode::ViewOnly,
            "session is view-only",
        )),
        SessionPolicy::BackgroundOnly
            if capability.guarantee != ActionDeliveryGuarantee::Background =>
        {
            Err(ProviderError::new(
                ProviderErrorCode::WouldRequireActivation,
                format!(
                    "action {} cannot satisfy the background-only policy",
                    capability.action
                ),
            ))
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Clone)]
pub struct AccessibilitySnapshot {
    pub snapshot_id: AccessibilitySnapshotId,
    pub state: Value,
}

pub trait AccessibilityProvider: Send + Sync + 'static {
    fn snapshot<'a>(
        &'a self,
        target: &'a ProviderTargetId,
    ) -> ProviderFuture<'a, Result<AccessibilitySnapshot, ProviderError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(guarantee: ActionDeliveryGuarantee) -> ActionCapability {
        ActionCapability {
            action: "type_text".into(),
            guarantee,
        }
    }

    #[test]
    fn backend_target_debug_never_reveals_native_key() {
        let key = BackendTargetKey::new("pid=42;window=99");
        let debug = format!("{key:?}");
        assert!(!debug.contains("42"));
        assert!(!debug.contains("99"));
    }

    #[test]
    fn background_policy_rejects_activation_without_fallback() {
        let error = enforce_action_policy(
            SessionPolicy::BackgroundOnly,
            &capability(ActionDeliveryGuarantee::MayActivate),
        )
        .unwrap_err();
        assert_eq!(error.code, ProviderErrorCode::WouldRequireActivation);
    }

    #[test]
    fn allow_activation_accepts_foreground_capability() {
        enforce_action_policy(
            SessionPolicy::AllowActivation,
            &capability(ActionDeliveryGuarantee::MayActivate),
        )
        .unwrap();
    }

    #[test]
    fn view_only_rejects_background_capability() {
        let error = enforce_action_policy(
            SessionPolicy::ViewOnly,
            &capability(ActionDeliveryGuarantee::Background),
        )
        .unwrap_err();
        assert_eq!(error.code, ProviderErrorCode::ViewOnly);
    }

    #[test]
    fn unsupported_is_not_misreported_as_activation_required() {
        let error = enforce_action_policy(
            SessionPolicy::BackgroundOnly,
            &capability(ActionDeliveryGuarantee::Unsupported),
        )
        .unwrap_err();
        assert_eq!(error.code, ProviderErrorCode::Unsupported);
    }

    #[test]
    fn bgra_validation_requires_tight_owned_frame() {
        let frame = OwnedFrame {
            bytes: Arc::from(vec![0; 31]),
            format: PixelFormat::Bgra8,
            width_px: 4,
            height_px: 2,
            bytes_per_row: Some(16),
            capture_timestamp_us: 1,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: true,
        };
        assert_eq!(
            frame.validate().unwrap_err().code,
            ProviderErrorCode::CaptureFailed
        );
    }

    #[test]
    fn h264_validation_requires_annex_b_and_codec_epoch() {
        let mut frame = OwnedFrame {
            bytes: Arc::from(
                [
                    0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65,
                ]
                .as_slice(),
            ),
            format: PixelFormat::H264AnnexB,
            width_px: 4,
            height_px: 2,
            bytes_per_row: None,
            capture_timestamp_us: 1,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: true,
        };
        frame.validate().unwrap();
        frame.codec_epoch = 0;
        assert_eq!(
            frame.validate().unwrap_err().code,
            ProviderErrorCode::CaptureFailed
        );
        frame.bytes = Arc::from([0, 0, 0, 1, 0x65].as_slice());
        assert_eq!(
            frame.validate().unwrap_err().code,
            ProviderErrorCode::CaptureFailed
        );
        frame.codec_epoch = 1;
        frame.bytes = Arc::from([0x65].as_slice());
        assert_eq!(
            frame.validate().unwrap_err().code,
            ProviderErrorCode::CaptureFailed
        );
    }
}
