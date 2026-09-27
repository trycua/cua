#!/usr/bin/env npx tsx

/** Generate the source-backed Sandbox package and image facts in public docs. */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { parseArgs } from 'node:util';

const ROOT = path.resolve(__dirname, '../..');
const FACTS_PATH = path.join(__dirname, 'sandbox-facts.json');
const RUNTIME_DOC = path.join(ROOT, 'docs/content/docs/reference/sandbox-sdk/runtime-support.mdx');
const CATALOG_DOC = path.join(ROOT, 'docs/content/docs/reference/sandbox-sdk/os-image-catalog.mdx');

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
  computerServerVersion: string;
  embeddedDriverConstraint: string;
  qualifiedDirectDriverVersion: string;
  linuxFleetImage: string;
  windowsFleetImage: string;
  linuxContainerImage: string;
  macosSequoiaImage: string;
  macosTahoeImage: string;
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

function pythonConstant(relativePath: string, name: string): string {
  const contents = read(relativePath);
  return capture(
    contents,
    new RegExp(`^${name}\\s*=\\s*"([^"]+)"`, 'm'),
    `${relativePath}:${name}`
  );
}

export function loadSourceFacts(): SourceFacts {
  const sandboxProject = read('libs/python/cua-sandbox/pyproject.toml');
  const computerServerProject = read('libs/python/computer-server/pyproject.toml');
  const runtimeImages = read('libs/python/cua-sandbox/cua_sandbox/runtime/images.py');
  const profile = JSON.parse(
    read('libs/cua-driver/hyprland-plugin/packaging/release/profiles/omarchy-stable-20260910.json')
  ) as { source?: { driver_version?: string } };
  return {
    sandboxVersion: projectVersion('libs/python/cua-sandbox/pyproject.toml'),
    cuaVersion: projectVersion('libs/python/cua/pyproject.toml'),
    pythonFleetVersion: capture(sandboxProject, /"cua-fleet==([^"]+)"/, 'cua-fleet pin'),
    typescriptFleetVersion: String(
      (JSON.parse(read('libs/typescript/fleet/package.json')) as { version: string }).version
    ),
    sandboxDriverVersion: capture(sandboxProject, /"cua-driver==([^"]+)"/, 'Sandbox Driver pin'),
    computerServerVersion: projectVersion('libs/python/computer-server/pyproject.toml'),
    embeddedDriverConstraint: capture(
      computerServerProject,
      /"cua-driver([^";]+)"/,
      'computer-server embedded Driver constraint'
    ),
    qualifiedDirectDriverVersion: string(
      profile.source?.driver_version,
      'Omarchy release profile Driver version',
      VERSION
    ),
    linuxFleetImage: pythonConstant(
      'libs/python/cua-sandbox/cua_sandbox/image.py',
      'DEFAULT_LINUX_REGISTRY_IMAGE'
    ),
    windowsFleetImage: pythonConstant(
      'libs/python/cua-sandbox/cua_sandbox/image.py',
      'DEFAULT_WINDOWS_REGISTRY_IMAGE'
    ),
    linuxContainerImage: pythonConstant(
      'libs/python/cua-sandbox/cua_sandbox/runtime/images.py',
      'UBUNTU_XFCE'
    ),
    macosSequoiaImage: capture(runtimeImages, /"15":\s*"([^"]+)"/, 'macOS Sequoia image mapping'),
    macosTahoeImage: capture(runtimeImages, /"26":\s*"([^"]+)"/, 'macOS Tahoe image mapping'),
  };
}

