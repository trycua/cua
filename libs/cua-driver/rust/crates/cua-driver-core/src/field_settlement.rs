//! Bounded quiet-value settling. This is field observation, never application commitment.
use serde::Serialize;
use std::time::Duration;

pub const POLL: Duration = Duration::from_millis(20);
pub const QUIET: Duration = Duration::from_millis(150);
pub const REACTION: Duration = Duration::from_millis(600);
pub const CAP: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Settled,
    NoReaction,
    ValueMismatch,
    TimedOut,
    Unavailable,
    Interrupted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub status: Status,
    pub reacted: bool,
    pub value_matches: bool,
    pub elapsed_ms: u64,
}
pub struct Tracker {
    before: String,
    expected: String,
    previous: String,
    quiet_since: Duration,
    reacted: bool,
}
impl Tracker {
    pub fn new(before: String, expected: String) -> Self {
        Self {
            previous: before.clone(),
            before,
            expected,
            quiet_since: Duration::ZERO,
            reacted: false,
        }
    }
    pub fn report(&self, status: Status, elapsed: Duration) -> Report {
        Report {
            status,
            reacted: self.reacted,
            value_matches: self.previous == self.expected,
            elapsed_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
        }
    }
    /// Every sample must come from the freshly checked exact native field.
    /// Missing binding/value evidence is unavailable, never quiet success.
    pub fn sample(
        &mut self,
        elapsed: Duration,
        value: Option<String>,
        guards_intact: bool,
    ) -> Option<Report> {
        if !guards_intact {
            return Some(self.report(Status::Interrupted, elapsed));
        }
        let Some(value) = value else {
            return Some(self.report(Status::Unavailable, elapsed));
        };
        if value != self.previous {
            self.reacted |= value != self.before;
            self.previous = value;
            self.quiet_since = elapsed;
        }
        if elapsed >= CAP {
            return Some(self.report(Status::TimedOut, elapsed));
        }
        // A verified no-op can settle, but must not be labelled a reaction.
        if (self.reacted || self.before == self.expected)
            && elapsed.saturating_sub(self.quiet_since) >= QUIET
        {
            return Some(self.report(
                if self.previous == self.expected {
                    Status::Settled
                } else {
                    Status::ValueMismatch
                },
                elapsed,
            ));
        }
        if !self.reacted && elapsed >= REACTION {
            return Some(self.report(Status::NoReaction, elapsed));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    #[test]
    fn reaction_requires_a_full_quiet_interval_and_resets_on_later_changes() {
        let mut t = Tracker::new("old".into(), "new".into());
        assert_eq!(t.sample(ms(20), Some("new".into()), true), None);
        assert_eq!(t.sample(ms(169), Some("new".into()), true), None);
        assert_eq!(t.sample(ms(170), Some("partial".into()), true), None);
        assert_eq!(t.sample(ms(300), Some("new".into()), true), None);
        assert_eq!(t.sample(ms(449), Some("new".into()), true), None);
        assert_eq!(
            t.sample(ms(450), Some("new".into()), true),
            Some(Report {
                status: Status::Settled,
                reacted: true,
                value_matches: true,
                elapsed_ms: 450
            })
        );
    }
    #[test]
    fn unchanged_wrong_value_is_no_reaction_and_a_noop_is_not_a_reaction() {
        let mut t = Tracker::new("old".into(), "new".into());
        assert_eq!(t.sample(ms(599), Some("old".into()), true), None);
        assert_eq!(
            t.sample(ms(600), Some("old".into()), true).unwrap(),
            Report {
                status: Status::NoReaction,
                reacted: false,
                value_matches: false,
                elapsed_ms: 600
            }
        );
        let mut t = Tracker::new("new".into(), "new".into());
        assert_eq!(
            t.sample(ms(150), Some("new".into()), true).unwrap(),
            Report {
                status: Status::Settled,
                reacted: false,
                value_matches: true,
                elapsed_ms: 150
            }
        );
    }
    #[test]
    fn missing_evidence_and_interference_never_become_quiet_success() {
        for (value, guards, status) in [
            (None, true, Status::Unavailable),
            (Some("new".into()), false, Status::Interrupted),
        ] {
            let mut t = Tracker::new("old".into(), "new".into());
            assert_eq!(t.sample(ms(200), value, guards).unwrap().status, status);
        }
    }
    #[test]
    fn wrong_quiet_value_and_continuous_changes_do_not_settle() {
        let mut t = Tracker::new("old".into(), "new".into());
        assert_eq!(t.sample(ms(20), Some("wrong".into()), true), None);
        assert_eq!(
            t.sample(ms(170), Some("wrong".into()), true)
                .unwrap()
                .status,
            Status::ValueMismatch
        );
        let mut t = Tracker::new("old".into(), "new".into());
        for n in (20..2000).step_by(20) {
            assert_eq!(t.sample(ms(n), Some(n.to_string()), true), None);
        }
        assert_eq!(
            t.sample(ms(2000), Some("new".into()), true).unwrap().status,
            Status::TimedOut
        );
    }
}
