// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Poses for a cursor surface that draws and animates the cursor itself.
//!
//! The GNOME Shell helper paints the agent cursor inside the compositor, so
//! it cannot run [`crate::paint_cursor`]. It still gets the same motion: a
//! [`PoseModel`] keeps one [`RenderStateCore`] per session, advanced by the
//! wall clock, and every planned move becomes a [`PoseTrack`] of hotspot
//! position, float and rotation sampled from that core. The helper only plays
//! the samples back, then runs the resting float described by [`PoseLook`]
//! until the next move. The planner, the float, the rotation modes and the
//! halo colour therefore come from one place for every backend.

use std::time::Instant;

use serde_json::json;

use crate::{
    CursorConfig, CursorKey, KeyedOverlayCommand, OverlayCommand, OverlayMsg, RenderMap,
    RenderStateCore,
};

/// Sample spacing of a [`PoseTrack`]. The helper interpolates between them.
pub const POSE_STEP_SECS: f64 = 1.0 / 60.0;
/// Upper bound on samples per track (the longest legal glide is 5 s; the
/// settle after it is short).
const MAX_SAMPLES: usize = 1_200;

/// One pose of the cursor: the hotspot, the float offset in points (negative
/// is up) and the rotation from upright in radians (clockwise, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseSample {
    /// Seconds from the start of the track.
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub float_dy: f64,
    pub rotation: f64,
}

/// A planned move (or a snap) as poses to play back.
#[derive(Debug, Clone, PartialEq)]
pub struct PoseTrack {
    /// In time order. The first is the start pose, the last the end pose.
    pub samples: Vec<PoseSample>,
    /// Seconds into the track at which the resting float loop restarts from
    /// 0 (the arrival, where the travel float has settled to 0). `None` for
    /// a snap: the resting loop carries on.
    pub rest_from: Option<f64>,
}

/// The halo as the surface should paint it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HaloLook {
    pub rgb: [u8; 3],
    pub radius_pt: f32,
    pub peak_alpha: f32,
    /// Gradient stops: (offset 0..1, share of `peak_alpha`).
    pub stops: [(f32, f32); 3],
    /// Centre of the arrow body from the hotspot, upright, in points.
    pub body_offset_pt: (f64, f64),
}

/// What the surface paints at rest and around the arrow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseLook {
    /// Peak lift of the resting float in points; 0 for no float.
    pub rest_lift_pt: f64,
    pub rest_secs: f64,
    /// `None` with `cursor.halo` off or on a surface that cannot blend.
    pub halo: Option<HaloLook>,
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

impl PoseTrack {
    /// Compact JSON for the helper: `{"samples": [[t, x, y, dy, rot], ...],
    /// "rest_from": t | null}`.
    pub fn to_json(&self) -> String {
        let samples: Vec<[f64; 5]> = self
            .samples
            .iter()
            .map(|s| {
                [
                    round(s.t, 4),
                    round(s.x, 2),
                    round(s.y, 2),
                    round(s.float_dy, 3),
                    round(s.rotation, 4),
                ]
            })
            .collect();
        json!({
            "samples": samples,
            "rest_from": self.rest_from.map(|t| round(t, 4)),
        })
        .to_string()
    }

    /// Seconds the hotspot takes to reach its target.
    pub fn arrival_secs(&self) -> f64 {
        self.rest_from
            .unwrap_or_else(|| self.samples.last().map_or(0.0, |s| s.t))
    }
}

impl PoseLook {
    /// JSON for the helper: `{"rest_lift": pt, "rest_secs": s, "halo":
    /// {"rgb": "#rrggbb", "radius": pt, "peak": a, "stops": [[o, k], ...],
    /// "body": [x, y]} | null}`.
    pub fn to_json(&self) -> String {
        let halo = self.halo.map(|halo| {
            let [r, g, b] = halo.rgb;
            json!({
                "rgb": format!("#{r:02x}{g:02x}{b:02x}"),
                "radius": halo.radius_pt,
                "peak": halo.peak_alpha,
                "stops": halo.stops.iter().map(|&(o, k)| [o, k]).collect::<Vec<_>>(),
                "body": [round(halo.body_offset_pt.0, 3), round(halo.body_offset_pt.1, 3)],
            })
        });
        json!({
            "rest_lift": round(self.rest_lift_pt, 3),
            "rest_secs": self.rest_secs,
            "halo": halo,
        })
        .to_string()
    }
}

