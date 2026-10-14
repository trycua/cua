// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Keep a platform overlay surface fitted to the live display geometry.
//!
//! Every cursor position, glide target and drag-tracking point is a global
//! screen coordinate. A platform overlay draws them into a surface that covers
//! a display frame, so the surface is only correct while that frame matches
//! the display. When the display is reconfigured after the overlay started (a
//! resolution switch, a VM display resize, a monitor attach), a surface still
//! fitted to the old frame draws every cursor displaced by the difference and
//! clips points outside the old bounds. A drag then shows the agent cursor
//! gliding along the pointer's path at a constant offset, which reads as the
//! cursor sticking to the dragged window instead of the pointer.
//!
//! [`SurfaceFit`] owns that contract once for every adapter. An adapter reads
//! its display geometry whenever [`SurfaceFit::due`] allows, hands the reading
//! to [`SurfaceFit::observe`], and refits its native surface (window frame,
//! pixmap size, backing scale) only when a new geometry comes back. Adapters
//! with display-change events (X11 RandR, Wayland output configure) may feed
//! those readings instead of polling.

use std::time::{Duration, Instant};

use crate::ScreenFrame;

/// How often a polling adapter re-reads the display geometry while its render
/// loop runs. Display reconfiguration is rare; this bounds the stale window to
/// a fraction of a second without adding a system call to every frame.
pub const SURFACE_REFIT_INTERVAL: Duration = Duration::from_millis(250);

/// Display geometry an overlay surface covers: the frame in global screen
/// points and the backing scale (device pixels per point).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGeometry {
    pub frame: ScreenFrame,
    pub scale: f64,
}

impl SurfaceGeometry {
    pub fn new(frame: ScreenFrame, scale: f64) -> Self {
        Self { frame, scale }
    }

    /// A reading an adapter may apply: finite, non-empty, positive scale.
    /// A display that is mid-reconfiguration can briefly report zero bounds;
    /// fitting the surface to that would hide every cursor.
    pub fn is_usable(&self) -> bool {
        let f = self.frame;
        [f.x, f.y, f.width, f.height, self.scale]
            .iter()
            .all(|v| v.is_finite())
            && f.width >= 1.0
            && f.height >= 1.0
            && self.scale > 0.0
    }

    /// Surface-local point, in points, for a global screen point.
    pub fn surface_point(&self, x: f64, y: f64) -> (f64, f64) {
        (x - self.frame.x, y - self.frame.y)
    }

    /// Surface size in device pixels, at least 1x1.
    pub fn pixel_size(&self) -> (u32, u32) {
        let scale = self.scale.max(1.0);
        (
            (self.frame.width * scale).round().max(1.0) as u32,
            (self.frame.height * scale).round().max(1.0) as u32,
        )
    }
}

/// The geometry an overlay surface was last fitted to, plus the poll cadence.
#[derive(Debug, Clone)]
pub struct SurfaceFit {
    applied: Option<SurfaceGeometry>,
    last_check: Option<Instant>,
    interval: Duration,
}

impl SurfaceFit {
    /// A fit for a surface created with `initial` (`None` when the adapter
    /// has not created its surface yet).
    pub fn new(initial: Option<SurfaceGeometry>) -> Self {
        Self::with_interval(initial, SURFACE_REFIT_INTERVAL)
    }

    pub fn with_interval(initial: Option<SurfaceGeometry>, interval: Duration) -> Self {
        Self {
            applied: initial.filter(SurfaceGeometry::is_usable),
            last_check: None,
            interval,
        }
    }

    /// The geometry the surface currently covers.
    pub fn applied(&self) -> Option<SurfaceGeometry> {
        self.applied
    }

