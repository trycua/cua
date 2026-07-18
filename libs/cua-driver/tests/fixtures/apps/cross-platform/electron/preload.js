const { contextBridge, ipcRenderer } = require('electron');

const fixtureConfig = ipcRenderer.sendSync('cua-e2e-config');
const fixtureJournalUrl = fixtureConfig.journalUrl || '';

contextBridge.exposeInMainWorld('cuaE2E', {
  journalUrl: fixtureJournalUrl,
  publishFixtureState(state) {
    if (fixtureJournalUrl) ipcRenderer.send('cua-e2e-fixture-state', state);
  },
});

const sentinelMode = fixtureConfig.sentinelMode;

function record(kind, details = {}) {
  if (!sentinelMode) return;
  ipcRenderer.send('cua-e2e-sentinel-event', {
    kind,
    at_ms: Date.now(),
    ...details,
  });
}

if (sentinelMode) {
  window.addEventListener('DOMContentLoaded', () => {
    document.body.innerHTML = `
      <main style="min-height:100vh;background:#146c43;color:white;display:grid;place-content:center;text-align:center;font:24px system-ui">
        <h1 style="font-size:52px;margin:0 0 16px">CUA OCCLUSION SENTINEL</h1>
        <p>CUA_OCCLUSION_SENTINEL_v1</p>
      </main>
    `;
    record('ready');
  });
  window.addEventListener('focus', () => record('focus'));
  window.addEventListener('blur', () => record('blur'));
  window.addEventListener('visibilitychange', () =>
    record('visibility', { state: document.visibilityState })
  );
  window.addEventListener('keydown', event =>
    record('keydown', { key: event.key, code: event.code })
  );
  window.addEventListener('keyup', event =>
    record('keyup', { key: event.key, code: event.code })
  );
  window.addEventListener('pointerdown', event =>
    record('pointerdown', { button: event.button, x: event.clientX, y: event.clientY })
  );
  window.addEventListener('pointerup', event =>
    record('pointerup', { button: event.button, x: event.clientX, y: event.clientY })
  );
  window.addEventListener('click', event =>
    record('click', { button: event.button, x: event.clientX, y: event.clientY })
  );
  window.addEventListener('wheel', event =>
    record('wheel', { delta_x: event.deltaX, delta_y: event.deltaY })
  );
  window.addEventListener('contextmenu', event =>
    record('contextmenu', { x: event.clientX, y: event.clientY })
  );
  setInterval(() => record('heartbeat'), 100);
}
