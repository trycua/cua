const { contextBridge } = require('electron');
const fs = require('fs');

contextBridge.exposeInMainWorld('cuaE2E', {
  journalUrl: process.env.CUA_E2E_FIXTURE_JOURNAL_URL || '',
});

const journalPath = process.env.CUA_E2E_SENTINEL_JOURNAL;
const sentinelMode = process.env.CUA_E2E_SENTINEL === '1' && journalPath;

function record(kind, details = {}) {
  if (!sentinelMode) return;
  fs.appendFileSync(
    journalPath,
    JSON.stringify({ kind, at_ms: Date.now(), ...details }) + '\n',
    'utf8'
  );
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
  window.addEventListener('keydown', event =>
    record('keydown', { key: event.key, code: event.code })
  );
  window.addEventListener('pointerdown', event =>
    record('pointerdown', { button: event.button, x: event.clientX, y: event.clientY })
  );
  window.addEventListener('wheel', event =>
    record('wheel', { delta_x: event.deltaX, delta_y: event.deltaY })
  );
  window.addEventListener('contextmenu', event =>
    record('contextmenu', { x: event.clientX, y: event.clientY })
  );
}
