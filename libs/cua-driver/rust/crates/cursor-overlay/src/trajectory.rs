//! Planned agent-cursor trajectories.
//!
//! A move is planned once, when the overlay receives `MoveTo`, as a list of
//! timed samples. Each frame then only advances a clock and interpolates
//! between two samples, so every platform plays the same motion at any frame
//! rate.
//!
//! The styles are ports of the motion lab in
//! `libs/cua-driver/tools/cursor-gallery/motion-lab/motion/`, which stays the
//! design source of truth: `candidates.js` (styles), `generators.js` and
//! `math.js` (paths, profiles, durations), `rng.js` (seeded randomness) and
//! `plan.js` (timing modes and heading). Generators work in milliseconds and
//! pointer (hotspot) coordinates like the lab, so golden fixtures exported
//! from the lab compare directly; see `tests/motion_parity.rs`.

use crate::motion::{MotionConfig, MotionStyle, MotionTiming, ResolvedEffects, DEFAULT_FIXED_MS};
use crate::path_planner::PathPlanner;
use std::f64::consts::PI;

/// Sample period of every generated trajectory, in milliseconds (120 Hz).
pub const DT_MS: f64 = 1000.0 / 120.0;

/// Target box assumed when a move has no element rect (raw coordinates).
pub const DEFAULT_TARGET_PT: f64 = 24.0;

/// Arrival fires once the hotspot is this close to the final point.
const ARRIVAL_TOLERANCE_PT: f64 = 1.0;

/// Reduced-motion move time.
const REDUCED_MOTION_MS: f64 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

fn dist(a: Pt, b: Pt) -> f64 {
    (b.x - a.x).hypot(b.y - a.y)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lerp_pt(a: Pt, b: Pt, t: f64) -> Pt {
    Pt::new(lerp(a.x, b.x, t), lerp(a.y, b.y, t))
}

fn unit(a: Pt, b: Pt) -> Pt {
    let d = dist(a, b);
    let d = if d == 0.0 { 1.0 } else { d };
    Pt::new((b.x - a.x) / d, (b.y - a.y) / d)
}

/// 90 degrees counter-clockwise in screen space (y down).
fn perp(u: Pt) -> Pt {
    Pt::new(-u.y, u.x)
}

/// JS `a % TAU` wrapped into `(-PI, PI]`, matching `math.js wrapAngle`.
pub fn wrap_angle(a: f64) -> f64 {
    let tau = 2.0 * PI;
    let mut r = a % tau;
    if r > PI {
        r -= tau;
    }
    if r < -PI {
        r += tau;
    }
    r
}

// ── Seeded randomness (rng.js) ───────────────────────────────────────────

/// FNV-1a over UTF-16 code units, like `rng.js hashString`.
pub fn hash_string(text: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for unit in text.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// mulberry32, bit-identical to `rng.js Rng`.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u32,
}

impl Rng {
    pub fn from_seed(seed: &str) -> Self {
        let state = hash_string(seed);
        Self {
            state: if state == 0 { 1 } else { state },
        }
    }

    pub fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}

// ── Easing and profiles (math.js, generators.js) ─────────────────────────

pub mod ease {
    pub fn min_jerk(t: f64) -> f64 {
        t * t * t * (10.0 - 15.0 * t + 6.0 * t * t)
    }
    pub fn smootherstep(t: f64) -> f64 {
        t * t * t * (t * (6.0 * t - 15.0) + 10.0)
    }
    pub fn in_out_cubic(t: f64) -> f64 {
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        }
    }
    pub fn in_out_sine(t: f64) -> f64 {
        0.5 - 0.5 * (std::f64::consts::PI * t).cos()
    }
}

/// `bumpProfile`: base plus smooth bumps; `over` pushes past the end late
/// and settles. Fractions of the distance.
fn bump_profile(base: fn(f64) -> f64, over: f64, over_at: f64) -> impl Fn(f64) -> f64 {
    let bump = move |tau: f64, at: f64| {
        let a = (at * 10.0).max(1.5);
        let b = ((1.0 - at) * 10.0).max(1.5);
        let peak = (a / (a + b)).powf(a) * (b / (a + b)).powf(b);
        (tau.powf(a) * (1.0 - tau).powf(b)) / peak
    };
    move |tau| base(tau) + over * bump(tau, over_at)
}

/// `wobbleProfile`: damped wobble around the end from `start` on.
fn wobble_profile(
    base: impl Fn(f64) -> f64,
    amp: f64,
    cycles: f64,
    decay: f64,
    start: f64,
) -> impl Fn(f64) -> f64 {
    move |tau| {
        if tau <= start {
            return base(tau);
        }
        let u = (tau - start) / (1.0 - start);
        let ramp = ease::smootherstep((u / 0.18).min(1.0));
        base(tau)
            + (amp * ramp * (-decay * u).exp() * (2.0 * PI * cycles * u).sin() * (1.0 - u).powi(2))
                / ((-decay * 0.12).exp() * 0.77).max(1e-6)
    }
}

// ── Paths (math.js makePath and friends) ─────────────────────────────────

/// A parametric path with an arc-length table, so profiles act on
/// distance. `at_fraction` extrapolates along the end tangents outside
/// `[0, 1]`, which is how overshoot leaves the path and comes back.
pub struct Path {
    f: Box<dyn Fn(f64) -> Pt>,
    us: Vec<f64>,
    ss: Vec<f64>,
    pub length: f64,
    p0: Pt,
    p1: Pt,
    t0: Pt,
    t1: Pt,
}

