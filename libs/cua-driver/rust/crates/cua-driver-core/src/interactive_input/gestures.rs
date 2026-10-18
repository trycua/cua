//! Folding interactive batches into one-shot gestures.
//!
//! Some platforms can deliver input only as whole gestures (a click, a drag,
//! a key press, a scroll of N wheel clicks), not as the separate pointer and
//! key edges an interactive batch carries. Hyprland is one: its input goes
//! through the cua-driver tools (virtual pointer and keyboard for the desktop,
//! the cua-hyprland-plugin seats for background window input), each of which
//! takes a complete gesture. [`GestureFolder`] is the shared, platform-neutral
//! translation, so every such adapter folds a batch the same way:
//!
//! - pointer down then up within [`CLICK_SLOP_PX`] is a click at the down
//!   point; further apart it is a drag from the down point to the up point;
//! - pointer moves with no button held (hover) produce nothing;
//! - a key down is one press of that key with its modifiers (a repeat is
//!   another press); key ups and bare modifier keys produce nothing;
//! - consecutive text commits are one text gesture;
//! - scroll deltas accumulate across events (pixel deltas in units of
//!   [`PRECISE_PIXELS_PER_CLICK`]) and whole wheel clicks are emitted, keeping
//!   the fractional rest for the next event, like the X11 session.
//!
//! Coordinates stay normalized to the target; the adapter maps them.

use super::{
    extract_integral, InteractiveInputEvent, KeyState, Modifier, PointerButton, PointerPhase,
};

/// Pixels per wheel click for precise (trackpad) scroll deltas.
pub const PRECISE_PIXELS_PER_CLICK: f64 = 40.0;

/// Largest pointer travel, in target pixels, between down and up that still
/// counts as a click rather than a drag.
pub const CLICK_SLOP_PX: f64 = 4.0;

/// One complete gesture. Points are normalized target coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Gesture {
    Click {
        x: f64,
        y: f64,
        button: PointerButton,
        modifiers: Vec<Modifier>,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
        button: PointerButton,
        modifiers: Vec<Modifier>,
    },
    /// Whole wheel clicks at a point: positive `dy` scrolls down, positive
    /// `dx` scrolls right (the X11 session's convention).
    Scroll {
        x: f64,
        y: f64,
        dx: i32,
        dy: i32,
    },
    Key {
        key: String,
        modifiers: Vec<Modifier>,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone)]
struct Press {
    button: PointerButton,
    from: (f64, f64),
    modifiers: Vec<Modifier>,
}

/// Stateful fold of interactive events into [`Gesture`]s. Keep one per input
/// session: a press and the scroll remainder carry over between batches.
#[derive(Debug, Default)]
pub struct GestureFolder {
    press: Option<Press>,
    scroll_residual_x: f64,
    scroll_residual_y: f64,
}

/// Whether `key` names a modifier on its own (it arrives as a modifier of the
/// next event instead).
pub fn is_modifier_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "shift"
            | "control"
            | "ctrl"
            | "alt"
            | "option"
            | "command"
            | "cmd"
            | "super"
            | "meta"
            | "fn"
            | "function"
            | "capslock"
    )
}