function assertQualificationSources(manifest: SandboxFactsManifest, source: SourceFacts): void {
  for (const image of manifest.qualifiedImages) {
    if (image.components.computerServer !== source.computerServerVersion) {
      throw new Error(`${image.id} computer-server version disagrees with its package manifest`);
    }
    if (!source.embeddedDriverConstraint.includes(image.components.embeddedDriver)) {
      throw new Error(`${image.id} embedded Driver is outside the computer-server package mapping`);
    }
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
  return `| Built-in image | Registry artifact |
| --- | --- |
| Ubuntu 24.04 VM | \`${source.linuxFleetImage}\` |
| Windows Server 2022 VM | \`${source.windowsFleetImage}\` |`;
}

export function renderEvidence(source: SourceFacts): string {
  return `- \`cua-sandbox\` ${source.sandboxVersion} source: image constructors, registry mappings, runtime
  selection, pool claims, Fleet validation, firmware, and services.
- \`cua\` ${source.cuaVersion} source: public Python exports.
- \`cua-fleet\` ${source.pythonFleetVersion} package mapping: Python bound-claim service requests.
- \`@trycua/fleet\` ${source.typescriptFleetVersion} package mapping: TypeScript service requests.
- \`cua-sandbox[driver]\` ${source.sandboxVersion} maps direct Driver use to \`cua-driver\`
  ${source.sandboxDriverVersion}. This client mapping is not the Driver version inside a particular
  published or deployment-qualified guest image.`;
}

function row(columns: string[]): string {
  return `| ${columns.join(' | ')} |`;
}

export function renderCatalog(manifest: SandboxFactsManifest, source: SourceFacts): string {
  const rows = [
    row([
      'Linux',
      'Ubuntu 24.04; artifact architecture not established by SDK mapping',
      'Artifact-dependent; not specified by SDK mapping',
      'Fleet containerDisk; local bare-metal QEMU',
      `\`${source.linuxFleetImage}\``,
      `\`Image.linux()\` mapping in \`cua-sandbox\` ${source.sandboxVersion}; versioned tag, not a digest pin. [SDK checks](/reference/sandbox-sdk/runtime-support#evidence-and-limits) establish selection, not a live pull, boot, or guest-service check.`,
    ]),
    row([
      'Linux',
      'Ubuntu 24.04',
      'XFCE runtime mapping',
      'Local Docker container',
      `\`${source.linuxContainerImage}\``,
      'Local container selector; mutable tag, not the Fleet VM disk.',
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
        `Deployment qualification recorded boot and readiness for \`server:${image.services.server}\` and \`mcp:${image.services.mcp}\`. The guest contains \`cua-computer-server\` ${image.components.computerServer} with embedded Driver ${image.components.embeddedDriver}; direct Driver qualification used ${image.components.directDriver}. These image-specific results do not change the published client mapping or certify every operation and desktop configuration.`,
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
      'Server 2022; artifact architecture not established by SDK mapping',
      'Native Windows desktop',
      'Fleet containerDisk; local Hyper-V or QEMU',
      `\`${source.windowsFleetImage}\``,
      `\`Image.windows()\` mapping in \`cua-sandbox\` ${source.sandboxVersion}; Fleet selects EFI. Versioned tag, not a digest pin. [SDK checks](/reference/sandbox-sdk/runtime-support#evidence-and-limits) establish selection and firmware, not a live pull, boot, or guest-service check.`,
    ]),
    row([
      'macOS',
      'Sequoia 15',
      'Native macOS desktop',
      'Local Lume VM',
      `\`${source.macosSequoiaImage}\``,
      'Compatible Apple silicon host required; mutable tag; no built-in Fleet mapping.',
    ]),
    row([
      'macOS',
      'Tahoe 26',
      'Native macOS desktop',
      'Local Lume VM',
      `\`${source.macosTahoeImage}\``,
      'Default `Image.macos()` version; compatible Apple silicon host required; mutable tag; no built-in Fleet mapping.',
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
  let runtime = fs.readFileSync(RUNTIME_DOC, 'utf8');
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
  let catalog = fs.readFileSync(CATALOG_DOC, 'utf8');
  catalog = replaceGeneratedSection(
    catalog,
    'sandbox-image-catalog',
    renderCatalog(manifest, source)
  );
  return new Map([
    [RUNTIME_DOC, runtime],
    [CATALOG_DOC, catalog],
  ]);
}

function main(): void {
  const { values } = parseArgs({ options: { check: { type: 'boolean' } } });
  const generated = generateDocuments();
  let drift = false;
  for (const [file, contents] of generated) {
    const current = fs.readFileSync(file, 'utf8');
    if (current === contents) continue;
    if (values.check) {
      console.error(`Generated Sandbox documentation is stale: ${path.relative(ROOT, file)}`);
      drift = true;
    } else {
      fs.writeFileSync(file, contents);
      console.log(`Generated ${path.relative(ROOT, file)}`);
    }
  }
  if (drift) process.exitCode = 1;
  else if (values.check) console.log('Sandbox package and image facts are up to date.');
}

if (require.main === module) main();
