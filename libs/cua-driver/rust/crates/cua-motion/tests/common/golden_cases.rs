// Golden cases shared by the Rust and TypeScript implementations.
//
// `examples/export_golden.rs` writes them to `fixtures/golden.json`;
// `tests/golden.rs` checks the fixture still matches this crate, and
// `libs/typescript/motion/tests/golden.test.ts` checks the TypeScript port
// against the same file.

use cua_motion::effects::{self, EffectFrame};
use cua_motion::{
    plan_move, plan_spec, ArcShape, Duration, Ease, Heading, MotionEffects, MotionParams,
    MotionSpec, MotionStyle, MotionTiming, MoveRequest, PathShape, Pt, ResolvedEffects, Rng,
    Settle, TrailSpec, Trajectory,
};
use serde_json::{json, Value};
use std::f64::consts::FRAC_PI_4;

/// Points of each trajectory written out, evenly spaced in time.
const GRID: usize = 24;

fn r(v: f64) -> Value {
    // 1e-9 is far below what either side needs to agree on.
    json!((v * 1e9).round() / 1e9)
}

fn moves() -> Vec<MoveRequest> {
    let mk = |from: (f64, f64), fh: f64, to: (f64, f64), target: Option<[f64; 4]>, seed: &str| {
        MoveRequest {
            from: Pt::new(from.0, from.1),
            from_heading: fh,
            to: Pt::new(to.0, to.1),
            end_heading: FRAC_PI_4,
            target,
            seed: seed.into(),
            reduced_motion: false,
        }
    };
    vec![
        mk(
            (100.0, 100.0),
            FRAC_PI_4,
            (600.0, 420.0),
            Some([560.0, 400.0, 80.0, 40.0]),
            "g|0",
        ),
        mk((1180.0, 640.0), FRAC_PI_4, (140.0, 90.0), None, "g|1"),
        mk(
            (300.0, 300.0),
            0.0,
            (318.0, 306.0),
            Some([312.0, 300.0, 12.0, 10.0]),
            "g|2",
        ),
        mk(
            (40.0, 700.0),
            2.5,
            (1260.0, 80.0),
            Some([1230.0, 60.0, 60.0, 40.0]),
            "g|3",
        ),
    ]
}

fn sample_grid(traj: &Trajectory) -> Value {
    let d = traj.duration();
    Value::Array(
        (0..=GRID)
            .map(|i| {
                let s = traj.sample_at(d * i as f64 / GRID as f64);
                json!([r(s.t), r(s.x), r(s.y), r(s.heading)])
            })
            .collect(),
    )
}

fn frame_json(f: &EffectFrame) -> Value {
    json!({
        "glow": f.glow.map(|g| json!([r(g.x), r(g.y), r(g.r), r(g.alpha)])),
        "trail": f.trail.iter().map(|s| json!([r(s.a.0), r(s.a.1), r(s.b.0), r(s.b.1), r(s.width), r(s.alpha)])).collect::<Vec<_>>(),
        "magnet": f.magnet.map(|m| json!([r(m.rect[0]), r(m.rect[1]), r(m.rect[2]), r(m.rect[3]), r(m.glow)])),
        "ripple": f.ripple.map(|p| json!([r(p.x), r(p.y), r(p.r), r(p.width), r(p.alpha)])),
        "squish": r(f.squish),
    })
}

fn traj_json(traj: &Trajectory, with_effects: bool) -> Value {
    let mut out = json!({
        "samples": traj.samples.len(),
        "duration": r(traj.duration()),
        "arrival_t": r(traj.arrival_t),
        "snap_t": traj.snap_t.map(r),
        "target": traj.target.iter().map(|v| r(*v)).collect::<Vec<_>>(),
        "target_known": traj.target_known,
        "effects": serde_json::to_value(traj.effects).unwrap(),
        "grid": sample_grid(traj),
    });
    if with_effects {
        let frames: Vec<Value> = [0.2, 0.5, 0.8, 1.0]
            .iter()
            .map(|k| {
                let t = traj.duration() * k;
                let s = traj.sample_at(t);
                let pos = cua_motion::anchor_for_pointer(s.x, s.y, s.heading);
                let mut frame = effects::motion_frame(traj, t, pos, true);
                let all = ResolvedEffects {
                    ripple: true,
                    squish: true,
                    ..ResolvedEffects::NONE
                };
                effects::add_click(&mut frame, all, 0.03 + 0.4 * k, (s.x, s.y));
                json!({ "t": r(t), "pos": [r(pos.0), r(pos.1)], "frame": frame_json(&frame) })
            })
            .collect();
        out["frames"] = Value::Array(frames);
        out["linger"] = r(effects::linger(traj));
    }
    out
}

fn req_json(req: &MoveRequest) -> Value {
    json!({
        "from": [req.from.x, req.from.y],
        "from_heading": req.from_heading,
        "to": [req.to.x, req.to.y],
        "end_heading": req.end_heading,
        "target": req.target,
        "seed": req.seed,
        "reduced_motion": req.reduced_motion,
    })
}

fn param_sets() -> Vec<(&'static str, MotionParams)> {
    let all_fx = MotionEffects {
        trail: Some(true),
        glow: Some(true),
        magnet: Some(true),
        ripple: None,
        squish: None,
    };
    vec![
        ("default", MotionParams::default()),
        (
            "tuned",
            MotionParams {
                arc_size: 0.45,
                arc_flow: -0.4,
                start_handle: 0.2,
                end_handle: 0.5,
                spring: 0.45,
                turn_radius: 50.0,
                effects: all_fx,
                ..MotionParams::default()
            },
        ),
    ]
}

