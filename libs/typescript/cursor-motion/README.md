# @trycua/cursor-motion

The agent cursor motions of [Cua Driver](https://cua.ai/docs/cua-driver) for
the web: six motion styles, Fitts timing, the comet trail and the other
effects, an API for designing your own motions, and a small canvas player
that draws the real Cua cursor.

It is a TypeScript port of the Rust crate
[`cua-cursor-motion`](https://github.com/trycua/cua/tree/main/libs/cua-driver/rust/crates/cua-cursor-motion),
which Cua Driver uses for every move. Both are tested against the same golden
trajectories, so a motion you design here plays the same in the driver. No
runtime dependencies; about 14 KB gzipped.

## Use it

Cua Cursor Motion is not published to npm. It lives in the
[trycua/cua](https://github.com/trycua/cua) repository. Build it once from a
checkout and copy the single-file ES module and its types into your project:

```bash
git clone --depth 1 https://github.com/trycua/cua
cd cua/libs/typescript
pnpm install
pnpm --filter @trycua/cursor-motion build
cp cursor-motion/dist/index.js   ../../../my-app/src/vendor/cua-cursor-motion.js
cp cursor-motion/dist/index.d.ts ../../../my-app/src/vendor/cua-cursor-motion.d.ts
```

```ts
import { MotionPlayer, planMove } from './vendor/cua-cursor-motion.js';
```

The file has no imports, so it also works from a plain `<script type="module">`.
In a TypeScript project with a bundler you can instead copy
`libs/typescript/cursor-motion/src/` into your source tree and import the
folder. `tests/vendoring.test.ts` checks both ways.

## Play a motion

```ts
import { MotionPlayer } from './vendor/cua-cursor-motion.js';

const player = new MotionPlayer(document.querySelector('canvas')!);
player.place({ x: 80, y: 300 });
await player.moveTo(
  { x: 640, y: 120 },
  { params: { style: 'comet_swoop' }, target: [600, 104, 80, 32], click: true }
);
```

`moveTo` resolves when the tip reaches the target; any follow-through or
settle keeps playing during the click, as in the driver. The cursor is drawn
inside the canvas only. The player never moves, hides or restyles the page's
real mouse pointer.

## Plan a move yourself

```ts
import { planMove, effects, anchorForPointer } from './vendor/cua-cursor-motion.js';

const trajectory = planMove(
  { style: 'spring_settle', timing: 'fitts' },
  { from: { x: 100, y: 100 }, to: { x: 700, y: 400 }, target: [660, 380, 80, 40] }
);

const t = 0.3; // seconds
const s = trajectory.sampleAt(t); // hotspot x, y and the arrow's heading
const frame = effects.motionFrame(trajectory, t, anchorForPointer(s.x, s.y, s.heading));
// frame.trail, frame.glow, frame.magnet: geometry to paint your way
```

| Style           | Feel                                                        |
| --------------- | ----------------------------------------------------------- |
| `signature_arc` | One confident arc with a small follow-through. The default. |
| `spring_settle` | An arc that lands with one soft bounce.                     |
| `magnetic`      | Slows near the target, then is pulled in.                   |
| `comet_swoop`   | A wide arc with a short trail.                              |
| `adaptive`      | Careful for small targets, a swoop for long moves.          |
| `classic`       | The original Dubins glide with an arrival spring.           |

Timing is `native`, `fitts` (`150 + 120 log2(D / W + 1)` ms, 300 to 1000 ms)
or `fixed` (`glideDurationMs`, 1430 ms when 0). The other knobs (`arcSize`,
`arcFlow`, `startHandle`, `endHandle`, `spring`, `turnRadius`) match Cua
Driver's `set_agent_cursor_motion`.

## Design your own motion

A `MotionSpec` is a path shape, a speed curve, an overshoot or settle, a
duration model, a heading mode, effects and a trail. The arc styles are
specs, so start from one:

```ts
import { planSpec, specForStyle } from './vendor/cua-cursor-motion.js';

const spec = specForStyle('spring_settle')!;
spec.ease = { type: 'cubic_bezier', x1: 0.3, y1: 0, x2: 0.1, y2: 1 };
spec.settle = {
  type: 'spring',
  amount: 0.06,
  maxPt: 10,
  cycles: 2,
  decay: 3,
  start: 0.5,
  glideEnd: 0.6,
};
spec.effects.trail = true;
spec.trail.secs = 0.25;

const trajectory = planSpec(spec, { from: { x: 100, y: 500 }, to: { x: 900, y: 200 } });
```

| Part       | Options                                                                                          |
| ---------- | ------------------------------------------------------------------------------------------------ |
| `path`     | `straight`, `arc` (the Cua cubic bezier), `bow`                                                  |
| `ease`     | `linear`, `min_jerk`, `smootherstep`, `in_out_cubic`, `in_out_sine`, `out_cubic`, `cubic_bezier` |
| `settle`   | `none`, `follow_through`, `spring`                                                               |
| `duration` | `fixed`, `fitts`, `distance`                                                                     |
| `heading`  | `tangent` (the tip leads), `fixed` (rest pose)                                                   |

Custom specs play in your renderer. Cua Driver takes one of the six styles;
`driverSnippets(params)` returns the matching `cua-driver config set` and
`cua-driver cursor motion` commands and the `set_agent_cursor_motion`
arguments.

## Playground

Pick a style, drag the start and end points, tune the curve and copy the
config:

```bash
cd libs/typescript
pnpm install
pnpm --filter @trycua/cursor-motion playground   # http://127.0.0.1:4173/playground/
```

## Tests

```bash
pnpm --filter @trycua/cursor-motion test
```

`tests/golden.test.ts` replays every case in
`libs/cua-driver/rust/crates/cua-cursor-motion/fixtures/golden.json` (all styles,
timings and custom specs, with effect frames) and must match the Rust crate
within 1e-6 pt. Today the worst deviation is 5e-10 pt.

## License

MIT
