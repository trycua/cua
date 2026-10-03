//! Create progress: what a sandbox create is doing right now (pulling the
//! image, booting, waiting for the guest), reported to whoever started it.
//!
//! The reporter is task-local, so the runtimes report without a progress
//! parameter on every trait method: a caller wraps its create in [`scope`]
//! and every [`report`] awaited inside it reaches that caller's sink. Outside
//! a scope [`report`] does nothing. Work spawned onto another task carries
//! the sink along with [`current`] + [`scope`].
//!
//! Phases are ordered ([`Phase::rank`]); a sink sees them in order, with
//! `fraction` set only when the step knows its size (an image pull), and
//! `bytes` when it moves data: bytes done and total and a smoothed rate,
//! throttled by a [`Meter`] to a few reports a second.

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;

/// What a create is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Choosing where it runs and resolving the image.
    Preparing,
    /// Downloading the image (container layers, a VM base image).
    Pulling,
    /// Making the instance (a container, a copy-on-write VM clone, a
    /// cloud claim).
    Creating,
    /// Starting it and waiting for the guest to come up.
    Booting,
    /// Waiting for the guest's services (cua-spacesd, declared ports).
    WaitingForServices,
    /// Connecting to it and registering it.
    Connecting,
    /// Ready.
    Ready,
}

impl Phase {
    /// The word for the phase (`pulling`, `booting`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Preparing => "preparing",
            Phase::Pulling => "pulling",
            Phase::Creating => "creating",
            Phase::Booting => "booting",
            Phase::WaitingForServices => "waiting_for_services",
            Phase::Connecting => "connecting",
            Phase::Ready => "ready",
        }
    }

    /// Parses [`Phase::as_str`].
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "preparing" => Phase::Preparing,
            "pulling" => Phase::Pulling,
            "creating" => Phase::Creating,
            "booting" => Phase::Booting,
            "waiting_for_services" => Phase::WaitingForServices,
            "connecting" => Phase::Connecting,
            "ready" => Phase::Ready,
            _ => return None,
        })
    }

    /// Order of the phases (0 first).
    pub fn rank(self) -> u8 {
        self as u8
    }
}

/// One progress report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// The phase.
    pub phase: Phase,
    /// How far through this phase, 0.0 to 1.0, when the step knows its size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    /// One short line on what is happening ("ghcr.io/trycua/linux:24.04").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Bytes moved so far, when the step transfers data it can count (an
    /// image download).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Transfer>,
    /// What is being created, when the caller knows (the Spaces layer sets
    /// the id the Space will have, `local:<name>`, so a UI can cancel it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
}

/// Bytes of a transfer: how many are done, of how many, and how fast.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transfer {
    /// Bytes done.
    pub done: u64,
    /// Bytes in all (0: not known yet).
    pub total: u64,
    /// Smoothed bytes per second, once there is a rate to report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_second: Option<f64>,
}

impl Transfer {
    /// "4.2 of 22.1 GB · 85 MB/s · about 4 min": bytes (binary units, as
    /// the apps size images), then the rate and the time left when known.
    pub fn describe(&self) -> String {
        let mut out = if self.total > 0 {
            let unit = unit_for(self.total);
            format!(
                "{} of {} {}",
                scaled(self.done, unit, 100.0),
                scaled(self.total, unit, 100.0),
                unit.1
            )
        } else {
            let unit = unit_for(self.done);
            format!("{} {}", scaled(self.done, unit, 100.0), unit.1)
        };
        if let Some(r) = self.per_second.filter(|r| r.is_finite() && *r > 0.0) {
            let unit = unit_for(r as u64);
            out.push_str(&format!(
                " \u{b7} {} {}/s",
                scaled(r as u64, unit, 10.0),
                unit.1
            ));
        }
        if let Some(eta) = self.eta_secs() {
            out.push_str(&format!(" \u{b7} {}", time_left(eta)));
        }
        out
    }

    /// Seconds left at the current rate, when both are known.
    pub fn eta_secs(&self) -> Option<f64> {
        let rate = self.per_second.filter(|r| r.is_finite() && *r > 0.0)?;
        (self.total > 0).then(|| self.total.saturating_sub(self.done) as f64 / rate)
    }
}

