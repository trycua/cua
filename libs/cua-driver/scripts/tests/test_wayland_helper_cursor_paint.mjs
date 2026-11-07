// Paints the GNOME Shell helper's cursor into a recording Cairo context and
// checks what lands on screen: the arrow's tip on the cursor point (CUA-3121),
// and one arrow and halo colour from rest, through a move, to the click.
import assert from 'node:assert/strict';
import { register } from 'node:module';

const extension = new URL('../../wayland-helper/winrects@cua/extension.js', import.meta.url);

const actors = [];
const timers = new Map();
let nextTimer = 1;
let currentContext = null;

class Actor {
  constructor(props = {}) {
    Object.assign(this, { x: 0, y: 0, opacity: 255, visible: false }, props);
    this.handlers = {};
    actors.push(this);
  }
  connect(signal, handler) { this.handlers[signal] = handler; return 1; }
  set_position(x, y) { this.x = x; this.y = y; }
  ease(props) { if ('x' in props) this.x = props.x; if ('y' in props) this.y = props.y; }
  show() { this.visible = true; }
  hide() { this.visible = false; }
  queue_repaint() {}
  remove_all_transitions() {}
  set_pivot_point() {}
  set_style() {}
  add_child() {}
  destroy() {}
  get_preferred_width() { return [0, 60]; }
  set_text(text) { this.text = text; }
  get_text() { return this.text ?? ''; }
  get_context() { return currentContext; }
}

// Cairo with a 2D affine matrix, recording each fill with its source colour
// and the device-space point its path started at.
class RecordingContext {
  constructor() {
    this.m = [1, 0, 0, 1, 0, 0];
    this.stack = [];
    this.path = [];
    this.source = null;
    this.fills = [];
    this.gradients = [];
    this.ops = 0;
  }
  apply(x, y) {
    const [a, b, c, d, e, f] = this.m;
    return [a * x + c * y + e, b * x + d * y + f];
  }
  multiply([a2, b2, c2, d2, e2, f2]) {
    const [a, b, c, d, e, f] = this.m;
    this.m = [a * a2 + c * b2, b * a2 + d * b2, a * c2 + c * d2, b * c2 + d * d2, a * e2 + c * f2 + e, b * e2 + d * f2 + f];
  }
  save() { this.stack.push([...this.m]); }
  restore() { this.m = this.stack.pop(); }
  translate(x, y) { this.multiply([1, 0, 0, 1, x, y]); }
  scale(x, y) { this.multiply([x, 0, 0, y, 0, 0]); }
  rotate(r) { this.multiply([Math.cos(r), Math.sin(r), -Math.sin(r), Math.cos(r), 0, 0]); }
  moveTo(x, y) { this.path.push(this.apply(x, y)); }
  lineTo(x, y) { this.path.push(this.apply(x, y)); }
  curveTo(_x1, _y1, _x2, _y2, x, y) { this.path.push(this.apply(x, y)); }
  arc(x, y) { this.path.push(this.apply(x, y)); }
  closePath() {}
  setSourceRGBA(r, g, b, a) { this.source = { rgb: [r, g, b].map((v) => Math.round(v * 255)), a }; }
  setSource(gradient) { this.source = { gradient }; this.gradients.push(gradient); }
  setSourceSurface() { this.source = { surface: true }; }
  paint() {}
  record() { this.ops++; this.fills.push({ source: this.source, start: this.path[0] }); }
  fill() { this.record(); this.path = []; }
  fillPreserve() { this.record(); }
  stroke() { this.ops++; this.path = []; }
  strokePreserve() { this.ops++; }
  setLineWidth() {}
  setLineCap() {}
  setLineJoin() {}
  $dispose() {}
}

class RadialGradient {
  constructor() { this.stops = []; }
  addColorStopRGBA(offset, r, g, b, a) {
    this.stops.push({ offset, rgb: [r, g, b].map((v) => Math.round(v * 255)), a });
  }
}

globalThis.helperMocks = {
  Gio: {
    _promisify() {},
    DBusExportedObject: { wrapJSObject: () => ({ export() {}, unexport() {} }) },
    DBus: { session: {} }, BusType: { SESSION: 0 }, BusNameOwnerFlags: { REPLACE: 0 },
    bus_own_name: () => 1, bus_unown_name() {},
  },
  GLib: {
    PRIORITY_DEFAULT: 0,
    SOURCE_REMOVE: false,
    SOURCE_CONTINUE: true,
    get_monotonic_time: () => globalThis.helperClock * 1_000_000,
    timeout_add: (_priority, _ms, callback) => { const id = nextTimer++; timers.set(id, callback); return id; },
    source_remove: (id) => { timers.delete(id); },
  },
  Shell: { Screenshot: class {} },
  St: { DrawingArea: Actor, Label: Actor, BoxLayout: Actor },
  Clutter: { ActorAlign: { CENTER: 0 }, AnimationMode: { EASE_OUT_CUBIC: 0 } },
  Cogl: {},
  cairo: {
    ImageSurface: class { finish() {} },
    Context: RecordingContext,
    RadialGradient,
    Format: { ARGB32: 0 },
    LineCap: { ROUND: 0 },
    LineJoin: { ROUND: 0 },
  },
  Main: { layoutManager: { addTopChrome() {} } },
};
register(`data:text/javascript,${encodeURIComponent(`
export async function resolve(specifier, context, next) {
  if (specifier.startsWith('gi://') || specifier.startsWith('resource:///') || specifier === 'cairo')
    return { url: 'mock:' + specifier, shortCircuit: true };
  return next(specifier, context);
}
export async function load(url, context, next) {
  if (url.startsWith('mock:')) {
    const name = url.slice(5);
    let source;
    if (name.endsWith('/extension.js')) source = 'export class Extension {}';
    else if (name.includes('/config.js')) source = 'export const PACKAGE_VERSION = "46"';
    else if (name.endsWith('/main.js')) source = 'export const layoutManager = globalThis.helperMocks.Main.layoutManager';
    else source = 'export default globalThis.helperMocks[' + JSON.stringify(name.replace('gi://', '')) + ']';
    return { format: 'module', source, shortCircuit: true };
  }
  const result = await next(url, context);
  if (new URL(url).pathname.endsWith('/extension.js')) return { ...result, format: 'module' };
  return result;
}
`)}`);

