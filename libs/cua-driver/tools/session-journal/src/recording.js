// Reads a Cua Driver trajectory recording: the directory written between
// `cua-driver recording start <dir>` and `cua-driver recording stop`.
//
// The layout is owned by rust/crates/cua-driver-core/src/recording.rs:
//   session.json              schema_version plus cursor and video metadata
//   cursor.jsonl              cursor samples {t_ms, x, y}, when recorded
//   turn-NNNNN/action.json    one per non-read-only tool call
//   turn-NNNNN/evidence.json  cua-turn-evidence/v1 capture statuses
//   turn-NNNNN/*.png          before, after, click and click_source images
//
// Nothing here interprets a turn; journal.js does that.

import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const TURN_DIR = /^turn-(\d+)$/;
const IMAGE_FILES = {
  before: 'before.png',
  after: 'after.png',
  click: 'click.png',
  clickSource: 'click_source.png',
};

export function readRecording(dir) {
  const session = readJson(join(dir, 'session.json'));
  const turns = [];
  const skipped = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const match = entry.isDirectory() ? TURN_DIR.exec(entry.name) : null;
    if (!match) continue;
    const turnDir = join(dir, entry.name);
    const action = readJson(join(turnDir, 'action.json'));
    // A turn folder without action.json is still being written, or was cut off.
    if (!action) {
      skipped.push(entry.name);
      continue;
    }
    turns.push({
      index: Number(match[1]),
      name: entry.name,
      action,
      evidence: readJson(join(turnDir, 'evidence.json')) ?? {},
      windowTitle:
        readWindowTitle(join(turnDir, 'before_state.json')) ??
        readWindowTitle(join(turnDir, 'after_state.json')),
      images: readImages(turnDir, entry.name),
    });
  }
  if (!session && turns.length === 0 && skipped.length === 0) {
    throw new Error(`${dir} is not a Cua Driver recording: no session.json or turn-NNNNN folders`);
  }
  turns.sort((a, b) => a.index - b.index);
  skipped.sort();
  return { dir, session, cursorEndMs: readCursorEnd(join(dir, 'cursor.jsonl')), turns, skipped };
}

// A PNG starts with an 8-byte signature and the IHDR chunk: length, type, width, height.
export function pngSize(bytes) {
  if (bytes.length < 24 || bytes.toString('latin1', 12, 16) !== 'IHDR') return {};
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

function readJson(path) {
  if (!existsSync(path)) return null;
  try {
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch {
    return null;
  }
}

function readImages(turnDir, turnName) {
  const images = {};
  for (const [role, file] of Object.entries(IMAGE_FILES)) {
    const path = join(turnDir, file);
    if (!existsSync(path)) continue;
    const bytes = readFileSync(path);
    images[role] = {
      path,
      file: `${turnName}/${file}`,
      sha256: createHash('sha256').update(bytes).digest('hex'),
      ...pngSize(bytes),
    };
  }
  return images;
}

// The root of the accessibility tree is the window: `- Window "Untitled - Notepad"`.
function readWindowTitle(path) {
  const markdown = readJson(path)?.tree_markdown;
  if (typeof markdown !== 'string') return null;
  return /^\s*-\s+\S+\s+"([^"\n]+)"/.exec(markdown)?.[1].trim() || null;
}

function readCursorEnd(path) {
  if (!existsSync(path)) return null;
  const last = readFileSync(path, 'utf8').trimEnd().split('\n').at(-1);
  try {
    return JSON.parse(last).t_ms ?? null;
  } catch {
    return null;
  }
}
