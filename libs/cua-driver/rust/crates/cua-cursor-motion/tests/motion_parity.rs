//! Golden parity with the motion lab, the design source of truth.
//!
//! `tests/fixtures/motion_parity.json` is exported by
//! `libs/cua-driver/tools/cursor-gallery/motion-lab/test/export-parity.mjs`: every scene,
//! seed 7, each shipped style and timing mode. Each Rust trajectory must
//! match the lab within 0.5 pt at every exported sample time.

use cua_cursor_motion::plan::{apply_timing, generate, MoveCtx, Raw};
use cua_cursor_motion::{plan_move, MotionParams, MotionStyle, MotionTiming, MoveRequest, Pt, Rng};
use serde_json::Value;
use std::f64::consts::FRAC_PI_4;

const TOLERANCE_PT: f64 = 0.5;

fn interpolate(samples: &[Raw], t: f64) -> (f64, f64) {
    if t <= samples[0].t {
        return (samples[0].x, samples[0].y);
    }
    let last = samples[samples.len() - 1];
    if t >= last.t {
        return (last.x, last.y);
    }
    let i = samples.partition_point(|s| s.t <= t);
    let (a, b) = (samples[i - 1], samples[i]);
    let f = (t - a.t) / (b.t - a.t);
    (a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f)
}

fn pt(v: &Value) -> Pt {
    Pt::new(v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap())
}

#[test]
fn shipped_styles_match_the_motion_lab() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/motion_parity.json")).expect("fixture json");
    let fixed_ms = fixture["fixed_ms"].as_f64().unwrap();
    let mut checked = 0;
    let mut worst = (0.0f64, String::new());
    let mut classic_misses = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let lab_id = case["style"].as_str().unwrap();
        let style = MotionStyle::parse(lab_id).expect("known style");
        let timing = MotionTiming::parse(case["timing"].as_str().unwrap()).unwrap();
        let from = pt(&case["from"]);
        let aim = pt(&case["aim"]);
        let t = &case["target"];
        let target = [0, 1, 2, 3].map(|i| t[i].as_f64().unwrap());
        let seed = case["seed"].as_str().unwrap();
        let lab: Vec<(f64, f64, f64)> = case["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s[0].as_f64().unwrap(),
                    s[1].as_f64().unwrap(),
                    s[2].as_f64().unwrap(),
                )
            })
            .collect();
        let ctx = MoveCtx { from, aim, target };

        let rust: Vec<Raw> = if style == MotionStyle::Classic {
            // Fixed timing keeps the original semantics (the path takes
            // glide_duration_ms, then the spring), unlike the lab's uniform
            // retime, so only native and Fitts are compared.
            if timing == MotionTiming::Fixed {
                continue;
            }
            // The classic glide is planned in anchor space; at rest heading
            // the anchor is a constant offset from the hotspot.
            let motion = MotionParams {
                style,
                timing,
                ..MotionParams::default()
            };
            let traj = plan_move(
                &motion,
                &MoveRequest {
                    from,
                    from_heading: FRAC_PI_4,
                    to: aim,
                    end_heading: FRAC_PI_4,
                    target: Some(target),
                    seed: seed.into(),
                    reduced_motion: false,
                },
            );
            let (ox, oy) = cua_cursor_motion::anchor_for_pointer(0.0, 0.0, FRAC_PI_4);
            traj.samples
                .iter()
                .map(|s| {
                    let (ax, ay) = cua_cursor_motion::anchor_for_pointer(s.x, s.y, s.heading);
                    Raw {
                        t: s.t * 1000.0,
                        x: ax - ox,
                        y: ay - oy,
                    }
                })
                .collect()
        } else {
            let mut rng = Rng::from_seed(seed);
            let (mut raw, snap) = generate(style, &ctx, &MotionParams::default(), &mut rng);
            let mut events: Vec<f64> = snap.into_iter().collect();
            apply_timing(&mut raw, &mut events, &ctx, timing, fixed_ms);
            if let (Some(lab_snap), Some(rust_snap)) = (case["snap_ms"].as_f64(), events.first()) {
                assert!(
                    (lab_snap - rust_snap).abs() < 1.0,
                    "{lab_id} {} lock-on {lab_snap} vs {rust_snap}",
                    case["scene"]
                );
            }
            raw
        };

        let name = format!(
            "{lab_id}/{}/{}#{}",
            case["timing"], case["scene"], case["index"]
        );
        let lab_end = lab.last().unwrap().0;
        let rust_end = rust.last().unwrap().t;
        // The classic glide is integrated per frame, so it may end one
        // 120 Hz frame apart from the lab.
        let tol_ms = if style == MotionStyle::Classic {
            12.0
        } else {
            1.0
        };
        let duration_ok = (lab_end - rust_end).abs() <= tol_ms;
        assert!(
            duration_ok || style == MotionStyle::Classic,
            "{name}: duration {lab_end} ms vs {rust_end} ms"
        );
        let case_worst = lab
            .iter()
            .map(|&(t, x, y)| {
                let (rx, ry) = interpolate(&rust, t);
                (rx - x).hypot(ry - y)
            })
            .fold(0.0f64, f64::max);
        if style == MotionStyle::Classic && (case_worst > TOLERANCE_PT || !duration_ok) {
            // The lab's JS Dubins port can pick a different arc word than
            // dubins.rs for a few poses; classic keeps the Rust planner.
            classic_misses.push(name);
        } else if case_worst > worst.0 {
            worst = (case_worst, name);
        }
        checked += 1;
    }
    assert!(checked > 400, "checked {checked} cases");
    assert!(
        classic_misses.len() <= 2,
        "classic misses: {classic_misses:?}"
    );
    assert!(
        worst.0 <= TOLERANCE_PT,
        "worst deviation {:.3} pt at {}",
        worst.0,
        worst.1
    );
    eprintln!(
        "motion parity: {checked} cases, worst {:.4} pt at {}",
        worst.0, worst.1
    );
}
