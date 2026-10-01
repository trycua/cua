#!/usr/bin/env npx tsx

/**
 * Generate the source-backed Sandbox package and image facts: the image
 * catalog and the runtime support matrix of the Cua SDK reference. The
 * curated body of each page lives in headers/cua-sdk/<page>.md; its
 * `GENERATED:<name>` regions are filled from source manifests here.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { parseArgs } from 'node:util';
import { HEADERS_DIR, renderPage } from './lib/mdx';
import { SOFTWARE_DIR, loadInventories, renderSoftware } from './image-software';

const ROOT = path.resolve(__dirname, '../..');
const FACTS_PATH = path.join(__dirname, 'sandbox-facts.json');
const RUNTIME_DOC = path.join(ROOT, 'docs/content/docs/cua-sdk/reference/runtime-support.mdx');
const CATALOG_DOC = path.join(ROOT, 'docs/content/docs/cua-sdk/reference/os-image-catalog.mdx');
const SOFTWARE_DOC = path.join(ROOT, 'docs/content/docs/cua-sdk/reference/image-software.mdx');
const RUNTIME_TEMPLATE = path.join(HEADERS_DIR, 'cua-sdk', 'runtime-support.md');
const CATALOG_TEMPLATE = path.join(HEADERS_DIR, 'cua-sdk', 'os-image-catalog.md');
const GENERATOR = 'pnpm --dir docs docs:generate:sandbox';
const SOURCE = 'scripts/docs-generators/sandbox-facts.json and the cua, cua-sandbox and cua-image sources';

type Services = { server: number; mcp: number };
type Components = { computerServer: string; embeddedDriver: string; directDriver: string };

export interface QualifiedImage {
  id: string;
  image: string;
  os: string;
  distribution: string;
  architecture: string;
  desktop: string;
  kind: string;
  services: Services;
  components: Components;
  qualification: { boot: true; serviceReadiness: true };
}

export interface PublishedCandidate {
  os: string;
  distribution: string;
  desktop: string;
  kind: string;
  image: string;
  limits: string;
}

export interface SandboxFactsManifest {
  $schema: string;
  schemaVersion: 1;
  qualifiedImages: QualifiedImage[];
  publishedCandidates: PublishedCandidate[];
}

interface SourceFacts {
  sandboxVersion: string;
  cuaVersion: string;
  pythonFleetVersion: string;
  typescriptFleetVersion: string;
  sandboxDriverVersion: string;
  qualifiedDirectDriverVersion: string;
  linuxImage: string;
  windowsImage: string;
  macosTahoeImage: string;
  macosSequoiaImage: string;
}

const VERSION = /^\d+\.\d+\.\d+$/;
const DIGEST_REF = /@sha256:[0-9a-f]{64}$/;

function object(value: unknown, label: string): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, keys: readonly string[], label: string): void {
  const expected = [...keys].sort();
  const actual = Object.keys(value).sort();
  if (actual.join('\n') !== expected.join('\n')) {
    throw new Error(`${label} keys must be ${expected.join(', ')}; got ${actual.join(', ')}`);
  }
}

function string(value: unknown, label: string, pattern?: RegExp): string {
  if (typeof value !== 'string' || value.length === 0 || (pattern && !pattern.test(value))) {
    throw new Error(`${label} is invalid`);
  }
  return value;
}

function port(value: unknown, label: string): number {
  if (!Number.isInteger(value) || Number(value) < 1 || Number(value) > 65535) {
    throw new Error(`${label} must be a TCP port`);
  }
  return Number(value);
}

/** Validate without a network fetch or an unpinned schema-validator dependency. */
export function validateManifest(value: unknown): SandboxFactsManifest {
  const root = object(value, 'manifest');
  exactKeys(
    root,
    ['$schema', 'schemaVersion', 'qualifiedImages', 'publishedCandidates'],
    'manifest'
  );
  if (root.$schema !== './sandbox-facts.schema.json' || root.schemaVersion !== 1) {
    throw new Error('manifest schema identity or version is invalid');
  }
  if (!Array.isArray(root.qualifiedImages) || root.qualifiedImages.length === 0) {
    throw new Error('qualifiedImages must be a non-empty array');
  }
  const ids = new Set<string>();
  const qualifiedImages = root.qualifiedImages.map((entry, index): QualifiedImage => {
    const item = object(entry, `qualifiedImages[${index}]`);
    exactKeys(
      item,
      [
        'id',
        'image',
        'os',
        'distribution',
        'architecture',
        'desktop',
        'kind',
        'services',
        'components',
        'qualification',
      ],
      `qualifiedImages[${index}]`
    );
    const id = string(item.id, `qualifiedImages[${index}].id`, /^[a-z0-9-]+$/);
    if (ids.has(id)) throw new Error(`duplicate qualified image id ${id}`);
    ids.add(id);
    const services = object(item.services, `${id}.services`);
    exactKeys(services, ['server', 'mcp'], `${id}.services`);
    const components = object(item.components, `${id}.components`);
    exactKeys(components, ['computerServer', 'embeddedDriver', 'directDriver'], `${id}.components`);
    const qualification = object(item.qualification, `${id}.qualification`);
    exactKeys(qualification, ['boot', 'serviceReadiness'], `${id}.qualification`);
    if (qualification.boot !== true || qualification.serviceReadiness !== true) {
      throw new Error(`${id}.qualification must record boot and service readiness`);
    }
    return {
      id,
      image: string(item.image, `${id}.image`, DIGEST_REF),
      os: string(item.os, `${id}.os`),
      distribution: string(item.distribution, `${id}.distribution`),
      architecture: string(item.architecture, `${id}.architecture`),
      desktop: string(item.desktop, `${id}.desktop`),
      kind: string(item.kind, `${id}.kind`),
      services: {
        server: port(services.server, `${id}.services.server`),
        mcp: port(services.mcp, `${id}.services.mcp`),
      },
      components: {
        computerServer: string(components.computerServer, `${id}.computerServer`, VERSION),
        embeddedDriver: string(components.embeddedDriver, `${id}.embeddedDriver`, VERSION),
        directDriver: string(components.directDriver, `${id}.directDriver`, VERSION),
      },
      qualification: { boot: true, serviceReadiness: true },
    };
  });
  if (!Array.isArray(root.publishedCandidates)) {
    throw new Error('publishedCandidates must be an array');
  }
  const publishedCandidates = root.publishedCandidates.map((entry, index): PublishedCandidate => {
    const item = object(entry, `publishedCandidates[${index}]`);
    exactKeys(
      item,
      ['os', 'distribution', 'desktop', 'kind', 'image', 'limits'],
      `publishedCandidates[${index}]`
    );
    return {
      os: string(item.os, `publishedCandidates[${index}].os`),
      distribution: string(item.distribution, `publishedCandidates[${index}].distribution`),
      desktop: string(item.desktop, `publishedCandidates[${index}].desktop`),
      kind: string(item.kind, `publishedCandidates[${index}].kind`),
      image: string(item.image, `publishedCandidates[${index}].image`),
      limits: string(item.limits, `publishedCandidates[${index}].limits`),
    };
  });
  return {
    $schema: './sandbox-facts.schema.json',
    schemaVersion: 1,
    qualifiedImages,
    publishedCandidates,
  };
}

