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
    assert.equal(helper.GetVersion(), 11);
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
  helper.SetCursorLook(JSON.stringify({ rest_lift: -1, rest_secs: 4, halo: null }));
  assert.equal(helper._look, null);
  const look = {
    rest_lift: 3.938,
    rest_secs: 4,
    halo: { rgb: '#5ec0e8', radius: 14, peak: 0.11, stops: [[0, 1], [0.45, 0.55], [1, 0]], body: [2, 11.9] },
  };
  helper.SetCursorLook(JSON.stringify(look));
  assert.deepEqual(helper._look.halo.rgb, [0x5e, 0xc0, 0xe8]);
  helper.SetCursorLook(JSON.stringify({ ...look, halo: null }));
  assert.equal(helper._look.halo, null, 'cursor.halo off');
  helper.SetCursorLook(JSON.stringify(look));

  // A move: poses interpolate, the float settles to 0 at the arrival and the
  // resting loop restarts there.
  const samples = [
    [0, 100, 100, -1, 0],
    [0.2, 300, 200, -2.2, 0.4],
    [0.4, 500, 300, 0, 0],
    [0.5, 501, 300, -0.01, 0],
  ];
  helper.PlayCursorPath(JSON.stringify({ samples, rest_from: 0.4 }));
  assert.equal(counts.timer, 1, 'one frame timer while the path plays');
  assert.ok(helper._path);
  near(helper._pose(100).dy, -1);
  near(helper._pose(100.1).dy, -1.6);
  near(helper._pose(100.1).rotation, 0.2);
  near(helper._pose(100.4).dy, 0);
  near(helper._pose(100.4).rotation, 0);
  near(helper._restOrigin, 100.4);

  // Malformed tracks change nothing.
  for (const bad of [
    '{}',
    JSON.stringify({ samples: [] }),
    JSON.stringify({ samples: [[0, 1, 2, 3]] }),
    JSON.stringify({ samples: [[0.2, 0, 0, 0, 0], [0.1, 0, 0, 0, 0]] }),
    JSON.stringify({ samples: [[0, Infinity, 0, 0, 0]] }),
    JSON.stringify({ samples: [[0, 0, 0, 0, 0]], rest_from: -1 }),
  ])
    helper.PlayCursorPath(bad);
  assert.deepEqual(helper._path.samples, samples);

  // A click on the point the move settles onto lets it finish.
  helper.ClickPulse(501, 300);
  assert.ok(helper._path, 'settling move kept');
  // A click elsewhere ends it and snaps.
  helper.ClickPulse(800, 600);
  assert.equal(helper._path, null);
  assert.equal(helper._cursorX, 800);
  assert.equal(counts.remove, 1);

  // After the path the resting float runs from the arrival: 0, then peak
  // lift halfway through the loop.
  globalThis.helperClock = 104.4;
  near(helper._pose(104.4).dy, 0);
  near(helper._pose(106.4).dy, -3.938);
  assert.equal(helper._pose(106.4).rotation, 0);

  // A snap places the cursor and leaves the resting loop running.
  helper.PlayCursorPath(JSON.stringify({ samples: [[0, 40, 50, -2, 0]], rest_from: null }));
  assert.equal(helper._path, null);
  assert.equal(helper._cursorX, 40);
  near(helper._restOrigin, 100.4);

  // Float off: the look carries no lift.
  helper.SetCursorLook(JSON.stringify({ ...look, rest_lift: 0 }));
  assert.equal(helper._pose(106.4).dy, 0);

  helper.PlayCursorPath(JSON.stringify({ samples, rest_from: 0.4 }));
  helper.HideCursor();
  assert.equal(helper._path, null, 'hiding stops the path');
  helper.disable();
  delete globalThis.helperClock;
}
