//! Keep Driver-owned overlay pixels out of Driver-owned desktop captures.
//!
//! The agent cursor and its session pill are drawn in a Driver-owned overlay
//! window that sits above the desktop. A desktop capture taken for the agent
//! must not contain those pixels: after a click the cursor rests on the
//! target, so a capture would bake the arrow and the pill over the very label
//! the agent is trying to read.
//!
//! The exclusion is scoped to the Driver's own agent-facing capture. External
//! recorders and the Driver's recording feature keep seeing the cursor, so no
//! adapter may exclude the overlay from capture permanently.
//!
//! This module owns the cross-platform semantics:
//!
//! - one desktop capture excludes overlays at a time, so one capture can never
//!   restore the overlay while another is still reading the screen;
//! - the overlay is restored exactly once after every exclusion, even when the
//!   capture fails or panics;
//! - the result always says what happened ([`AgentOverlayCapture`]). A
//!   platform that cannot exclude still returns the capture, but reports
//!   `not_excluded` with a reason instead of pretending the pixels are clean.
//!
//! Adapters implement [`OverlayCaptureExcluder`] with the native mechanism
//! (window display affinity, an emptied X11 shape, a ScreenCaptureKit filter)
//! and nothing else.

use std::sync::Mutex;

pub use cua_driver_contract::{AgentOverlayCapture, AgentOverlayCaptureStatus};

/// Outcome of asking a platform to keep its overlays out of the next capture.
#[derive(Debug)]
pub enum ExclusionStart<H> {
    /// No Driver overlay pixels are on screen; the capture runs as is.
    NotPresent,
    /// The overlay is out of the captures that follow until
    /// [`OverlayCaptureExcluder::restore`] runs. `hidden` carries whatever the
    /// adapter needs to verify the capture or to restore the overlay.
    Excluded { method: &'static str, hidden: H },
    /// The overlay may be on screen and could not be kept out.
    Unsupported { reason: String },
}

/// What an adapter found when it inspected the captured pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidualCheck {
    /// Nothing of the overlay is left in the capture.
    Clean,
    /// Overlay pixels were still present and were replaced with the desktop
    /// the overlay last saw under them. `method` names that repair.
    Repaired { method: &'static str },
    /// Overlay pixels are still present in the capture.
    Residual { reason: String },
}

/// Native mechanism that hides or excludes a platform's overlay windows.
pub trait OverlayCaptureExcluder {
    /// Adapter state carried from [`Self::exclude`] to the capture and to
    /// [`Self::restore`].
    type Hidden;

    /// Keep every Driver overlay out of the captures that follow.
    fn exclude(&self) -> ExclusionStart<Self::Hidden>;

    /// Undo a successful [`Self::exclude`]. Called exactly once per
    /// `Excluded` start, after the capture, whether or not it succeeded.
    fn restore(&self, hidden: Self::Hidden);
}

/// Serializes overlay exclusion across concurrent desktop captures.
static EXCLUSION_GATE: Mutex<()> = Mutex::new(());

/// Run `capture` with every Driver overlay kept out of the screen pixels and
/// report what happened.
///
/// `capture` receives the adapter state when the overlay was excluded, and
/// returns the capture plus its own inspection of the pixels. An inspection is
/// only meaningful after an exclusion; for a capture that ran without one the
/// check is ignored.
pub fn capture_excluding_overlays<X, T, E>(
    excluder: &X,
    capture: impl FnOnce(Option<&X::Hidden>) -> Result<(T, ResidualCheck), E>,
) -> Result<(T, AgentOverlayCapture), E>
where
    X: OverlayCaptureExcluder,
{
    // A poisoned gate only means another capture panicked while holding it;
    // its restore already ran from the guard below, so the gate is still sound.
    let _gate = EXCLUSION_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    match excluder.exclude() {
        ExclusionStart::NotPresent => {
            let (value, _) = capture(None)?;
            Ok((value, AgentOverlayCapture::not_present()))
        }
        ExclusionStart::Unsupported { reason } => {
            let (value, _) = capture(None)?;
            Ok((value, AgentOverlayCapture::not_excluded(reason)))
        }
        ExclusionStart::Excluded { method, hidden } => {
            let restore = RestoreOnDrop {
                excluder,
                hidden: Some(hidden),
            };
            let result = capture(restore.hidden.as_ref());
            drop(restore);
            let (value, check) = result?;
            let report = match check {
                ResidualCheck::Clean => AgentOverlayCapture::excluded(method),
                ResidualCheck::Repaired { method: repair } => {
                    AgentOverlayCapture::excluded(format!("{method}+{repair}"))
                }
                ResidualCheck::Residual { reason } => AgentOverlayCapture::not_excluded(reason),
            };
            Ok((value, report))
        }
    }
}

/// Restores the overlay when the capture returns or unwinds.
struct RestoreOnDrop<'a, X: OverlayCaptureExcluder> {
    excluder: &'a X,
    hidden: Option<X::Hidden>,
}

