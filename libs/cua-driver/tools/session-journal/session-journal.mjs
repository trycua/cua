#!/usr/bin/env node
// Builds a self-contained HTML journal from a Cua Driver recording directory.
// See README.md.

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

import { buildJournal } from './src/journal.js';
import { readRecording } from './src/recording.js';
import { renderJournal } from './src/render.js';

const USAGE = `usage: session-journal.mjs <recording-dir> [options]

Builds one HTML page from a recording made with
\`cua-driver recording start <recording-dir>\` ... \`cua-driver recording stop\`.

options:
  -o, --out <file>     where to write the page (default: <recording-dir>/journal.html)
  --title <text>       cover title (default: the recording folder name)
  --show-typed-text    show typed text, values and scripts instead of their length
  --include-desktop    show full-desktop screenshots, which are hidden by default
  --link-images        link screenshots by relative path instead of embedding them
  -h, --help           show this help`;

// The Cua mark at the repository root, when the tool runs from a checkout.
const LOGO = fileURLToPath(new URL('../../../../img/logo_white.svg', import.meta.url));

function main(argv) {
  let parsed;
  try {
    parsed = parseArgs({
      args: argv,
      allowPositionals: true,
      options: {
        out: { type: 'string', short: 'o' },
        title: { type: 'string' },
        'show-typed-text': { type: 'boolean' },
        'include-desktop': { type: 'boolean' },
        'link-images': { type: 'boolean' },
        help: { type: 'boolean', short: 'h' },
      },
    });
  } catch (error) {
    console.error(`${error.message}\n\n${USAGE}`);
    return 2;
  }
  const { values, positionals } = parsed;
  if (values.help) {
    console.log(USAGE);
    return 0;
  }
  if (positionals.length !== 1) {
    console.error(USAGE);
    return 2;
  }

  const dir = resolve(positionals[0]);
  const out = resolve(values.out ?? join(dir, 'journal.html'));
  const journal = buildJournal(readRecording(dir), {
    title: values.title,
    showTypedText: values['show-typed-text'],
    includeDesktop: values['include-desktop'],
  });
  const imageUrl = values['link-images']
    ? (image) => relativeUrl(dirname(out), image.path)
    : (image) => dataUrl('image/png', image.path);
  const logoUrl = existsSync(LOGO) ? dataUrl('image/svg+xml', LOGO) : null;
  writeFileSync(out, renderJournal(journal, { imageUrl, logoUrl }));

  console.log(`session-journal: ${journal.panels.length} turns -> ${out}`);
  const look = journal.panels.filter((panel) => panel.attention).map((panel) => panel.index);
  if (look.length) console.log(`  needs a look: turn ${look.join(', ')}`);
  const { typedHidden, desktopHidden } = journal.privacy;
  if (typedHidden) {
    console.log(`  typed text hidden in ${typedHidden} turn(s); --show-typed-text shows it`);
  }
  if (desktopHidden) {
    console.log(
      `  ${desktopHidden} full-desktop screenshot(s) hidden; --include-desktop shows them`
    );
  }
  return 0;
}

function dataUrl(type, path) {
  return `data:${type};base64,${readFileSync(path).toString('base64')}`;
}

function relativeUrl(from, to) {
  const path = relative(from, to);
  if (isAbsolute(path)) return pathToFileURL(to).href;
  return path.split(sep).map(encodeURIComponent).join('/');
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  console.error(`session-journal: ${error.message}`);
  process.exitCode = 1;
}
