import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';

import { buildJournal, cropAround } from '../src/journal.js';
import { pngSize, readRecording } from '../src/recording.js';
import { renderJournal } from '../src/render.js';

const CLI = fileURLToPath(new URL('../session-journal.mjs', import.meta.url));
const SECRET = 'correct horse battery staple';

const WINDOW = png(4, 3, [255, 255, 255]);
const WINDOW_AFTER = png(4, 3, [240, 240, 240]);
const DESKTOP = png(1920, 1080, [20, 40, 60]);
const DESKTOP_AFTER = png(1920, 1080, [60, 40, 20]);

const captured = { screenshot: { status: 'captured' }, state: { status: 'captured' } };
const noTargetPid = {
  screenshot: { status: 'captured' },
  state: { status: 'not_applicable', classification: 'no_target_pid' },
};

// Shaped like a real Windows recording of Notepad: a launch without a target
// pid, background typing that never landed, a delivered click, a stale-token
// refusal, an accelerator failure and a turn folder still being written.
const TURNS = [
  {
    action: action('launch_app', { path: 'C:\\Windows\\System32\\notepad.exe' }, 0, {
      result_summary: '✅ Launched notepad.exe (pid 42) in background.',
    }),
    evidence: { before: noTargetPid, after: noTargetPid },
    files: { 'before.png': DESKTOP, 'after.png': DESKTOP_AFTER },
  },
  {
    action: action('type_text', { pid: 42, window_id: 7, text: SECRET }, 1, {
      result_summary: `📨 Sent “${SECRET}” via PostMessage (not verified).`,
      action_truth: truth({
        effect: 'unverifiable',
        escalation: { kind: 'retry_with_foreground_delivery', detail: 'retry with foreground' },
      }),
    }),
    evidence: { before: captured, after: captured },
    files: { 'before.png': WINDOW, 'after.png': WINDOW },
  },
  {
    action: action('click', { pid: 42, window_id: 7, x: 2, y: 1 }, 2, {
      click_point: { x: 2, y: 1 },
      action_truth: truth({ effect: 'confirmed', actual_delivery: 'foreground' }),
    }),
    evidence: { before: captured, after: captured, click: { status: 'captured' } },
    files: { 'before.png': WINDOW, 'after.png': WINDOW_AFTER, 'click.png': WINDOW_AFTER },
  },
  {
    action: action('click', { pid: 42, window_id: 7, element_token: 's00000001:999' }, 3, {
      result_error: true,
      result_summary: 'element_token is stale; call get_window_state again to refresh',
      action_truth: truth({
        effect: 'refused',
        actual_delivery: null,
        refusal: { code: 'stale_element_token', hint: 'call get_window_state again' },
      }),
    }),
    evidence: { before: captured, after: captured },
    files: { 'before.png': WINDOW, 'after.png': WINDOW },
  },
  {
    action: action('hotkey', { pid: 42, window_id: 7, keys: ['ctrl', 'a'] }, 4, {
      result_error: true,
      result_summary: 'hotkey could not find a UIA AcceleratorKey matching ctrl+a',
    }),
    evidence: { before: captured, after: captured },
    files: { 'before.png': WINDOW, 'after.png': WINDOW },
  },
  { files: { 'before.png': WINDOW } },
];

let dir;
test.before(() => {
  dir = writeRecording(TURNS);
});
test.after(() => rmSync(dir, { recursive: true, force: true }));

test('reads turns in order, with image sizes, and skips folders without action.json', () => {
  const recording = readRecording(dir);
  assert.deepEqual(
    recording.turns.map((turn) => turn.action.tool),
    ['launch_app', 'type_text', 'click', 'click', 'hotkey']
  );
  assert.deepEqual(recording.skipped, ['turn-00006']);
  assert.equal(recording.turns[0].images.before.width, 1920);
  assert.equal(recording.turns[1].images.after.file, 'turn-00002/after.png');
  assert.equal(recording.cursorEndMs, 9000);
});