export function loadManifest(file = FACTS_PATH): SandboxFactsManifest {
  return validateManifest(JSON.parse(fs.readFileSync(file, 'utf8')));
}

function read(relativePath: string): string {
  return fs.readFileSync(path.join(ROOT, relativePath), 'utf8');
}

function capture(contents: string, pattern: RegExp, label: string): string {
  const match = contents.match(pattern);
  if (!match?.[1]) throw new Error(`Could not extract ${label}`);
  return match[1];
}

function projectVersion(relativePath: string): string {
  const contents = read(relativePath);
  return capture(contents, /\[project\][\s\S]*?\nversion\s*=\s*"([^"]+)"/, relativePath);
}

const CANONICAL_RS = 'libs/cua/crates/cua-image/src/canonical.rs';

/** A `pub const NAME: &str = "..."` of the canonical-image module. */
function rustConstant(contents: string, name: string): string {
  return capture(
    contents,
    new RegExp(`^pub const ${name}: &str = "([^"]+)";`, 'm'),
    `${CANONICAL_RS}:${name}`
  );
}

export function loadSourceFacts(): SourceFacts {
  const sandboxProject = read('libs/python/cua-sandbox/pyproject.toml');
  const canonical = read(CANONICAL_RS);
  const profile = JSON.parse(
    read('libs/cua-driver/hyprland-plugin/packaging/release/profiles/omarchy-stable-20260910.json')
  ) as { source?: { driver_version?: string } };
  return {
    sandboxVersion: projectVersion('libs/python/cua-sandbox/pyproject.toml'),
    cuaVersion: projectVersion('libs/cua/python/pyproject.toml'),
    pythonFleetVersion: capture(sandboxProject, /"cua-fleet==([^"]+)"/, 'cua-fleet pin'),
    typescriptFleetVersion: String(
      (JSON.parse(read('libs/typescript/fleet/package.json')) as { version: string }).version
    ),
    sandboxDriverVersion: capture(sandboxProject, /"cua-driver==([^"]+)"/, 'Sandbox Driver pin'),
    qualifiedDirectDriverVersion: string(
      profile.source?.driver_version,
      'Omarchy release profile Driver version',
      VERSION
    ),
    linuxImage: `${rustConstant(canonical, 'LINUX_REPO')}:${rustConstant(canonical, 'LINUX_DEFAULT_VERSION')}`,
    windowsImage: `${rustConstant(canonical, 'WINDOWS_REPO')}:${rustConstant(canonical, 'WINDOWS_DEFAULT_VERSION')}`,
    macosTahoeImage: `${rustConstant(canonical, 'MACOS_REPO')}:${rustConstant(canonical, 'MACOS_DEFAULT_VERSION')}`,
    // `Image.macos("15")` and the `macos:sequoia` alias (canonical.rs version_tag).
    macosSequoiaImage: `${rustConstant(canonical, 'MACOS_REPO')}:${capture(
      canonical,
      /\(Self::Macos, "sequoia"\) => "([^"]+)"/,
      `${CANONICAL_RS}:sequoia`
    )}`,
  };
}

