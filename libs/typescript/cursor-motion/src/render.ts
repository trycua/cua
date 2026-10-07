/**
 * A small canvas renderer that plays motions with the Cua Driver cursor.
 *
 * It draws inside a `<canvas>` only. It never moves, hides or restyles the
 * page's real mouse pointer.
 */
import {
  addClick,
  CLICK_FX_SECS,
  effectRgb,
  emptyFrame,
  linger,
  MAGNET_INFLATE,
  motionFrame,
  type EffectFrame,
} from './effects';
import { anchorForPointer, type Pt, type Rect } from './geom';
import type { MotionParams } from './params';
import { planMove, planSpec, REST_HEADING, type Trajectory } from './plan';
import type { MotionSpec } from './spec';
import { NO_EFFECTS, type ResolvedEffects } from './style';

/**
 * The Cua Driver cursor body (`cursor-overlay/assets/build_default_theme.py`
 * `CURSOR_PATH`) on its 128-unit canvas, hotspot at (55, 30).
 */
export const CURSOR_PATH =
  'M55 30 C48 28 42 33 43 41 C43 41 64 98 64 98 C67 106 73 106 77 99 C77 99 86 79 86 79 C88 75 91 72 95 70 C95 70 108 63 108 63 C115 59 114 53 107 50 C107 50 55 30 55 30 Z';
export const CURSOR_HOTSPOT: Pt = { x: 55, y: 30 };
const CANVAS_SIZE = 128;
/** On-screen size of the 128-unit cursor canvas, pt (Cua Driver's `DISPLAY_SIZE`). */
export const CURSOR_DISPLAY_SIZE = 42;
/** Cua blue, the default cursor fill. */
export const CUA_BLUE = '#5ec0e8';
/** The glow strokes under the body: width (canvas units) and opacity. */
const GLOW_LAYERS: [number, number][] = [
  [44, 0.02],
  [36, 0.024],
  [29, 0.03],
  [23, 0.038],
  [18, 0.048],
  [14, 0.06],
  [10, 0.075],
  [7, 0.095],
];
const FLOAT_SECS = 4;

let cursorPath: Path2D | null = null;