test('rejects a directory that is not a recording', () => {
  const empty = mkdtempSync(join(tmpdir(), 'session-journal-empty-'));
  try {
    assert.throws(() => readRecording(empty), /not a Cua Driver recording/);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});

test('outcomes: a refusal wins over result_error, and tools without action_truth are done or failed', () => {
  const { panels } = buildJournal(readRecording(dir));
  assert.deepEqual(
    panels.map((panel) => panel.outcome),
    ['done', 'unverifiable', 'confirmed', 'refused', 'failed']
  );
  assert.deepEqual(panels[3].refusal, {
    code: 'stale_element_token',
    hint: 'call get_window_state again',
  });
});

test('flags unverified input that left the screen unchanged', () => {
  const { panels, stats } = buildJournal(readRecording(dir));
  assert.deepEqual(
    panels.map((panel) => panel.screen),
    ['changed', 'unchanged', 'changed', 'unchanged', 'unchanged']
  );
  assert.deepEqual(
    panels.map((panel) => panel.attention),
    [false, true, false, true, true]
  );
  assert.equal(stats.unchanged, 1);
  assert.equal(stats.failed + stats.refused, 2);
});

test('describes actions in plain words', () => {
  const { panels } = buildJournal(readRecording(dir));
  assert.deepEqual(
    panels.map((panel) => [panel.title, panel.target]),
    [
      ['Launch', 'notepad.exe'],
      ['Type', '28 characters, hidden'],
      ['Click', 'at (2, 1)'],
      ['Click', 'element s00000001:999'],
      ['Hotkey', 'Ctrl+A'],
    ]
  );
  assert.deepEqual(
    panels.map((panel) => panel.delivery),
    [null, 'background', 'foreground', null, null]
  );
  assert.equal(panels[3].requestedDelivery, 'background');
});

test('hides typed text everywhere unless asked', () => {
  const hidden = buildJournal(readRecording(dir));
  const typing = hidden.panels[1];
  assert.equal(typing.arguments.text, '‹hidden: 28 characters›');
  assert.equal(typing.summary, 'Sent “‹hidden›” via PostMessage (not verified).');
  assert.equal(hidden.privacy.typedHidden, 1);
  assert.ok(!renderJournal(hidden).includes(SECRET));

  const shown = buildJournal(readRecording(dir), { showTypedText: true });
  assert.equal(shown.panels[1].target, `“${SECRET}”`);
  assert.ok(renderJournal(shown).includes(SECRET));
});

test('hides full-desktop screenshots unless asked', () => {
  const hidden = buildJournal(readRecording(dir));
  assert.deepEqual(hidden.panels[0].frame, { hidden: 'desktop' });
  assert.equal(hidden.panels[0].pair, null);
  assert.equal(hidden.privacy.desktopHidden, 1);

  const shown = buildJournal(readRecording(dir), { includeDesktop: true });
  assert.equal(shown.panels[0].frame.image.file, 'turn-00001/after.png');
});

test('shows the click marker and keeps small captures whole', () => {
  const { panels } = buildJournal(readRecording(dir));
  assert.equal(panels[2].frame.image.file, 'turn-00003/click.png');
  assert.equal(panels[2].frame.crop, null);
});

test('crops large screenshots to a window around the click, inside the image', () => {
  assert.deepEqual(cropAround({ x: 960, y: 540 }, { width: 1920, height: 1080 }), {
    x: 528,
    y: 270,
    width: 864,
    height: 540,
  });
  assert.deepEqual(cropAround({ x: 1915, y: 3 }, { width: 1920, height: 1080 }), {
    x: 1056,
    y: 0,
    width: 864,
    height: 540,
  });
  assert.equal(cropAround(null, { width: 1920, height: 1080 }), null);
});

test('renders one panel per turn and loads nothing from the network', () => {
  const html = renderJournal(buildJournal(readRecording(dir)), {
    imageUrl: (image) => `data:image/png;base64,${image.sha256}`,
  });
  for (let i = 1; i <= 5; i++) assert.match(html, new RegExp(`id="turn-0000${i}"`));
  assert.doesNotMatch(html, /\b(?:src|href)="(?:https?:)?\/\//);
  assert.doesNotMatch(html, /url\("(?:https?:)?\/\//);
  assert.match(html, /default-src 'none'/);
  // Eight panel and before/after images, but only two distinct screenshots.
  assert.equal(html.match(/\.i\d+\{background-image/g).length, 2);
});

test('the command line writes a page and fails clearly on a bad directory', () => {
  const out = join(dir, 'journal.html');
  const ok = spawnSync(process.execPath, [CLI, dir, '--out', out], { encoding: 'utf8' });
  assert.equal(ok.status, 0, ok.stderr);
  assert.match(ok.stdout, /5 turns/);
  assert.ok(readFileSync(out, 'utf8').startsWith('<!doctype html>'));

  const missing = spawnSync(process.execPath, [CLI, join(dir, 'missing')], { encoding: 'utf8' });
  assert.equal(missing.status, 1);
  assert.match(missing.stderr, /^session-journal: /);
  assert.ok(!existsSync(join(dir, 'missing', 'journal.html')));
});

test('pngSize reads the IHDR header', () => {
  assert.deepEqual(pngSize(DESKTOP), { width: 1920, height: 1080 });
  assert.deepEqual(pngSize(Buffer.from('not a png')), {});
});

function action(tool, args, index, extra = {}) {
  return {
    tool,
    arguments: args,
    result_summary: 'ok',
    result_error: false,
    timestamp: new Date(Date.UTC(2026, 9, 28, 12, 0, index)).toISOString(),
    t_start_ms_from_session_start: index * 1000 + 500,
    t_ms_from_session_start: index * 1000 + 900,
    ...extra,
  };
}

function truth(fields) {
  return {
    actual_delivery: 'background',
    requested_delivery: 'background',
    route: 'synthetic_events',
    transport: 'windows_post_message',
    attempts: [],
    fallbacks: [],
    evidence: [],
    escalation: null,
    refusal: null,
    ...fields,
  };
}

function writeRecording(turns) {
  const root = mkdtempSync(join(tmpdir(), 'session-journal-'));
  writeFileSync(join(root, 'session.json'), JSON.stringify({ schema_version: 1 }));
  writeFileSync(join(root, 'cursor.jsonl'), '{"t_ms":0,"x":1,"y":1}\n{"t_ms":9000,"x":2,"y":2}\n');
  turns.forEach((turn, i) => {
    const turnDir = join(root, `turn-${String(i + 1).padStart(5, '0')}`);
    mkdirSync(turnDir);
    if (turn.action) writeFileSync(join(turnDir, 'action.json'), JSON.stringify(turn.action));
    if (turn.evidence) writeFileSync(join(turnDir, 'evidence.json'), JSON.stringify(turn.evidence));
    for (const [file, bytes] of Object.entries(turn.files ?? {})) {
      writeFileSync(join(turnDir, file), bytes);
    }
  });
  return root;
}

// A solid-colour RGB PNG, so the fixture needs no binary files.
function png(width, height, rgb) {
  const row = Buffer.alloc(1 + width * 3);
  for (let x = 0; x < width; x++) row.set(rgb, 1 + x * 3);
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(Buffer.concat(Array.from({ length: height }, () => row)))),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

function chunk(type, data) {
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data]);
  const out = Buffer.alloc(body.length + 8);
  out.writeUInt32BE(data.length, 0);
  body.copy(out, 4);
  out.writeUInt32BE(crc32(body), body.length + 4);
  return out;
}

function crc32(bytes) {
  let crc = ~0;
  for (const byte of bytes) {
    crc ^= byte;
    for (let k = 0; k < 8; k++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return ~crc >>> 0;
}