impl Path {
    pub fn new(f: impl Fn(f64) -> Pt + 'static, n: usize) -> Self {
        let mut us = Vec::with_capacity(n + 1);
        let mut ss = Vec::with_capacity(n + 1);
        us.push(0.0);
        ss.push(0.0);
        let mut prev = f(0.0);
        let mut total = 0.0;
        for i in 1..=n {
            let u = i as f64 / n as f64;
            let p = f(u);
            total += (p.x - prev.x).hypot(p.y - prev.y);
            us.push(u);
            ss.push(total);
            prev = p;
        }
        let p0 = f(0.0);
        let p1 = f(1.0);
        let t0 = unit(f(1e-3), p0);
        let t1 = unit(f(1.0 - 1e-3), p1);
        Self {
            f: Box::new(f),
            us,
            ss,
            length: total,
            p0,
            p1,
            t0,
            t1,
        }
    }

    pub fn at_fraction(&self, frac: f64) -> Pt {
        let len = self.length;
        if len < 1e-9 {
            return (self.f)(frac.clamp(0.0, 1.0));
        }
        if frac > 1.0 {
            return Pt::new(
                self.p1.x + self.t1.x * (frac - 1.0) * len,
                self.p1.y + self.t1.y * (frac - 1.0) * len,
            );
        }
        if frac < 0.0 {
            return Pt::new(
                self.p0.x + self.t0.x * -frac * len,
                self.p0.y + self.t0.y * -frac * len,
            );
        }
        let target = frac * len;
        let (mut lo, mut hi) = (0usize, self.ss.len() - 1);
        while hi - lo > 1 {
            let mid = (lo + hi) >> 1;
            if self.ss[mid] < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let span = self.ss[hi] - self.ss[lo];
        let span = if span == 0.0 { 1.0 } else { span };
        let u = lerp(self.us[lo], self.us[hi], (target - self.ss[lo]) / span);
        (self.f)(u)
    }
}

fn cubic(p0: Pt, p1: Pt, p2: Pt, p3: Pt) -> impl Fn(f64) -> Pt {
    move |u| {
        let v = 1.0 - u;
        let a = v * v * v;
        let b = 3.0 * v * v * u;
        let c = 3.0 * v * u * u;
        let d = u * u * u;
        Pt::new(
            a * p0.x + b * p1.x + c * p2.x + d * p3.x,
            a * p0.y + b * p1.y + c * p2.y + d * p3.y,
        )
    }
}

fn quad(p0: Pt, p1: Pt, p2: Pt) -> impl Fn(f64) -> Pt {
    move |u| {
        let v = 1.0 - u;
        Pt::new(
            v * v * p0.x + 2.0 * v * u * p1.x + u * u * p2.x,
            v * v * p0.y + 2.0 * v * u * p1.y + u * u * p2.y,
        )
    }
}

/// Arc knobs of the bezier styles (`build_motion_bezier` / lab `cuaBezier`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcShape {
    pub start_handle: f64,
    pub end_handle: f64,
    pub arc_size: f64,
    pub arc_flow: f64,
}

/// The Cua bezier (`bezier.rs build_motion_bezier`) as a path.
pub fn cua_path(a: Pt, b: Pt, shape: ArcShape) -> Path {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = dx.hypot(dy).max(1.0);
    let px = -dy / len;
    let py = dx / len;
    let deflection = len * shape.arc_size;
    let flow = (shape.arc_flow + 1.0) / 2.0;
    let c1d = deflection * (1.0 - 0.5 * flow);
    let c2d = deflection * (1.0 - 0.5 * (1.0 - flow));
    let c1 = Pt::new(
        a.x + dx * shape.start_handle + px * c1d,
        a.y + dy * shape.start_handle + py * c1d,
    );
    let c2 = Pt::new(
        b.x - dx * shape.end_handle + px * c2d,
        b.y - dy * shape.end_handle + py * c2d,
    );
    Path::new(cubic(a, c1, c2, b), 256)
}

/// Gentle single-sided quadratic curve (`paths.bow`).
pub fn bow_path(a: Pt, b: Pt, amount: f64) -> Path {
    let d = dist(a, b);
    let n = perp(unit(a, b));
    let m = lerp_pt(a, b, 0.5);
    let c = Pt::new(m.x + n.x * amount * d, m.y + n.y * amount * d);
    Path::new(quad(a, c, b), 256)
}

/// Chord side that bends paths upward for horizontal moves (`naturalSide`).
pub fn natural_side(a: Pt, b: Pt) -> f64 {
    if unit(a, b).x >= 0.0 {
        -1.0
    } else {
        1.0
    }
}

// ── Samples and generators (generators.js) ───────────────────────────────

/// One generated sample: time in ms from the start of the move and the
/// hotspot position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Raw {
    pub t: f64,
    pub x: f64,
    pub y: f64,
}

fn sample_timed(pos: impl Fn(f64) -> Pt, duration_ms: f64) -> Vec<Raw> {
    let n = ((duration_ms / DT_MS).ceil() as usize).max(2);
    (0..=n)
        .map(|i| {
            let tau = i as f64 / n as f64;
            let p = pos(tau);
            Raw {
                t: tau * duration_ms,
                x: p.x,
                y: p.y,
            }
        })
        .collect()
}

fn pin_ends(mut samples: Vec<Raw>, from: Pt, aim: Pt) -> Vec<Raw> {
    if let Some(first) = samples.first_mut() {
        first.x = from.x;
        first.y = from.y;
    }
    if let Some(last) = samples.last_mut() {
        last.x = aim.x;
        last.y = aim.y;
    }
    samples
}

fn glide(
    from: Pt,
    aim: Pt,
    path: &Path,
    profile: impl Fn(f64) -> f64,
    duration_ms: f64,
) -> Vec<Raw> {
    pin_ends(
        sample_timed(|tau| path.at_fraction(profile(tau)), duration_ms),
        from,
        aim,
    )
}