impl Progress {
    /// A phase with no size and no detail.
    pub fn phase(phase: Phase) -> Self {
        Self {
            phase,
            fraction: None,
            detail: String::new(),
            bytes: None,
            target: String::new(),
        }
    }

    /// With byte counts.
    pub fn bytes(mut self, bytes: Transfer) -> Self {
        self.bytes = Some(bytes);
        self
    }

    /// With a detail line.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// With a fraction (clamped to 0.0..=1.0; NaN is dropped).
    pub fn fraction(mut self, fraction: f64) -> Self {
        self.fraction = (!fraction.is_nan()).then(|| fraction.clamp(0.0, 1.0));
        self
    }
}

/// The unit a count reads best in: (bytes per unit, name). Binary units
/// labeled GB/MB/KB, as the apps size disks, memory and downloads.
fn unit_for(n: u64) -> (f64, &'static str) {
    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    match n {
        n if n >= GIB => (GIB as f64, "GB"),
        n if n >= MIB => (MIB as f64, "MB"),
        n if n >= KIB => (KIB as f64, "KB"),
        _ => (1.0, "bytes"),
    }
}

/// `n` in `unit`: one decimal under `below` of it, whole numbers above.
fn scaled(n: u64, unit: (f64, &str), below: f64) -> String {
    let v = n as f64 / unit.0;
    if unit.0 > 1.0 && v < below {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

/// "about 4 min", "less than a minute", "about 1 h 20 min".
pub fn time_left(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return String::new();
    }
    if secs < 60.0 {
        return "less than a minute".into();
    }
    let mins = (secs / 60.0).round() as u64;
    if mins < 60 {
        format!("about {mins} min")
    } else if mins.is_multiple_of(60) {
        format!("about {} h", mins / 60)
    } else {
        format!("about {} h {} min", mins / 60, mins % 60)
    }
}

/// Where reports go.
pub type Sink = Arc<dyn Fn(&Progress) + Send + Sync>;

tokio::task_local! {
    static SINK: Sink;
}

/// Runs `fut` with `sink` receiving every [`report`] made inside it.
pub async fn scope<F: Future>(sink: Sink, fut: F) -> F::Output {
    SINK.scope(sink, fut).await
}

/// Runs `fut` inside the current scope's sink, if any (for work handed to
/// another task).
pub async fn carry<F: Future>(sink: Option<Sink>, fut: F) -> F::Output {
    match sink {
        Some(sink) => scope(sink, fut).await,
        None => fut.await,
    }
}

/// The sink of the scope this task runs in.
pub fn current() -> Option<Sink> {
    SINK.try_with(Clone::clone).ok()
}

/// Reports to the current scope's sink; a no-op outside one.
pub fn report(progress: Progress) {
    let _ = SINK.try_with(|sink| sink(&progress));
}

/// Turns a stream of byte counts into [`Transfer`] reports: at most one per
/// [`Meter::INTERVAL`] (the first and the last always pass), with a rate
/// smoothed over the recent samples so the time left does not jump around.
#[derive(Debug, Clone)]
pub struct Meter {
    last_report: Option<std::time::Instant>,
    sample: Option<(std::time::Instant, u64)>,
    rate: Option<f64>,
    /// The bytes of the last report.
    reported: Option<u64>,
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

impl Meter {
    /// Shortest time between two reports (about 6 a second).
    pub const INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);
    /// The shortest window a rate sample is taken over.
    const SAMPLE: std::time::Duration = std::time::Duration::from_millis(500);
    /// Weight of a new rate sample.
    const SMOOTHING: f64 = 0.3;

    /// A meter with nothing counted yet.
    pub fn new() -> Self {
        Self {
            last_report: None,
            sample: None,
            rate: None,
            reported: None,
        }
    }

    /// Records `done` of `total` bytes at `now`; returns the transfer to
    /// report when one is due (the first sample, every [`Meter::INTERVAL`],
    /// and completion).
    pub fn sample_at(
        &mut self,
        now: std::time::Instant,
        done: u64,
        total: u64,
    ) -> Option<Transfer> {
        let done = if total > 0 { done.min(total) } else { done };
        match self.sample {
            None => self.sample = Some((now, done)),
            Some((at, bytes)) => {
                let dt = now.saturating_duration_since(at);
                if done < bytes {
                    // Counted from a lower base (a retried part): restart the
                    // sample, keep the rate.
                    self.sample = Some((now, done));
                } else if dt >= Self::SAMPLE {
                    let r = (done - bytes) as f64 / dt.as_secs_f64();
                    self.rate = Some(match self.rate {
                        Some(old) => Self::SMOOTHING * r + (1.0 - Self::SMOOTHING) * old,
                        None => r,
                    });
                    self.sample = Some((now, done));
                }
            }
        }
        let finished = total > 0 && done >= total && self.reported.is_none_or(|r| r < total);
        let due = match self.last_report {
            None => true,
            Some(at) => {
                now.saturating_duration_since(at) >= Self::INTERVAL && self.reported != Some(done)
            }
        };
        if !(due || finished) {
            return None;
        }
        self.last_report = Some(now);
        self.reported = Some(done);
        Some(Transfer {
            done,
            total,
            per_second: self.rate,
        })
    }

    /// `done` of `total` with the current rate, without counting it as a
    /// report (for a report some other change is due for).
    pub fn peek(&self, done: u64, total: u64) -> Transfer {
        Transfer {
            done: if total > 0 { done.min(total) } else { done },
            total,
            per_second: self.rate,
        }
    }

    /// [`Meter::sample_at`] now.
    pub fn sample(&mut self, done: u64, total: u64) -> Option<Transfer> {
        self.sample_at(std::time::Instant::now(), done, total)
    }
}

/// Byte counts of a multi-part download (image layers), folded into one
/// fraction. Parts report `(current, total)` independently; parts whose size
/// is not known yet do not count.
#[derive(Debug, Default, Clone)]
pub struct Bytes {
    parts: std::collections::BTreeMap<String, (u64, u64)>,
}

impl Bytes {
    /// Records `part`'s progress and returns the overall fraction, when any
    /// part has a known size.
    pub fn update(&mut self, part: &str, current: u64, total: u64) -> Option<f64> {
        if total > 0 {
            self.parts
                .insert(part.to_string(), (current.min(total), total));
        }
        self.fraction()
    }

    /// Marks `part` complete (a layer that finished or already existed).
    pub fn complete(&mut self, part: &str) -> Option<f64> {
        if let Some(p) = self.parts.get_mut(part) {
            p.0 = p.1;
        }
        self.fraction()
    }

    /// `part`'s own fraction, once its size is known.
    pub fn fraction_of(&self, part: &str) -> Option<f64> {
        self.parts
            .get(part)
            .map(|(c, t)| *c as f64 / (*t).max(1) as f64)
    }

    /// Bytes done and in all, over the parts whose size is known.
    pub fn totals(&self) -> (u64, u64) {
        self.parts
            .values()
            .fold((0u64, 0u64), |(d, t), (c, n)| (d + c, t + n))
    }

    /// The overall fraction.
    pub fn fraction(&self) -> Option<f64> {
        let (done, total) = self
            .parts
            .values()
            .fold((0u64, 0u64), |(d, t), (c, n)| (d + c, t + n));
        (total > 0).then(|| done as f64 / total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn reports_reach_the_scope_and_nothing_else() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let sink: Sink = Arc::new(move |p: &Progress| s.lock().unwrap().push(p.clone()));
        report(Progress::phase(Phase::Preparing)); // outside: dropped
        scope(sink, async {
            report(Progress::phase(Phase::Pulling).fraction(0.5));
            let carried = current();
            tokio::spawn(carry(carried, async {
                report(Progress::phase(Phase::Booting));
            }))
            .await
            .unwrap();
        })
        .await;
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.iter().map(|p| p.phase).collect::<Vec<_>>(),
            [Phase::Pulling, Phase::Booting]
        );
        assert_eq!(seen[0].fraction, Some(0.5));
    }

    #[test]
    fn bytes_fold_layers() {
        let mut b = Bytes::default();
        assert_eq!(b.fraction(), None);
        assert_eq!(b.update("a", 50, 100), Some(0.5));
        assert_eq!(b.update("b", 0, 300), Some(50.0 / 400.0));
        assert_eq!(b.complete("b"), Some(350.0 / 400.0));
        assert_eq!(b.update("a", 500, 100), Some(1.0), "clamped to the total");
        assert_eq!(b.totals(), (400, 400));
    }

    #[test]
    fn a_meter_throttles_and_smooths() {
        let t0 = std::time::Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut m = Meter::new();
        let first = m.sample_at(t0, 0, 1000).expect("the first sample reports");
        assert_eq!((first.done, first.total, first.per_second), (0, 1000, None));
        assert!(m.sample_at(ms(100), 50, 1000).is_none(), "throttled");
        let r = m.sample_at(ms(600), 300, 1000).expect("due again");
        // 300 bytes over 0.6 s.
        assert_eq!(r.per_second.map(|v| v.round()), Some(500.0));
        let r = m.sample_at(ms(1100), 500, 1000).expect("due");
        // 0.3 * 400 + 0.7 * 500.
        assert_eq!(r.per_second.map(|v| v.round()), Some(470.0));
        assert_eq!(r.eta_secs().map(|s| (s * 10.0).round()), Some(11.0));
        assert!(m.sample_at(ms(1150), 700, 1000).is_none(), "throttled");
        let done = m
            .sample_at(ms(1160), 1200, 1000)
            .expect("completion always reports");
        assert_eq!(done.done, 1000, "clamped to the total");
        assert!(
            m.sample_at(ms(1170), 1000, 1000).is_none(),
            "completion reports once"
        );
        assert!(
            m.sample_at(ms(5000), 1000, 1000).is_none(),
            "nothing new, nothing to report"
        );
    }

    #[test]
    fn a_transfer_reads_as_bytes_rate_and_time_left() {
        const MIB: u64 = 1 << 20;
        let t = Transfer {
            done: 4_509_715_661,   // 4.2 GiB
            total: 23_779_654_034, // macos:26, 22.1 GiB
            per_second: Some((85 * MIB) as f64),
        };
        // 17.9 GiB left at 85 MiB/s: 216 s.
        assert_eq!(
            t.describe(),
            "4.2 of 22.1 GB \u{b7} 85 MB/s \u{b7} about 4 min"
        );
        let small = Transfer {
            done: 420 * MIB,
            total: 900 * MIB,
            per_second: None,
        };
        assert_eq!(small.describe(), "420 of 900 MB");
        let slow = Transfer {
            done: 0,
            total: 10 << 30,
            per_second: Some(900.0 * 1024.0),
        };
        assert_eq!(
            slow.describe(),
            "0.0 of 10.0 GB \u{b7} 900 KB/s \u{b7} about 3 h 14 min"
        );
        assert_eq!(time_left(30.0), "less than a minute");
        assert_eq!(time_left(3600.0), "about 1 h");
    }

    #[test]
    fn a_meter_restarts_its_sample_when_counts_go_back() {
        let t0 = std::time::Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut m = Meter::new();
        m.sample_at(t0, 400, 1000);
        // A retried part counts from a lower base: no negative rate.
        let r = m.sample_at(ms(700), 100, 1000).unwrap();
        assert_eq!(r.per_second, None);
        let r = m.sample_at(ms(1400), 450, 1000).unwrap();
        assert_eq!(r.per_second.map(|v| v.round()), Some(500.0));
    }

    #[test]
    fn phase_words_round_trip_in_order() {
        let all = [
            Phase::Preparing,
            Phase::Pulling,
            Phase::Creating,
            Phase::Booting,
            Phase::WaitingForServices,
            Phase::Connecting,
            Phase::Ready,
        ];
        for (i, p) in all.iter().enumerate() {
            assert_eq!(Phase::parse(p.as_str()), Some(*p));
            assert_eq!(p.rank() as usize, i);
        }
        assert_eq!(
            Progress::phase(Phase::Pulling).fraction(f64::NAN).fraction,
            None
        );
    }
}
