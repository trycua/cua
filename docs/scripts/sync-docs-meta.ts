#!/usr/bin/env npx tsx

/**
 * Generates all meta.json files under content/docs/ from the single source of
 * truth in src/lib/docs-sites.ts.
 *
 * Run automatically via prebuild/predev, or manually: pnpm docs:sync-meta
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { docsSites, sidebarPages } from '../src/lib/docs-sites';

const contentDir = path.resolve(__dirname, '..', 'content', 'docs');

// Root meta.json — controls sidebar section order
const rootMeta = {
  title: 'Home',
  description: 'Documentation Home',
  pages: sidebarPages,
};
fs.writeFileSync(path.join(contentDir, 'meta.json'), JSON.stringify(rootMeta, null, 2) + '\n');
console.log(`Wrote content/docs/meta.json  pages: [${sidebarPages.join(', ')}]`);

// Per-section meta.json — controls title, description, and tab order
for (const site of docsSites) {
  const sectionDir = path.join(contentDir, site.slug);
  if (!fs.existsSync(sectionDir)) continue;

  const pages = site.navTabs.map((tab) => tab.name.toLowerCase());
  const sectionMeta = {
    title: site.name,
    description: site.description,
    pages,
  };

  fs.writeFileSync(path.join(sectionDir, 'meta.json'), JSON.stringify(sectionMeta, null, 2) + '\n');
  console.log(`Wrote content/docs/${site.slug}/meta.json  title: "${site.name}"  pages: [${pages.join(', ')}]`);
}
