use std::time::{Duration, Instant};

use crate::{Point, Rect};

pub const CONTROL_ARM_DELAY: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionOutcome {
    None,
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Accept,
    Decline,
}

/// Consent interaction state with defenses against a click already queued at
/// the pointer position before the protected card appeared.
pub struct ConsentInteraction {
    shown_at: Instant,
    accept_rect: Rect,
    decline_rect: Rect,
    accept_entered_after_arm: bool,
    pressed: Option<Target>,
}

impl ConsentInteraction {
    pub fn new(shown_at: Instant, accept_rect: Rect, decline_rect: Rect) -> Self {
        Self {
            shown_at,
            accept_rect,
            decline_rect,
            accept_entered_after_arm: false,
            pressed: None,
        }
    }

    pub fn accept_armed(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.shown_at) >= CONTROL_ARM_DELAY
            && self.accept_entered_after_arm
    }

    pub fn pointer_moved(&mut self, point: Point, now: Instant) {
        if now.saturating_duration_since(self.shown_at) >= CONTROL_ARM_DELAY
            && self.accept_rect.contains(point)
        {
            self.accept_entered_after_arm = true;
        }
    }

    pub fn pointer_down(
        &mut self,
        point: Point,
        button: PointerButton,
        now: Instant,
    ) -> InteractionOutcome {
        if button != PointerButton::Primary {
            return InteractionOutcome::None;
        }
        self.pressed = if self.decline_rect.contains(point) {
            Some(Target::Decline)
        } else if self.accept_rect.contains(point) && self.accept_armed(now) {
            Some(Target::Accept)
        } else {
            None
        };
        InteractionOutcome::None
    }

    pub fn pointer_up(&mut self, point: Point) -> InteractionOutcome {
        match self.pressed.take() {
            Some(Target::Accept) if self.accept_rect.contains(point) => InteractionOutcome::Accept,
            Some(Target::Decline) if self.decline_rect.contains(point) => {
                InteractionOutcome::Decline
            }
            _ => InteractionOutcome::None,
        }
    }

    pub fn escape(&mut self) -> InteractionOutcome {
        self.pressed = None;
        InteractionOutcome::Cancel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interaction(now: Instant) -> ConsentInteraction {
        ConsentInteraction::new(
            now,
            Rect {
                x: 220.0,
                y: 190.0,
                width: 170.0,
                height: 44.0,
            },
            Rect {
                x: 30.0,
                y: 190.0,
                width: 170.0,
                height: 44.0,
            },
        )
    }

    #[test]
    fn queued_accept_click_is_rejected() {
        let now = Instant::now();
        let mut state = interaction(now);
        let point = Point { x: 300.0, y: 210.0 };
        state.pointer_down(point, PointerButton::Primary, now);
        assert_eq!(state.pointer_up(point), InteractionOutcome::None);
    }

    #[test]
    fn accept_requires_post_arm_entry_and_press_release() {
        let now = Instant::now();
        let later = now + CONTROL_ARM_DELAY;
        let point = Point { x: 300.0, y: 210.0 };
        let mut state = interaction(now);
        state.pointer_moved(point, later);
        state.pointer_down(point, PointerButton::Primary, later);
        assert_eq!(state.pointer_up(point), InteractionOutcome::Accept);
    }

    #[test]
    fn dragging_out_cancels_accept() {
        let now = Instant::now();
        let later = now + CONTROL_ARM_DELAY;
        let mut state = interaction(now);
        state.pointer_moved(Point { x: 300.0, y: 210.0 }, later);
        state.pointer_down(Point { x: 300.0, y: 210.0 }, PointerButton::Primary, later);
        assert_eq!(
            state.pointer_up(Point { x: 10.0, y: 10.0 }),
            InteractionOutcome::None
        );
    }

    #[test]
    fn decline_is_available_immediately() {
        let now = Instant::now();
        let mut state = interaction(now);
        let point = Point { x: 100.0, y: 210.0 };
        state.pointer_down(point, PointerButton::Primary, now);
        assert_eq!(state.pointer_up(point), InteractionOutcome::Decline);
    }
}