/// `speedShaped`: integrate a speed shape v(s) into a timed glide.
fn speed_shaped(
    from: Pt,
    aim: Pt,
    path: &Path,
    shape: impl Fn(f64) -> f64,
    duration_ms: f64,
) -> Vec<Raw> {
    const N: usize = 600;
    let mut ts = vec![0.0; N + 1];
    for i in 1..=N {
        let s = (i as f64 - 0.5) / N as f64;
        ts[i] = ts[i - 1] + 1.0 / shape(s).max(1e-3);
    }
    let total = ts[N];
    let samples = sample_timed(
        |tau| {
            let target = tau * total;
            let (mut lo, mut hi) = (0usize, N);
            while hi - lo > 1 {
                let mid = (lo + hi) >> 1;
                if ts[mid] < target {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let f = (target - ts[lo]) / (ts[hi] - ts[lo]).max(1e-9);
            path.at_fraction((lo as f64 + f) / N as f64)
        },
        duration_ms,
    );
    pin_ends(samples, from, aim)
}

/// What one move needs to know about where it is going.
#[derive(Debug, Clone, Copy)]
pub struct MoveCtx {
    pub from: Pt,
    pub aim: Pt,
    /// Target rect `[x, y, w, h]`.
    pub target: [f64; 4],
}

impl MoveCtx {
    fn d(&self) -> f64 {
        dist(self.from, self.aim)
    }
    /// `targetWidth`: the target's smaller side, at least 4 pt.
    pub fn target_width(&self) -> f64 {
        self.target[2].min(self.target[3]).max(4.0)
    }
    fn side(&self) -> f64 {
        natural_side(self.from, self.aim)
    }
}

/// Fitts' law (Shannon form): `a + b log2(D / W + 1)`.
fn fitts(d: f64, w: f64, a: f64, b: f64) -> f64 {
    a + b * (d / w.max(4.0) + 1.0).log2()
}

/// Lab `fittsMs` helper.
fn fitts_ms(ctx: &MoveCtx) -> f64 {
    fitts(ctx.d(), ctx.target_width(), 50.0, 150.0).clamp(180.0, 1400.0)
}

/// Director's-cut duration (`dcMs`).
fn dc_ms(ctx: &MoveCtx, scale: f64) -> f64 {
    (150.0 + 120.0 * (ctx.d() / ctx.target_width() + 1.0).log2()).clamp(300.0, 1000.0) * scale
}

/// Global Fitts timing mode (`plan.js fittsTimingMs`).
pub fn fitts_timing_ms(d: f64, w: f64) -> f64 {
    (150.0 + 120.0 * (d / w.max(4.0) + 1.0).log2()).clamp(300.0, 1000.0)
}

/// Scale the style's arc by the caller's `arc_size` (0.25, the default, keeps
/// the style's own arc; 0 is straight) and shift its flow by `arc_flow`.
fn knob_shape(motion: &MotionConfig, arc_size: f64, arc_flow: f64) -> ArcShape {
    let default = MotionConfig::default();
    ArcShape {
        start_handle: motion.start_handle,
        end_handle: motion.end_handle,
        arc_size: arc_size * (motion.arc_size / default.arc_size),
        arc_flow: (arc_flow + motion.arc_flow).clamp(-1.0, 1.0),
    }
}

/// Signature arc: Cua arc 0.16, min-jerk with a 1.8% follow-through capped
/// at 8 pt (`dc-signature-arc`).
pub fn signature_arc(ctx: &MoveCtx, shape: ArcShape) -> Vec<Raw> {
    let path = cua_path(
        ctx.from,
        ctx.aim,
        ArcShape {
            arc_size: shape.arc_size * ctx.side(),
            ..shape
        },
    );
    let over = 0.018f64.min(8.0 / ctx.d().max(1.0));
    glide(
        ctx.from,
        ctx.aim,
        &path,
        bump_profile(ease::min_jerk, over, 0.82),
        dc_ms(ctx, 1.1),
    )
}

/// Spring settle: arc 0.12, min-jerk into one damped 6 pt overshoot
/// (`dc-spring-settle`).
pub fn spring_settle(ctx: &MoveCtx, shape: ArcShape) -> Vec<Raw> {
    let path = cua_path(
        ctx.from,
        ctx.aim,
        ArcShape {
            arc_size: shape.arc_size * ctx.side(),
            ..shape
        },
    );
    let amp = 0.05f64.min(6.0 / ctx.d().max(1.0));
    let profile = wobble_profile(|t| ease::min_jerk((t / 0.68).min(1.0)), amp, 1.3, 2.6, 0.55);
    glide(ctx.from, ctx.aim, &path, profile, dc_ms(ctx, 1.35))
}

/// Comet swoop: wide arc 0.24, flow 0.2, easeInOutCubic (`dc-comet-swoop`).
pub fn comet_swoop(ctx: &MoveCtx, shape: ArcShape) -> Vec<Raw> {
    let path = cua_path(
        ctx.from,
        ctx.aim,
        ArcShape {
            arc_size: shape.arc_size * ctx.side(),
            ..shape
        },
    );
    glide(
        ctx.from,
        ctx.aim,
        &path,
        ease::in_out_cubic,
        dc_ms(ctx, 1.15),
    )
}

/// Magnetic lock-on (`dc-magnetic` = `magnetic-snap` with a 40 pt capture
/// radius, pull 0.45, 300 pt/s entry). Returns the samples and the
/// lock-on time in ms.
pub fn magnetic(ctx: &MoveCtx) -> (Vec<Raw>, Option<f64>) {
    let path = bow_path(ctx.from, ctx.aim, 0.04 * ctx.side());
    let len = path.length;
    let radius = 40f64.min(len * 0.5);
    let pull = 0.45;
    let enter_speed = 300.0;
    let mut out = vec![Raw {
        t: 0.0,
        x: ctx.from.x,
        y: ctx.from.y,
    }];
    let (mut s, mut v, mut t) = (0.0f64, 0.0f64, 0.0f64);
    let mut snap_t = None;
    let dt = DT_MS / 1000.0;
    while s < len && t < 4.0 {
        let rem = len - s;
        if rem > radius {
            v = (1500f64)
                .min(v + 7000.0 * dt)
                .min(enter_speed + 5.5 * (rem - radius));
        } else {
            if snap_t.is_none() {
                snap_t = Some(t * 1000.0);
            }
            v += 26000.0 * pull * (radius / rem.max(6.0)) * dt;
        }
        s = len.min(s + v * dt);
        t += dt;
        let q = path.at_fraction(s / len);
        out.push(Raw {
            t: t * 1000.0,
            x: q.x,
            y: q.y,
        });
    }
    (
        pin_ends(out, ctx.from, ctx.aim),
        Some(snap_t.unwrap_or(t * 1000.0)),
    )
}

/// Fitts min-jerk with a 2% bow (`fitts-minjerk`).
pub fn fitts_minjerk(ctx: &MoveCtx) -> Vec<Raw> {
    let path = bow_path(ctx.from, ctx.aim, 0.02 * ctx.side());
    glide(ctx.from, ctx.aim, &path, ease::min_jerk, fitts_ms(ctx))
}

/// Keynote swoop: arc 0.25-0.35, flow 0.2, easeInOutCubic (`keynote-swoop`).
pub fn keynote_swoop(ctx: &MoveCtx, rng: &mut Rng) -> Vec<Raw> {
    let arc = rng.range(0.25, 0.35);
    let path = cua_path(
        ctx.from,
        ctx.aim,
        ArcShape {
            start_handle: 0.3,
            end_handle: 0.3,
            arc_size: arc * ctx.side(),
            arc_flow: 0.2,
        },
    );
    glide(
        ctx.from,
        ctx.aim,
        &path,
        ease::in_out_cubic,
        (350.0 + 0.35 * ctx.d()).clamp(450.0, 1100.0),
    )
}

/// Precise click: cruise, then the final 15% at most 35% speed, no
/// overshoot (`precise-click`).
pub fn precise_click(ctx: &MoveCtx) -> Vec<Raw> {
    let fp = 0.15;
    let fs = 0.35;
    let shape = move |s: f64| {
        if s < 0.4 {
            0.04 + (PI * s / 0.8).sin()
        } else if s < 1.0 - fp {
            1.0 - (1.0 - fs) * ease::in_out_sine((s - 0.4) / (0.6 - fp))
        } else {
            0.02 + fs * ((1.0 - s) / fp).max(0.0).sqrt()
        }
    };
    let path = bow_path(ctx.from, ctx.aim, 0.03 * ctx.side());
    speed_shaped(ctx.from, ctx.aim, &path, shape, fitts_ms(ctx) * 1.2)
}

/// Which generator adaptive dispatch picks (`adaptive-auto`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptivePick {
    PreciseClick,
    KeynoteSwoop,
    FittsMinJerk,
}

pub fn adaptive_pick(ctx: &MoveCtx) -> AdaptivePick {
    if ctx.target_width() < 16.0 {
        AdaptivePick::PreciseClick
    } else if ctx.d() > 900.0 {
        AdaptivePick::KeynoteSwoop
    } else {
        AdaptivePick::FittsMinJerk
    }
}

pub fn adaptive(ctx: &MoveCtx, rng: &mut Rng) -> Vec<Raw> {
    match adaptive_pick(ctx) {
        AdaptivePick::PreciseClick => precise_click(ctx),
        AdaptivePick::KeynoteSwoop => keynote_swoop(ctx, rng),
        AdaptivePick::FittsMinJerk => fitts_minjerk(ctx),
    }
}

/// Uniformly rescale sample times (`plan.js retime`).
fn retime(samples: &mut [Raw], target_ms: f64) {
    let total = samples.last().map_or(0.0, |s| s.t);
    if total.is_nan() || total <= 0.0 || samples.len() < 3 {
        return;
    }
    let k = target_ms / total;
    for s in samples {
        s.t *= k;
    }
}

/// Generate one move for a lab-ported style in lab space (ms, hotspot),
/// before heading. `classic` is not generated here (see [`plan_move`]).
/// Returns the samples and an optional lock-on time in ms.
pub fn generate(
    style: MotionStyle,
    ctx: &MoveCtx,
    motion: &MotionConfig,
    rng: &mut Rng,
) -> (Vec<Raw>, Option<f64>) {
    match style {
        MotionStyle::SignatureArc => (signature_arc(ctx, knob_shape(motion, 0.16, 0.15)), None),
        MotionStyle::SpringSettle => (spring_settle(ctx, knob_shape(motion, 0.12, 0.0)), None),
        MotionStyle::CometSwoop => (comet_swoop(ctx, knob_shape(motion, 0.24, 0.2)), None),
        MotionStyle::Magnetic => magnetic(ctx),
        MotionStyle::Adaptive => (adaptive(ctx, rng), None),
        MotionStyle::Classic => (fitts_minjerk(ctx), None),
    }
}

/// Apply a global timing mode to a generated move (`plan.js plan`).
pub fn apply_timing(
    samples: &mut [Raw],
    events: &mut [f64],
    ctx: &MoveCtx,
    timing: MotionTiming,
    fixed_ms: f64,
) {
    let total = samples.last().map_or(0.0, |s| s.t);
    let want = match timing {
        MotionTiming::Native => return,
        MotionTiming::Fixed => fixed_ms,
        MotionTiming::Fitts => {
            let last = samples.last().map_or(ctx.aim, |s| Pt::new(s.x, s.y));
            fitts_timing_ms(dist(ctx.from, last), ctx.target[2].min(ctx.target[3]))
        }
    };
    if total > 0.0 {
        retime(samples, want);
        for e in events {
            *e = *e * want / total;
        }
    }
}

// ── Played trajectories ──────────────────────────────────────────────────

/// One timed trajectory sample: seconds from the start of the move, the
/// hotspot position and the visual heading (the renderer's convention,
/// rest pose = `end_heading`, usually pi/4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub heading: f64,
}

