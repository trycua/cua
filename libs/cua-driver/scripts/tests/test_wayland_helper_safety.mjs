import assert from 'node:assert/strict';
import { register } from 'node:module';
import { readFile } from 'node:fs/promises';

const extension = new URL('../../wayland-helper/winrects@cua/extension.js', import.meta.url);
const counts = {};
const bump = key => { counts[key] = (counts[key] ?? 0) + 1; return 1; };
const stub = new Proxy(function () {}, {
  get: () => stub,
  apply: () => stub,
  construct: () => stub,
});
globalThis.helperMocks = {
  Gio: {
    _promisify() {},
    DBusExportedObject: { wrapJSObject: () => {
      bump('wrap');
      return { export: () => bump('export'), unexport: () => bump('unexport') };
    } },
    DBus: { session: {} }, BusType: { SESSION: 0 }, BusNameOwnerFlags: { REPLACE: 0 },
    bus_own_name: () => bump('own'), bus_unown_name: () => bump('unown'),
  },
  GLib: {
    get_monotonic_time: () => (globalThis.helperClock ?? 0) * 1_000_000,
    timeout_add: () => bump('timer'), source_remove: () => bump('remove'),
  },
  Shell: { Screenshot: class {} },
  St: new Proxy({}, { get: () => class { constructor() { bump('actor'); return stub; } } }),
  Clutter: stub, Cogl: stub,
  cairo: new Proxy(function () {}, {
    get: () => globalThis.helperMocks.cairo,
    construct: () => { bump('cairo'); return stub; },
  }),
  Main: { layoutManager: stub },
};
register(`data:text/javascript,${encodeURIComponent(`
export async function resolve(specifier, context, next) {
  if (specifier.startsWith('gi://') || specifier.startsWith('resource:///') || specifier === 'cairo')
    return { url: 'mock:' + specifier + (specifier.endsWith('/config.js') ? new URL(context.parentURL).search : ''), shortCircuit: true };
  return next(specifier, context);
}
export async function load(url, context, next) {
  if (url.startsWith('mock:')) {
    const name = url.slice(5);
    let source;
    if (name.endsWith('/extension.js')) source = 'export class Extension {}';
    else if (name.includes('/config.js')) source = 'export const PACKAGE_VERSION = globalThis.shellVersion';
    else if (name.endsWith('/main.js')) source = 'export const layoutManager = globalThis.helperMocks.Main.layoutManager';
    else source = 'export default globalThis.helperMocks[' + JSON.stringify(name.replace('gi://', '')) + ']';
    return { format: 'module', source, shortCircuit: true };
  }
  const result = await next(url, context);
  if (new URL(url).pathname.endsWith('/extension.js')) return { ...result, format: 'module' };
  return result;
}
`)}`);

for (const version of ['50', '50.1', '50.beta', '50.rc', '51', '45', '46', '47', '48', '49']) {
  globalThis.shellVersion = version;
  const { default: Extension } = await import(`${extension}?${version}`);
  const helper = new Extension();
  for (const key of Object.keys(counts)) delete counts[key];
  if (Number.parseInt(version, 10) >= 50) {
    assert.throws(() => helper.enable(), {
      message: 'winrects@cua is unsupported on GNOME Shell 50 or newer for session safety',
    });
    assert.deepEqual(counts, {});
    helper.disable();
    assert.deepEqual(counts, {});
  } else {
    helper.enable();
    assert.equal(helper.GetVersion(), 12);
    assert.equal(counts.export, 1);
    assert.equal(counts.own, 1);
    assert.equal(counts.actor, 5);
    assert.equal(counts.timer, 1);
    helper.disable();
    assert.equal(counts.unexport, 1);
    assert.equal(counts.unown, 1);
    assert.equal(counts.remove, 1);
  }
}
const metadata = JSON.parse(await readFile(new URL('metadata.json', extension)));
assert.deepEqual(metadata['shell-version'], ['45', '46', '47', '48', '49']);

