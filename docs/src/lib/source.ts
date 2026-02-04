import { docs } from '@/.source';
import { loader } from 'fumadocs-core/source';
import { icons } from 'lucide-react';
import { createElement } from 'react';

import fs from 'node:fs/promises';
import path from 'node:path';

// Custom brand icons (not available in Lucide)
const brandIcons: Record<string, () => React.ReactElement> = {
  github: () =>
    createElement(
      'svg',
      {
        viewBox: '0 0 24 24',
        fill: 'currentColor',
        className: 'w-4 h-4',
      },
      createElement('path', {
        d: 'M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z',
      })
    ),
  discord: () =>
    createElement(
      'svg',
      {
        viewBox: '0 0 24 24',
        fill: 'currentColor',
        className: 'w-4 h-4',
      },
      createElement('path', {
        d: 'M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286zM8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189zm7.9748 0c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189Z',
      })
    ),
  crab: () =>
    createElement(
      'svg',
      {
        viewBox: '0 0 24 24',
        fill: 'none',
        stroke: 'currentColor',
        strokeWidth: 2,
        strokeLinecap: 'round',
        strokeLinejoin: 'round',
        className: 'w-4 h-4',
      },
      createElement('path', { d: 'M7.5 14A6 6 0 1 1 10 2.36L8 5l2 2S7 8 2 8' }),
      createElement('path', { d: 'M16.5 14A6 6 0 1 0 14 2.36L16 5l-2 2s3 1 8 1' }),
      createElement('path', { d: 'M10 13v-2' }),
      createElement('path', { d: 'M14 13v-2' }),
      createElement('ellipse', { cx: 12, cy: 17.5, rx: 7, ry: 4.5 }),
      createElement('path', { d: 'M2 16c2 0 3 1 3 1' }),
      createElement('path', { d: 'M2 22c0-1.7 1.3-3 3-3' }),
      createElement('path', { d: 'M19 17s1-1 3-1' }),
      createElement('path', { d: 'M19 19c1.7 0 3 1.3 3 3' })
    ),
  lobster: () =>
    createElement(
      'svg',
      {
        viewBox: '0 0 24 24',
        fill: 'none',
        stroke: 'currentColor',
        strokeWidth: 2,
        strokeLinecap: 'round',
        strokeLinejoin: 'round',
        className: 'w-4 h-4',
      },
      // Body segments
      createElement('ellipse', { cx: 12, cy: 13, rx: 3, ry: 2 }),
      createElement('ellipse', { cx: 12, cy: 16.5, rx: 2.5, ry: 1.5 }),
      createElement('ellipse', { cx: 12, cy: 19.5, rx: 2, ry: 1.5 }),
      // Tail fan
      createElement('path', { d: 'M10 21l-1 2' }),
      createElement('path', { d: 'M12 21v2' }),
      createElement('path', { d: 'M14 21l1 2' }),
      // Claws
      createElement('path', { d: 'M9 13c-2-1-4-1-5 1-1 2 1 3 2 3s2-1 3-2' }),
      createElement('path', { d: 'M15 13c2-1 4-1 5 1 1 2-1 3-2 3s-2-1-3-2' }),
      createElement('path', { d: 'M3 12l-1-2' }),
      createElement('path', { d: 'M21 12l1-2' }),
      // Antennae
      createElement('path', { d: 'M10 11l-2-2' }),
      createElement('path', { d: 'M14 11l2-2' }),
      // Eyes
      createElement('circle', { cx: 10.5, cy: 12, r: 0.5 }),
      createElement('circle', { cx: 13.5, cy: 12, r: 0.5 })
    ),
};

// Icon resolver
function iconResolver(icon: string | undefined) {
  if (!icon) return;
  // Check brand icons first (github, discord, etc.)
  if (icon in brandIcons) return brandIcons[icon]();
  // Fall back to Lucide icons
  if (icon in icons) return createElement(icons[icon as keyof typeof icons]);
}

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
      .sort((a, b) => b.label.localeCompare(a.label, undefined, { numeric: true })),
  ];
}

// Single docs source
export const source = loader({
  baseUrl: '/',
  source: docs.toFumadocsSource(),
  icon: iconResolver,
});