impl GestureFolder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget a held button and any scroll remainder.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Fold `events` (already validated) on a target `extent` pixels large,
    /// in order.
    pub fn fold(&mut self, events: &[InteractiveInputEvent], extent: (u32, u32)) -> Vec<Gesture> {
        let mut gestures = Vec::new();
        for event in events {
            match event {
                InteractiveInputEvent::TextCommit { text } => {
                    if let Some(Gesture::Text { text: pending }) = gestures.last_mut() {
                        pending.push_str(text);
                    } else if !text.is_empty() {
                        gestures.push(Gesture::Text { text: text.clone() });
                    }
                }
                InteractiveInputEvent::Key {
                    key,
                    state: KeyState::Down,
                    modifiers,
                    ..
                } if !key.is_empty() && !is_modifier_key(key) => gestures.push(Gesture::Key {
                    key: key.clone(),
                    modifiers: modifiers.clone(),
                }),
                InteractiveInputEvent::Key { .. } => {}
                InteractiveInputEvent::Pointer {
                    phase,
                    button,
                    x_normalized,
                    y_normalized,
                    modifiers,
                } => {
                    let point = (*x_normalized, *y_normalized);
                    let button = button.unwrap_or(PointerButton::Left);
                    match phase {
                        PointerPhase::Move => {}
                        PointerPhase::Down => {
                            self.press = Some(Press {
                                button,
                                from: point,
                                modifiers: modifiers.clone(),
                            });
                        }
                        PointerPhase::Up => {
                            let Some(press) = self.press.take() else {
                                continue;
                            };
                            if press.button != button {
                                // Another button's release: keep waiting.
                                self.press = Some(press);
                                continue;
                            }
                            let travel = (
                                (point.0 - press.from.0) * f64::from(extent.0),
                                (point.1 - press.from.1) * f64::from(extent.1),
                            );
                            let mut held = press.modifiers;
                            for modifier in modifiers {
                                if !held.contains(modifier) {
                                    held.push(*modifier);
                                }
                            }
                            gestures.push(if travel.0.hypot(travel.1) <= CLICK_SLOP_PX {
                                Gesture::Click {
                                    x: press.from.0,
                                    y: press.from.1,
                                    button,
                                    modifiers: held,
                                }
                            } else {
                                Gesture::Drag {
                                    from: press.from,
                                    to: point,
                                    button,
                                    modifiers: held,
                                }
                            });
                        }
                        PointerPhase::Cancel => self.press = None,
                    }
                }
                InteractiveInputEvent::Scroll {
                    x_normalized,
                    y_normalized,
                    delta_x,
                    delta_y,
                    precise,
                    ..
                } => {
                    let scale = if *precise {
                        1.0 / PRECISE_PIXELS_PER_CLICK
                    } else {
                        1.0
                    };
                    self.scroll_residual_x += delta_x * scale;
                    self.scroll_residual_y += delta_y * scale;
                    let dx = extract_integral(&mut self.scroll_residual_x);
                    let dy = extract_integral(&mut self.scroll_residual_y);
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let point = (*x_normalized, *y_normalized);
                    match gestures.last_mut() {
                        // Consecutive scrolls are one gesture at the latest point.
                        Some(Gesture::Scroll {
                            x,
                            y,
                            dx: total_x,
                            dy: total_y,
                        }) => {
                            (*x, *y) = point;
                            *total_x = total_x.saturating_add(dx);
                            *total_y = total_y.saturating_add(dy);
                        }
                        _ => gestures.push(Gesture::Scroll {
                            x: point.0,
                            y: point.1,
                            dx,
                            dy,
                        }),
                    }
                }
            }
        }
        gestures
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive_input::GesturePhase;

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InteractiveInputEvent {
        InteractiveInputEvent::Pointer {
            phase,
            button: (phase != PointerPhase::Move).then_some(PointerButton::Left),
            x_normalized: x,
            y_normalized: y,
            modifiers: Vec::new(),
        }
    }

    fn key(key: &str, state: KeyState, modifiers: Vec<Modifier>) -> InteractiveInputEvent {
        InteractiveInputEvent::Key {
            key: key.into(),
            state,
            modifiers,
            repeat: false,
        }
    }

    fn scroll(delta_y: f64, precise: bool) -> InteractiveInputEvent {
        InteractiveInputEvent::Scroll {
            x_normalized: 0.5,
            y_normalized: 0.5,
            delta_x: 0.0,
            delta_y,
            phase: GesturePhase::Changed,
            momentum_phase: GesturePhase::None,
            precise,
        }
    }

    const EXTENT: (u32, u32) = (1000, 500);

    #[test]
    fn down_and_up_in_place_is_a_click_at_the_down_point() {
        let mut folder = GestureFolder::new();
        let gestures = folder.fold(
            &[
                pointer(PointerPhase::Move, 0.1, 0.1),
                pointer(PointerPhase::Down, 0.25, 0.5),
                pointer(PointerPhase::Up, 0.252, 0.5),
            ],
            EXTENT,
        );
        assert_eq!(
            gestures,
            vec![Gesture::Click {
                x: 0.25,
                y: 0.5,
                button: PointerButton::Left,
                modifiers: Vec::new(),
            }]
        );
    }

    #[test]
    fn a_press_held_across_batches_and_moved_is_a_drag() {
        let mut folder = GestureFolder::new();
        assert!(folder
            .fold(&[pointer(PointerPhase::Down, 0.1, 0.1)], EXTENT)
            .is_empty());
        assert!(folder
            .fold(&[pointer(PointerPhase::Move, 0.3, 0.2)], EXTENT)
            .is_empty());
        assert_eq!(
            folder.fold(&[pointer(PointerPhase::Up, 0.5, 0.4)], EXTENT),
            vec![Gesture::Drag {
                from: (0.1, 0.1),
                to: (0.5, 0.4),
                button: PointerButton::Left,
                modifiers: Vec::new(),
            }]
        );
    }

    #[test]
    fn cancel_and_unmatched_releases_produce_nothing() {
        let mut folder = GestureFolder::new();
        let gestures = folder.fold(
            &[
                pointer(PointerPhase::Up, 0.5, 0.5),
                pointer(PointerPhase::Down, 0.5, 0.5),
                pointer(PointerPhase::Cancel, 0.5, 0.5),
                pointer(PointerPhase::Up, 0.5, 0.5),
            ],
            EXTENT,
        );
        assert!(gestures.is_empty());
    }

    #[test]
    fn key_downs_are_presses_and_ups_and_bare_modifiers_are_dropped() {
        let mut folder = GestureFolder::new();
        let gestures = folder.fold(
            &[
                key("shift", KeyState::Down, Vec::new()),
                key("enter", KeyState::Down, Vec::new()),
                key("enter", KeyState::Up, Vec::new()),
                key("c", KeyState::Down, vec![Modifier::Control]),
                key("c", KeyState::Up, vec![Modifier::Control]),
            ],
            EXTENT,
        );
        assert_eq!(
            gestures,
            vec![
                Gesture::Key {
                    key: "enter".into(),
                    modifiers: Vec::new(),
                },
                Gesture::Key {
                    key: "c".into(),
                    modifiers: vec![Modifier::Control],
                },
            ]
        );
    }

    #[test]
    fn consecutive_text_commits_are_one_text_gesture() {
        let mut folder = GestureFolder::new();
        let gestures = folder.fold(
            &[
                InteractiveInputEvent::TextCommit { text: "he".into() },
                InteractiveInputEvent::TextCommit { text: "llo".into() },
                key("enter", KeyState::Down, Vec::new()),
                InteractiveInputEvent::TextCommit { text: "!".into() },
            ],
            EXTENT,
        );
        assert_eq!(
            gestures,
            vec![
                Gesture::Text {
                    text: "hello".into()
                },
                Gesture::Key {
                    key: "enter".into(),
                    modifiers: Vec::new(),
                },
                Gesture::Text { text: "!".into() },
            ]
        );
    }

    #[test]
    fn scroll_accumulates_whole_clicks_and_keeps_the_remainder() {
        let mut folder = GestureFolder::new();
        // 30 px + 30 px of trackpad travel is one and a half clicks.
        assert_eq!(
            folder.fold(&[scroll(30.0, true), scroll(30.0, true)], EXTENT),
            vec![Gesture::Scroll {
                x: 0.5,
                y: 0.5,
                dx: 0,
                dy: 1,
            }]
        );
        // The half click left over completes with the next 20 px.
        assert_eq!(
            folder.fold(&[scroll(20.0, true)], EXTENT),
            vec![Gesture::Scroll {
                x: 0.5,
                y: 0.5,
                dx: 0,
                dy: 1,
            }]
        );
        assert_eq!(
            folder.fold(&[scroll(-3.0, false)], EXTENT),
            vec![Gesture::Scroll {
                x: 0.5,
                y: 0.5,
                dx: 0,
                dy: -3,
            }]
        );
    }
}
