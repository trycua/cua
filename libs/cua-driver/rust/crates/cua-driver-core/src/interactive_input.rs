//! Cross-platform input-delivery contract for in-process embedders.
//!
//! Automation tools (`click`, `type_text`, ...) trade latency for
//! verification and human-readable results. An embedder that remotes a
//! desktop (for example the cua-spacesd media plane) needs two lower-level
//! contracts instead, and this module owns their platform-neutral semantics so
//! every platform adapter implements the same thing:
//!
//! - **Interactive batches**: ordered pointer/key/scroll/text events from a
//!   streaming client, addressed in normalized target coordinates, delivered
//!   with as little work as possible between receipt and native dispatch.
//!   [`InteractiveInputBatch`] / [`InteractiveInputReceipt`] /
//!   [`InteractiveInputError`], validated by [`validate_batch`].
//! - **Targeted one-shot delivery**: one pointer or keyboard operation with an
//!   explicit per-call [`TargetedDelivery`] (`Auto`, `Background`,
//!   `Foreground`). The report says which delivery was used and whether focus
//!   or the real pointer changed. A delivery that cannot reach the target
//!   without activation is refused with
//!   [`TargetedInputError::WouldRequireActivation`] rather than reported as
//!   delivered; [`resolve_delivery`] is the shared decision.
//!
//! Nothing here performs I/O, so the whole decision matrix is testable in
//! ordinary CI. Platform crates provide the native sessions
//! (`platform_macos::input::interactive`, `platform_linux::input::interactive`
//! and `platform_linux::input::targeted`). A platform that can only deliver
//! whole gestures (Hyprland, through the driver tools) folds batches with
//! [`gestures::GestureFolder`].

use thiserror::Error;

pub mod gestures;

/// Upper bound on events in one interactive batch.
pub const MAX_BATCH_EVENTS: usize = 256;

/// Controls whether interactive events stay target-routed or use the
/// foreground (global) input queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveDeliveryMode {
    /// Deliver to the target without activating it. A platform that cannot do
    /// that for the target refuses when the session opens.
    Background,
    /// Keep the target frontmost for the lifetime of the input session and
    /// deliver through the same global queue as physical input.
    PersistentForeground,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Command,
    Shift,
    Option,
    Control,
    Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerPhase {
    Move,
    Down,
    Up,
    Cancel,
}

