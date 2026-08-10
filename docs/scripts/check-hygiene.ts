#!/usr/bin/env npx tsx

import * as fs from 'node:fs/promises';
import * as path from 'node:path';
import fg from 'fast-glob';

const DOCS_DIR = path.resolve(__dirname, '..');
const CONTENT_DIR = path.join(DOCS_DIR, 'content/docs');

const bannedPatterns: Array<[RegExp, string]> = [
  [/\bSTOLE FOCUS\b/, 'internal modality recorder verdict'],
  [/\bax-bg\b|\bpx-bg\b|\bpx-fg\b|\bax-fg\b/, 'internal modality recorder lane'],
  [/\bderec\.sh\b/, 'internal test harness script'],
  [/\baz exec\b/, 'internal CI/container access detail'],
  [/\bTEST_SUITE\.md\b|\bFINDINGS\.md\b/, 'repo-side contributor document reference'],
  [/\bNousResearch\b|#47065\b|#22865\b/, 'private partner or issue reference'],
  [/\bdocumented by a separate agent\b/i, 'authoring note'],
  [/\bDo not imply\b/i, 'authoring instruction'],
  [/\bdiorama\b/i, 'misnamed docs framework'],
];

async function main() {
  const files = await fg('**/*.mdx', { cwd: CONTENT_DIR });
  const failures: string[] = [];

  for (const file of files) {
    const abs = path.join(CONTENT_DIR, file);
    const content = await fs.readFile(abs, 'utf8');
    const lines = content.split(/\r?\n/);
    let inFence = false;

    for (const [lineIndex, line] of lines.entries()) {
      if (/^\s*(?:```|~~~)/.test(line)) {
        inFence = !inFence;
        continue;
      }

      if (!inFence && /^#\s+/.test(line)) {
        failures.push(
          `${file}:${lineIndex + 1}: body-level H1 duplicates the renderer-owned frontmatter title: ${line.trim()}`
        );
      }

      for (const [pattern, reason] of bannedPatterns) {
        if (pattern.test(line)) {
          failures.push(`${file}:${lineIndex + 1}: ${reason}: ${line.trim()}`);
        }
      }
    }
  }

  if (failures.length > 0) {
    console.error('Public docs hygiene check failed:\n');
    console.error(failures.join('\n'));
    process.exit(1);
  }

  console.log('Public docs hygiene check passed.');
}

main().catch((error) => {
  console.error('Fatal error:', error);
  process.exit(1);
});