function assertQualificationSources(manifest: SandboxFactsManifest, source: SourceFacts): void {
  // computer-server left this repository; its version and embedded Driver in a
  // qualified image are facts about that published artifact, recorded in the
  // schema-checked manifest rather than cross-checked against source here.
  for (const image of manifest.qualifiedImages) {
    if (image.components.directDriver !== source.qualifiedDirectDriverVersion) {
      throw new Error(
        `${image.id} direct Driver disagrees with the public Omarchy release profile`
      );
    }
  }
}

function generatedSection(name: string, body: string): string {
  return `{/* GENERATED:${name}:start */}\n${body.trim()}\n{/* GENERATED:${name}:end */}`;
}

export function replaceGeneratedSection(document: string, name: string, body: string): string {
  const start = `{/* GENERATED:${name}:start */}`;
  const end = `{/* GENERATED:${name}:end */}`;
  const startIndex = document.indexOf(start);
  const endIndex = document.indexOf(end);
  if (startIndex < 0 || endIndex < startIndex || document.indexOf(start, startIndex + 1) >= 0) {
    throw new Error(`Expected one generated section named ${name}`);
  }
  return `${document.slice(0, startIndex)}${generatedSection(name, body)}${document.slice(
    endIndex + end.length
  )}`;
}

export function renderRuntimePackageFacts(source: SourceFacts): string {
  return `This reference describes the Python Sandbox SDK in \`cua-sandbox\` **${source.sandboxVersion}**, exported
by \`cua\` **${source.cuaVersion}**. An accepted image specification or generated Fleet template is
evidence of SDK behavior, not proof that a guest boots on every host or deployment.`;
}

export function renderBuiltinMappings(source: SourceFacts): string {
  return `| Constructor | Canonical image | Variant a backend runs |
| --- | --- | --- |
| \`Image.linux()\` | \`${source.linuxImage}\` | Rootfs for containers; the \`-disk\` containerDisk for \`kind="vm"\` |
| \`Image.windows()\` | \`${source.windowsImage}\` | The \`-disk\` containerDisk (VM) |
| \`Image.macos()\` | \`${source.macosTahoeImage}\` | Lume, local only (\`Image.macos("15")\`: \`${source.macosSequoiaImage}\`) |`;
}

export function renderEvidence(source: SourceFacts): string {
  return `- \`cua-sandbox\` ${source.sandboxVersion} source: image constructors, runtime selection, pool
  claims, Fleet validation, firmware, and services.
- \`cua-image\` source (\`canonical.rs\`): the canonical image references.
- \`cua\` ${source.cuaVersion} source: public Python exports.
- \`cua-fleet\` ${source.pythonFleetVersion} package mapping: Python bound-claim service requests.
- \`@trycua/fleet\` ${source.typescriptFleetVersion} package mapping: TypeScript service requests.
- \`cua-sandbox[driver]\` ${source.sandboxVersion} maps direct Driver use to \`cua-driver\`
  ${source.sandboxDriverVersion}. This client mapping is not the Driver version inside a particular
  published or deployment-qualified guest image.`;
}