/// A planned move, played back by time.
#[derive(Debug, Clone)]
pub struct Trajectory {
    pub samples: Vec<Sample>,
    /// When the hotspot first reaches the final point (within 1 pt). The
    /// overlay releases the waiting action then, so any follow-through or
    /// settle keeps playing during the click.
    pub arrival_t: f64,
    /// Magnetic lock-on time, seconds.
    pub snap_t: Option<f64>,
    /// Target rect `[x, y, w, h]` used for timing and the magnet glow.
    pub target: [f64; 4],
    /// Whether the target rect came from the action (not the default box).
    pub target_known: bool,
    pub effects: ResolvedEffects,
    pub style: MotionStyle,
}

impl Trajectory {
    pub fn duration(&self) -> f64 {
        self.samples.last().map_or(0.0, |s| s.t)
    }

    pub fn end(&self) -> Sample {
        *self.samples.last().expect("a trajectory has samples")
    }

    /// Interpolated sample at `t` seconds (clamped to the ends).
    pub fn sample_at(&self, t: f64) -> Sample {
        let first = self.samples[0];
        if t <= first.t {
            return first;
        }
        let last = self.end();
        if t >= last.t {
            return last;
        }
        let idx = self.samples.partition_point(|s| s.t <= t);
        let a = self.samples[idx - 1];
        let b = self.samples[idx];
        let f = (t - a.t) / (b.t - a.t);
        Sample {
            t,
            x: lerp(a.x, b.x, f),
            y: lerp(a.y, b.y, f),
            heading: a.heading + wrap_angle(b.heading - a.heading) * f,
        }
    }