globalThis.helperClock = 100;
const { default: Extension } = await import(extension);
const helper = new Extension();
helper.enable();
const cursor = actors.find((actor) => actor.handlers.repaint && actor.width === 112);
assert.ok(cursor, 'the cursor actor');

// Paint one frame at `now`. The arrow body is the opaque fill in the session
// colour; its path starts at the tip.
function paint(now) {
  globalThis.helperClock = now;
  currentContext = new RecordingContext();
  cursor.handlers.repaint(cursor);
  const fill = helper._fillColor;
  const body = currentContext.fills.find(
    ({ source }) => source?.a === 1 && source.rgb?.every((v, i) => v === fill[i])
  );
  assert.ok(body, 'the arrow body is painted');
  const [tx, ty] = body.start;
  return {
    tip: [cursor.x + tx, cursor.y + ty],
    body: body.source.rgb,
    halo: currentContext.gradients.map((g) => g.stops.map((stop) => stop.rgb.join(','))).flat(),
  };
}
const near = (actual, expected, what) =>
  assert.ok(
    Math.abs(actual[0] - expected[0]) < 1e-6 && Math.abs(actual[1] - expected[1]) < 1e-6,
    `${what}: tip at ${actual} instead of ${expected}`
  );

const session = '#dd71ec';
helper.SetCursorColor(session);
const float = { secs: 4, sway: 1.6406, bob: 1.9688, lift: 1.6406, tilt: 0.04363, hold: 0.5, ease_in: 0.8, ease_out: 0.2 };
helper.SetCursorLook(JSON.stringify({
  float,
  halo: { rgb: session, radius: 14, peak: 0.11, stops: [[0, 1], [0.45, 0.55], [1, 0]], body: [2, 11.9] },
}));

// Rest: placed and settled, so the tip is exactly on the point.
helper.PlayCursorPath(JSON.stringify({ samples: [[0, 300, 200, 0, 0, 0]], arrival: null, rest: [0, 0, 0] }));
const rest = paint(100);
near(rest.tip, [300, 200], 'at rest');

// Travel: the float moves the whole cursor and the heading turns it about
// its tip, so the tip sits on the planned point plus the float.
const samples = [
  [0, 300, 200, 0, 0, 0],
  [0.2, 600, 450, 1.2, -1.6, 0.6],
  [0.4, 900, 600, 0, 0, 0],
  [0.5, 900, 600, 0, 0, 0],
];
helper.PlayCursorPath(JSON.stringify({ samples, arrival: 0.4, rest: [0.5, 0.1, 0] }));
const [frameTimer] = [...timers.entries()].at(-1);
globalThis.helperClock = 100.2;
timers.get(frameTimer)();
const travel = paint(100.2);
near(travel.tip, [600 + 1.2, 450 - 1.6], 'while travelling');

// Arrival: settled and upright on the destination.
globalThis.helperClock = 100.4;
timers.get(frameTimer)();
near(paint(100.4).tip, [900, 600], 'on arrival');
globalThis.helperClock = 100.5;
timers.get(frameTimer)();

// The click lands where the tip points, also for a click that snaps.
helper.ClickPulse(900, 600);
const click = paint(100.55);
near(click.tip, [900, 600], 'clicking the settled point');
helper.ClickPulse(1234, 567);
near(paint(100.6).tip, [1234, 567], 'clicking elsewhere');

// One colour throughout: the arrow and the halo never change from rest,
// through travel, to arrival and the click.
for (const [what, frame] of Object.entries({ travel, click })) {
  assert.deepEqual(frame.body, rest.body, `arrow colour ${what}`);
  assert.deepEqual(frame.halo, rest.halo, `halo colour ${what}`);
}
assert.deepEqual(rest.body, [0xdd, 0x71, 0xec]);
assert.ok(rest.halo.length > 0 && rest.halo.every((rgb) => rgb === '221,113,236'), 'halo in the session colour');

// A self-played click (no `cursor.marks`) squishes the arrow about its tip
// but paints no click marks.
const drawOps = () => {
  currentContext = new RecordingContext();
  cursor.handlers.repaint(cursor);
  return currentContext.ops;
};
globalThis.helperClock = 101;
helper.ClickPulse(1234, 567);
globalThis.helperClock = 101.15;
near(paint(101.15).tip, [1234, 567], 'mid-squish');
const selfPlayed = drawOps();
helper.SetCursorState('idle', '', '', true);
const idle = drawOps();
assert.equal(selfPlayed, idle, 'no click marks without cursor.marks');
// A click cue cua-driver asks for still draws its marks.
helper.SetCursorState('click', '', '', true);
globalThis.helperClock = 101.3;
assert.ok(drawOps() > idle, 'requested click marks are drawn');

helper.disable();
console.log('helper cursor paint: ok');
