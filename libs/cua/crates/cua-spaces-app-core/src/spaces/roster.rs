// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Space list state machine: which Spaces are listed (MRU order), which
//! is selected, keyboard focus in the notch switcher, the switcher's mode and
//! the transient notice. `sync` merges a registry refresh without
//! reshuffling rows under the user.

use super::{sort_by_mru, touch_space};
use crate::model::{Space, SpaceStatus};
use crate::notch::WindowMode;
use serde::{Deserialize, Serialize};

/// Focus index of the "+ New" tile.
pub const NEW_TILE_INDEX: i32 = -1;
/// Drop-target id of the "+ New" tile (never a real Space id).
pub const NEW_SPACE_DROP_ID: &str = "__new_space__";

/// What a notice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeKind {
    /// Switched Spaces.
    Switch,
    /// Created (or creating) Spaces.
    Create,
}

/// A transient confirmation line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    /// Sequence id (clear by id).
    pub id: u32,
    /// Kind.
    pub kind: NoticeKind,
    /// Text.
    pub text: String,
}

/// The list's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterState {
    /// The notch switcher's mode.
    pub mode: WindowMode,
    /// Spaces, MRU order.
    pub spaces: Vec<Space>,
    /// The selected Space id (empty when none).
    pub selected_id: String,
    /// Keyboard focus: an index into `spaces`, or [`NEW_TILE_INDEX`].
    pub focus_index: i32,
    /// The notice, if one shows.
    pub notice: Option<Notice>,
    /// Last notice id.
    pub notice_seq: u32,
}

/// An input to the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RosterAction {
    /// Open the switcher.
    Expand,
    /// Back to ambient.
    Collapse,
    /// Open the create panel.
    OpenCreate,
    /// Close it.
    CancelCreate,
    /// Focus a tile.
    Focus {
        /// Tile index or [`NEW_TILE_INDEX`].
        index: i32,
    },
    /// Move focus by one, wrapping through the New tile.
    FocusMove {
        /// `1` or `-1`.
        delta: i32,
    },
    /// Select a Space (touches its MRU time).
    Select {
        /// Space id.
        id: String,
        /// Epoch ms.
        now: i64,
    },
    /// Clear the notice with this id.
    ClearNotice {
        /// Notice id.
        id: u32,
    },
    /// Show a notice.
    Notify {
        /// Text.
        text: String,
        /// Kind (`create` when omitted).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<NoticeKind>,
    },
    /// Replace the list with a registry refresh.
    SyncSpaces {
        /// The fresh list.
        spaces: Vec<Space>,
    },
}

/// The first state for a list.
pub fn initial(spaces: &[Space]) -> RosterState {
    let sorted = sort_by_mru(spaces);
    RosterState {
        mode: WindowMode::Ambient,
        selected_id: sorted.first().map(|s| s.id.clone()).unwrap_or_default(),
        spaces: sorted,
        focus_index: 0,
        notice: None,
        notice_seq: 0,
    }
}

/// Advances the list.
pub fn reduce(state: &RosterState, action: &RosterAction) -> RosterState {
    let mut next = state.clone();
    match action {
        RosterAction::Expand => {
            if state.mode != WindowMode::Ambient {
                return next;
            }
            let selected = state.spaces.iter().position(|s| s.id == state.selected_id);
            next.mode = WindowMode::Switcher;
            next.focus_index = selected.map(|i| i as i32).unwrap_or(0).max(0);
        }
        RosterAction::Collapse => {
            if state.mode != WindowMode::Ambient {
                next.mode = WindowMode::Ambient;
            }
        }
        RosterAction::OpenCreate => next.mode = WindowMode::CreateFleet,
        RosterAction::CancelCreate => {
            if state.mode == WindowMode::CreateFleet {
                next.mode = WindowMode::Switcher;
            }
        }
        RosterAction::Focus { index } => next.focus_index = *index,
        RosterAction::FocusMove { delta } => {
            let n = state.spaces.len() as i32;
            let mut order: Vec<i32> = (0..n).collect();
            order.push(NEW_TILE_INDEX);
            let len = order.len() as i32;
            let current = order
                .iter()
                .position(|i| *i == state.focus_index)
                .map(|p| p as i32)
                .unwrap_or(-1);
            let idx = (current + delta.signum() + len).rem_euclid(len);
            next.focus_index = order[idx as usize];
        }
        RosterAction::Select { id, now } => {
            let Some(target) = state.spaces.iter().find(|s| &s.id == id) else {
                return next;
            };
            let text = if target.id == crate::host::THIS_MACHINE_ID
                || target.status == SpaceStatus::Local
            {
                "Back on This Mac".to_string()
            } else {
                format!("Switching to {}", target.name)
            };
            next.spaces = sort_by_mru(&touch_space(&state.spaces, id, *now));
            next.selected_id = id.clone();
            next.focus_index = 0;
            next.notice_seq = state.notice_seq + 1;
            next.notice = Some(Notice {
                id: next.notice_seq,
                kind: NoticeKind::Switch,
                text,
            });
        }
        RosterAction::ClearNotice { id } => {
            if state.notice.as_ref().is_some_and(|n| n.id == *id) {
                next.notice = None;
            }
        }
        RosterAction::Notify { text, kind } => {
            next.notice_seq = state.notice_seq + 1;
            next.notice = Some(Notice {
                id: next.notice_seq,
                kind: kind.unwrap_or(NoticeKind::Create),
                text: text.clone(),
            });
        }
        RosterAction::SyncSpaces { spaces } => {
            let merged = sort_by_mru(
                &spaces
                    .iter()
                    .map(|s| match state.spaces.iter().find(|p| p.id == s.id) {
                        Some(prev) if prev.last_used_at > s.last_used_at => Space {
                            last_used_at: prev.last_used_at,
                            ..s.clone()
                        },
                        _ => s.clone(),
                    })
                    .collect::<Vec<_>>(),
            );
            next.selected_id = if merged.iter().any(|s| s.id == state.selected_id) {
                state.selected_id.clone()
            } else {
                merged.first().map(|s| s.id.clone()).unwrap_or_default()
            };
            next.focus_index = if state.focus_index == NEW_TILE_INDEX {
                NEW_TILE_INDEX
            } else {
                state
                    .focus_index
                    .max(0)
                    .min((merged.len() as i32 - 1).max(0))
            };
            next.spaces = merged;
        }
    }
    next
}