/// Scroll gesture phase. Keeping phase and momentum distinct is important for
/// inertial scrolling and overscroll behavior where the platform has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GesturePhase {
    None,
    MayBegin,
    Began,
    Changed,
    Ended,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InteractiveInputEvent {
    /// Commit already-composed Unicode text. This is intentionally distinct
    /// from physical key events so IME/dead-key composition is not replayed.
    TextCommit { text: String },
    Key {
        key: String,
        state: KeyState,
        modifiers: Vec<Modifier>,
        repeat: bool,
    },
    Pointer {
        phase: PointerPhase,
        button: Option<PointerButton>,
        x_normalized: f64,
        y_normalized: f64,
        modifiers: Vec<Modifier>,
    },
    Scroll {
        x_normalized: f64,
        y_normalized: f64,
        delta_x: f64,
        delta_y: f64,
        phase: GesturePhase,
        momentum_phase: GesturePhase,
        /// Pixel (trackpad) deltas rather than wheel lines.
        precise: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct InteractiveInputBatch {
    pub first_sequence: u64,
    pub events: Vec<InteractiveInputEvent>,
}

/// Native-dispatch acknowledgement for one batch: every event was accepted by
/// the platform input system (and, where the platform offers one, a
/// round-trip confirmed it was processed). Not application-state verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractiveInputReceipt {
    pub through_sequence: u64,
    pub event_count: usize,
    pub dispatch_micros: u64,
}

#[derive(Debug, Error)]
pub enum InteractiveInputError {
    #[error("interactive input target is invalid: {0}")]
    InvalidTarget(String),
    #[error("interactive input batch is invalid: {0}")]
    InvalidBatch(String),
    #[error("interactive input queue is full")]
    Backpressure,
    #[error("interactive input session is closed")]
    Closed,
    #[error("native input delivery failed: {0}")]
    Native(String),
    /// Background delivery cannot reach this target without activating it
    /// (for example a toolkit that drops synthetic events). Reopen the
    /// session with [`InteractiveDeliveryMode::PersistentForeground`].
    #[error("interactive input would require activation: {0}")]
    WouldRequireActivation(String),
}

/// Validate a batch and return its last sequence number.
pub fn validate_batch(batch: &InteractiveInputBatch) -> Result<u64, InteractiveInputError> {
    if batch.events.is_empty() {
        return Err(InteractiveInputError::InvalidBatch(
            "events must not be empty".to_owned(),
        ));
    }
    if batch.events.len() > MAX_BATCH_EVENTS {
        return Err(InteractiveInputError::InvalidBatch(format!(
            "at most {MAX_BATCH_EVENTS} events are allowed"
        )));
    }
    let through = batch
        .first_sequence
        .checked_add(batch.events.len() as u64 - 1)
        .ok_or_else(|| InteractiveInputError::InvalidBatch("sequence overflow".to_owned()))?;

    for event in &batch.events {
        match event {
            InteractiveInputEvent::TextCommit { text } if text.is_empty() => {
                return Err(InteractiveInputError::InvalidBatch(
                    "text commits must not be empty".to_owned(),
                ));
            }
            InteractiveInputEvent::Pointer {
                x_normalized,
                y_normalized,
                ..
            }
            | InteractiveInputEvent::Scroll {
                x_normalized,
                y_normalized,
                ..
            } if !normalized(*x_normalized) || !normalized(*y_normalized) => {
                return Err(InteractiveInputError::InvalidBatch(
                    "pointer coordinates must be finite and within [0, 1]".to_owned(),
                ));
            }
            InteractiveInputEvent::Scroll {
                delta_x, delta_y, ..
            } if !delta_x.is_finite() || !delta_y.is_finite() => {
                return Err(InteractiveInputError::InvalidBatch(
                    "scroll deltas must be finite".to_owned(),
                ));
            }
            _ => {}
        }
    }
    Ok(through)
}

/// Finite and within `[0, 1]`.
pub fn normalized(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

/// Take the integral part out of a fractional accumulator, keeping the rest,
/// so sub-unit scroll deltas are preserved until they form whole units.
pub fn extract_integral(residual: &mut f64) -> i32 {
    let value = residual.trunc().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    *residual -= f64::from(value);
    value
}

/// Map a normalized coordinate onto `[0, extent - 1]` pixels.
pub fn denormalize(value: f64, extent: u32) -> i32 {
    (value * f64::from(extent.saturating_sub(1))).round() as i32
}

// ---------------------------------------------------------------------------
// Targeted one-shot delivery
// ---------------------------------------------------------------------------

/// Requested delivery for one targeted operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetedDelivery {
    /// Background whenever the target can be addressed and accepts
    /// target-routed input; foreground otherwise (when the caller allows it).
    Auto,
    /// Never activate or raise anything; refuse when that cannot reach the
    /// target.
    Background,
    /// Activate the target (when there is one) and inject through the global
    /// input queue. The real pointer moves and focus may change.
    Foreground,
}

/// Delivery actually used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryUsed {
    Background,
    Foreground,
}

/// What a targeted operation did.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryReport {
    pub delivery: DeliveryUsed,
    pub focus_changed: bool,
    pub pointer_moved: bool,
    /// Native path, for diagnostics.
    pub detail: String,
    /// Why AUTO chose what it chose, when that is not the obvious default.
    pub note: Option<String>,
}

/// Pointer buttons addressable by targeted operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetedButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl TargetedButton {
    /// `left` (default for anything unknown), `right`, `middle`, `back`,
    /// `forward`; case-insensitive.
    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "right" => Self::Right,
            "middle" => Self::Middle,
            "back" => Self::Back,
            "forward" => Self::Forward,
            _ => Self::Left,
        }
    }
}

/// One pointer operation at a screen point (the platform's global pixel
/// space for the target display).
#[derive(Debug, Clone, PartialEq)]
pub enum PointerOp {
    Click {
        button: TargetedButton,
        count: u32,
        modifiers: Vec<String>,
    },
    Move,
    Down {
        button: TargetedButton,
    },
    Up {
        button: TargetedButton,
    },
    /// Screen-point path after the start point; one continuous held drag.
    Drag {
        path: Vec<(i32, i32)>,
        button: TargetedButton,
        modifiers: Vec<String>,
    },
    /// Wheel clicks: positive y scrolls down, positive x scrolls right.
    Scroll {
        dx: i32,
        dy: i32,
    },
}