    /// Hotspot velocity at `t` in pt/s (central difference over ~16 ms).
    pub fn velocity_at(&self, t: f64) -> (f64, f64) {
        let h = 0.008;
        if t > self.duration() + h {
            return (0.0, 0.0);
        }
        let a = self.sample_at(t - h);
        let b = self.sample_at(t + h);
        ((b.x - a.x) / (2.0 * h), (b.y - a.y) / (2.0 * h))
    }
}

/// Inputs of one planned move.
#[derive(Debug, Clone)]
pub struct MoveRequest {
    /// Current hotspot.
    pub from: Pt,
    /// Current visual heading.
    pub from_heading: f64,
    /// Requested hotspot.
    pub to: Pt,
    /// Rest heading on arrival.
    pub end_heading: f64,
    /// Target element rect, when the action knows it.
    pub target: Option<[f64; 4]>,
    /// Seed string, e.g. `"<cursor id>|<move counter>"`.
    pub seed: String,
    /// Reduced motion: a short straight glide without effects.
    pub reduced_motion: bool,
}

fn default_target(p: Pt) -> [f64; 4] {
    let h = DEFAULT_TARGET_PT / 2.0;
    [p.x - h, p.y - h, DEFAULT_TARGET_PT, DEFAULT_TARGET_PT]
}