    /// Whether a fresh display reading is due at `now`. The first check is
    /// always due, so a loop woken after a long idle refits before it paints.
    pub fn due(&self, now: Instant) -> bool {
        self.last_check
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval)
    }

    /// Record a display reading taken at `now`. Returns the geometry the
    /// adapter must refit its surface to, or `None` when the surface already
    /// matches or the reading is unusable (the surface keeps its last good
    /// fit rather than collapsing).
    pub fn observe(
        &mut self,
        now: Instant,
        reading: Option<SurfaceGeometry>,
    ) -> Option<SurfaceGeometry> {
        self.last_check = Some(now);
        let reading = reading.filter(SurfaceGeometry::is_usable)?;
        if self.applied == Some(reading) {
            return None;
        }
        self.applied = Some(reading);
        Some(reading)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        paint_cursor, track_pointer_command, CompiledAnimation, CompiledDrawCommand, CompiledFrame,
        CompiledGeometry, CompiledTheme, CompiledTransform, CursorAction, CursorConfig,
        RenderStateCore,
    };

    fn display(width: f64, height: f64) -> SurfaceGeometry {
        SurfaceGeometry::new(ScreenFrame::new(0.0, 0.0, width, height), 1.0)
    }

    /// A theme whose only artwork is a small disc centred on its hotspot, so
    /// the painted centroid is where the cursor points.
    fn hotspot_marker_theme() -> Arc<CompiledTheme> {
        let animation = CompiledAnimation {
            still_frame: 0,
            frames: vec![CompiledFrame {
                commands: vec![CompiledDrawCommand {
                    geometries: vec![CompiledGeometry::Ellipse {
                        center: [55.0, 30.0],
                        size: [8.0, 8.0],
                    }],
                    transform: CompiledTransform::default(),
                    opacity: 1.0,
                    fill: Some([255, 0, 0, 255]),
                    stroke: None,
                }],
            }],
        };
        Arc::new(CompiledTheme {
            id: "com.example.hotspot".into(),
            name: "Hotspot".into(),
            version: "1.0.0".into(),
            author: "Example Author".into(),
            license: "MIT".into(),
            profile: crate::THEME_PROFILE.into(),
            source_hash: [0; 32],
            hotspot: [55, 30],
            actions: CursorAction::ALL
                .into_iter()
                .map(|action| (action.as_str().to_owned(), animation.clone()))
                .collect(),
        })
    }

    /// Paint one cursor tracking the pointer at `pointer` into a surface
    /// fitted to `surface`, as every adapter's render loop does, and return
    /// the painted hotspot in global screen points (`None` when clipped).
    fn painted_pointer(surface: SurfaceGeometry, pointer: (f64, f64)) -> Option<(f64, f64)> {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.theme = Some(hotspot_marker_theme());
        core.apply_command_base(track_pointer_command(pointer.0, pointer.1), false, false);
        let (w, h) = surface.pixel_size();
        let mut pixmap = tiny_skia::Pixmap::new(w, h).unwrap();
        paint_cursor(
            &mut pixmap,
            &core,
            surface.frame.x,
            surface.frame.y,
            None,
            surface.scale as f32,
        );
        let (weight, x_sum, y_sum) = pixmap.data().chunks_exact(4).enumerate().fold(
            (0.0, 0.0, 0.0),
            |(weight, x_sum, y_sum), (index, pixel)| {
                let alpha = f64::from(pixel[3]);
                let x = (index % w as usize) as f64 + 0.5;
                let y = (index / w as usize) as f64 + 0.5;
                (weight + alpha, x_sum + x * alpha, y_sum + y * alpha)
            },
        );
        (weight > 0.0).then(|| {
            (
                x_sum / weight / surface.scale + surface.frame.x,
                y_sum / weight / surface.scale + surface.frame.y,
            )
        })
    }

    /// The owner test for the shared contract: after the display grows from
    /// 1024x768 to 1440x900 while the overlay runs (the Lume guest resize that
    /// made a tab-strip drag show the cursor ~132 pt below the pointer), the
    /// refitted surface draws each drag-tracking point on the pointer,
    /// including points outside the old bounds that a stale surface clips.
    #[test]
    fn refit_after_display_resize_keeps_tracked_pointer_on_the_pointer() {
        let started = Instant::now();
        let stale = display(1024.0, 768.0);
        let mut fit = SurfaceFit::new(Some(stale));
        assert!(fit.due(started), "the first check is always due");
        assert_eq!(fit.observe(started, Some(stale)), None);

        let later = started + SURFACE_REFIT_INTERVAL;
        assert!(fit.due(later));
        let refit = fit
            .observe(later, Some(display(1440.0, 900.0)))
            .expect("a resized display refits the surface");
        assert_eq!(refit, display(1440.0, 900.0));
        assert_eq!(fit.applied(), Some(refit));

        // A drag across the top of the resized display: start, midpoint and
        // drop point.
        for pointer in [(1176.0, 48.0), (825.0, 57.0), (474.0, 66.0)] {
            let (x, y) = painted_pointer(refit, pointer).expect("the cursor is painted");
            assert!(
                (x - pointer.0).abs() <= 0.5 && (y - pointer.1).abs() <= 0.5,
                "cursor drawn at ({x:.2}, {y:.2}) for pointer {pointer:?}"
            );
        }
        // The stale surface cannot show the drag start at all.
        assert_eq!(painted_pointer(stale, (1176.0, 48.0)), None);
    }

    #[test]
    fn unchanged_or_unusable_readings_keep_the_last_fit() {
        let now = Instant::now();
        let mut fit = SurfaceFit::new(Some(display(1440.0, 900.0)));
        assert_eq!(fit.observe(now, Some(display(1440.0, 900.0))), None);
        assert_eq!(fit.observe(now, None), None);
        assert_eq!(fit.observe(now, Some(display(0.0, 0.0))), None);
        assert_eq!(fit.observe(now, Some(display(f64::NAN, 900.0))), None);
        let zero_scale = SurfaceGeometry::new(ScreenFrame::new(0.0, 0.0, 1440.0, 900.0), 0.0);
        assert_eq!(fit.observe(now, Some(zero_scale)), None);
        assert_eq!(fit.applied(), Some(display(1440.0, 900.0)));
    }

    #[test]
    fn scale_or_origin_change_refits_and_checks_are_throttled() {
        let start = Instant::now();
        let mut fit = SurfaceFit::new(None);
        assert_eq!(fit.applied(), None);
        let retina = SurfaceGeometry::new(ScreenFrame::new(0.0, 0.0, 1440.0, 900.0), 2.0);
        assert_eq!(fit.observe(start, Some(retina)), Some(retina));
        assert!(!fit.due(start + SURFACE_REFIT_INTERVAL / 2));
        assert!(fit.due(start + SURFACE_REFIT_INTERVAL));

        let virtual_screen =
            SurfaceGeometry::new(ScreenFrame::new(-1920.0, 0.0, 3360.0, 1080.0), 2.0);
        let later = start + SURFACE_REFIT_INTERVAL;
        assert_eq!(
            fit.observe(later, Some(virtual_screen)),
            Some(virtual_screen)
        );
        assert_eq!(virtual_screen.surface_point(-1900.0, 10.0), (20.0, 10.0));
        assert_eq!(virtual_screen.pixel_size(), (6720, 2160));
        // A surface whose origin is not the global origin (a secondary
        // display) draws the global pointer at the same place.
        let offset = SurfaceGeometry::new(ScreenFrame::new(1440.0, 100.0, 1600.0, 600.0), 2.0);
        let (x, y) = painted_pointer(offset, (1700.0, 400.0)).unwrap();
        assert!((x - 1700.0).abs() <= 0.5 && (y - 400.0).abs() <= 0.5);
    }
}