impl RenderStateCore {
    /// The rest of the current planned move as poses, `step` seconds apart,
    /// plus an exact sample at the arrival. Floats and rotation come from
    /// [`Self::float_dy`] and [`Self::painted_rotation`], so they are 0 and
    /// upright at the arrival. `None` when no move is in flight.
    pub fn pose_track(&self, step: f64) -> Option<PoseTrack> {
        let traj = self.trajectory.as_ref()?;
        let step = if step.is_finite() && step > 0.0 {
            step
        } else {
            POSE_STEP_SECS
        };
        let (start, end, arrival) = (self.motion_t, traj.duration(), traj.arrival_t);
        let pose = |t: f64| {
            // The resting loop restarts at the arrival (see `tick_motion`).
            let float_secs = if t >= arrival {
                t - arrival
            } else {
                self.float_secs + (t - start)
            };
            let s = traj.sample_at(t);
            PoseSample {
                t: t - start,
                x: s.x,
                y: s.y,
                float_dy: self.float_dy_at(t, float_secs),
                rotation: self.rotation_at(t),
            }
        };
        let mut samples = Vec::new();
        let mut t = start;
        let mut arrival_added = arrival < start;
        while t < end && samples.len() < MAX_SAMPLES {
            if !arrival_added && t >= arrival {
                if t > arrival {
                    samples.push(pose(arrival));
                }
                arrival_added = true;
            }
            samples.push(pose(t));
            t += step;
        }
        if !arrival_added && arrival < end {
            samples.push(pose(arrival));
        }
        samples.push(pose(end.max(start)));
        Some(PoseTrack {
            samples,
            rest_from: Some((arrival - start).max(0.0)),
        })
    }

    /// The resting float and halo this cursor paints with now.
    pub fn pose_look(&self) -> PoseLook {
        // A cursor that never hides rests still, like `has_resting_motion`.
        let rests_floating = self.floats() && self.motion.idle_hide_ms > 0.0;
        let options = self.options();
        PoseLook {
            rest_lift_pt: if rests_floating {
                crate::cursor_pose::REST_FLOAT_LIFT_PT
            } else {
                0.0
            },
            rest_secs: crate::cursor_pose::REST_FLOAT_SECS,
            halo: (options.halo && self.effects_capable).then(|| HaloLook {
                rgb: crate::cursor_pose::halo_rgb(self.theme.as_deref(), &self.cfg.cursor_id),
                radius_pt: crate::cursor_pose::HALO_RADIUS_PT,
                peak_alpha: crate::cursor_pose::HALO_PEAK_ALPHA,
                stops: crate::cursor_pose::HALO_STOPS,
                body_offset_pt: crate::cursor_pose::BODY_OFFSET_PT,
            }),
        }
    }

    fn hotspot(&self) -> (f64, f64) {
        crate::pointer_for_anchor(self.pos.0, self.pos.1, self.heading)
    }
}

/// Per-session cursor state for a self-animating surface, advanced by the
/// wall clock instead of a render loop.
pub struct PoseModel {
    map: RenderMap<RenderStateCore>,
    last_tick: Option<Instant>,
    /// Hotspot the surface last showed, so a session drawing on it for the
    /// first time starts from there instead of jumping.
    surface_hotspot: Option<(f64, f64)>,
}

impl PoseModel {
    pub fn new(template: CursorConfig) -> Self {
        Self {
            map: RenderMap::new(template, ()),
            last_tick: None,
            surface_hotspot: None,
        }
    }

    fn advance(&mut self, now: Instant) {
        if let Some(last) = self.last_tick {
            let dt = now.saturating_duration_since(last).as_secs_f64();
            if dt > 0.0 {
                self.map.tick_all(dt);
            }
        }
        self.last_tick = Some(now);
    }

    /// `key`'s cursor, created when absent. `None` for an ended session.
    pub fn cursor_mut(&mut self, key: &str) -> Option<&mut RenderStateCore> {
        self.map.cursor_mut(key)
    }

