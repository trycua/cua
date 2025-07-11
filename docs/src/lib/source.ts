import { docs } from '@/.source';
import { loader } from 'fumadocs-core/source';
import { icons } from 'lucide-react';
import { createElement } from 'react';

import fs from 'node:fs/promises';
import path from 'node:path';

/**
 * Returns available API doc versions for a given section (e.g., 'agent').
 * Each version is an object: { label, slug }
 * - 'Current' (index.mdx) → slug: []
 * - '[version].mdx' → slug: [version]
 */
export async function getApiVersions(
  section: string
): Promise<{ label: string; slug: string[] }[]> {
  const dir = path.join(process.cwd(), 'content/docs/api', section);
  let files: string[] = [];
  try {
    files = (await fs.readdir(dir)).filter((f) => f.endsWith('.mdx'));
  } catch (_e) {
    return [];
  }
  const versions = files.map((file) => {
    if (file === 'index.mdx') {
      return { label: 'Current', slug: [] };
    }
    const version = file.replace(/\.mdx$/, '');
    return { label: version, slug: [version] };
  });
  // Always put 'Current' first, then others sorted descending (semver-ish)
  return [
    ...versions.filter((v) => v.label === 'Current'),
    ...versions
      .filter((v) => v.label !== 'Current')
      .sort((a, b) =>
        b.label.localeCompare(a.label, undefined, { numeric: true })
      ),
  ];
}

// See https://fumadocs.vercel.app/docs/headless/source-api for more info
export const source = loader({
  // it assigns a URL to your pages
  baseUrl: '/',
  source: docs.toFumadocsSource(),
  icon(icon) {
    if (!icon) return;
    if (icon in icons) return createElement(icons[icon as keyof typeof icons]);
  },
});
