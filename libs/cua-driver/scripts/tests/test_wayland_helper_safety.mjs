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
    get_monotonic_time: () => 0,
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
    assert.equal(helper.GetVersion(), 10);
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
