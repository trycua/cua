// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Input delegation to cua-driver.
//!
//! cua-spacesd performs no input injection of its own. This module only
//! translates between the RCDP/cua.env contracts and cua-driver's
//! cross-platform input contract (`cua_driver_core::interactive_input`),
//! whose native sessions live in the cua-driver platform crates:
//!
//! - interactive media-plane batches: `platform_macos::input::interactive` and
//!   `platform_linux::input::interactive`;
//! - targeted one-shot pointer/keyboard delivery (Linux gRPC Computer service
//!   and media-plane actions): `platform_linux::input::targeted`;
//! - everything else goes through the cua-driver tool registry.
//!
//! What stays here is targeting (catalog handles → native ids), leases,
//! session policy, and error mapping.

// Windows has no cua-driver interactive session yet; only the policy and
// error mapping are used there.
#![cfg_attr(target_os = "windows", allow(dead_code))]

use cua_driver_core::interactive_input as drv;
use cua_media_protocol::{
    InputGesturePhase, InputKeyState, InputModifier, InputPointerButton, InputPointerPhase,
    InteractiveInputBatch, InteractiveInputEvent, SessionPolicy,
};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use cua_spacesd_provider_api::InteractiveInputLease;
use cua_spacesd_provider_api::{InteractiveInputOutcome, ProviderError, ProviderErrorCode};

/// Whether AUTO may fall back to foreground (moves the real pointer and may
/// activate the window) when background delivery cannot reach the target. On
/// by default: the spacesd runs in a sandbox session with no competing
/// local user. `CUA_ENV_AUTO_FOREGROUND=0` turns it off (shared desktops), and
/// AUTO then fails with `WouldRequireActivation`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn auto_foreground_allowed() -> bool {
    !matches!(
        std::env::var("CUA_ENV_AUTO_FOREGROUND").as_deref(),
        Ok("0") | Ok("false") | Ok("no") | Ok("never")
    )
}

/// The interactive delivery mode a session policy grants (`None`: view only).
pub(crate) fn interactive_mode(policy: SessionPolicy) -> Option<drv::InteractiveDeliveryMode> {
    match policy {
        SessionPolicy::ViewOnly => None,
        SessionPolicy::BackgroundOnly => Some(drv::InteractiveDeliveryMode::Background),
        SessionPolicy::AllowActivation => Some(drv::InteractiveDeliveryMode::PersistentForeground),
    }
}

/// The targeted delivery a session policy grants for one-shot actions.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn targeted_delivery(policy: SessionPolicy) -> drv::TargetedDelivery {
    match policy {
        SessionPolicy::AllowActivation => drv::TargetedDelivery::Foreground,
        _ => drv::TargetedDelivery::Background,
    }
}

