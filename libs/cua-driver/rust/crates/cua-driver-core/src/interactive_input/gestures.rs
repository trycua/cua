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
//! - a click's `count` is its place in a sequence of quick presses of one
//!   button at one spot (within [`DOUBLE_CLICK_INTERVAL`] and
//!   [`CLICK_SLOP_PX`] of the previous press), up to [`MAX_CLICK_COUNT`]; the
//!   batch carries no timestamps, so presses are timed at receipt. A viewer's
//!   double-click is then a `count: 1` click followed by a `count: 2` click,
//!   which the tools deliver as the sequence a double-click needs (a
//!   `WM_LBUTTONDBLCLK` on Windows, a two-press train on Hyprland);
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
    extract_integral, ClickCounter, InteractiveInputEvent, KeyState, Modifier, PointerButton,
    PointerPhase,
};
use std::time::{Duration, Instant};

/// Pixels per wheel click for precise (trackpad) scroll deltas.
pub const PRECISE_PIXELS_PER_CLICK: f64 = 40.0;

/// Largest pointer travel, in target pixels, between down and up that still
/// counts as a click rather than a drag.
pub const CLICK_SLOP_PX: f64 = 4.0;

/// Longest pause between two presses that still continues a click sequence
/// (the Windows default; Hyprland has no system-wide setting).
pub const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// The highest click count the `click` tool takes; the next press starts a
/// new sequence.
pub const MAX_CLICK_COUNT: u32 = 3;

/// One complete gesture. Points are normalized target coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Gesture {
    /// `count` is 1 for a single click, 2 for the second press of a
    /// double-click, 3 for the third of a triple-click.
    Click {
        x: f64,
        y: f64,
        button: PointerButton,
        count: u32,
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
    count: u32,
    modifiers: Vec<Modifier>,
}

/// Stateful fold of interactive events into [`Gesture`]s. Keep one per input
/// session: a press, the click sequence and the scroll remainder carry over
/// between batches.
#[derive(Debug)]
pub struct GestureFolder {
    press: Option<Press>,
    clicks: ClickCounter,
    scroll_residual_x: f64,
    scroll_residual_y: f64,
}

impl Default for GestureFolder {
    fn default() -> Self {
        Self {
            press: None,
            clicks: ClickCounter::new(DOUBLE_CLICK_INTERVAL, CLICK_SLOP_PX),
            scroll_residual_x: 0.0,
            scroll_residual_y: 0.0,
        }
    }
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

