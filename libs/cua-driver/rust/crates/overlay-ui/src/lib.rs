//! Shared native overlay UI for Cua Driver.
//!
//! This crate deliberately contains no windowing toolkit and no web runtime.
//! It turns small, bounded UI models into premultiplied RGBA pixmaps and owns
//! the placement and interaction state machines. Platform crates supply only
//! native windows, pointer/key events, and capture/privacy controls.

mod interaction;
mod placement;
mod render;
mod sanitize;

pub use interaction::{ConsentInteraction, InteractionOutcome, PointerButton, CONTROL_ARM_DELAY};
pub use placement::{place_near_pointer, Point, Rect, Size};
pub use render::{
    render_consent, render_indicator, ConsentVisualState, RenderError, ACCEPT_RECT, CONSENT_SIZE,
    DECLINE_RECT, INDICATOR_SIZE, STOP_RECT,
};
pub use sanitize::{sanitize_label, sanitize_summary};

use serde::{Deserialize, Serialize};

/// Input sent over the inherited private pipe to the protected helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "surface", rename_all = "snake_case")]
pub enum HelperRequest {
    Consent(ConsentCard),
    Indicator(IndicatorCard),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentCard {
    pub operation: String,
    pub risk_label: String,
    pub summary: String,
    pub request_digest: String,
    pub expires_unix_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndicatorCard {
    pub indicator_id: String,
    pub summary: String,
}

/// Output emitted by the helper. The parent accepts events only from the
/// child connected to its inherited pipe, never from public stdio or a tool
/// argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HelperEvent {
    Ready,
    Decision {
        action: HelperDecision,
        request_digest: String,
    },
    Stop {
        indicator_id: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperDecision {
    Accept,
    Decline,
    Cancel,
}