pub(crate) fn interactive_error(error: drv::InteractiveInputError) -> ProviderError {
    use drv::InteractiveInputError as E;
    let code = match &error {
        E::InvalidTarget(_) | E::Closed => ProviderErrorCode::TargetUnavailable,
        E::WouldRequireActivation(_) => ProviderErrorCode::WouldRequireActivation,
        E::InvalidBatch(_) | E::Backpressure | E::Native(_) => ProviderErrorCode::DeliveryFailed,
    };
    ProviderError::new(code, error.to_string())
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn targeted_error(error: drv::TargetedInputError) -> ProviderError {
    use drv::TargetedInputError as E;
    let code = match &error {
        E::WouldRequireActivation(_) => ProviderErrorCode::WouldRequireActivation,
        E::TargetUnavailable(_) => ProviderErrorCode::TargetUnavailable,
        E::Unsupported(_) => ProviderErrorCode::Unsupported,
        E::DeliveryFailed(_) => ProviderErrorCode::DeliveryFailed,
    };
    ProviderError::new(code, error.to_string())
}

fn unsupported_input(message: &str) -> ProviderError {
    ProviderError::new(ProviderErrorCode::Unsupported, message)
}

/// Protocol interactive events → cua-driver interactive events.
pub(crate) fn driver_batch(
    batch: &InteractiveInputBatch,
) -> Result<drv::InteractiveInputBatch, ProviderError> {
    Ok(drv::InteractiveInputBatch {
        first_sequence: batch.first_sequence,
        events: batch
            .events
            .iter()
            .map(driver_event)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn driver_event(
    event: &InteractiveInputEvent,
) -> Result<drv::InteractiveInputEvent, ProviderError> {
    let map_modifiers = |modifiers: &[InputModifier]| {
        modifiers
            .iter()
            .map(|modifier| match modifier {
                InputModifier::Command => Ok(drv::Modifier::Command),
                InputModifier::Shift => Ok(drv::Modifier::Shift),
                InputModifier::Option => Ok(drv::Modifier::Option),
                InputModifier::Control => Ok(drv::Modifier::Control),
                InputModifier::Function => Ok(drv::Modifier::Function),
                InputModifier::Unknown => Err(unsupported_input("unknown modifier")),
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let map_phase = |phase: InputGesturePhase| match phase {
        InputGesturePhase::None => Ok(drv::GesturePhase::None),
        InputGesturePhase::MayBegin => Ok(drv::GesturePhase::MayBegin),
        InputGesturePhase::Began => Ok(drv::GesturePhase::Began),
        InputGesturePhase::Changed => Ok(drv::GesturePhase::Changed),
        InputGesturePhase::Ended => Ok(drv::GesturePhase::Ended),
        InputGesturePhase::Cancelled => Ok(drv::GesturePhase::Cancelled),
        InputGesturePhase::Unknown => Err(unsupported_input("unknown scroll phase")),
    };

    Ok(match event {
        InteractiveInputEvent::TextCommit { text } => {
            drv::InteractiveInputEvent::TextCommit { text: text.clone() }
        }
        InteractiveInputEvent::Key {
            key,
            state,
            modifiers,
            repeat,
        } => drv::InteractiveInputEvent::Key {
            key: key.clone(),
            state: match state {
                InputKeyState::Down => drv::KeyState::Down,
                InputKeyState::Up => drv::KeyState::Up,
                InputKeyState::Unknown => return Err(unsupported_input("unknown key state")),
            },
            modifiers: map_modifiers(modifiers)?,
            repeat: *repeat,
        },
        InteractiveInputEvent::Pointer {
            phase,
            button,
            x_normalized,
            y_normalized,
            modifiers,
        } => drv::InteractiveInputEvent::Pointer {
            phase: match phase {
                InputPointerPhase::Move => drv::PointerPhase::Move,
                InputPointerPhase::Down => drv::PointerPhase::Down,
                InputPointerPhase::Up => drv::PointerPhase::Up,
                InputPointerPhase::Cancel => drv::PointerPhase::Cancel,
                InputPointerPhase::Unknown => {
                    return Err(unsupported_input("unknown pointer phase"));
                }
            },
            button: match button {
                None => None,
                Some(InputPointerButton::Left) => Some(drv::PointerButton::Left),
                Some(InputPointerButton::Right) => Some(drv::PointerButton::Right),
                Some(InputPointerButton::Middle) => Some(drv::PointerButton::Middle),
                Some(InputPointerButton::Unknown) => {
                    return Err(unsupported_input("unknown pointer button"));
                }
            },
            x_normalized: *x_normalized,
            y_normalized: *y_normalized,
            modifiers: map_modifiers(modifiers)?,
        },
        InteractiveInputEvent::Scroll {
            x_normalized,
            y_normalized,
            delta_x,
            delta_y,
            phase,
            momentum_phase,
            precise,
        } => drv::InteractiveInputEvent::Scroll {
            x_normalized: *x_normalized,
            y_normalized: *y_normalized,
            delta_x: *delta_x,
            delta_y: *delta_y,
            phase: map_phase(*phase)?,
            momentum_phase: map_phase(*momentum_phase)?,
            precise: *precise,
        },
    })
}

fn outcome(receipt: drv::InteractiveInputReceipt) -> InteractiveInputOutcome {
    InteractiveInputOutcome {
        through_sequence: receipt.through_sequence,
        event_count: receipt.event_count,
        dispatch_micros: receipt.dispatch_micros,
    }
}

/// An interactive lease over one cua-driver macOS input session.
#[cfg(target_os = "macos")]
pub(crate) struct MacosLease(pub(crate) platform_macos::input::InteractiveInputSession);

#[cfg(target_os = "macos")]
impl InteractiveInputLease for MacosLease {
    fn dispatch(
        &self,
        batch: &InteractiveInputBatch,
    ) -> Result<InteractiveInputOutcome, ProviderError> {
        tracing::debug!(
            target: "cua_spacesd_client::host_input",
            first_sequence = batch.first_sequence,
            event_count = batch.events.len(),
            "dispatching interactive input through cua-driver (macOS)"
        );
        self.0
            .dispatch(driver_batch(batch)?)
            .map(outcome)
            .map_err(interactive_error)
    }
}

/// An interactive lease over one cua-driver Linux (X11) input session.
#[cfg(target_os = "linux")]
pub(crate) struct LinuxLease(
    pub(crate) platform_linux::input::interactive::InteractiveInputSession,
);

#[cfg(target_os = "linux")]
impl InteractiveInputLease for LinuxLease {
    fn dispatch(
        &self,
        batch: &InteractiveInputBatch,
    ) -> Result<InteractiveInputOutcome, ProviderError> {
        self.0
            .dispatch(&driver_batch(batch)?)
            .map(outcome)
            .map_err(interactive_error)
    }

    fn release_all(&self) {
        self.0.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_map_to_driver_delivery() {
        assert_eq!(interactive_mode(SessionPolicy::ViewOnly), None);
        assert_eq!(
            interactive_mode(SessionPolicy::BackgroundOnly),
            Some(drv::InteractiveDeliveryMode::Background)
        );
        assert_eq!(
            interactive_mode(SessionPolicy::AllowActivation),
            Some(drv::InteractiveDeliveryMode::PersistentForeground)
        );
        assert_eq!(
            targeted_delivery(SessionPolicy::BackgroundOnly),
            drv::TargetedDelivery::Background
        );
        assert_eq!(
            targeted_delivery(SessionPolicy::AllowActivation),
            drv::TargetedDelivery::Foreground
        );
    }

    #[test]
    fn protocol_events_map_to_driver_events() {
        let batch = InteractiveInputBatch {
            session_id: cua_media_protocol::WindowSessionId(String::new()),
            first_sequence: 3,
            events: vec![
                InteractiveInputEvent::Key {
                    key: "a".into(),
                    state: InputKeyState::Down,
                    modifiers: vec![InputModifier::Shift],
                    repeat: false,
                },
                InteractiveInputEvent::Pointer {
                    phase: InputPointerPhase::Down,
                    button: Some(InputPointerButton::Right),
                    x_normalized: 0.25,
                    y_normalized: 0.5,
                    modifiers: Vec::new(),
                },
            ],
        };
        let mapped = driver_batch(&batch).unwrap();
        assert_eq!(mapped.first_sequence, 3);
        assert_eq!(
            mapped.events[0],
            drv::InteractiveInputEvent::Key {
                key: "a".into(),
                state: drv::KeyState::Down,
                modifiers: vec![drv::Modifier::Shift],
                repeat: false,
            }
        );
        let unknown = InteractiveInputBatch {
            session_id: cua_media_protocol::WindowSessionId(String::new()),
            first_sequence: 0,
            events: vec![InteractiveInputEvent::Key {
                key: "a".into(),
                state: InputKeyState::Unknown,
                modifiers: Vec::new(),
                repeat: false,
            }],
        };
        assert_eq!(
            driver_batch(&unknown).unwrap_err().code,
            ProviderErrorCode::Unsupported
        );
    }

    #[test]
    fn driver_errors_keep_their_meaning() {
        assert_eq!(
            interactive_error(drv::InteractiveInputError::WouldRequireActivation(
                "x".into()
            ))
            .code,
            ProviderErrorCode::WouldRequireActivation
        );
        assert_eq!(
            targeted_error(drv::TargetedInputError::WouldRequireActivation("x".into())).code,
            ProviderErrorCode::WouldRequireActivation
        );
        assert_eq!(
            targeted_error(drv::TargetedInputError::Unsupported("x".into())).code,
            ProviderErrorCode::Unsupported
        );
    }
}