function hexRgb(hex: string): [number, number, number] {
  const h = hex.replace('#', '');
  const n = parseInt(h.length === 3 ? h.replace(/./g, (c) => c + c) : h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export interface CursorStyle {
  /** Body fill. Default Cua blue. */
  fill?: string;
  /** Scale on top of the 42 pt display size. */
  scale?: number;
  /** Click squish, 0..1 of the size. */
  squish?: number;
  alpha?: number;
  /** Seconds into the driver's gentle float (it restarts with each move), or null for none. */
  floatT?: number | null;
}

/** Draw the Cua cursor with its tip on `(x, y)`, rotated to `heading` (rest = pi/4). */
export function drawCursor(
  g: CanvasRenderingContext2D,
  x: number,
  y: number,
  heading: number,
  style: CursorStyle = {}
): void {
  const fill = style.fill ?? CUA_BLUE;
  const s = (CURSOR_DISPLAY_SIZE / CANVAS_SIZE) * (style.scale ?? 1) * (1 - (style.squish ?? 0));
  let fx = 0;
  let fy = 0;
  let fr = 0;
  if (style.floatT != null) {
    const a = ((style.floatT % FLOAT_SECS) / FLOAT_SECS) * 2 * Math.PI;
    fx = Math.sin(a) * 5;
    fy = 6 * Math.cos(a) - 5;
    fr = ((2.5 * Math.PI) / 180) * Math.cos(a);
  }
  cursorPath ??= new Path2D(CURSOR_PATH);
  g.save();
  g.globalAlpha = style.alpha ?? 1;
  g.translate(x + fx * s, y + fy * s);
  g.rotate(heading - Math.PI / 4 + fr);
  g.scale(s, s);
  g.translate(-CURSOR_HOTSPOT.x, -CURSOR_HOTSPOT.y);
  g.lineCap = 'round';
  g.lineJoin = 'round';
  for (const [width, opacity] of GLOW_LAYERS) {
    g.globalAlpha = (style.alpha ?? 1) * opacity;
    g.strokeStyle = fill;
    g.lineWidth = width;
    g.stroke(cursorPath);
  }
  g.globalAlpha = style.alpha ?? 1;
  g.fillStyle = fill;
  g.fill(cursorPath);
  g.strokeStyle = '#ffffff';
  g.lineWidth = 5;
  g.stroke(cursorPath);
  g.restore();
}

const rgba = (c: [number, number, number], a: number) =>
  `rgba(${c[0]},${c[1]},${c[2]},${Math.min(Math.max(a, 0), 1)})`;

/** Paint the effects that sit under the cursor: glow, trail, magnet. */
export function drawEffectsUnder(
  g: CanvasRenderingContext2D,
  f: EffectFrame,
  fill = CUA_BLUE
): void {
  const c = effectRgb(hexRgb(fill));
  if (f.glow) {
    const grad = g.createRadialGradient(f.glow.x, f.glow.y, 0, f.glow.x, f.glow.y, f.glow.r);
    grad.addColorStop(0, rgba(c, f.glow.alpha));
    grad.addColorStop(1, rgba(c, 0));
    g.fillStyle = grad;
    g.beginPath();
    g.arc(f.glow.x, f.glow.y, f.glow.r, 0, 2 * Math.PI);
    g.fill();
  }
  g.lineCap = 'round';
  for (const seg of f.trail) {
    g.strokeStyle = rgba(c, seg.alpha);
    g.lineWidth = seg.width;
    g.beginPath();
    g.moveTo(seg.a[0], seg.a[1]);
    g.lineTo(seg.b[0], seg.b[1]);
    g.stroke();
  }
  if (f.magnet) {
    const [x, y, w, h] = f.magnet.rect;
    const i = MAGNET_INFLATE;
    for (const [width, a] of [
      [14, 0.1],
      [8, 0.22],
      [3, 0.9],
    ] as const) {
      g.strokeStyle = rgba(c, a * f.magnet.glow);
      g.lineWidth = width;
      g.beginPath();
      g.roundRect(
        x - i,
        y - i,
        w + 2 * i,
        h + 2 * i,
        Math.min(8, (w + 2 * i) / 2, (h + 2 * i) / 2)
      );
      g.stroke();
    }
  }
}

/** Paint the click ripple, which sits over the cursor. */
export function drawEffectsOver(
  g: CanvasRenderingContext2D,
  f: EffectFrame,
  fill = CUA_BLUE
): void {
  if (!f.ripple) return;
  g.strokeStyle = rgba(effectRgb(hexRgb(fill)), f.ripple.alpha);
  g.lineWidth = f.ripple.width;
  g.beginPath();
  g.arc(f.ripple.x, f.ripple.y, f.ripple.r, 0, 2 * Math.PI);
  g.stroke();
}

export interface MoveOptions {
  /** A built-in style and its knobs. */
  params?: Partial<MotionParams>;
  /** Or a custom motion. */
  spec?: MotionSpec;
  /** Target rect, for Fitts timing and the magnet glow. */
  target?: Rect | null;
  /** Click on arrival (ripple and squish when the motion has them). */
  click?: boolean;
  reducedMotion?: boolean;
}

export interface PlayerOptions {
  fill?: string;
  /** Called before the cursor each frame, in canvas points; draw your scene here. */
  drawScene?: (g: CanvasRenderingContext2D, player: MotionPlayer) => void;
  /** The driver's gentle float. Default true. */
  float?: boolean;
}

/**
 * Plays motions on a canvas. Coordinates are CSS pixels of the canvas.
 *
 * ```ts
 * const player = new MotionPlayer(canvas);
 * player.place({ x: 80, y: 80 });
 * await player.moveTo({ x: 600, y: 320 }, { params: { style: 'comet_swoop' }, click: true });
 * ```
 */
export class MotionPlayer {
  /** Hotspot. */
  pos: Pt = { x: 0, y: 0 };
  heading = REST_HEADING;
  trajectory: Trajectory | null = null;
  /** Playback speed; 0.25 plays at quarter speed. */
  timeScale = 1;
  private motionT = 0;
  private arrival: (() => void) | null = null;
  private clickAge: number | null = null;
  private clickPoint: [number, number] = [0, 0];
  private clickEffects: ResolvedEffects = { ...NO_EFFECTS };
  private pendingClick = false;
  private idleT = 0;
  private last = 0;
  private raf = 0;
  private moves = 0;
  private readonly g: CanvasRenderingContext2D;

  constructor(
    readonly canvas: HTMLCanvasElement,
    readonly options: PlayerOptions = {}
  ) {
    const g = canvas.getContext('2d');
    if (!g) throw new Error('canvas 2d context unavailable');
    this.g = g;
    this.raf = requestAnimationFrame(this.frame);
  }

  /** Put the cursor at rest on `p` without motion. */
  place(p: Pt): void {
    const pending = this.arrival;
    this.arrival = null;
    pending?.();
    this.pos = { ...p };
    this.heading = REST_HEADING;
    this.trajectory = null;
  }

  /** Move to `to`; resolves when the tip arrives (any settle keeps playing). */
  moveTo(to: Pt, opts: MoveOptions = {}): Promise<void> {
    this.arrival?.();
    const req = {
      from: this.pos,
      fromHeading: this.heading,
      to,
      target: opts.target ?? null,
      seed: `player|${++this.moves}`,
      reducedMotion: opts.reducedMotion ?? false,
    };
    this.trajectory = opts.spec ? planSpec(opts.spec, req) : planMove(opts.params ?? {}, req);
    this.motionT = 0;
    this.idleT = 0;
    this.pendingClick = !!opts.click;
    return new Promise((resolve) => (this.arrival = resolve));
  }

  /** Show a click at the current tip. */
  click(
    effects: ResolvedEffects = this.trajectory?.effects ?? {
      ...NO_EFFECTS,
      ripple: true,
      squish: true,
    }
  ): void {
    this.clickAge = 0;
    this.clickPoint = [this.pos.x, this.pos.y];
    this.clickEffects = effects;
  }

  destroy(): void {
    cancelAnimationFrame(this.raf);
  }

  private frame = (now: number) => {
    const dt = this.last ? Math.min((now - this.last) / 1000, 0.1) : 0;
    this.last = now;
    this.tick(dt * this.timeScale);
    this.paint();
    this.raf = requestAnimationFrame(this.frame);
  };

  private tick(dt: number) {
    const traj = this.trajectory;
    if (traj) {
      this.motionT += dt;
      const s = traj.sampleAt(this.motionT);
      this.pos = { x: s.x, y: s.y };
      this.heading = s.heading;
      if (this.arrival && this.motionT >= traj.arrivalT) {
        if (this.pendingClick) {
          this.pendingClick = false;
          this.click(traj.effects);
        }
        const done = this.arrival;
        this.arrival = null;
        done();
      }
      if (this.motionT >= linger(traj)) this.trajectory = null;
    }
    this.idleT += dt;
    if (this.clickAge !== null) {
      this.clickAge += dt;
      if (this.clickAge >= CLICK_FX_SECS) this.clickAge = null;
    }
  }

  /** Effects at the current moment. */
  effectFrame(): EffectFrame {
    const frame = this.trajectory
      ? motionFrame(
          this.trajectory,
          this.motionT,
          anchorForPointer(this.pos.x, this.pos.y, this.heading)
        )
      : emptyFrame();
    if (this.clickAge !== null) addClick(frame, this.clickEffects, this.clickAge, this.clickPoint);
    return frame;
  }

  private paint() {
    const { canvas, g } = this;
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, h);
    this.options.drawScene?.(g, this);
    const fill = this.options.fill ?? CUA_BLUE;
    const f = this.effectFrame();
    drawEffectsUnder(g, f, fill);
    drawCursor(g, this.pos.x, this.pos.y, this.heading, {
      fill,
      squish: f.squish,
      floatT: this.options.float === false ? null : this.idleT,
    });
    drawEffectsOver(g, f, fill);
  }
}