/// One keyboard operation.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyOp {
    /// Composed text (IME-safe: characters are committed, never replayed as
    /// composition key sequences).
    Type(String),
    Press {
        key: String,
        modifiers: Vec<String>,
        repeat: u32,
    },
    /// Modifiers then the final key.
    Hotkey(Vec<String>),
    Down(String),
    Up(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TargetedInputError {
    /// The request cannot be delivered without activating the target (or,
    /// for foreground delivery, the point is covered by another window).
    #[error("{0}")]
    WouldRequireActivation(String),
    #[error("{0}")]
    TargetUnavailable(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    DeliveryFailed(String),
}

impl TargetedInputError {
    /// Stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::WouldRequireActivation(_) => "would_require_activation",
            Self::TargetUnavailable(_) => "target_unavailable",
            Self::Unsupported(_) => "unsupported",
            Self::DeliveryFailed(_) => "delivery_failed",
        }
    }
}

/// The shared delivery decision.
///
/// - `has_target`: a window (or the window under the point / the focused
///   window) can be addressed.
/// - `ignores_synthetic`: the target's toolkit when it is known to drop
///   target-routed (synthetic) input.
/// - `allow_auto_foreground`: whether AUTO may escalate to foreground.
/// - `what`: e.g. "pointer events", for messages.
///
/// Background input to a toolkit that drops it never reports success: AUTO
/// escalates to foreground when allowed and otherwise refuses; an explicit
/// BACKGROUND request always refuses.
pub fn resolve_delivery(
    requested: TargetedDelivery,
    has_target: bool,
    ignores_synthetic: Option<&str>,
    allow_auto_foreground: bool,
    what: &str,
) -> Result<(DeliveryUsed, Option<String>), TargetedInputError> {
    let used = match requested {
        TargetedDelivery::Background => DeliveryUsed::Background,
        TargetedDelivery::Foreground => return Ok((DeliveryUsed::Foreground, None)),
        TargetedDelivery::Auto if has_target => DeliveryUsed::Background,
        TargetedDelivery::Auto if allow_auto_foreground => {
            return Ok((DeliveryUsed::Foreground, None))
        }
        TargetedDelivery::Auto => {
            return Err(TargetedInputError::WouldRequireActivation(format!(
                "no window can be addressed for background {what}, and AUTO may not use \
                 foreground delivery here; request FOREGROUND"
            )))
        }
    };
    let Some(toolkit) = ignores_synthetic else {
        return Ok((used, None));
    };
    match requested {
        TargetedDelivery::Auto if allow_auto_foreground => Ok((
            DeliveryUsed::Foreground,
            Some(format!(
                "AUTO chose foreground: {toolkit} ignores synthetic {what}"
            )),
        )),
        TargetedDelivery::Auto => Err(TargetedInputError::WouldRequireActivation(format!(
            "{toolkit} ignores synthetic {what}, and AUTO may not use foreground delivery \
             here; request FOREGROUND or use an accessibility action"
        ))),
        _ => Err(TargetedInputError::WouldRequireActivation(format!(
            "background {what} cannot reach this {toolkit} window: it ignores synthetic \
             events; request FOREGROUND or AUTO, or use an accessibility action"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointer(x: f64, y: f64) -> InteractiveInputEvent {
        InteractiveInputEvent::Pointer {
            phase: PointerPhase::Move,
            button: None,
            x_normalized: x,
            y_normalized: y,
            modifiers: Vec::new(),
        }
    }

    #[test]
    fn batches_are_validated_and_report_their_last_sequence() {
        let batch = InteractiveInputBatch {
            first_sequence: 7,
            events: vec![pointer(0.0, 1.0), pointer(0.5, 0.5)],
        };
        assert_eq!(validate_batch(&batch).unwrap(), 8);
        let empty = InteractiveInputBatch {
            first_sequence: 1,
            events: Vec::new(),
        };
        assert!(matches!(
            validate_batch(&empty),
            Err(InteractiveInputError::InvalidBatch(_))
        ));
        let out_of_range = InteractiveInputBatch {
            first_sequence: 1,
            events: vec![pointer(1.01, 0.0)],
        };
        assert!(validate_batch(&out_of_range).is_err());
        let overflow = InteractiveInputBatch {
            first_sequence: u64::MAX,
            events: vec![pointer(0.0, 0.0), pointer(0.0, 0.0)],
        };
        assert!(validate_batch(&overflow).is_err());
        let too_many = InteractiveInputBatch {
            first_sequence: 0,
            events: vec![pointer(0.0, 0.0); MAX_BATCH_EVENTS + 1],
        };
        assert!(validate_batch(&too_many).is_err());
        let empty_text = InteractiveInputBatch {
            first_sequence: 0,
            events: vec![InteractiveInputEvent::TextCommit {
                text: String::new(),
            }],
        };
        assert!(validate_batch(&empty_text).is_err());
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_coordinates() {
        for value in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
            assert!(!normalized(value));
        }
        assert!(normalized(0.0));
        assert!(normalized(1.0));
    }

    #[test]
    fn preserves_fractional_scroll_until_it_forms_units() {
        let mut residual = 0.0;
        residual += 0.4;
        assert_eq!(extract_integral(&mut residual), 0);
        residual += 0.8;
        assert_eq!(extract_integral(&mut residual), 1);
        assert!((residual - 0.2).abs() < f64::EPSILON * 4.0);
        residual = -1.5;
        assert_eq!(extract_integral(&mut residual), -1);
    }

    #[test]
    fn denormalize_maps_onto_the_last_pixel() {
        assert_eq!(denormalize(0.0, 100), 0);
        assert_eq!(denormalize(1.0, 100), 99);
        assert_eq!(denormalize(0.5, 0), 0);
    }

    #[test]
    fn button_names_default_to_left() {
        assert_eq!(TargetedButton::from_name("RIGHT"), TargetedButton::Right);
        assert_eq!(TargetedButton::from_name("middle"), TargetedButton::Middle);
        assert_eq!(TargetedButton::from_name("back"), TargetedButton::Back);
        assert_eq!(
            TargetedButton::from_name("forward"),
            TargetedButton::Forward
        );
        assert_eq!(TargetedButton::from_name("other"), TargetedButton::Left);
    }

    #[test]
    fn explicit_modes_are_honoured_for_ordinary_targets() {
        let r = |d, t| resolve_delivery(d, t, None, true, "pointer events").unwrap();
        assert_eq!(
            r(TargetedDelivery::Background, true).0,
            DeliveryUsed::Background
        );
        assert_eq!(
            r(TargetedDelivery::Foreground, true).0,
            DeliveryUsed::Foreground
        );
        assert_eq!(
            r(TargetedDelivery::Foreground, false).0,
            DeliveryUsed::Foreground
        );
        assert_eq!(
            r(TargetedDelivery::Auto, true),
            (DeliveryUsed::Background, None)
        );
        assert_eq!(r(TargetedDelivery::Auto, false).0, DeliveryUsed::Foreground);
    }

    #[test]
    fn toolkits_that_drop_synthetic_input_are_never_faked() {
        // AUTO escalates, with a note.
        let (used, note) = resolve_delivery(
            TargetedDelivery::Auto,
            true,
            Some("GTK3"),
            true,
            "key events",
        )
        .unwrap();
        assert_eq!(used, DeliveryUsed::Foreground);
        assert!(note.unwrap().contains("GTK3"));
        // AUTO without permission to escalate refuses.
        let error = resolve_delivery(
            TargetedDelivery::Auto,
            true,
            Some("GTK3"),
            false,
            "key events",
        )
        .unwrap_err();
        assert_eq!(error.code(), "would_require_activation");
        // Explicit BACKGROUND refuses even when escalation would be allowed.
        let error = resolve_delivery(
            TargetedDelivery::Background,
            true,
            Some("Chromium"),
            true,
            "pointer events",
        )
        .unwrap_err();
        assert!(matches!(
            error,
            TargetedInputError::WouldRequireActivation(_)
        ));
        // Explicit FOREGROUND is unaffected by the toolkit.
        assert_eq!(
            resolve_delivery(TargetedDelivery::Foreground, true, Some("GTK4"), false, "x")
                .unwrap()
                .0,
            DeliveryUsed::Foreground
        );
    }

    #[test]
    fn auto_without_a_target_or_escalation_refuses() {
        let error = resolve_delivery(TargetedDelivery::Auto, false, None, false, "pointer events")
            .unwrap_err();
        assert_eq!(error.code(), "would_require_activation");
    }
}
