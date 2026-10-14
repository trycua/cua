//! Keeping a host-served link to a Space alive across guest daemon restarts.
//!
//! The Cua Volume and the network hotspot are served from this host over a
//! socket the guest's `cua-spacesd` holds. When that daemon restarts (an
//! update, a crash, a reboot) the socket closes, the guest drops the mount
//! and the proxy, and nothing on this side starts them again. [`supervise`]
//! watches one such link and re-creates it, backing off while the guest is
//! still coming up.

use std::future::Future;
use std::time::Duration;

/// What a link looks like right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Served and the guest agrees.
    Healthy,
    /// The socket closed, or the guest lost its half (a restarted daemon).
    Down,
    /// Nobody wants it any more (stopped, released, replaced): stop
    /// supervising.
    Gone,
}

/// What a re-attach attempt did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// Attached; keep watching.
    Attached,
    /// Nothing to attach (the guest has no such backend): stop supervising.
    Unsupported,
}

/// Timing of [`supervise`].
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// How often the link is checked.
    pub poll: Duration,
    /// The wait after the first failed attempt.
    pub initial_backoff: Duration,
    /// The wait never grows past this.
    pub max_backoff: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(3),
            initial_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(60),
        }
    }
}

/// Exponential backoff between failed attempts.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    initial: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(policy: &Policy) -> Self {
        Self {
            initial: policy.initial_backoff,
            max: policy.max_backoff,
            next: policy.initial_backoff,
        }
    }

    /// The wait after a failure; the one after that doubles, up to the cap.
    pub fn fail(&mut self) -> Duration {
        let wait = self.next;
        self.next = (self.next * 2).min(self.max);
        wait
    }

    /// An attempt worked (or the link was healthy): start over.
    pub fn reset(&mut self) {
        self.next = self.initial;
    }
}

/// Checks a link every `policy.poll` and calls `reattach` while it is
/// [`Health::Down`], waiting out an exponential backoff after each failure.
/// The first check is immediate, so a link that was never made is made at
/// once. Returns when `health` says [`Health::Gone`] or `reattach` says
/// [`Attempt::Unsupported`].
pub async fn supervise<H, HF, R, RF>(policy: Policy, mut health: H, mut reattach: R)
where
    H: FnMut() -> HF,
    HF: Future<Output = Health>,
    R: FnMut() -> RF,
    RF: Future<Output = Result<Attempt, String>>,
{
    let mut backoff = Backoff::new(&policy);
    let mut retry_at: Option<tokio::time::Instant> = None;
    loop {
        match health().await {
            Health::Gone => return,
            Health::Healthy => {
                backoff.reset();
                retry_at = None;
            }
            Health::Down => {
                if retry_at.is_none_or(|t| tokio::time::Instant::now() >= t) {
                    match reattach().await {
                        Ok(Attempt::Attached) => {
                            backoff.reset();
                            retry_at = None;
                        }
                        Ok(Attempt::Unsupported) => return,
                        Err(why) => {
                            let wait = backoff.fail();
                            tracing::warn!(error = %why, retry_in = ?wait, "re-attach failed");
                            retry_at = Some(tokio::time::Instant::now() + wait);
                        }
                    }
                }
            }
        }
        tokio::time::sleep(policy.poll).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn policy() -> Policy {
        Policy {
            poll: Duration::from_secs(1),
            initial_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(8),
        }
    }

    #[test]
    fn backoff_doubles_to_the_cap_and_resets() {
        let mut b = Backoff::new(&policy());
        let waits: Vec<_> = (0..5).map(|_| b.fail().as_secs()).collect();
        assert_eq!(waits, [2, 4, 8, 8, 8]);
        b.reset();
        assert_eq!(b.fail().as_secs(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_that_goes_down_is_reattached_once() {
        // Healthy for two polls, down for one, then healthy again.
        let polls = Arc::new(AtomicUsize::new(0));
        let attaches = Arc::new(AtomicUsize::new(0));
        let (p, a) = (polls.clone(), attaches.clone());
        supervise(
            policy(),
            move || {
                let n = p.fetch_add(1, Ordering::SeqCst);
                let a = a.load(Ordering::SeqCst);
                async move {
                    match n {
                        0 | 1 => Health::Healthy,
                        _ if a == 0 => Health::Down,
                        2..=5 => Health::Healthy,
                        _ => Health::Gone,
                    }
                }
            },
            {
                let a = attaches.clone();
                move || {
                    a.fetch_add(1, Ordering::SeqCst);
                    async { Ok(Attempt::Attached) }
                }
            },
        )
        .await;
        assert_eq!(attaches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failures_back_off_and_a_guest_that_returns_is_attached() {
        let start = tokio::time::Instant::now();
        let times = Arc::new(std::sync::Mutex::new(Vec::new()));
        let t = times.clone();
        supervise(
            policy(),
            || async { Health::Down },
            move || {
                let t = t.clone();
                async move {
                    let mut t = t.lock().unwrap();
                    t.push(start.elapsed().as_secs());
                    // The guest daemon is back on the fourth try.
                    if t.len() < 4 {
                        Err("connection refused".to_string())
                    } else {
                        Ok(Attempt::Unsupported)
                    }
                }
            },
        )
        .await;
        let t = times.lock().unwrap().clone();
        assert_eq!(t.len(), 4);
        // Immediate, then waits of 2 s, 4 s, 8 s (rounded up to the 1 s poll).
        assert_eq!(t[0], 0);
        assert!((2..=3).contains(&(t[1] - t[0])), "{t:?}");
        assert!((4..=5).contains(&(t[2] - t[1])), "{t:?}");
        assert!((8..=9).contains(&(t[3] - t[2])), "{t:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_healthy_link_is_left_alone_and_gone_stops() {
        let polls = Arc::new(AtomicUsize::new(0));
        let p = polls.clone();
        supervise(
            policy(),
            move || {
                let n = p.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 10 {
                        Health::Healthy
                    } else {
                        Health::Gone
                    }
                }
            },
            || async { panic!("a healthy link must not be re-attached") },
        )
        .await;
    }
}