/** One entry of `libs/images/sandbox-images.json`, the image list the apps and docs share. */
export type SandboxImage = {
  ref: string;
  group: 'canonical' | 'benchmark' | 'distro';
  os: 'linux' | 'windows' | 'macos';
  name: string;
  /** The guest's distribution: its id picks the apps' OS icon, its name is
   * what they show until a Space reports its own. */
  distro: { id: string; name: string };
  variant: 'container' | 'vm';
  summary: string;
  spacesd: boolean;
  /** Browsers the image ships; cua-driver's browser tools drive the Chromium-family ones. */
  browsers: string[];
  /** Platforms the image is published for. */
  arch: Array<'amd64' | 'arm64'>;
  /** Optional SDK constructor that names the image (`Image.linux()`). */
  sdk?: string;
  local: 'container' | 'qemu' | 'lume' | null;
  cloud: 'gvisor' | 'kubevirt' | null;
  published: boolean;
  /** Canonical images: `slim`, `full` (the default, no tag suffix) or `xcode` (macOS). */
  tier?: 'slim' | 'full' | 'xcode';
  /** Optional docs path of the image's guide (benchmark images: their Cua Bench guide). */
  guide?: string;
  /** Benchmark images: the bench publisher's lock file (promoted tags and digests). */
  lock?: string;
  /** The published digest the moving tag was verified at (`sha256:<hex>`). */
  digest?: string;
  /** Benchmark images: the benchmarks the image runs, for the docs tables. */
  benchmarks?: Benchmark[];
};

/** One benchmark a benchmark image runs. */
export type Benchmark = { name: string; what: string; tasks: number; guide: string };

export type SandboxImageList = {
  schemaVersion: 1;
  /** `picker: false` keeps a group out of pickers and the image tables (benchmark images). */
  groups: Array<{ id: SandboxImage['group']; label: string; picker?: boolean }>;
  images: SandboxImage[];
};

export const IMAGE_LIST_PATH = 'libs/images/sandbox-images.json';

const OPTIONAL_IMAGE_FIELDS = ['guide', 'lock', 'benchmarks', 'sdk', 'digest', 'sizes', 'tier'];

/** The tag suffix each tier puts before `-disk`: `<os-version>[-<tier>][-disk]`. */
const TIER_SUFFIX: Record<string, string> = { slim: '-slim', full: '', xcode: '-xcode' };

const IMAGE_FIELDS = [
  'ref',
  'group',
  'os',
  'name',
  'distro',
  'variant',
  'summary',
  'spacesd',
  'browsers',
  'arch',
  'local',
  'cloud',
  'published',
];