// API 11: the helper plays cua-driver's planned poses and paints its halo.
{
  globalThis.shellVersion = '49';
  globalThis.helperClock = 100;
  const { default: Extension } = await import(`${extension}?poses`);
  const helper = new Extension();
  helper.enable();
  for (const key of Object.keys(counts)) delete counts[key];
  const near = (actual, expected) => assert.ok(Math.abs(actual - expected) < 1e-9, `${actual} != ${expected}`);

  // Until a look arrives the older glow stays; a bad look is ignored.
  assert.equal(helper._look, null);
  helper.SetCursorLook('not json');
  const float = { secs: 4, sway: 1.6406, bob: 1.9688, lift: 1.6406, tilt: 0.04363, hold: 0.5, ease_in: 0.8, ease_out: 0.2 };
  helper.SetCursorLook(JSON.stringify({ float: { ...float, sway: -1 }, halo: null }));
  helper.SetCursorLook(JSON.stringify({ float: { ...float, ease_out: 0 }, halo: null }));
  assert.equal(helper._look, null);
  const look = {
    float,
    halo: { rgb: '#5ec0e8', radius: 14, peak: 0.11, stops: [[0, 1], [0.45, 0.55], [1, 0]], body: [2, 11.9] },
  };
  helper.SetCursorLook(JSON.stringify(look));
  assert.deepEqual(helper._look.halo.rgb, [0x5e, 0xc0, 0xe8]);
  helper.SetCursorLook(JSON.stringify({ ...look, halo: null }));
  assert.equal(helper._look.halo, null, 'cursor.halo off');
  helper.SetCursorLook(JSON.stringify(look));

  // Before any track a fresh cursor floats fully: the historical loop.
  near(helper._pose(100).dy, float.bob - float.lift);
  near(helper._pose(100).rotation, float.tilt);
  near(helper._pose(101).dx, float.sway);
  near(helper._pose(102).dy, -float.bob - float.lift);

  // A move: poses interpolate, and the float is exactly 0 at the arrival.
  const samples = [
    [0, 100, 100, 0.5, -1, 0],
    [0.2, 300, 200, 1, -2.2, 0.4],
    [0.4, 500, 300, 0, 0, 0],
    [0.5, 501, 300, 0, 0, 0],
  ];
  // The float clock at the last sample: phase 7.3 s, held 0.1 s from 0.
  const rest = [7.3, 0.1, 0];
  helper.PlayCursorPath(JSON.stringify({ samples, arrival: 0.4, rest }));
  assert.equal(counts.timer, 1, 'one frame timer while the path plays');
  assert.ok(helper._path);
  near(helper._pose(100).dy, -1);
  near(helper._pose(100.1).dx, 0.75);
  near(helper._pose(100.1).dy, -1.6);
  near(helper._pose(100.1).rotation, 0.2);
  near(helper._pose(100.4).dy, 0);
  near(helper._pose(100.4).dx, 0);
  near(helper._pose(100.4).rotation, 0);
  near(helper._rest.at, 100.5);

  // Malformed tracks change nothing.
  for (const bad of [
    '{}',
    JSON.stringify({ samples: [], rest }),
    JSON.stringify({ samples: [[0, 1, 2, 3, 4]], rest }),
    JSON.stringify({ samples: [[0, 0, 0, 0, 0]], rest }),
    JSON.stringify({ samples: [[0.2, 0, 0, 0, 0, 0], [0.1, 0, 0, 0, 0, 0]], rest }),
    JSON.stringify({ samples: [[0, Infinity, 0, 0, 0, 0]], rest }),
    JSON.stringify({ samples: [[0, 0, 0, 0, 0, 0]], arrival: -1, rest }),
    JSON.stringify({ samples: [[0, 0, 0, 0, 0, 0]], arrival: null }),
    JSON.stringify({ samples: [[0, 0, 0, 0, 0, 0]], arrival: null, rest: [0, -1, 0] }),
  ])
    helper.PlayCursorPath(bad);
  assert.deepEqual(helper._path.samples, samples);

  // A click on the point the move settles onto lets it finish.
  helper.ClickPulse(501, 300);
  assert.ok(helper._path, 'settling move kept');
  near(helper._rest.phase, 7.3);
  // A click elsewhere ends it and snaps.
  helper.ClickPulse(800, 600);
  assert.equal(helper._path, null);
  assert.equal(helper._cursorX, 800);
  assert.equal(counts.remove, 1);

  // At rest the float carries on from the track's clock: still held at 0
  // just after, then eased back in on the same phase (no restart).
  helper.PlayCursorPath(JSON.stringify({ samples: [[0, 800, 600, 0, 0, 0]], arrival: null, rest }));
  near(helper._rest.at, 100);
  assert.equal(helper._pose(100.3).dx, 0);
  assert.equal(helper._pose(100.3).rotation, 0);
  // 2 s on: phase 9.3 s (0.325 of a loop), amplitude 1.
  const angle = (9.3 % 4) / 4 * 2 * Math.PI;
  near(helper._pose(102).dx, float.sway * Math.sin(angle));
  near(helper._pose(102).dy, float.bob * Math.cos(angle) - float.lift);
  near(helper._pose(102).rotation, float.tilt * Math.cos(angle));

  // A cue at rest (typing, scroll) eases it out from where it is, keeps
  // the phase and holds it at exactly 0.
  globalThis.helperClock = 102;
  const before = helper._pose(102);
  helper.SetCursorState('text', '', '', true);
  near(helper._rest.phase, 9.3);
  near(helper._pose(102).dy, before.dy);
  assert.equal(helper._pose(102.25).dy, 0);
  assert.equal(helper._pose(102.25).rotation, 0);
  assert.ok(Math.abs(helper._pose(104).dy) > 0.1, 'back in after the hold');
  // And a click at rest holds it at 0 too.
  globalThis.helperClock = 104;
  helper.ClickPulse(800, 600);
  globalThis.helperClock = 104.3;
  assert.equal(helper._pose(104.3).dx, 0);

  // Float off: the look carries no float.
  helper.SetCursorLook(JSON.stringify({ ...look, float: null }));
  assert.equal(helper._pose(106.4).dy, 0);
  assert.equal(helper._pose(106.4).rotation, 0);

  helper.PlayCursorPath(JSON.stringify({ samples, arrival: 0.4, rest }));
  helper.HideCursor();
  assert.equal(helper._path, null, 'hiding stops the path');
  helper.disable();
  delete globalThis.helperClock;
}
