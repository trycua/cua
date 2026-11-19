// The journal (events.jsonl) keeps every line small: an event that carries an image (a tool
// reading a PNG returns it as base64, hundreds of KB, twice in one event) or megabytes of
// output is journaled with the data cut or replaced by a note, so the reader (which takes
// whole lines) and every later turn that reads the log never meet a line they cannot hold.
//
//   node --test libs/cua/crates/cua-agents/runner/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { here } from "./helpers.mjs";

// The runner is one file shipped as one string, so the function is taken from its source.
const src = fs.readFileSync(path.join(here, "..", "acp-runner.mjs"), "utf8");
const start = src.indexOf("export const JOURNAL_STRING_MAX");
const end = src.indexOf("const SYNC_EVENTS");
assert.ok(start > 0 && end > start, "the journal helpers are in the runner");
const { shrink, JOURNAL_STRING_MAX } = new Function(
  `${src.slice(start, end).replaceAll("export ", "")}; return { shrink, JOURNAL_STRING_MAX };`,
)();

test("image data is replaced by a note, text is kept whole when small", () => {
  const png = Buffer.alloc(300_000, 7).toString("base64");
  const ev = {
    update: {
      content: [{ type: "image", data: png }, { type: "text", text: "growth.png (1500x250)" }],
      _meta: { claudeCode: { toolResponse: { file: { base64: png, type: "image/png" } } } },
    },
  };
  const line = JSON.stringify(shrink(ev));
  assert.ok(line.length < 2000, `line is ${line.length} bytes`);
  assert.match(line, /image or binary data, 400000 characters/);
  assert.match(line, /growth\.png \(1500x250\)/);
});

test("a long output is cut with a note of how much was left out", () => {
  const out = "line of output\n".repeat(20_000); // ~300 KB
  const s = shrink({ stdout: out }).stdout;
  assert.ok(s.length < JOURNAL_STRING_MAX + 100);
  assert.ok(s.startsWith("line of output"));
  assert.match(s, /\[\d+ more characters not kept in the log\]$/);
});

test("small values, numbers and nesting pass through unchanged", () => {
  const ev = { a: 1, b: null, c: ["x", { d: "short" }], e: true };
  assert.deepEqual(shrink(ev), ev);
});