/** Parses and validates the shared image list; canonical entries must match canonical.rs. */
export function validateImageList(value: unknown, source: SourceFacts): SandboxImageList {
  const root = value as Partial<SandboxImageList> & Record<string, unknown>;
  if (root.schemaVersion !== 1 || !Array.isArray(root.images) || !Array.isArray(root.groups)) {
    throw new Error(`${IMAGE_LIST_PATH}: expected schemaVersion 1 with groups and images`);
  }
  const groups = new Set(root.groups.map((g) => g.id));
  const seen = new Set<string>();
  for (const image of root.images) {
    const record = image as unknown as Record<string, unknown>;
    const keys = Object.keys(record)
      .filter((k) => !OPTIONAL_IMAGE_FIELDS.includes(k))
      .sort();
    if (keys.join() !== [...IMAGE_FIELDS].sort().join()) {
      throw new Error(`${IMAGE_LIST_PATH}: ${String(record.ref)} has fields ${keys.join(', ')}`);
    }
    if (!/^ghcr\.io\/trycua\/[a-z0-9-]+:[A-Za-z0-9._-]+$/.test(image.ref)) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} is not a ghcr.io/trycua tag`);
    }
    if (seen.has(image.ref)) throw new Error(`${IMAGE_LIST_PATH}: duplicate ${image.ref}`);
    if (image.digest !== undefined && !/^sha256:[0-9a-f]{64}$/.test(image.digest)) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} digest must be sha256:<64 hex>`);
    }
    // Sizes are measured at the digest (scripts/images/record-image-sizes.py).
    const sizes = (record.sizes ?? undefined) as { digest?: string } | undefined;
    if ((sizes === undefined) !== (image.digest === undefined) || (sizes && sizes.digest !== image.digest)) {
      throw new Error(
        `${IMAGE_LIST_PATH}: ${image.ref} sizes must be measured at its digest (run python3 scripts/images/record-image-sizes.py)`,
      );
    }
    seen.add(image.ref);
    if (!groups.has(image.group)) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} has unknown group ${image.group}`);
    }
    if (!['linux', 'windows', 'macos'].includes(image.os)) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} has unknown os ${image.os}`);
    }
    const distro = image.distro as unknown as Record<string, unknown> | null;
    if (
      typeof distro !== 'object' ||
      distro === null ||
      Object.keys(distro).sort().join() !== 'id,name' ||
      typeof distro.id !== 'string' ||
      !/^[a-z0-9-]+$/.test(distro.id) ||
      typeof distro.name !== 'string' ||
      distro.name.trim() === ''
    ) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} distro must be { id, name }`);
    }
    if (!Array.isArray(image.browsers) || image.browsers.some((b) => typeof b !== 'string')) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} browsers must be a list of names`);
    }
    if (
      !Array.isArray(image.arch) ||
      image.arch.length === 0 ||
      image.arch.some((a) => a !== 'amd64' && a !== 'arm64')
    ) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} arch must list amd64 and/or arm64`);
    }
    if (image.sdk !== undefined && !/^Image\.[a-z]+\(.*\)$/.test(image.sdk)) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} sdk ${image.sdk} is not an Image constructor`);
    }
    if (image.guide !== undefined && !/^\/[a-z0-9/#-]+$/.test(image.guide)) {
      throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} guide ${image.guide} is not a docs path`);
    }
    if (image.group === 'benchmark' && !/^ghcr\.io\/trycua\/bench-/.test(image.ref)) {
      throw new Error(`${IMAGE_LIST_PATH}: benchmark image ${image.ref} is not a bench-* image`);
    }
    if (image.group === 'benchmark') {
      if (!image.lock || !fs.existsSync(path.join(ROOT, image.lock))) {
        throw new Error(`${IMAGE_LIST_PATH}: benchmark image ${image.ref} needs an existing lock file`);
      }
      const lock = JSON.parse(read(image.lock)) as { repository?: string };
      if (lock.repository !== image.ref.split(':')[0]) {
        throw new Error(`${IMAGE_LIST_PATH}: ${image.lock} is not the lock of ${image.ref}`);
      }
    } else if (image.lock !== undefined || image.benchmarks !== undefined) {
      throw new Error(`${IMAGE_LIST_PATH}: only benchmark images have a lock or benchmarks`);
    }
    if (image.tier !== undefined) {
      if (image.group !== 'canonical' || !(image.tier in TIER_SUFFIX)) {
        throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} tier must be slim, full or xcode on a canonical image`);
      }
      if (image.tier === 'xcode' && image.os !== 'macos') {
        throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} the xcode tier is macOS only`);
      }
      const base = image.ref.split(':')[1].replace(/-disk$/, '');
      const suffix = base.match(/-(slim|xcode(-[0-9.]+)?)$/)?.[0] ?? '';
      if (suffix.replace(/-[0-9.]+$/, '') !== TIER_SUFFIX[image.tier]) {
        throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} tag does not match tier ${image.tier}`);
      }
    }
    for (const b of image.benchmarks ?? []) {
      const keys = Object.keys(b).sort().join();
      if (
        keys !== 'guide,name,tasks,what' ||
        !Number.isInteger(b.tasks) ||
        !/^\/[a-z0-9/#-]+$/.test(b.guide)
      ) {
        throw new Error(`${IMAGE_LIST_PATH}: ${image.ref} has an invalid benchmark ${JSON.stringify(b)}`);
      }
    }
  }
  const canonical = root.images
    .filter((i) => i.group === 'canonical')
    .map((i) => i.ref)
    .sort();
  const expected = [
    source.linuxImage,
    `${source.linuxImage}-disk`,
    source.windowsImage,
    `${source.windowsImage}-disk`,
    `${source.linuxImage}-slim`,
    `${source.linuxImage}-slim-disk`,
    source.macosTahoeImage,
    `${source.macosTahoeImage}-slim`,
    `${source.macosTahoeImage}-xcode`,
    source.macosSequoiaImage,
  ].sort();
  if (canonical.join() !== expected.join()) {
    throw new Error(
      `${IMAGE_LIST_PATH}: canonical images ${canonical.join(', ')} disagree with ${CANONICAL_RS} (${expected.join(', ')})`
    );
  }
  return root as SandboxImageList;
}

export function loadImageList(source: SourceFacts = loadSourceFacts()): SandboxImageList {
  return validateImageList(JSON.parse(read(IMAGE_LIST_PATH)), source);
}

/** The images every picker and the docs show: published entries, in file order. */
export function pickerImages(list: SandboxImageList): SandboxImage[] {
  const offered = new Set(list.groups.filter((g) => g.picker !== false).map((g) => g.id));
  return list.images.filter((image) => image.published && offered.has(image.group));
}

