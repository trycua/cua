// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Is that machine still there? One detector per peer, fed by heartbeat
//! arrivals on the control link.
//!
//! The rule is built so a bad network never causes a takeover and a closed
//! laptop is noticed quickly:
//!
//! - *Adaptive threshold.* The silence that makes a peer suspect is
//!   `max(floor, mean + 8 * deviation)` of the heartbeat gaps seen so far
//!   (an exponentially weighted mean, like TCP's retransmit timer). Jitter
//!   and loss widen the gaps and so widen the threshold.
//! - *Confirmation.* A suspect peer is declared dead only after the silence
//!   has lasted `confirm` more. Any heartbeat before that clears the
//!   suspicion.
//! - *Recovery needs a run.* A dead peer is alive again after `recover`
//!   heartbeats in a row, each within the threshold of the one before, so a
//!   link that flaps does not bounce ownership back and forth.
//! - *A goodbye is believed at once.* A machine going to sleep says so
//!   (`Bye`); it counts as away immediately, without waiting.
//! - *Our own pauses are not theirs.* If this process was frozen (our lid
//!   was closed, a SIGSTOP), every peer looks silent. [`Detector::resume`]
//!   forgives a gap in our own clock so a waking laptop never declares the
//!   studio dead because it slept.

use std::time::{Duration, Instant};

/// Timing knobs (all derived from the heartbeat interval by default).
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Heartbeat interval.
    pub interval: Duration,
    /// The least silence that raises suspicion.
    pub floor: Duration,
    /// How long suspicion must last before dead.
    pub confirm: Duration,
    /// Heartbeats in a row that revive a dead peer.
    pub recover: u32,
}

impl Timing {
    /// 2 s heartbeats, suspect after 6 s, dead 12 s later, 3 in a row to
    /// come back.
    pub fn standard() -> Timing {
        Timing {
            interval: Duration::from_secs(2),
            floor: Duration::from_secs(6),
            confirm: Duration::from_secs(12),
            recover: 3,
        }
    }
}

/// What the detector believes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Heartbeats arrive.
    Alive,
    /// Silent longer than the threshold; not yet declared dead.
    Suspect,
    /// Silent through the confirmation window.
    Dead,
    /// Said goodbye (asleep, shutting down, network gone).
    Away,
}

impl Health {
    /// Dead or away: nothing to wait for.
    pub fn is_down(self) -> bool {
        matches!(self, Health::Dead | Health::Away)
    }
}

/// One peer's detector.
#[derive(Clone, Debug)]
pub struct Detector {
    t: Timing,
    last: Option<Instant>,
    mean_ms: f64,
    dev_ms: f64,
    state: Health,
    /// Down on the machine's own word (goodbye) or on refusals, not on silence.
    explicit: bool,
    suspect_since: Option<Instant>,
    run: u32,
    /// How many times it went from alive to dead (a flap counter for the
    /// reliability score).
    pub downs: u32,
}

impl Detector {
    pub fn new(t: Timing, now: Instant) -> Detector {
        Detector {
            t,
            last: Some(now),
            mean_ms: t.interval.as_millis() as f64,
            dev_ms: 0.0,
            state: Health::Alive,
            explicit: false,
            suspect_since: None,
            run: 0,
            downs: 0,
        }
    }

    /// The silence that makes this peer suspect now.
    pub fn threshold(&self) -> Duration {
        let adaptive = self.mean_ms + 8.0 * self.dev_ms;
        Duration::from_millis(adaptive as u64).max(self.t.floor)
    }

    /// A heartbeat (or any control message) arrived.
    pub fn arrival(&mut self, now: Instant) {
        if let Some(last) = self.last {
            let gap = now.saturating_duration_since(last).as_millis() as f64;
            // A gap far past the threshold is an outage, not a sample.
            if gap < 6.0 * self.threshold().as_millis() as f64 {
                let err = gap - self.mean_ms;
                self.mean_ms += 0.125 * err;
                self.dev_ms += 0.25 * (err.abs() - self.dev_ms);
            }
            let within = now.saturating_duration_since(last) <= self.threshold();
            match self.state {
                Health::Dead | Health::Away => {
                    self.run = if within || self.run == 0 {
                        self.run + 1
                    } else {
                        1
                    };
                    if self.run >= self.t.recover {
                        self.state = Health::Alive;
                        self.explicit = false;
                        self.suspect_since = None;
                        self.run = 0;
                    }
                }
                Health::Suspect => {
                    self.state = Health::Alive;
                    self.suspect_since = None;
                }
                Health::Alive => {}
            }
        }
        self.last = Some(now);
    }

    /// The peer said goodbye.
    pub fn bye(&mut self, now: Instant) {
        if !self.state.is_down() {
            self.downs += 1;
        }
        self.state = Health::Away;
        self.explicit = true;
        self.run = 0;
        self.last = Some(now);
    }

    /// The machine answered a dial with a refusal, or accepted and hung up
    /// before saying hello, several times in a row: someone is there to say
    /// no, and nobody is running the node. That is far better evidence than
    /// silence, so it skips the confirmation window (never the first
    /// refusal alone: the caller counts).
    pub fn refused(&mut self, now: Instant) {
        if !self.state.is_down() {
            self.downs += 1;
        }
        self.state = Health::Dead;
        self.explicit = true;
        self.run = 0;
        self.last = Some(now);
    }