    /// Apply one overlay message at `now`. `draws` says whether it reaches
    /// the surface. Returns the poses to play for a move or snap that does.
    pub fn apply(&mut self, msg: &OverlayMsg, draws: bool, now: Instant) -> Option<PoseTrack> {
        self.advance(now);
        let OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }) = msg else {
            if let OverlayMsg::Remove(key) = msg {
                if draws {
                    self.surface_hotspot = None;
                }
                self.map.remove(key.clone());
            } else if let OverlayMsg::Revive(key) = msg {
                self.map.revive(key.clone());
            }
            return None;
        };
        if let OverlayCommand::MoveTo { x, y, .. } = cmd {
            let from = self.surface_hotspot.filter(|_| draws);
            if let Some(core) = self.map.cursor_mut(key).filter(|core| !core.placed) {
                core.pos = match from {
                    Some((hx, hy)) => crate::anchor_for_pointer(hx, hy, core.heading),
                    None => crate::seed_position(*x, *y, None),
                };
                core.placed = true;
            }
        }
        self.map.apply_command(key.clone(), cmd.clone());
        if !draws {
            return None;
        }
        let core = self.map.cursors.get(key)?;
        let track = match cmd {
            OverlayCommand::MoveTo { .. } => core.pose_track(POSE_STEP_SECS),
            OverlayCommand::SnapTo { .. } => {
                let (x, y) = core.hotspot();
                Some(PoseTrack {
                    samples: vec![PoseSample {
                        t: 0.0,
                        x,
                        y,
                        float_dy: core.float_dy(),
                        rotation: 0.0,
                    }],
                    rest_from: None,
                })
            }
            OverlayCommand::ClickPulse { x, y } => {
                self.surface_hotspot = Some((*x, *y));
                None
            }
            _ => None,
        };
        if let Some(end) = track.as_ref().and_then(|track| track.samples.last()) {
            self.surface_hotspot = Some((end.x, end.y));
        }
        track
    }

    /// The look `key`'s cursor paints with now.
    pub fn look(&mut self, key: &str) -> Option<PoseLook> {
        self.map.cursor_mut(key).map(|core| core.pose_look())
    }

    /// Moves planned so far for `key`; a later move replaces an earlier one.
    pub fn move_seq(&self, key: &CursorKey) -> Option<u64> {
        self.map.cursors.get(key).map(|core| core.move_seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor_options::RotateMode;
    use crate::cursor_pose::{LEAN_MAX_DRAG_RADIANS, LEAN_MAX_RADIANS, REST_FLOAT_LIFT_PT};
    use crate::CursorOptions;
    use std::time::Duration;

    fn cmd(key: &str, cmd: OverlayCommand) -> OverlayMsg {
        OverlayMsg::Cmd(KeyedOverlayCommand {
            key: key.to_owned(),
            cmd,
        })
    }

    fn move_to(x: f64, y: f64) -> OverlayCommand {
        OverlayCommand::MoveTo {
            x,
            y,
            end_heading_radians: std::f64::consts::FRAC_PI_4,
            target: None,
        }
    }

    fn model_with(key: &str, options: CursorOptions) -> PoseModel {
        let mut model = PoseModel::new(CursorConfig::default());
        model.cursor_mut(key).unwrap().options_override = Some(options);
        model
    }

    fn opts(rotate: RotateMode) -> CursorOptions {
        CursorOptions {
            float: true,
            halo: true,
            rotate,
            ..CursorOptions::default()
        }
    }

    fn arrival_sample(track: &PoseTrack) -> PoseSample {
        let at = track.rest_from.unwrap();
        *track.samples.iter().find(|s| s.t == at).unwrap()
    }

    fn placed_model(key: &str, options: CursorOptions, now: Instant) -> PoseModel {
        let mut model = model_with(key, options);
        model.apply(
            &cmd(
                key,
                OverlayCommand::SnapTo {
                    x: 100.0,
                    y: 100.0,
                    heading_radians: None,
                },
            ),
            true,
            now,
        );
        model
    }

    #[test]
    fn a_move_floats_while_travelling_and_arrives_settled_and_upright() {
        for mode in [RotateMode::Off, RotateMode::Lean, RotateMode::Heading] {
            let now = Instant::now();
            let mut model = placed_model("s", opts(mode), now);
            let track = model
                .apply(&cmd("s", move_to(800.0, 500.0)), true, now)
                .expect("a move plays");
            let first = track.samples[0];
            let last = *track.samples.last().unwrap();
            assert_eq!(first.t, 0.0);
            assert!((last.x - 800.0).abs() < 1.0 && (last.y - 500.0).abs() < 1.0);
            let arrival = arrival_sample(&track);
            assert_eq!(arrival.float_dy, 0.0, "{mode:?}: settled at arrival");
            assert_eq!(arrival.rotation, 0.0, "{mode:?}: upright at arrival");
            assert!(
                track.samples.iter().any(|s| s.float_dy < -1.5),
                "{mode:?}: lifts while travelling"
            );
            assert!(track.samples.windows(2).all(|w| w[0].t < w[1].t));
            let peak = track
                .samples
                .iter()
                .map(|s| s.rotation.abs())
                .fold(0.0, f64::max);
            match mode {
                RotateMode::Off => assert_eq!(peak, 0.0),
                RotateMode::Lean => assert!(peak > 0.1 && peak <= LEAN_MAX_RADIANS + 1e-9),
                RotateMode::Heading => assert!(peak > LEAN_MAX_RADIANS),
            }
        }
    }

    #[test]
    fn a_drag_leans_at_most_15_degrees_and_drops_settled() {
        let now = Instant::now();
        let mut model = placed_model("s", opts(RotateMode::Lean), now);
        model.apply(&cmd("s", OverlayCommand::SetPressed(true)), true, now);
        let track = model
            .apply(&cmd("s", move_to(700.0, 160.0)), true, now)
            .unwrap();
        let peak = track
            .samples
            .iter()
            .map(|s| s.rotation.abs())
            .fold(0.0, f64::max);
        assert!(
            peak > 0.05 && peak <= LEAN_MAX_DRAG_RADIANS + 1e-9,
            "{peak}"
        );
        assert!(track.samples.iter().any(|s| s.float_dy < -1.0));
        let drop = arrival_sample(&track);
        assert_eq!((drop.float_dy, drop.rotation), (0.0, 0.0));
    }

    #[test]
    fn short_hops_stay_upright_and_snaps_keep_the_resting_loop() {
        let now = Instant::now();
        let mut model = placed_model("s", opts(RotateMode::Heading), now);
        let hop = model
            .apply(&cmd("s", move_to(120.0, 110.0)), true, now)
            .unwrap();
        assert!(hop.samples.iter().all(|s| s.rotation == 0.0));
        let snap = model
            .apply(
                &cmd(
                    "s",
                    OverlayCommand::SnapTo {
                        x: 400.0,
                        y: 300.0,
                        heading_radians: None,
                    },
                ),
                true,
                now + Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(snap.samples.len(), 1);
        assert_eq!(snap.rest_from, None, "a snap does not restart the float");
    }

    #[test]
    fn the_float_carries_over_from_rest_without_a_jump() {
        let now = Instant::now();
        let mut model = placed_model("s", opts(RotateMode::Off), now);
        // Two seconds into the resting loop the cursor is at its peak lift.
        let later = now + Duration::from_secs(2);
        let track = model
            .apply(&cmd("s", move_to(900.0, 600.0)), true, later)
            .unwrap();
        assert!(
            (track.samples[0].float_dy + REST_FLOAT_LIFT_PT).abs() < 1e-6,
            "starts at the resting lift: {}",
            track.samples[0].float_dy
        );
        let steps = track
            .samples
            .windows(2)
            .map(|w| (w[1].float_dy - w[0].float_dy).abs())
            .fold(0.0, f64::max);
        assert!(steps < 1.0, "no jump between samples: {steps}");
    }

    #[test]
    fn float_off_and_reduced_motion_send_still_poses() {
        let now = Instant::now();
        let options = CursorOptions {
            float: false,
            ..opts(RotateMode::Lean)
        };
        let mut model = placed_model("s", options, now);
        let track = model
            .apply(&cmd("s", move_to(900.0, 600.0)), true, now)
            .unwrap();
        assert!(track.samples.iter().all(|s| s.float_dy == 0.0));
        assert_eq!(model.look("s").unwrap().rest_lift_pt, 0.0);

        let mut model = placed_model("r", opts(RotateMode::Heading), now);
        model.cursor_mut("r").unwrap().visual.reduced_motion = crate::ReducedMotion::On;
        let track = model
            .apply(&cmd("r", move_to(900.0, 600.0)), true, now)
            .unwrap();
        assert!(track
            .samples
            .iter()
            .all(|s| s.float_dy == 0.0 && s.rotation == 0.0));
        let look = model.look("r").unwrap();
        assert_eq!(look.rest_lift_pt, 0.0);
        assert!(look.halo.is_some(), "reduced motion keeps the halo");
    }

    #[test]
    fn the_look_follows_cursor_halo_and_the_session_colour() {
        let mut model = model_with("a", opts(RotateMode::Off));
        let look = model.look("a").unwrap();
        assert!((look.rest_lift_pt - REST_FLOAT_LIFT_PT).abs() < 1e-9);
        let halo = look.halo.unwrap();
        assert_eq!(halo.rgb, crate::cursor_pose::halo_rgb(None, "a"));
        assert_eq!(halo.peak_alpha, crate::cursor_pose::HALO_PEAK_ALPHA);
        assert_eq!(halo.radius_pt, crate::cursor_pose::HALO_RADIUS_PT);
        model.cursor_mut("b").unwrap().options_override = Some(opts(RotateMode::Off));
        assert_ne!(model.look("b").unwrap().halo.unwrap().rgb, halo.rgb);

        model.cursor_mut("a").unwrap().options_override = Some(CursorOptions {
            halo: false,
            ..opts(RotateMode::Off)
        });
        assert_eq!(model.look("a").unwrap().halo, None);
        model.cursor_mut("b").unwrap().effects_capable = false;
        assert_eq!(model.look("b").unwrap().halo, None);
    }

    #[test]
    fn a_new_session_starts_from_where_the_surface_shows_the_cursor() {
        let now = Instant::now();
        let mut model = placed_model("a", opts(RotateMode::Off), now);
        model.apply(&cmd("a", move_to(500.0, 400.0)), true, now);
        model.cursor_mut("b").unwrap().options_override = Some(opts(RotateMode::Off));
        let track = model
            .apply(&cmd("b", move_to(900.0, 700.0)), true, now)
            .unwrap();
        let start = track.samples[0];
        assert!((start.x - 500.0).abs() < 1.0 && (start.y - 400.0).abs() < 1.0);
        // A session that does not reach the surface sends nothing.
        assert!(model
            .apply(&cmd("c", move_to(10.0, 10.0)), false, now)
            .is_none());
    }

    #[test]
    fn the_json_carries_every_sample_and_the_look() {
        let now = Instant::now();
        let mut model = placed_model("s", opts(RotateMode::Lean), now);
        let track = model
            .apply(&cmd("s", move_to(640.0, 480.0)), true, now)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&track.to_json()).unwrap();
        let samples = parsed["samples"].as_array().unwrap();
        assert_eq!(samples.len(), track.samples.len());
        assert!(samples.iter().all(|s| s.as_array().unwrap().len() == 5));
        assert_eq!(
            parsed["rest_from"].as_f64().unwrap(),
            round(track.rest_from.unwrap(), 4)
        );
        assert!(track.arrival_secs() > 0.0);

        let look: serde_json::Value =
            serde_json::from_str(&model.look("s").unwrap().to_json()).unwrap();
        assert!(look["halo"]["rgb"].as_str().unwrap().starts_with('#'));
        assert_eq!(look["halo"]["stops"].as_array().unwrap().len(), 3);
        assert_eq!(look["rest_secs"].as_f64().unwrap(), 4.0);
    }

    #[test]
    fn an_ended_session_plays_nothing() {
        let now = Instant::now();
        let mut model = placed_model("s", opts(RotateMode::Off), now);
        model.apply(&OverlayMsg::Remove("s".into()), true, now);
        assert!(model
            .apply(&cmd("s", move_to(300.0, 300.0)), true, now)
            .is_none());
    }
}