const LOCAL_RUNTIME: Record<string, string> = {
  container: 'Container (gVisor when installed)',
  qemu: 'QEMU VM',
  lume: 'Lume VM (Apple silicon)',
};
const CLOUD_RUNTIME: Record<string, string> = {
  gvisor: 'gVisor container',
  kubevirt: 'KubeVirt VM',
};

const BROWSER_NAME: Record<string, string> = {
  chromium: 'Chromium',
  chrome: 'Google Chrome',
  firefox: 'Firefox',
  safari: 'Safari',
  edge: 'Edge',
};

function browserName(id: string): string {
  return BROWSER_NAME[id] ?? id;
}

export function renderImageList(list: SandboxImageList): string {
  const lines = [
    '| Image | Group | Name | Tier | Local | Cloud | cua-spacesd | Browsers |',
    '| --- | --- | --- | --- | --- | --- | --- | --- |',
  ];
  const label = new Map(list.groups.map((g) => [g.id, g.label]));
  for (const image of pickerImages(list)) {
    lines.push(
      `| \`${image.ref}\` | ${label.get(image.group)} | ${image.guide ? `[${image.name}](${image.guide})` : image.name} | ${image.tier ?? '-'} | ${
        image.local ? LOCAL_RUNTIME[image.local] : 'Not available'
      } | ${image.cloud ? CLOUD_RUNTIME[image.cloud] : 'Not available yet'} | ${image.spacesd ? 'Yes' : 'No'} | ${image.browsers.map(browserName).join(', ') || 'None'} |`
    );
  }
  return lines.join('\n');
}

/**
 * Every canonical tier, published or not: `<os-version>[-<tier>][-disk]`
 * refs with the SDK call that names each one.
 */
export function renderTiers(list: SandboxImageList): string {
  const lines = ['| Image | Tier | SDK | Status |', '| --- | --- | --- | --- |'];
  for (const image of list.images.filter((i) => i.tier !== undefined && !i.ref.endsWith('-disk'))) {
    lines.push(
      `| \`${image.ref}\` | ${image.tier} | ${image.sdk ? `\`${image.sdk}\`` : '-'} | ${
        // An unpublished tier's ref is not pullable yet: the docs image-ref
        // gate skips a line carrying the unverified marker.
        image.published ? 'Published' : 'Not published yet {/* cua-image-unverified */}'
      } |`
    );
  }
  return lines.join('\n');
}

/** The published Cua images (not benchmarks): the table hand-written pages embed. */
export function renderCuaImages(list: SandboxImageList): string {
  const lines = [
    '| Image | Name | Local | Cloud | cua-spacesd |',
    '| --- | --- | --- | --- | --- |',
  ];
  for (const image of pickerImages(list)) {
    lines.push(
      `| \`${image.ref}\` | ${image.name} | ${image.local ? LOCAL_RUNTIME[image.local] : 'Not available'} | ${
        image.cloud ? CLOUD_RUNTIME[image.cloud] : 'Not available yet'
      } | ${image.spacesd ? 'Yes' : 'No'} |`
    );
  }
  return lines.join('\n');
}

/** Every benchmark a benchmark image runs, with its guide and the image's container tag. */
export function renderBenchmarks(list: SandboxImageList): string {
  const lines = ['| Benchmark | What | Tasks | Image |', '| --- | --- | --- | --- |'];
  for (const image of list.images.filter((i) => i.group === 'benchmark')) {
    for (const b of image.benchmarks ?? []) {
      lines.push(
        `| [${b.name}](${b.guide}) | ${b.what} | ${b.tasks.toLocaleString('en-US')} | \`${image.ref}\` |`
      );
    }
  }
  return lines.join('\n');
}

const OS_HEADING: Record<SandboxImage['os'], string> = {
  linux: 'Linux',
  windows: 'Windows',
  macos: 'macOS',
};

/** A 1-2 word label for a bullet, only where the ref alone is ambiguous. */
function imageLabel(image: SandboxImage): string {
  if (image.os === 'macos') {
    // "macOS Tahoe 26" -> "Tahoe"
    return image.name.replace(/^macOS\s+/, '').replace(/\s+\d+$/, '');
  }
  return image.variant === 'vm' && image.ref.endsWith('-disk') ? 'VM' : '';
}

/**
 * The images, terse: a heading per OS and one for benchmarks,
 * one bullet per ref, a short label only where needed. Published Cua images
 * and every benchmark image (its container tag links the guide).
 */
