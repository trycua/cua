import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { build } from 'esbuild';

const packageDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repositoryRoot = resolve(packageDirectory, '../../..');
const bindingDirectory = join(repositoryRoot, 'libs/fleet/sdk-bindings/ts-uniffi-browser');
const generatedDirectory = join(bindingDirectory, 'ts');
const generatedEntry = join(generatedDirectory, 'index.web.ts');
const distributionDirectory = join(packageDirectory, 'dist');
const declarationsDirectory = join(distributionDirectory, '.declarations');
const rustToolchainFile = join(repositoryRoot, 'libs/fleet/rust-toolchain.toml');
const rustToolchainMatch = readFileSync(rustToolchainFile, 'utf8').match(
  /^channel\s*=\s*"([^"]+)"/m
);
if (!rustToolchainMatch) {
  throw new Error(`Could not read the Rust channel from ${rustToolchainFile}`);
}
const rustToolchain = rustToolchainMatch[1];
const wasmBindgenVersion = '0.2.126';

function run(command, args, workingDirectory = packageDirectory) {
  execFileSync(command, args, {
    cwd: workingDirectory,
    stdio: 'inherit',
  });
}

function hasWasmBindgenVersion() {
  const result = spawnSync('wasm-bindgen', ['--version'], {
    encoding: 'utf8',
  });
  return result.status === 0 && result.stdout.trim() === `wasm-bindgen ${wasmBindgenVersion}`;
}

rmSync(distributionDirectory, { force: true, recursive: true });
mkdirSync(distributionDirectory, { recursive: true });

run('rustup', ['toolchain', 'install', rustToolchain, '--profile', 'minimal']);
run('rustup', ['target', 'add', 'wasm32-unknown-unknown', '--toolchain', rustToolchain]);
if (!hasWasmBindgenVersion()) {
  run('cargo', [
    `+${rustToolchain}`,
    'install',
    'wasm-bindgen-cli',
    '--version',
    wasmBindgenVersion,
    '--locked',
  ]);
}

run('npm', ['ci'], bindingDirectory);
run('npm', ['run', 'build:wasm'], bindingDirectory);

await build({
  entryPoints: [generatedEntry],
  bundle: true,
  format: 'esm',
  platform: 'browser',
  target: ['es2022'],
  outdir: distributionDirectory,
  entryNames: 'index',
  assetNames: 'fleet',
  loader: {
    '.wasm': 'file',
  },
});

const bundlePath = join(distributionDirectory, 'index.js');
const wasmReference = '"./fleet.wasm"';
const bundle = readFileSync(bundlePath, 'utf8');
if (bundle.split(wasmReference).length !== 2) {
  throw new Error('Expected exactly one bundled WebAssembly asset reference');
}
writeFileSync(
  bundlePath,
  bundle.replace(wasmReference, 'new URL("./fleet.wasm", import.meta.url)')
);

const typescript = join(packageDirectory, 'node_modules/.bin/tsc');
run(typescript, [
  '--declaration',
  '--emitDeclarationOnly',
  '--outDir',
  declarationsDirectory,
  '--module',
  'esnext',
  '--moduleResolution',
  'bundler',
  '--target',
  'es2022',
  '--skipLibCheck',
  '--allowArbitraryExtensions',
  'true',
  generatedEntry,
]);

for (const file of [
  'cyclops_sdk_schema-ffi.d.ts',
  'cyclops_sdk_schema.d.ts',
  'fleet_sdk-ffi.d.ts',
  'fleet_sdk.d.ts',
]) {
  cpSync(join(declarationsDirectory, file), join(distributionDirectory, file));
}
renameSync(
  join(declarationsDirectory, 'index.web.d.ts'),
  join(distributionDirectory, 'index.d.ts')
);
const declarationPath = join(distributionDirectory, 'index.d.ts');
writeFileSync(declarationPath, readFileSync(declarationPath, 'utf8').replaceAll('.//', './'));
rmSync(declarationsDirectory, { force: true, recursive: true });
