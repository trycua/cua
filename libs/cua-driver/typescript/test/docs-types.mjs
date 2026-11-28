import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const packageRoot = fileURLToPath(new URL('../', import.meta.url));
const repoRoot = path.resolve(packageRoot, '../../..');
const readme = 'libs/cua-driver/typescript/README.md';
const guide = 'docs/content/docs/cua-driver/guides/use-the-sdk.mdx';
const examples = [
  [readme, 'SDK example', 1],
  [readme, 'Daemon-backed MCP hosts', 2],
  [guide, 'Capture the desktop', 1],
  [guide, 'Act on a window and verify it', 1],
  [guide, 'Expose MCP from a desktop app', 1],
];

const sources = new Map();
for (const [file, heading, count] of examples) {
  const markdown = readFileSync(path.join(repoRoot, file), 'utf8').replace(/\r\n/g, '\n');
  const sections = markdown.split(/^## /m).filter((section) => section.startsWith(`${heading}\n`));
  assert.equal(sections.length, 1, `${file}: expected section '${heading}'`);
  const blocks = [
    ...sections[0].matchAll(/^```(?:ts|typescript)(?:[ \t]+[^\n]*)?\n([\s\S]*?)^```[ \t]*$/gm),
  ];
  assert.equal(blocks.length, count, `${file}: TypeScript example count in '${heading}'`);
  const sectionName = heading.toLowerCase().replaceAll(' ', '-');
  for (const [index, [, source]] of blocks.entries()) {
    assert.ok(source.trim(), `${file}: empty TypeScript example in '${heading}'`);
    const filename = path.join(packageRoot, 'test', `docs-${sectionName}-${index + 1}.mts`);
    sources.set(filename, source);
  }
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
  console.log(`Typechecked ${sources.size} documentation examples.`);
}
