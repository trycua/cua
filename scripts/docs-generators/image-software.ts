/**
 * The per-image "what's installed" tables (image-software.mdx), from the
 * inventories under libs/images/software. Each inventory is what
 * `cua-spacesd doctor --strict` measured and enforced in a booted guest
 * (`scripts/images/record-software.py` writes it from the doctor report),
 * so the docs list only versions a passing gate saw.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';

export const SOFTWARE_DIR = 'libs/images/software';

export type SoftwareInventory = {
  /** The image's primary ref (never the `-disk` sibling). */
  image: string;
  os: string;
  version: string;
  /** `slim`, `full`, `xcode` or `xcode-<X.Y>`. */
  tier: string;
  /** Report date, runtime/arch and build source. */
  recorded_from: string;
  apps: Record<string, string>;
  tools: Record<string, string>;
  simulator_runtimes: string[];
};

const KEYS = 'apps,image,os,recorded_from,simulator_runtimes,tier,tools,version';
const TIER_ORDER = ['slim', 'full', 'xcode'];

function versions(value: unknown, label: string): Record<string, string> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${label} must be an object`);
  for (const [name, version] of Object.entries(value)) {
    if (typeof version !== 'string' || !version || version === 'unavailable') {
      throw new Error(`${label}.${name} must be a recorded version`);
    }
  }
  return value as Record<string, string>;
}

/** Validates one inventory file's contents. */
export function validateInventory(value: unknown, file: string): SoftwareInventory {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${file}: expected an object`);
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record).sort().join(',');
  if (keys !== KEYS) throw new Error(`${file}: has fields ${keys}, expected ${KEYS}`);
  for (const key of ['image', 'os', 'version', 'tier', 'recorded_from']) {
    if (typeof record[key] !== 'string' || !record[key]) throw new Error(`${file}: ${key} must be a string`);
  }
  const inv = record as unknown as SoftwareInventory;
  if (!/^ghcr\.io\/trycua\/[a-z0-9-]+:[0-9][0-9.]*(-(slim|xcode(-[0-9.]+)?))?$/.test(inv.image)) {
    throw new Error(`${file}: image ${inv.image} is not a canonical tier tag`);
  }
  if (path.basename(file) !== `${inv.os}-${inv.version}-${inv.tier}.json`) {
    throw new Error(`${file}: expected the name ${inv.os}-${inv.version}-${inv.tier}.json`);
  }
  versions(inv.apps, `${file}: apps`);
  versions(inv.tools, `${file}: tools`);
  if (!Array.isArray(inv.simulator_runtimes) || inv.simulator_runtimes.some((r) => typeof r !== 'string')) {
    throw new Error(`${file}: simulator_runtimes must be a list of names`);
  }
  return inv;
}

/** Every inventory under `dir` (absolute), sorted by OS, version and tier. */
export function loadInventories(dir: string): SoftwareInventory[] {
  if (!fs.existsSync(dir)) return [];
  const tierRank = (t: string) => {
    const i = TIER_ORDER.indexOf(t.split('-')[0]);
    return i < 0 ? TIER_ORDER.length : i;
  };
  return fs
    .readdirSync(dir)
    .filter((f) => f.endsWith('.json'))
    .map((f) => validateInventory(JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8')), path.join(dir, f)))
    .sort(
      (a, b) =>
        a.os.localeCompare(b.os) ||
        a.version.localeCompare(b.version, 'en', { numeric: true }) ||
        tierRank(a.tier) - tierRank(b.tier) ||
        a.tier.localeCompare(b.tier)
    );
}

function tierLabel(tier: string): string {
  if (tier === 'full') return 'full (default)';
  return tier;
}

function table(inv: SoftwareInventory): string {
  const rows = [
    ...Object.entries(inv.apps).map(([name, v]) => `| App | ${name} | \`${v}\` |`),
    ...Object.entries(inv.tools).map(([name, v]) => `| Tool | ${name} | \`${v}\` |`),
    ...inv.simulator_runtimes.map((r) => `| Simulator runtime | ${r} | |`),
  ];
  if (rows.length === 0) return 'The image claims no apps or tools.';
  return ['| Kind | Name | Version |', '| --- | --- | --- |', ...rows].join('\n');
}

/** A catalog image that should have an inventory. */
export type CatalogImage = { ref: string; published: boolean };

/** Marks the next line's image ref as not (yet) pullable for the docs image-ref gate. */
const UNVERIFIED = '{/* cua-image-unverified */}';

/**
 * The page body. `images` are the catalog images that should have an
 * inventory (canonical images with cua-spacesd). Published ones without an
 * inventory are listed as not recorded yet; an inventory of an image that
 * is not published yet is shown with the docs gate's unverified marker.
 */
export function renderSoftware(inventories: SoftwareInventory[], images: CatalogImage[]): string {
  const intro = `Each table lists what \`cua-spacesd doctor --strict\` measured in a booted guest of the image: the apps and tools the image claims, and on Xcode tiers its simulator runtimes. A missing tool or a wrong version fails the doctor, so a published tier always matches its table. The \`-disk\` variant of each image carries the same software.

Tiers: \`-slim\` is the minimum for every cua-spacesd feature, the untagged image (\`Image.linux()\`, \`Image.macos()\`) adds developer tooling, and \`-xcode\` (macOS) adds Xcode. See [Image catalog details](/cua-sdk/reference/os-image-catalog).`;
  const recorded = new Set(inventories.map((i) => i.image));
  const published = new Set(images.filter((i) => i.published).map((i) => i.ref));
  const sections = inventories.map((inv) => {
    const heading = `## \`${inv.image}\``;
    const status = published.has(inv.image) ? '' : ' Not published yet.';
    return `${published.has(inv.image) ? heading : `${UNVERIFIED}\n${heading}`}

Tier: ${tierLabel(inv.tier)}.${status} Recorded from ${inv.recorded_from}.

${table(inv)}`;
  });
  const pending = [...published].filter((ref) => !recorded.has(ref));
  const parts = [intro];
  if (sections.length === 0) {
    parts.push('No image inventory is recorded yet. Tables appear here once an image tier passes `cua-spacesd doctor --strict`.');
  } else {
    parts.push(...sections);
  }
  if (pending.length > 0) {
    parts.push(`## Not recorded yet

${pending.map((ref) => `- \`${ref}\``).join('\n')}`);
  }
  return parts.join('\n\n');
}
