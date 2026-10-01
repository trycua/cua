import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const packageRoot = fileURLToPath(new URL('../', import.meta.url));
const repoRoot = path.resolve(packageRoot, '../../..');
const guide = 'docs/content/docs/how-to-guides/driver/';
const examples = [
  ['sdk', 'libs/cua-driver/typescript/README.md', ['SDK example']],
  ['host', 'libs/cua-driver/typescript/README.md', ['Daemon-backed MCP hosts']],
  [
    'implicit-session',
    `${guide}use-sdk-in-process.mdx`,
    ['Capture the desktop with an implicit session'],
  ],
  [
    'native-window',
    `${guide}use-sdk-in-process.mdx`,
    ['Discover and act on an exact native window'],
  ],
  [
    'mcp-host',
    `${guide}expose-mcp-from-desktop-app.mdx`,
    ['Start the host from the permission-owning process', 'Shut down in dependency order'],
  ],
  [
    'embedding',
    'docs/content/docs/reference/cua-driver/embedding.mdx',
    ['Node and Electron hosts'],
  ],
];

const sources = new Map();
for (const [name, file, headings] of examples) {
  const markdown = readFileSync(path.join(repoRoot, file), 'utf8').replace(/\r\n/g, '\n');
  const snippets = headings.flatMap((heading) => {
    const sections = markdown
      .split(/^## /m)
      .filter((section) => section.startsWith(`${heading}\n`));
    assert.equal(sections.length, 1, `${file}: expected section '${heading}'`);
    const blocks = [...sections[0].matchAll(/^```(?:ts|typescript)\r?\n([\s\S]*?)^```/gm)];
    assert.ok(blocks.length, `${file}: no TypeScript example in '${heading}'`);
    return blocks.map((block) => {
      assert.ok(block[1].trim(), `${file}: empty TypeScript example in '${heading}'`);
      return block[1];
    });
  });
  // Each example is an independent module. The MCP guide's startup and
  // cleanup blocks share a scope; its application-owned backend sketch does not.
  const filename = path.join(packageRoot, 'test', `docs-${name}.mts`);
  sources.set(filename, snippets.join('\n'));
  console.log(`${name}: ${file} (${headings.join('; ')})`);
}

const options = {
  noEmit: true,
  strict: true,
  skipLibCheck: true,
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.NodeNext,
  moduleResolution: ts.ModuleResolutionKind.NodeNext,
  // Check the candidate package without loading a native library or relying
  // on a previously built dist directory.
  paths: {
    '@trycua/cua-driver': [path.join(packageRoot, 'src/index.ts')],
    '@trycua/cua-driver/*': [path.join(packageRoot, 'src/*.ts')],
  },
};
const host = ts.createCompilerHost(options);
const getSourceFile = host.getSourceFile.bind(host);
host.getSourceFile = (filename, languageVersion, ...args) => {
  const source = sources.get(filename);
  return source === undefined
    ? getSourceFile(filename, languageVersion, ...args)
    : ts.createSourceFile(filename, source, languageVersion, true);
};
const program = ts.createProgram([...sources.keys()], options, host);
const diagnostics = ts.getPreEmitDiagnostics(program);
if (diagnostics.length) {
  console.error(
    ts.formatDiagnostics(diagnostics, {
      getCanonicalFileName: (filename) => filename,
      getCurrentDirectory: () => packageRoot,
      getNewLine: () => '\n',
    })
  );
  process.exitCode = 1;
} else {
  console.log(`Typechecked ${examples.length} documentation examples.`);
}
