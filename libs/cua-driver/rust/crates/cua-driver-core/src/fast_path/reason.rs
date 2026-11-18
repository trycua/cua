// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Why a guard refused, in the wire vocabulary of the design note.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The element no longer exists.
    Gone,
    /// Its label or identifier changed (row reuse, swapped content).
    Relabelled,
    RoleChanged,
    /// The frame moved beyond tolerance (pixel delivery only).
    Moved,
    Resized,
    /// Another element is under the click point.
    HitMismatch,
    Offscreen,
    /// The target kept changing for the whole settle budget.
    Animating,
    /// Sparse target: the pixels around the point are stable but different.
    RegionChanged,
    WindowChanged,
    Occluded,
    NotVisible,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gone => "gone",
            Self::Relabelled => "relabelled",
            Self::RoleChanged => "role_changed",
            Self::Moved => "moved",
            Self::Resized => "resized",
            Self::HitMismatch => "hit_mismatch",
            Self::Offscreen => "offscreen",
            Self::Animating => "animating",
            Self::RegionChanged => "region_changed",
            Self::WindowChanged => "window_changed",
            Self::Occluded => "occluded",
            Self::NotVisible => "not_visible",
        }
    }

    /// The wire `code` of the refusal.
    pub fn code(self) -> &'static str {
        match self {
            Self::WindowChanged | Self::Occluded | Self::NotVisible => "window_changed",
            _ => "target_changed",
        }
    }

    /// Plain-words message for the model.
    pub fn message(self) -> &'static str {
        match self {
            Self::Gone => "The element is gone since you read it. Act on a token from the fresh state.",
            Self::Relabelled => "The element now shows different content than when you read it. Act on a token from the fresh state.",
            Self::RoleChanged => "The element changed type since you read it. Act on a token from the fresh state.",
            Self::Moved => "The element moved since you read it. Act on the new token or the new point.",
            Self::Resized => "The element changed size since you read it. Act on a token from the fresh state.",
            Self::HitMismatch => "Another element is now at that point. Act on a token from the fresh state.",
            Self::Offscreen => "The target is off screen now.",
            Self::Animating => "The target area kept changing (animation or live content). Wait, or pass verify_target:\"window\" for live content.",
            Self::RegionChanged => "The target area changed since you read it. Look at the fresh screenshot and pick the point again.",
            Self::WindowChanged => "The window changed since you read it (closed, resized or replaced). Read it again.",
            Self::Occluded => "Another window covers the target point.",
            Self::NotVisible => "The window is not visible (minimized or on another Space), so the target cannot be checked.",
        }
    }

    /// Auto-retry once is allowed only for a pure move with proven identity.
    pub fn retryable(self) -> bool {
        matches!(self, Self::Moved)
    }
}

/// The verdict every guard returns.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Ok,
    /// Identity proven, only the position changed: act at the new point once.
    Relocated {
        dx: f64,
        dy: f64,
    },
    Refuse {
        reason: Reason,
        detail: String,
    },
}

impl Verdict {
    pub fn refuse(reason: Reason, detail: impl Into<String>) -> Self {
        Self::Refuse {
            reason,
            detail: detail.into(),
        }
    }
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
    pub fn reason(&self) -> Option<Reason> {
        match self {
            Self::Refuse { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}