fn specs() -> Vec<(&'static str, MotionSpec)> {
    let base = MotionSpec::default();
    let fx = ResolvedEffects {
        trail: true,
        glow: true,
        ..ResolvedEffects::NONE
    };
    vec![
        ("signature_arc", base.clone()),
        (
            "straight_linear_fixed",
            MotionSpec {
                path: PathShape::Straight,
                ease: Ease::Linear,
                settle: Settle::None,
                duration: Duration::Fixed { ms: 500.0 },
                ..base.clone()
            },
        ),
        (
            "bow_smootherstep_distance",
            MotionSpec {
                path: PathShape::Bow { amount: -0.08 },
                ease: Ease::Smootherstep,
                duration: Duration::Distance {
                    base_ms: 200.0,
                    ms_per_pt: 0.5,
                    min_ms: 300.0,
                    max_ms: 900.0,
                },
                heading: Heading::Fixed,
                ..base.clone()
            },
        ),
        (
            "arc_bezier_spring",
            MotionSpec {
                path: PathShape::Arc(ArcShape {
                    start_handle: 0.15,
                    end_handle: 0.6,
                    arc_size: 0.3,
                    arc_flow: 0.5,
                }),
                ease: Ease::CubicBezier {
                    x1: 0.2,
                    y1: 0.0,
                    x2: 0.0,
                    y2: 1.0,
                },
                settle: Settle::Spring {
                    amount: 0.08,
                    max_pt: 12.0,
                    cycles: 2.0,
                    decay: 3.0,
                    start: 0.5,
                    glide_end: 0.6,
                },
                duration: Duration::fitts(1.5),
                effects: fx,
                trail: TrailSpec {
                    secs: 0.3,
                    head_width: 16.0,
                    alpha: 0.5,
                    ..TrailSpec::default()
                },
                ..base.clone()
            },
        ),
        (
            "out_cubic_follow_through",
            MotionSpec {
                ease: Ease::OutCubic,
                settle: Settle::FollowThrough {
                    amount: 0.05,
                    max_pt: 20.0,
                    at: 0.7,
                },
                effects: fx,
                ..base.clone()
            },
        ),
        (
            "in_out_sine",
            MotionSpec {
                ease: Ease::InOutSine,
                ..base
            },
        ),
    ]
}

pub fn golden() -> Value {
    let mut cases = Vec::new();
    for style in MotionStyle::ALL {
        for timing in MotionTiming::ALL {
            for (pname, params) in param_sets() {
                for (i, req) in moves().into_iter().enumerate() {
                    let params = MotionParams {
                        style,
                        timing,
                        ..params.clone()
                    };
                    let traj = plan_move(&params, &req);
                    let with_effects = timing == MotionTiming::Native && i < 2;
                    cases.push(json!({
                        "name": format!("{}/{}/{pname}/{i}", style.as_str(), timing.as_str()),
                        "params": serde_json::to_value(&params).unwrap(),
                        "request": req_json(&req),
                        "out": traj_json(&traj, with_effects),
                    }));
                }
            }
        }
    }
    // Reduced motion and the legacy fixed glide.
    for (name, params, reduced) in [
        (
            "reduced",
            MotionParams {
                style: MotionStyle::CometSwoop,
                ..MotionParams::default()
            },
            true,
        ),
        (
            "legacy_glide",
            MotionParams {
                glide_duration_ms: 400.0,
                ..MotionParams::default()
            },
            false,
        ),
        (
            "legacy_glide_classic",
            MotionParams {
                style: MotionStyle::Classic,
                glide_duration_ms: 400.0,
                ..MotionParams::default()
            },
            false,
        ),
    ] {
        let req = MoveRequest {
            reduced_motion: reduced,
            ..moves()[0].clone()
        };
        cases.push(json!({
            "name": name,
            "params": serde_json::to_value(&params).unwrap(),
            "request": req_json(&req),
            "out": traj_json(&plan_move(&params, &req), false),
        }));
    }
    let mut spec_cases = Vec::new();
    for (name, spec) in specs() {
        for (i, req) in moves().into_iter().enumerate() {
            spec_cases.push(json!({
                "name": format!("{name}/{i}"),
                "spec": serde_json::to_value(&spec).unwrap(),
                "request": req_json(&req),
                "out": traj_json(&plan_spec(&spec, &req), i == 0),
            }));
        }
    }
    let rng: Vec<Value> = ["", "abc", "7|keynote-swoop|0", "g|3"]
        .iter()
        .map(|seed| {
            let mut rng = Rng::from_seed(seed);
            json!({
                "seed": seed,
                "hash": cua_motion::rng::hash_string(seed),
                "values": (0..4).map(|_| rng.next_f64()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let eases: Vec<Value> = [
        Ease::Linear,
        Ease::MinJerk,
        Ease::Smootherstep,
        Ease::InOutCubic,
        Ease::InOutSine,
        Ease::OutCubic,
        Ease::CubicBezier {
            x1: 0.25,
            y1: 0.1,
            x2: 0.25,
            y2: 1.0,
        },
        Ease::CubicBezier {
            x1: 0.68,
            y1: -0.55,
            x2: 0.27,
            y2: 1.55,
        },
    ]
    .iter()
    .map(|e| {
        json!({
            "ease": serde_json::to_value(e).unwrap(),
            "values": (0..=10).map(|i| r(e.apply(i as f64 / 10.0))).collect::<Vec<_>>(),
        })
    })
    .collect();
    json!({
        "source": "libs/cua-driver/rust/crates/cua-motion/examples/export_golden.rs",
        "grid": GRID,
        "rng": rng,
        "eases": eases,
        "cases": cases,
        "spec_cases": spec_cases,
    })
}