export function renderImageBullets(list: SandboxImageList): string {
  const sections: Array<[string, string[]]> = [];
  const push = (heading: string, line: string) => {
    let section = sections.find(([h]) => h === heading);
    if (!section) sections.push((section = [heading, []]));
    section[1].push(line);
  };
  for (const image of pickerImages(list)) {
    const label = imageLabel(image);
    push(OS_HEADING[image.os], `- \`${image.ref}\`${label ? ` (${label})` : ''}`);
  }
  for (const image of list.images.filter((i) => i.group === 'benchmark')) {
    const label = imageLabel(image);
    const runs = (image.benchmarks ?? []).map((b) => `[${b.name}](${b.guide})`).join(', ');
    const note = label || (runs ? `run ${runs}` : image.guide ? `[guide](${image.guide})` : '');
    push('Adapter benchmark images', `- \`${image.ref}\`${note ? ` (${note})` : ''}`);
  }
  return sections.map(([h, lines]) => [`### ${h}`, '', ...lines].join('\n')).join('\n\n');
}

/**
 * Regions hand-written docs pages embed: `GENERATED:<name>:start` and `:end`
 * markers in MDX comments. Any page under docs/content may carry them, and
 * the generator fills every one it finds.
 */
export const CATALOG_REGIONS: Record<string, (list: SandboxImageList) => string> = {
  'catalog-cua-images': renderCuaImages,
  'catalog-benchmarks': renderBenchmarks,
  'catalog-image-list': renderImageBullets,
};

function walk(dir: string): string[] {
  const out: string[] = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else if (entry.name.endsWith('.mdx')) out.push(full);
  }
  return out;
}

/** Fills every catalog region of `document`. */
export function fillCatalogRegions(document: string, list: SandboxImageList): string {
  let out = document;
  for (const [name, render] of Object.entries(CATALOG_REGIONS)) {
    const start = `{/* GENERATED:${name}:start */}`;
    const end = `{/* GENERATED:${name}:end */}`;
    let from = 0;
    for (;;) {
      const i = out.indexOf(start, from);
      if (i < 0) break;
      const j = out.indexOf(end, i);
      if (j < 0) throw new Error(`GENERATED:${name} has no end marker`);
      const body = `${start}\n${render(list).trim()}\n${end}`;
      out = out.slice(0, i) + body + out.slice(j + end.length);
      from = i + body.length;
    }
  }
  return out;
}

/** Hand-written pages with catalog regions, filled: path -> contents. */
export function catalogRegionDocuments(list: SandboxImageList): Map<string, string> {
  const out = new Map<string, string>();
  for (const f of walk(path.join(ROOT, 'docs/content/docs'))) {
    const text = fs.readFileSync(f, 'utf8');
    if (!text.includes('GENERATED:catalog-')) continue;
    out.set(f, fillCatalogRegions(text, list));
  }
  return out;
}

function row(columns: string[]): string {
  return `| ${columns.join(' | ')} |`;
}

