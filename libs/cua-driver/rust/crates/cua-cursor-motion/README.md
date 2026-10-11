# cua-cursor-motion

The agent cursor motions of [Cua Driver](https://cua.ai/docs/cua-driver) as a
small Rust library: six motion styles, three timing modes, the comet trail and
the other effects, and an API for designing your own motions.

Cua Driver plans every agent cursor move with this crate, and the
[`@trycua/cursor-motion`](https://github.com/trycua/cua/tree/main/libs/typescript/cursor-motion) web package is a port that is
tested against the same golden trajectories. There is no runtime dependency;
the `serde` feature adds `Serialize`/`Deserialize` to the public types.

## Use it

Cua Cursor Motion is not published to crates.io. It lives in the
[trycua/cua](https://github.com/trycua/cua) repository; add it as a git
dependency:

```toml
[dependencies]
cua-cursor-motion = { git = "https://github.com/trycua/cua" }
# Pin a commit for reproducible builds, and add features = ["serde"] if needed:
# cua-cursor-motion = { git = "https://github.com/trycua/cua", rev = "<commit>", features = ["serde"] }
```

Cargo finds the crate inside the repository's Cua Driver workspace by name.

## Plan a move

A move is planned once, as samples at 120 Hz. Play it back by time at any
frame rate:

```rust
use cua_cursor_motion::{plan_move, MotionParams, MotionStyle, MotionTiming, MoveRequest, Pt};

let params = MotionParams {
    style: MotionStyle::CometSwoop,
    timing: MotionTiming::Fitts,
    ..Default::default()
};
let request = MoveRequest {
    target: Some([560.0, 400.0, 80.0, 40.0]), // the element's rect, if known
    seed: "my-cursor|1".into(),               // same seed, same motion
    ..MoveRequest::new(Pt::new(100.0, 100.0), Pt::new(600.0, 420.0))
};
let trajectory = plan_move(&params, &request);

let t = 0.25; // seconds into the move
let s = trajectory.sample_at(t); // hotspot x, y and the arrow's heading
assert!(trajectory.arrival_t <= trajectory.duration());
```

`arrival_t` is when the tip first reaches the target. Cua Driver clicks then,
and lets any follow-through or settle finish during the click.

| Style | Feel |
| --- | --- |
| `signature_arc` | One confident arc with a small follow-through. The default. |
| `spring_settle` | An arc that lands with one soft bounce. |
| `magnetic` | Slows near the target, then is pulled in. |
| `comet_swoop` | A wide arc with a short trail. |
| `adaptive` | Careful for small targets, a swoop for long moves. |
| `classic` | The original Dubins glide with an arrival spring. |

Timing is `native` (the style's own), `fitts`
(`150 + 120 log2(D / W + 1)` ms, 300 to 1000 ms, where `W` is the target's
smaller side) or `fixed` (`glide_duration_ms`, 1430 ms when 0).

## Effects

`effects` returns the geometry to paint at a moment of the move: the comet
trail, the speed glow, the magnet glow, the click ripple and the click squish.
Painting is up to you.

```rust
# use cua_cursor_motion::{plan_move, MotionParams, MoveRequest, Pt};
use cua_cursor_motion::{anchor_for_pointer, effects};
# let trajectory = plan_move(&MotionParams::default(), &MoveRequest::new(Pt::new(0.0, 0.0), Pt::new(500.0, 200.0)));
let t = 0.3;
let s = trajectory.sample_at(t);
let body = anchor_for_pointer(s.x, s.y, s.heading); // 16 pt behind the tip
let frame = effects::motion_frame(&trajectory, t, body, true);
for seg in &frame.trail {
    // stroke seg.a -> seg.b, round caps, seg.width wide, seg.alpha opaque
}
```

The trail is anchored on the arrow's body, 16 pt behind the tip along the
arrow's axis, and drawn under the cursor, so it flows out from behind the
arrow and the tip stays clean.

## Design your own motion

A `MotionSpec` describes a motion as a path shape, a speed curve, an overshoot
or settle, a duration model, a heading mode, effects and a trail.
`signature_arc`, `spring_settle` and `comet_swoop` are specs themselves, so
start from one of them:

```rust
use cua_cursor_motion::{
    plan_spec, ArcShape, Duration, Ease, MotionParams, MotionSpec, MotionStyle, MoveRequest,
    PathShape, Pt, Settle, TrailSpec,
};

let mut spec = MotionSpec::for_style(MotionStyle::SpringSettle, &MotionParams::default()).unwrap();
spec.path = PathShape::Arc(ArcShape { start_handle: 0.2, end_handle: 0.4, arc_size: 0.3, arc_flow: 0.5 });
spec.ease = Ease::CubicBezier { x1: 0.3, y1: 0.0, x2: 0.1, y2: 1.0 };
spec.settle = Settle::Spring { amount: 0.06, max_pt: 10.0, cycles: 2.0, decay: 3.0, start: 0.5, glide_end: 0.6 };
spec.duration = Duration::Fitts { a: 150.0, b: 120.0, min_ms: 300.0, max_ms: 1000.0, scale: 1.3 };
spec.effects.trail = true;
spec.trail = TrailSpec { secs: 0.25, ..TrailSpec::default() };

let trajectory = plan_spec(&spec, &MoveRequest::new(Pt::new(100.0, 500.0), Pt::new(900.0, 200.0)));
```

| Part | Options |
| --- | --- |
| `path` | `Straight`, `Arc` (the Cua cubic bezier: handles, size, flow), `Bow` (a gentle quadratic) |
| `ease` | `Linear`, `MinJerk`, `Smootherstep`, `InOutCubic`, `InOutSine`, `OutCubic`, `CubicBezier` |
| `settle` | `None`, `FollowThrough` (overshoot and return), `Spring` (damped wobble) |
| `duration` | `Fixed`, `Fitts`, `Distance` |
| `heading` | `Tangent` (the tip leads), `Fixed` (rest pose) |

Custom specs run in your own renderer. Cua Driver itself takes one of the six
styles plus the knobs of `set_agent_cursor_motion`.

## Tests

```bash
cd libs/cua-driver/rust
cargo test -p cua-cursor-motion --all-features
```

- `tests/motion_parity.rs` checks every style and timing mode against the
  motion lab (`libs/cua-driver/tools/cursor-gallery/motion-lab/`), the design
  source, within 0.5 pt.
- `tests/golden.rs` checks `fixtures/golden.json`, the trajectories that the
  TypeScript port must reproduce. Regenerate it only for an intended motion
  change:

  ```bash
  cargo run -p cua-cursor-motion --features serde --example export_golden \
    > crates/cua-cursor-motion/fixtures/golden.json
  ```

- `cursor-overlay`'s `motion_is_bit_identical_to_the_pre_cua_cursor_motion_driver`
  proves the driver's trajectories and effect frames did not change when the
  math moved here.

## License

MIT.