    /// Forget a held button, the click sequence and any scroll remainder.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Fold `events` (already validated) on a target `extent` pixels large,
    /// in order. `now` is the batch's receipt time, which times its presses.
    pub fn fold(
        &mut self,
        events: &[InteractiveInputEvent],
        extent: (u32, u32),
        now: Instant,
    ) -> Vec<Gesture> {
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
                            if self.clicks.count() >= MAX_CLICK_COUNT {
                                self.clicks.reset();
                            }
                            let pixels =
                                (point.0 * f64::from(extent.0), point.1 * f64::from(extent.1));
                            self.press = Some(Press {
                                button,
                                from: point,
                                count: self.clicks.press(button, pixels, now),
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
                                    count: press.count,
                                    modifiers: held,
                                }
                            } else {
                                // A drag ends the click sequence.
                                self.clicks.reset();
                                Gesture::Drag {
                                    from: press.from,
                                    to: point,
                                    button,
                                    modifiers: held,
                                }
                            });
                        }
                        PointerPhase::Cancel => {
                            self.press = None;
                            self.clicks.reset();
                        }
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

    fn click(x: f64, y: f64, count: u32) -> Gesture {
        Gesture::Click {
            x,
            y,
            button: PointerButton::Left,
            count,
            modifiers: Vec::new(),
        }
    }

    fn press(x: f64, y: f64) -> [InteractiveInputEvent; 2] {
        [
            pointer(PointerPhase::Down, x, y),
            pointer(PointerPhase::Up, x, y),
        ]
    }

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
            Instant::now(),
        );
        assert_eq!(gestures, vec![click(0.25, 0.5, 1)]);
    }

    #[test]
    fn quick_presses_at_one_spot_count_up_across_batches_up_to_three() {
        let mut folder = GestureFolder::new();
        let start = Instant::now();
        let mut batch = press(0.5, 0.5).to_vec();
        batch.extend(press(0.501, 0.5));
        assert_eq!(
            folder.fold(&batch, EXTENT, start),
            vec![click(0.5, 0.5, 1), click(0.501, 0.5, 2)]
        );
        // The third press arrives in the next batch, still inside the interval.
        assert_eq!(
            folder.fold(
                &press(0.502, 0.5),
                EXTENT,
                start + Duration::from_millis(400)
            ),
            vec![click(0.502, 0.5, 3)]
        );
        // The tool takes at most a triple-click: the fourth press starts over.
        assert_eq!(
            folder.fold(&press(0.5, 0.5), EXTENT, start + Duration::from_millis(500)),
            vec![click(0.5, 0.5, 1)]
        );
    }

    #[test]
    fn a_late_far_or_dragged_press_starts_a_new_click_sequence() {
        let mut folder = GestureFolder::new();
        let start = Instant::now();
        assert_eq!(
            folder.fold(&press(0.5, 0.5), EXTENT, start),
            vec![click(0.5, 0.5, 1)]
        );
        // Too late: over DOUBLE_CLICK_INTERVAL after the previous press.
        assert_eq!(
            folder.fold(&press(0.5, 0.5), EXTENT, start + Duration::from_millis(600)),
            vec![click(0.5, 0.5, 1)]
        );
        // Too far: 10 px on a 1000 px wide target.
        assert_eq!(
            folder.fold(
                &press(0.51, 0.5),
                EXTENT,
                start + Duration::from_millis(700)
            ),
            vec![click(0.51, 0.5, 1)]
        );
        // A drag in between ends the sequence even at the same spot and time.
        let at = start + Duration::from_millis(800);
        assert_eq!(
            folder.fold(&press(0.51, 0.5), EXTENT, at),
            vec![click(0.51, 0.5, 2)]
        );
        let dragged = folder.fold(
            &[
                pointer(PointerPhase::Down, 0.51, 0.5),
                pointer(PointerPhase::Up, 0.9, 0.5),
            ],
            EXTENT,
            at,
        );
        assert!(matches!(dragged[..], [Gesture::Drag { .. }]));
        assert_eq!(
            folder.fold(&press(0.51, 0.5), EXTENT, at),
            vec![click(0.51, 0.5, 1)]
        );
    }

    #[test]
    fn a_press_held_across_batches_and_moved_is_a_drag() {
        let mut folder = GestureFolder::new();
        assert!(folder
            .fold(
                &[pointer(PointerPhase::Down, 0.1, 0.1)],
                EXTENT,
                Instant::now()
            )
            .is_empty());
        assert!(folder
            .fold(
                &[pointer(PointerPhase::Move, 0.3, 0.2)],
                EXTENT,
                Instant::now()
            )
            .is_empty());
        assert_eq!(
            folder.fold(
                &[pointer(PointerPhase::Up, 0.5, 0.4)],
                EXTENT,
                Instant::now()
            ),
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
            Instant::now(),
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
            Instant::now(),
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
            Instant::now(),
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
            folder.fold(
                &[scroll(30.0, true), scroll(30.0, true)],
                EXTENT,
                Instant::now()
            ),
            vec![Gesture::Scroll {
                x: 0.5,
                y: 0.5,
                dx: 0,
                dy: 1,
            }]
        );
        // The half click left over completes with the next 20 px.
        assert_eq!(
            folder.fold(&[scroll(20.0, true)], EXTENT, Instant::now()),
            vec![Gesture::Scroll {
                x: 0.5,
                y: 0.5,
                dx: 0,
                dy: 1,
            }]
        );
        assert_eq!(
            folder.fold(&[scroll(-3.0, false)], EXTENT, Instant::now()),
            vec![Gesture::Scroll {
                x: 0.5,
                y: 0.5,
                dx: 0,
                dy: -3,
            }]
        );
    }
}