export function renderCatalog(manifest: SandboxFactsManifest, source: SourceFacts): string {
  const legacyAgent =
    'Ships the legacy `cua-computer-server` guest agent, not cua-spacesd; reach it through services you declare.';
  const lumeAgentless =
    '`cua sb exec`, `cua sb screenshot` and `cua sb view` (and `sb.shell`, `sb.screen`) reach it without cua-spacesd, over SSH and VNC.';
  const rows = [
    row([
      'Linux',
      'Ubuntu 24.04, amd64 and arm64',
      'XFCE on Xvfb, HTML5 viewer on 3211 (`/viewer/`)',
      'Container (local: Docker or Podman, gVisor when installed; cloud: gVisor); `kind="vm"`: the `-disk` containerDisk (local QEMU, cloud KubeVirt)',
      `\`${source.linuxImage}\``,
      `\`Image.linux()\` default. Built from \`libs/images/linux\`; ships cua-spacesd on 3211 (service \`env\`). Warm by default in the cloud. The resolver pins the digest.`,
    ]),
  ];
  for (const image of manifest.qualifiedImages) {
    rows.push(
      row([
        image.os,
        image.distribution,
        image.desktop,
        image.kind,
        `\`${image.image}\``,
        `Deployment qualification recorded boot and readiness for \`server:${image.services.server}\` and \`mcp:${image.services.mcp}\` with \`cua-computer-server\` ${image.components.computerServer} (legacy guest agent) and embedded Driver ${image.components.embeddedDriver}; direct Driver qualification used ${image.components.directDriver}. The \`cua-sandbox\` interfaces need cua-spacesd, which this artifact does not ship; use its declared services directly.`,
      ])
    );
  }
  for (const image of manifest.publishedCandidates) {
    rows.push(
      row([
        image.os,
        image.distribution,
        image.desktop,
        image.kind,
        `\`${image.image}\``,
        image.limits,
      ])
    );
  }
  rows.push(
    row([
      'Windows',
      'Server 2022, amd64',
      'Native Windows desktop',
      'VM from the `-disk` containerDisk (local QEMU, cloud KubeVirt); EFI firmware',
      `\`${source.windowsImage}\``,
      `\`Image.windows()\` default. Ships cua-spacesd (a logon task in the desktop session) with input through the Windows cua-driver. Warm by default in the cloud.`,
    ]),
    row([
      'macOS',
      'Tahoe 26',
      'Native macOS desktop',
      'Local Lume VM',
      `\`${source.macosTahoeImage}\``,
      `\`Image.macos()\` default (full tier; \`tier="slim"\` for \`:26-slim\`). Compatible Apple silicon host required; no cloud variant yet. Built from \`libs/images/macos\`; ships cua-spacesd at login on 3211 (service \`env\`), gated on \`cua-spacesd doctor --strict\` and the accessibility probe. The resolver pins the digest.`,
    ]),
    row([
      'macOS',
      'Sequoia 15',
      'Native macOS desktop',
      'Local Lume VM',
      `\`${source.macosSequoiaImage}\``,
      `\`Image.macos("15")\`. Compatible Apple silicon host required; no cloud variant yet. ${legacyAgent} ${lumeAgentless}`,
    ])
  );
  return `| OS | Distribution / version / architecture | Desktop / window manager | Image kind / runtime | Registry image or source | Evidence / limits |
| --- | --- | --- | --- | --- | --- |
${rows.join('\n')}`;
}

export function generateDocuments(): Map<string, string> {
  const manifest = loadManifest();
  const source = loadSourceFacts();
  assertQualificationSources(manifest, source);
  let runtime = fs.readFileSync(RUNTIME_TEMPLATE, 'utf8');
  runtime = replaceGeneratedSection(
    runtime,
    'sandbox-package-facts',
    renderRuntimePackageFacts(source)
  );
  runtime = replaceGeneratedSection(
    runtime,
    'sandbox-builtin-mappings',
    renderBuiltinMappings(source)
  );
  runtime = replaceGeneratedSection(runtime, 'sandbox-evidence', renderEvidence(source));
  let catalog = fs.readFileSync(CATALOG_TEMPLATE, 'utf8');
  catalog = replaceGeneratedSection(
    catalog,
    'sandbox-image-catalog',
    renderCatalog(manifest, source)
  );
  catalog = replaceGeneratedSection(catalog, 'sandbox-image-list', renderImageList(loadImageList(source)));
  catalog = replaceGeneratedSection(catalog, 'sandbox-image-tiers', renderTiers(loadImageList(source)));
  const list = loadImageList(source);
  const page = (title: string, description: string, body: string) =>
    renderPage({ title, description, generator: GENERATOR, source: SOURCE, version: `cua ${source.cuaVersion}`, body });
  return new Map([
    [
      RUNTIME_DOC,
      page(
        'Runtime support',
        'Which images run on which local runtime or in the cloud, what customization each accepts, and how ports reach them.',
        runtime
      ),
    ],
    [
      CATALOG_DOC,
      page('Image catalog details', 'Every image variant, its runtimes, guest services and limits.', catalog),
    ],
    [
      SOFTWARE_DOC,
      page(
        "What's installed",
        'The apps, tools and simulator runtimes each image tier ships, as the strict doctor measured them.',
        renderSoftware(
          loadInventories(path.join(ROOT, SOFTWARE_DIR)),
          list.images
            .filter((i) => i.group === 'canonical' && i.spacesd && !i.ref.endsWith('-disk'))
            .map((i) => ({ ref: i.ref, published: i.published }))
        )
      ),
    ],
    ...catalogRegionDocuments(list),
  ]);
}

function main(): void {
  const { values } = parseArgs({ options: { check: { type: 'boolean' } } });
  const generated = generateDocuments();
  let drift = false;
  for (const [file, contents] of generated) {
    const current = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null;
    if (current === contents) continue;
    if (values.check) {
      console.error(`Generated Sandbox documentation is stale: ${path.relative(ROOT, file)}`);
      drift = true;
    } else {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, contents);
      console.log(`Generated ${path.relative(ROOT, file)}`);
    }
  }
  if (drift) process.exitCode = 1;
  else if (values.check) console.log('Sandbox package and image facts are up to date.');
}

if (require.main === module) main();