/// Plan one move for `motion.style`.
pub fn plan_move(motion: &MotionConfig, req: &MoveRequest) -> Trajectory {
    let target_known = req
        .target
        .is_some_and(|r| r.iter().all(|v| v.is_finite()) && r[2] > 0.0 && r[3] > 0.0);
    let target = if target_known {
        req.target.unwrap()
    } else {
        default_target(req.to)
    };
    let ctx = MoveCtx {
        from: req.from,
        aim: req.to,
        target,
    };
    let fixed_ms = if motion.glide_duration_ms > 0.0 {
        motion.glide_duration_ms
    } else {
        DEFAULT_FIXED_MS
    };
    // A fixed glide_duration_ms predates `timing`; keep honoring it.
    let timing = if motion.timing == MotionTiming::Native && motion.glide_duration_ms > 0.0 {
        MotionTiming::Fixed
    } else {
        motion.timing
    };

    if req.reduced_motion {
        let (from, to) = (req.from, req.to);
        let path = Path::new(move |u| lerp_pt(from, to, u), 8);
        let raw = glide(req.from, req.to, &path, ease::min_jerk, REDUCED_MOTION_MS);
        return finish(
            raw,
            None,
            req,
            target,
            target_known,
            ResolvedEffects::NONE,
            motion.style,
            HeadingMode::Fixed,
        );
    }

    if motion.style == MotionStyle::Classic {
        return plan_classic(motion, req, target, target_known, timing, fixed_ms);
    }

    let mut rng = Rng::from_seed(&req.seed);
    let (mut raw, snap) = generate(motion.style, &ctx, motion, &mut rng);
    let mut events: Vec<f64> = snap.into_iter().collect();
    apply_timing(&mut raw, &mut events, &ctx, timing, fixed_ms);
    let heading = if motion.style == MotionStyle::Magnetic {
        HeadingMode::Fixed
    } else {
        HeadingMode::Tangent
    };
    finish(
        raw,
        events.first().copied(),
        req,
        target,
        target_known,
        motion.resolved_effects(),
        motion.style,
        heading,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeadingMode {
    Fixed,
    Tangent,
}

/// The lab's tip angle at rest, in screen space.
const TIP_ANGLE: f64 = -0.75 * PI;

/// Convert a lab-space move into a played trajectory: compute the heading
/// channel (`plan.js applyHeading`), add a short tail so the heading settles
/// at rest, and find the arrival time.
#[allow(clippy::too_many_arguments)]
fn finish(
    raw: Vec<Raw>,
    snap_ms: Option<f64>,
    req: &MoveRequest,
    target: [f64; 4],
    target_known: bool,
    effects: ResolvedEffects,
    style: MotionStyle,
    mode: HeadingMode,
) -> Trajectory {
    let mut raw = raw;
    if raw.is_empty() {
        raw.push(Raw {
            t: 0.0,
            x: req.to.x,
            y: req.to.y,
        });
    }
    let n = raw.len();
    let mut rot = wrap_angle(req.from_heading - req.end_heading);
    let mut samples = Vec::with_capacity(n + 32);
    for i in 0..n {
        let a = raw[i.saturating_sub(2)];
        let b = raw[(i + 2).min(n - 1)];
        let dt = ((b.t - a.t) / 1000.0).max(1e-3);
        let vx = (b.x - a.x) / dt;
        let vy = (b.y - a.y) / dt;
        let speed = vx.hypot(vy);
        let want = match mode {
            HeadingMode::Tangent => {
                let w = ((speed - 40.0) / 260.0).clamp(0.0, 1.0);
                wrap_angle(vy.atan2(vx) - TIP_ANGLE) * w
            }
            HeadingMode::Fixed => 0.0,
        };
        let step = if i > 0 {
            (raw[i].t - raw[i - 1].t) / 1000.0
        } else {
            0.0
        };
        let k = 1.0 - (-step * 22.0).exp();
        rot += wrap_angle(want - rot) * k;
        samples.push(Sample {
            t: raw[i].t / 1000.0,
            x: raw[i].x,
            y: raw[i].y,
            heading: req.end_heading + rot,
        });
    }
    // Let the heading come back to rest at the final point.
    let end = *samples.last().unwrap();
    let mut t = end.t;
    let dt = DT_MS / 1000.0;
    for _ in 0..36 {
        if rot.abs() < 0.002 {
            break;
        }
        t += dt;
        rot -= rot * (1.0 - (-dt * 22.0).exp());
        samples.push(Sample {
            t,
            heading: req.end_heading + rot,
            ..end
        });
    }
    if let Some(last) = samples.last_mut() {
        last.heading = req.end_heading;
    }
    let arrival_t = arrival_time(&samples, req.to);
    Trajectory {
        samples,
        arrival_t,
        snap_t: snap_ms.map(|ms| ms / 1000.0),
        target,
        target_known,
        effects,
        style,
    }
}

fn arrival_time(samples: &[Sample], to: Pt) -> f64 {
    samples
        .iter()
        .find(|s| (s.x - to.x).hypot(s.y - to.y) <= ARRIVAL_TOLERANCE_PT)
        .map_or_else(|| samples.last().map_or(0.0, |s| s.t), |s| s.t)
}

/// The original glide: Dubins arc-straight-arc path, smootherstep speed
/// envelope, then an arrival spring. Planned in anchor space like before
/// so its shape is unchanged. The spring uses the former macOS constants
/// (k = 400, c = 17 at the default `spring` 0.72, impulse 0.8) on every
/// platform, and `spring` scales the damping.
fn plan_classic(
    motion: &MotionConfig,
    req: &MoveRequest,
    target: [f64; 4],
    target_known: bool,
    timing: MotionTiming,
    fixed_ms: f64,
) -> Trajectory {
    const SPRING_K: f64 = 400.0;
    const SPRING_OVERSHOOT: f64 = 0.8;
    let spring_c = 17.0 * motion.spring / 0.72;
    let dt = DT_MS / 1000.0;
    let (ax, ay) = crate::anchor_for_pointer(req.from.x, req.from.y, req.from_heading);
    let (tx, ty) = crate::anchor_for_pointer(req.to.x, req.to.y, req.end_heading);
    let th0 = req.from_heading + PI;
    let th1 = req.end_heading + PI;
    let path = PathPlanner::plan(
        ax,
        ay,
        th0,
        tx,
        ty,
        th1,
        req.end_heading,
        motion.turn_radius,
    );
    let path_len = path.length.max(1.0);
    let fixed = timing == MotionTiming::Fixed;
    let mut out: Vec<Sample> = Vec::with_capacity(256);
    let push = |out: &mut Vec<Sample>, t: f64, x: f64, y: f64, heading: f64| {
        let (px, py) = crate::pointer_for_anchor(x, y, heading);
        out.push(Sample {
            t,
            x: px,
            y: py,
            heading,
        });
    };
    push(&mut out, 0.0, ax, ay, req.from_heading);
    let (mut d, mut t, mut speed) = (0.0f64, 0.0f64, 0.0f64);
    let mut guard = 0;
    while d < path_len && guard < 20_000 {
        guard += 1;
        let u = (d / path_len).min(1.0);
        let profile = (30.0 * u * u * (1.0 - u) * (1.0 - u)) / 1.875;
        let floor = if u < 0.5 {
            motion.min_start_speed
        } else {
            motion.min_end_speed
        };
        speed = if fixed {
            path_len / (fixed_ms / 1000.0)
        } else {
            floor + (motion.peak_speed - floor) * profile
        };
        d += speed * dt;
        t += dt;
        if d >= path_len {
            break;
        }
        let s = path.sample(d);
        push(&mut out, t, s.x, s.y, s.heading + PI);
    }
    let end = path.sample(path_len);
    push(&mut out, t, tx, ty, req.end_heading);
    let arrival_t = t;
    let impulse = if fixed { motion.min_end_speed } else { speed };
    let (mut ox, mut oy) = (0.0f64, 0.0f64);
    let mut vx = impulse * SPRING_OVERSHOOT * end.heading.cos();
    let mut vy = impulse * SPRING_OVERSHOOT * end.heading.sin();
    for _ in 0..600 {
        let sdt = dt / 4.0;
        for _ in 0..4 {
            vx += (-SPRING_K * ox - spring_c * vx) * sdt;
            vy += (-SPRING_K * oy - spring_c * vy) * sdt;
            ox += vx * sdt;
            oy += vy * sdt;
        }
        t += dt;
        if ox.hypot(oy) < 0.3 && vx.hypot(vy) < 2.0 {
            break;
        }
        push(&mut out, t, tx + ox, ty + oy, req.end_heading);
    }
    push(&mut out, t + dt, tx, ty, req.end_heading);
    let mut traj = Trajectory {
        samples: out,
        arrival_t,
        snap_t: None,
        target,
        target_known,
        effects: motion.resolved_effects(),
        style: MotionStyle::Classic,
    };
    if timing == MotionTiming::Fitts {
        let total = traj.duration();
        if total > 0.0 {
            let want = fitts_timing_ms(dist(req.from, req.to), target[2].min(target[3])) / 1000.0;
            let k = want / total;
            for s in &mut traj.samples {
                s.t *= k;
            }
            traj.arrival_t *= k;
        }
    }
    traj
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(from: Pt, to: Pt, target: Option<[f64; 4]>) -> MoveRequest {
        MoveRequest {
            from,
            from_heading: PI / 4.0,
            to,
            end_heading: PI / 4.0,
            target,
            seed: "test|0".into(),
            reduced_motion: false,
        }
    }

    fn motion(style: MotionStyle, timing: MotionTiming) -> MotionConfig {
        MotionConfig {
            style,
            timing,
            ..MotionConfig::default()
        }
    }

    const MOVES: [(Pt, Pt, [f64; 4]); 6] = [
        (
            Pt::new(100.0, 100.0),
            Pt::new(600.0, 420.0),
            [560.0, 400.0, 80.0, 40.0],
        ),
        (
            Pt::new(600.0, 420.0),
            Pt::new(120.0, 80.0),
            [100.0, 70.0, 40.0, 20.0],
        ),
        (
            Pt::new(50.0, 700.0),
            Pt::new(1240.0, 60.0),
            [1220.0, 50.0, 40.0, 20.0],
        ),
        (
            Pt::new(300.0, 300.0),
            Pt::new(310.0, 304.0),
            [305.0, 300.0, 10.0, 8.0],
        ),
        (
            Pt::new(800.0, 200.0),
            Pt::new(800.0, 640.0),
            [760.0, 620.0, 80.0, 40.0],
        ),
        (
            Pt::new(10.0, 10.0),
            Pt::new(10.0, 10.0),
            [0.0, 0.0, 20.0, 20.0],
        ),
    ];

    fn inside(p: Sample, r: [f64; 4]) -> bool {
        p.x >= r[0] - 0.01
            && p.x <= r[0] + r[2] + 0.01
            && p.y >= r[1] - 0.01
            && p.y <= r[1] + r[3] + 0.01
    }

    #[test]
    fn every_style_starts_at_from_ends_at_to_with_finite_increasing_samples() {
        for style in MotionStyle::ALL {
            for timing in [
                MotionTiming::Native,
                MotionTiming::Fitts,
                MotionTiming::Fixed,
            ] {
                for (from, to, target) in MOVES {
                    let traj = plan_move(&motion(style, timing), &req(from, to, Some(target)));
                    let first = traj.samples[0];
                    assert!(
                        (first.x - from.x).abs() < 1e-6 && (first.y - from.y).abs() < 1e-6,
                        "{style:?} start"
                    );
                    let end = traj.end();
                    assert!(
                        (end.x - to.x).abs() < 1e-6 && (end.y - to.y).abs() < 1e-6,
                        "{style:?} end"
                    );
                    assert!(inside(end, target), "{style:?} lands in target");
                    assert!(
                        (end.heading - PI / 4.0).abs() < 1e-9,
                        "{style:?} rests at the end heading"
                    );
                    for pair in traj.samples.windows(2) {
                        assert!(
                            pair[1].t > pair[0].t,
                            "{style:?}/{timing:?} time must increase"
                        );
                    }
                    for s in &traj.samples {
                        assert!(
                            s.x.is_finite()
                                && s.y.is_finite()
                                && s.t.is_finite()
                                && s.heading.is_finite()
                        );
                    }
                    assert!(traj.arrival_t <= traj.duration() + 1e-9);
                }
            }
        }
    }

    #[test]
    fn same_seed_gives_the_same_trajectory() {
        for style in MotionStyle::ALL {
            let r = req(Pt::new(50.0, 700.0), Pt::new(1240.0, 60.0), None);
            let a = plan_move(&motion(style, MotionTiming::Native), &r);
            let b = plan_move(&motion(style, MotionTiming::Native), &r);
            assert_eq!(a.samples, b.samples);
        }
    }

    #[test]
    fn peak_speed_and_overshoot_stay_bounded() {
        for style in MotionStyle::ALL {
            for (from, to, target) in MOVES {
                let traj = plan_move(
                    &motion(style, MotionTiming::Native),
                    &req(from, to, Some(target)),
                );
                let d = dist(from, to);
                let mut peak = 0.0f64;
                let mut over = 0.0f64;
                let u = if d > 1.0 {
                    Pt::new((to.x - from.x) / d, (to.y - from.y) / d)
                } else {
                    Pt::new(0.0, 0.0)
                };
                for pair in traj.samples.windows(2) {
                    let dt = pair[1].t - pair[0].t;
                    peak = peak.max((pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y) / dt);
                    over = over.max((pair[1].x - to.x) * u.x + (pair[1].y - to.y) * u.y);
                }
                assert!(peak < 20_000.0, "{style:?} peak {peak}");
                if style != MotionStyle::Classic {
                    assert!(over <= 14.0, "{style:?} overshoot {over}");
                }
            }
        }
    }

    #[test]
    fn fixed_timing_ignores_distance_and_fitts_grows_with_it() {
        let m = motion(MotionStyle::SignatureArc, MotionTiming::Fixed);
        let short = plan_move(&m, &req(Pt::new(0.0, 0.0), Pt::new(120.0, 0.0), None));
        let long = plan_move(&m, &req(Pt::new(0.0, 0.0), Pt::new(1400.0, 0.0), None));
        let move_time = |t: &Trajectory| {
            // Exclude the heading-settle tail.
            t.samples
                .iter()
                .rev()
                .find(|s| (s.x - t.end().x).abs() > 1e-9 || (s.y - t.end().y).abs() > 1e-9)
                .map_or(0.0, |s| s.t)
        };
        assert!((move_time(&short) - 1.43).abs() < 0.04 && (move_time(&long) - 1.43).abs() < 0.04);

        let m = motion(MotionStyle::SignatureArc, MotionTiming::Fitts);
        let near = plan_move(&m, &req(Pt::new(0.0, 0.0), Pt::new(120.0, 0.0), None));
        let far = plan_move(&m, &req(Pt::new(0.0, 0.0), Pt::new(1400.0, 0.0), None));
        assert!(far.arrival_t > near.arrival_t + 0.1);
        let small = plan_move(
            &m,
            &req(
                Pt::new(0.0, 0.0),
                Pt::new(600.0, 0.0),
                Some([595.0, -5.0, 10.0, 10.0]),
            ),
        );
        let big = plan_move(
            &m,
            &req(
                Pt::new(0.0, 0.0),
                Pt::new(600.0, 0.0),
                Some([500.0, -100.0, 200.0, 200.0]),
            ),
        );
        assert!(
            small.arrival_t > big.arrival_t,
            "smaller targets take longer"
        );
    }

    #[test]
    fn legacy_glide_duration_still_fixes_classic_and_new_styles() {
        for style in [MotionStyle::Classic, MotionStyle::SignatureArc] {
            let m = MotionConfig {
                style,
                glide_duration_ms: 300.0,
                ..MotionConfig::default()
            };
            for to in [Pt::new(120.0, 0.0), Pt::new(1400.0, 0.0)] {
                let traj = plan_move(
                    &m,
                    &MoveRequest {
                        from_heading: 0.0,
                        end_heading: 0.0,
                        ..req(Pt::new(0.0, 0.0), to, None)
                    },
                );
                assert!(
                    (traj.arrival_t - 0.3).abs() < 0.05,
                    "{style:?} arrival {}",
                    traj.arrival_t
                );
            }
        }
    }

    #[test]
    fn classic_speed_timing_grows_with_distance() {
        let m = motion(MotionStyle::Classic, MotionTiming::Native);
        let r = |x| MoveRequest {
            from_heading: 0.0,
            end_heading: 0.0,
            ..req(Pt::new(0.0, 0.0), Pt::new(x, 0.0), None)
        };
        assert!(plan_move(&m, &r(1400.0)).arrival_t > plan_move(&m, &r(120.0)).arrival_t + 0.2);
    }

    #[test]
    fn arrival_fires_before_the_follow_through_finishes() {
        let m = motion(MotionStyle::SpringSettle, MotionTiming::Native);
        let traj = plan_move(&m, &req(Pt::new(100.0, 100.0), Pt::new(700.0, 300.0), None));
        assert!(traj.arrival_t < traj.duration() - 0.05);
        let at = traj.sample_at(traj.arrival_t);
        assert!((at.x - 700.0).hypot(at.y - 300.0) <= 1.0);
    }

    #[test]
    fn adaptive_dispatches_on_target_size_and_distance() {
        let ctx = |to: Pt, w: f64| MoveCtx {
            from: Pt::new(0.0, 0.0),
            aim: to,
            target: [to.x - w / 2.0, to.y - w / 2.0, w, w],
        };
        assert_eq!(
            adaptive_pick(&ctx(Pt::new(300.0, 0.0), 10.0)),
            AdaptivePick::PreciseClick
        );
        assert_eq!(
            adaptive_pick(&ctx(Pt::new(1200.0, 0.0), 40.0)),
            AdaptivePick::KeynoteSwoop
        );
        assert_eq!(
            adaptive_pick(&ctx(Pt::new(300.0, 0.0), 40.0)),
            AdaptivePick::FittsMinJerk
        );
    }

    #[test]
    fn magnetic_reports_a_lock_on_time_inside_the_move() {
        let m = motion(MotionStyle::Magnetic, MotionTiming::Native);
        let traj = plan_move(&m, &req(Pt::new(100.0, 100.0), Pt::new(500.0, 300.0), None));
        let snap = traj.snap_t.expect("lock-on time");
        assert!(snap > 0.0 && snap < traj.duration());
    }

    #[test]
    fn reduced_motion_is_a_short_glide_without_effects() {
        let m = motion(MotionStyle::CometSwoop, MotionTiming::Native);
        let traj = plan_move(
            &m,
            &MoveRequest {
                reduced_motion: true,
                ..req(Pt::new(0.0, 0.0), Pt::new(900.0, 500.0), None)
            },
        );
        assert!(traj.duration() <= 0.13);
        assert_eq!(traj.effects, ResolvedEffects::NONE);
    }

    #[test]
    fn arc_size_knob_scales_the_arc() {
        let apex = |arc: f64| {
            let m = MotionConfig {
                arc_size: arc,
                ..motion(MotionStyle::SignatureArc, MotionTiming::Native)
            };
            let traj = plan_move(&m, &req(Pt::new(0.0, 0.0), Pt::new(800.0, 0.0), None));
            traj.samples.iter().map(|s| s.y.abs()).fold(0.0, f64::max)
        };
        assert!(apex(0.0) < 0.5, "arc_size 0 is straight");
        assert!(apex(0.5) > apex(0.25) * 1.8);
    }

    #[test]
    fn rng_matches_the_lab() {
        // Values from motion-lab/motion/rng.js: new Rng('7|keynote-swoop|0').
        assert_eq!(hash_string("abc"), 0x1a47_e90b);
        let mut rng = Rng::from_seed("7|keynote-swoop|0");
        let v = rng.next_f64();
        assert!((0.0..1.0).contains(&v));
    }
}