impl<X: OverlayCaptureExcluder> Drop for RestoreOnDrop<'_, X> {
    fn drop(&mut self) {
        if let Some(hidden) = self.hidden.take() {
            self.excluder.restore(hidden);
        }
    }
}

/// Append the one-line caveat an agent reading only the text content needs:
/// nothing for a clean capture, the reason when overlay pixels may be present.
pub fn append_summary_note(summary: &mut String, report: &AgentOverlayCapture) {
    if report.is_clean() {
        return;
    }
    summary.push_str("; the Driver's agent cursor overlay may appear in this image");
    if let Some(reason) = &report.reason {
        summary.push_str(" (");
        summary.push_str(reason);
        summary.push(')');
    }
}

/// Excluder for builds and platforms with no overlay window at all.
pub struct NoOverlay;

impl OverlayCaptureExcluder for NoOverlay {
    type Hidden = ();

    fn exclude(&self) -> ExclusionStart<()> {
        ExclusionStart::NotPresent
    }

    fn restore(&self, _hidden: ()) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct Counters {
        excluded: AtomicUsize,
        restored: AtomicUsize,
        hidden_now: AtomicUsize,
        max_hidden: AtomicUsize,
    }

    enum Mode {
        Present,
        Absent,
        Unsupported,
    }

    struct Fake {
        mode: Mode,
        counters: Arc<Counters>,
    }

    impl Fake {
        fn new(mode: Mode) -> Self {
            Self {
                mode,
                counters: Arc::default(),
            }
        }
    }

    impl OverlayCaptureExcluder for Fake {
        type Hidden = u32;

        fn exclude(&self) -> ExclusionStart<u32> {
            match self.mode {
                Mode::Present => {
                    self.counters.excluded.fetch_add(1, Ordering::SeqCst);
                    let now = self.counters.hidden_now.fetch_add(1, Ordering::SeqCst) + 1;
                    self.counters.max_hidden.fetch_max(now, Ordering::SeqCst);
                    ExclusionStart::Excluded {
                        method: "fake_hide",
                        hidden: 7,
                    }
                }
                Mode::Absent => ExclusionStart::NotPresent,
                Mode::Unsupported => ExclusionStart::Unsupported {
                    reason: "fake platform cannot hide".into(),
                },
            }
        }

