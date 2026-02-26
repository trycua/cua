#!/usr/bin/env npx tsx

/**
 * Generates content/docs/meta.json from the single source of truth in src/lib/docs-sites.ts.
 * Run automatically via prebuild/predev, or manually: pnpm docs:sync-meta
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { sidebarPages } from '../src/lib/docs-sites';

const metaPath = path.resolve(__dirname, '..', 'content', 'docs', 'meta.json');
const meta = {
  title: 'Home',
  description: 'Documentation Home',
  pages: sidebarPages,
};

fs.writeFileSync(metaPath, JSON.stringify(meta, null, 2) + '\n');
console.log(`Wrote ${metaPath} with pages: [${sidebarPages.join(', ')}]`);