    /// This process was paused: forgive the gap.
    pub fn resume(&mut self, now: Instant) {
        if matches!(self.state, Health::Alive | Health::Suspect) {
            self.state = Health::Alive;
            self.suspect_since = None;
        }
        self.last = Some(now);
    }

    /// What it believes at `now`.
    pub fn poll(&mut self, now: Instant) -> Health {
        let Some(last) = self.last else {
            return self.state;
        };
        let silence = now.saturating_duration_since(last);
        match self.state {
            Health::Alive if silence > self.threshold() => {
                self.state = Health::Suspect;
                self.suspect_since = Some(now);
            }
            Health::Suspect => {
                if let Some(since) = self.suspect_since
                    && now.saturating_duration_since(since) >= self.t.confirm
                    && silence > self.threshold()
                {
                    self.state = Health::Dead;
                    self.explicit = false;
                    self.downs += 1;
                    self.run = 0;
                }
            }
            _ => {}
        }
        self.state
    }

    /// How long since the peer was last heard from.
    pub fn silence(&self, now: Instant) -> Duration {
        self.last
            .map_or(Duration::MAX, |l| now.saturating_duration_since(l))
    }

    /// Down by the detector's verdict *and* still silent: a machine that
    /// has just started talking again is not down, whatever the verdict
    /// says until it has been heard from a few times (a link that comes
    /// back must not start a takeover on the strength of an old verdict).
    pub fn down_and_silent(&self, now: Instant) -> bool {
        self.state.is_down()
            && (self.explicit || self.silence(now) >= self.threshold().min(Duration::from_secs(30)))
    }

    pub fn state(&self) -> Health {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Timing {
        Timing::standard()
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn silence_becomes_suspect_then_dead_after_confirmation() {
        let b = Instant::now();
        let mut d = Detector::new(t(), b);
        assert_eq!(d.poll(at(b, 5_000)), Health::Alive);
        assert_eq!(d.poll(at(b, 7_000)), Health::Suspect);
        assert_eq!(d.poll(at(b, 18_000)), Health::Suspect, "confirm window");
        assert_eq!(d.poll(at(b, 19_100)), Health::Dead);
        assert_eq!(d.downs, 1);
    }

    #[test]
    fn a_late_heartbeat_clears_suspicion() {
        let b = Instant::now();
        let mut d = Detector::new(t(), b);
        assert_eq!(d.poll(at(b, 7_000)), Health::Suspect);
        d.arrival(at(b, 10_000));
        assert_eq!(d.poll(at(b, 10_100)), Health::Alive);
        assert_eq!(d.downs, 0);
    }

    #[test]
    fn jitter_widens_the_threshold_so_no_false_death() {
        // Heartbeats whose gaps swing between 1 and 9 seconds: a fixed 6 s
        // rule would suspect constantly; the adaptive rule must never
        // declare dead.
        let b = Instant::now();
        let mut d = Detector::new(t(), b);
        let mut now = 0u64;
        let gaps = [1_000, 9_000, 2_000, 8_000, 1_500, 9_500, 3_000, 7_000];
        for round in 0..40 {
            now += gaps[round % gaps.len()];
            // Poll just before each arrival (the worst moment).
            let h = d.poll(at(b, now - 1));
            assert_ne!(h, Health::Dead, "round {round}");
            d.arrival(at(b, now));
        }
        assert_eq!(d.downs, 0);
        assert!(d.threshold() > Duration::from_secs(6));
    }

    #[test]
    fn a_dead_peer_needs_a_run_of_heartbeats_to_return() {
        let b = Instant::now();
        let mut d = Detector::new(t(), b);
        d.poll(at(b, 7_000));
        d.poll(at(b, 19_500));
        assert_eq!(d.state(), Health::Dead);
        d.arrival(at(b, 30_000));
        assert_eq!(d.state(), Health::Dead);
        d.arrival(at(b, 32_000));
        assert_eq!(d.state(), Health::Dead);
        d.arrival(at(b, 34_000));
        assert_eq!(d.state(), Health::Alive);
    }

    #[test]
    fn repeated_refusals_skip_the_confirmation_window() {
        let b = Instant::now();
        let mut d = Detector::new(t(), b);
        d.refused(at(b, 1_000));
        assert_eq!(d.poll(at(b, 1_001)), Health::Dead);
        assert_eq!(d.downs, 1);
        // It comes back by the usual run of heartbeats.
        d.arrival(at(b, 3_000));
        d.arrival(at(b, 5_000));
        d.arrival(at(b, 7_000));
        assert_eq!(d.state(), Health::Alive);
    }

    #[test]
    fn a_goodbye_counts_at_once_and_resume_forgives_our_own_sleep() {
        let b = Instant::now();
        let mut d = Detector::new(t(), b);
        d.bye(at(b, 100));
        assert_eq!(d.poll(at(b, 101)), Health::Away);
        assert!(d.state().is_down());
        // We slept for an hour: nothing is suspect after waking.
        let mut e = Detector::new(t(), b);
        e.resume(at(b, 3_600_000));
        assert_eq!(e.poll(at(b, 3_600_100)), Health::Alive);
    }
}