        fn restore(&self, hidden: u32) {
            assert_eq!(hidden, 7, "restore receives the state exclude produced");
            self.counters.hidden_now.fetch_sub(1, Ordering::SeqCst);
            self.counters.restored.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn ok_clean(
        value: &'static str,
    ) -> impl FnOnce(Option<&u32>) -> Result<(&'static str, ResidualCheck), String> {
        move |_| Ok((value, ResidualCheck::Clean))
    }

    #[test]
    fn excluded_capture_restores_once_and_reports_the_method() {
        let fake = Fake::new(Mode::Present);
        let (value, report) = capture_excluding_overlays(&fake, |hidden| {
            assert_eq!(hidden, Some(&7));
            assert_eq!(fake.counters.restored.load(Ordering::SeqCst), 0);
            Ok::<_, String>(("png", ResidualCheck::Clean))
        })
        .unwrap();
        assert_eq!(value, "png");
        assert_eq!(report, AgentOverlayCapture::excluded("fake_hide"));
        assert!(report.is_clean());
        assert_eq!(fake.counters.excluded.load(Ordering::SeqCst), 1);
        assert_eq!(fake.counters.restored.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_capture_still_restores_the_overlay() {
        let fake = Fake::new(Mode::Present);
        let error = capture_excluding_overlays(&fake, |_| {
            Err::<(&str, ResidualCheck), _>("grab failed".to_string())
        })
        .unwrap_err();
        assert_eq!(error, "grab failed");
        assert_eq!(fake.counters.restored.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn panicking_capture_still_restores_the_overlay() {
        let fake = Fake::new(Mode::Present);
        let counters = Arc::clone(&fake.counters);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = capture_excluding_overlays(&fake, |_| -> Result<((), ResidualCheck), ()> {
                panic!("capture backend panicked")
            });
        }));
        assert!(outcome.is_err());
        assert_eq!(counters.restored.load(Ordering::SeqCst), 1);
        // The gate is usable again after the panic.
        let (_, report) = capture_excluding_overlays(&fake, ok_clean("again")).unwrap();
        assert_eq!(report.status, AgentOverlayCaptureStatus::Excluded);
        assert_eq!(counters.restored.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn absent_overlay_reports_not_present_without_restoring() {
        let fake = Fake::new(Mode::Absent);
        let (_, report) = capture_excluding_overlays(&fake, |hidden| {
            assert!(hidden.is_none());
            // A residual verdict without an exclusion carries no meaning.
            Ok::<_, String>((
                (),
                ResidualCheck::Residual {
                    reason: "ignored".into(),
                },
            ))
        })
        .unwrap();
        assert_eq!(report, AgentOverlayCapture::not_present());
        assert!(report.is_clean());
        assert_eq!(fake.counters.restored.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unsupported_platform_captures_and_publishes_the_limitation() {
        let fake = Fake::new(Mode::Unsupported);
        let (value, report) = capture_excluding_overlays(&fake, ok_clean("png")).unwrap();
        assert_eq!(value, "png");
        assert_eq!(
            report,
            AgentOverlayCapture::not_excluded("fake platform cannot hide")
        );
        assert!(!report.is_clean());
        assert_eq!(fake.counters.restored.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn residual_pixels_downgrade_the_report() {
        let fake = Fake::new(Mode::Present);
        let (_, report) = capture_excluding_overlays(&fake, |_| {
            Ok::<_, String>((
                (),
                ResidualCheck::Residual {
                    reason: "window did not repaint".into(),
                },
            ))
        })
        .unwrap();
        assert_eq!(
            report,
            AgentOverlayCapture::not_excluded("window did not repaint")
        );
        assert_eq!(fake.counters.restored.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn repaired_pixels_name_both_mechanisms() {
        let fake = Fake::new(Mode::Present);
        let (_, report) = capture_excluding_overlays(&fake, |_| {
            Ok::<_, String>((
                (),
                ResidualCheck::Repaired {
                    method: "save_under",
                },
            ))
        })
        .unwrap();
        assert_eq!(
            report,
            AgentOverlayCapture::excluded("fake_hide+save_under")
        );
    }

    #[test]
    fn concurrent_captures_never_overlap_their_exclusions() {
        let fake = Arc::new(Fake::new(Mode::Present));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let fake = Arc::clone(&fake);
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        capture_excluding_overlays(fake.as_ref(), |_| {
                            std::thread::yield_now();
                            Ok::<_, String>(((), ResidualCheck::Clean))
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(fake.counters.excluded.load(Ordering::SeqCst), 200);
        assert_eq!(fake.counters.restored.load(Ordering::SeqCst), 200);
        assert_eq!(fake.counters.max_hidden.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn summary_note_is_added_only_when_overlay_pixels_may_remain() {
        let mut clean = String::from("desktop screenshot");
        append_summary_note(&mut clean, &AgentOverlayCapture::excluded("m"));
        append_summary_note(&mut clean, &AgentOverlayCapture::not_present());
        assert_eq!(clean, "desktop screenshot");

        let mut caveat = String::from("desktop screenshot");
        append_summary_note(&mut caveat, &AgentOverlayCapture::not_excluded("no API"));
        assert_eq!(
            caveat,
            "desktop screenshot; the Driver's agent cursor overlay may appear in this image (no API)"
        );
    }

    #[test]
    fn no_overlay_excluder_reports_not_present() {
        let (_, report) =
            capture_excluding_overlays(&NoOverlay, |_| Ok::<_, String>(((), ResidualCheck::Clean)))
                .unwrap();
        assert_eq!(report.status, AgentOverlayCaptureStatus::NotPresent);
    }
}
