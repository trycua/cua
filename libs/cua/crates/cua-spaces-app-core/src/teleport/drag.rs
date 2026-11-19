// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Dragging a real window to the notch.
//!
//! The dragged window is never moved, hidden or repositioned; the shells
//! only draw an additive "ghost" of it. The notch morphs in place:
//!
//! - `idle`: nothing (the ambient notch);
//! - `prompt`: the drag started; the notch grows into the "Teleport to Cua"
//!   box;
//! - `selector`: the window reached the notch; the Space tiles open as drop
//!   targets.
//!
//! The shells hit-test the global cursor and feed events; the only effects
//! are the ghost capture and the final commit.

use serde::{Deserialize, Serialize};

/// The phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DragOverlayPhase {
    /// Nothing showing.
    Idle,
    /// The "Teleport to Cua" box.
    Prompt,
    /// The Space tiles as drop targets.
    Selector,
}

/// The state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragOverlayState {
    /// Phase.
    pub phase: DragOverlayPhase,
    /// CoreGraphics id of the dragged window.
    pub window_id: Option<u32>,
    /// The dragged app's name.
    pub app_name: Option<String>,
    /// The tile under the cursor (selector only).
    pub target_space_id: Option<String>,
    /// The captured ghost (a data URL or a shell-local image key).
    pub ghost: Option<String>,
}

/// The idle state.
pub fn initial() -> DragOverlayState {
    DragOverlayState {
        phase: DragOverlayPhase::Idle,
        window_id: None,
        app_name: None,
        target_space_id: None,
        ghost: None,
    }
}

/// An input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DragOverlayEvent {
    /// A supported window drag began.
    #[serde(rename_all = "camelCase")]
    Start {
        /// Window id.
        window_id: Option<u32>,
        /// App name.
        app_name: Option<String>,
    },
    /// The cursor reached the notch box.
    EnterNotch,
    /// The cursor left the selector.
    LeaveNotch,
    /// Over a Space tile.
    #[serde(rename_all = "camelCase")]
    Over {
        /// Space id.
        space_id: String,
    },
    /// Off every tile.
    Out,
    /// The ghost capture finished.
    GhostReady {
        /// The image, or none.
        ghost: Option<String>,
    },
    /// Released; the tile under the cursor, if any.
    #[serde(rename_all = "camelCase")]
    Drop {
        /// Space id.
        space_id: Option<String>,
    },
    /// Abort.
    Cancel,
}

/// A side effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DragOverlayEffect {
    /// Capture the ghost (needs Screen Recording; none is fine).
    #[serde(rename_all = "camelCase")]
    Capture {
        /// Window id.
        window_id: Option<u32>,
    },
    /// Teleport to this Space.
    #[serde(rename_all = "camelCase")]
    Commit {
        /// Space id.
        space_id: String,
    },
}

/// A transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DragOverlayTransition {
    /// New state.
    pub state: DragOverlayState,
    /// Effects.
    pub effects: Vec<DragOverlayEffect>,
}

/// Advances the drag. Events that do not apply leave it unchanged.
pub fn apply(state: &DragOverlayState, event: &DragOverlayEvent) -> DragOverlayTransition {
    use DragOverlayPhase::*;
    let same = || DragOverlayTransition {
        state: state.clone(),
        effects: vec![],
    };
    let to = |s: DragOverlayState| DragOverlayTransition {
        state: s,
        effects: vec![],
    };
    match event {
        DragOverlayEvent::Start {
            window_id,
            app_name,
        } => DragOverlayTransition {
            state: DragOverlayState {
                phase: Prompt,
                window_id: *window_id,
                app_name: app_name.clone(),
                target_space_id: None,
                ghost: None,
            },
            effects: vec![DragOverlayEffect::Capture {
                window_id: *window_id,
            }],
        },
        DragOverlayEvent::EnterNotch if state.phase == Prompt => to(DragOverlayState {
            phase: Selector,
            target_space_id: None,
            ..state.clone()
        }),
        DragOverlayEvent::LeaveNotch if state.phase == Selector => to(DragOverlayState {
            phase: Prompt,
            target_space_id: None,
            ..state.clone()
        }),
        DragOverlayEvent::Over { space_id } if state.phase == Selector => to(DragOverlayState {
            target_space_id: Some(space_id.clone()),
            ..state.clone()
        }),
        DragOverlayEvent::Out if state.phase == Selector => to(DragOverlayState {
            target_space_id: None,
            ..state.clone()
        }),
        DragOverlayEvent::GhostReady { ghost } if state.phase != Idle => to(DragOverlayState {
            ghost: ghost.clone(),
            ..state.clone()
        }),
        DragOverlayEvent::Drop { space_id } => {
            let target = space_id.clone().or_else(|| state.target_space_id.clone());
            DragOverlayTransition {
                state: initial(),
                effects: match target {
                    Some(space_id) if state.phase != Idle => {
                        vec![DragOverlayEffect::Commit { space_id }]
                    }
                    _ => vec![],
                },
            }
        }
        DragOverlayEvent::Cancel => to(initial()),
        _ => same(),
    }
}
