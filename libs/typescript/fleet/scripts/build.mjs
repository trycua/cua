import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repositoryRoot = resolve(packageDirectory, '../../..');
const bindingDirectory = join(repositoryRoot, 'libs/fleet/sdk-bindings/ts-uniffi-browser');
const generatedDirectory = join(bindingDirectory, 'ts');
const browserEntry = join(generatedDirectory, 'package.browser.ts');
const nodeEntry = join(generatedDirectory, 'package.node.ts');
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

export const RESPONSE_LIMIT_ERROR = 'HTTP response exceeds configured size limit';

export function createTimeoutSignal(timeoutMs, timeoutFactory = AbortSignal.timeout) {
  if (typeof timeoutFactory === 'function') {
    return { signal: timeoutFactory(timeoutMs), dispose() {} };
  }

  const controller = new AbortController();
  const timer = setTimeout(
    () => controller.abort(new DOMException('Timed out', 'TimeoutError')),
    timeoutMs
  );
  timer.unref?.();
  return {
    signal: controller.signal,
    dispose() {
      clearTimeout(timer);
    },
  };
}

export function composeAbortSignals(signals, signalFactory = AbortSignal.any) {
  const activeSignals = signals.filter((signal) => signal !== undefined);
  if (activeSignals.length < 2) {
    return { signal: activeSignals[0], dispose() {} };
  }
  if (typeof signalFactory === 'function') {
    return { signal: signalFactory(activeSignals), dispose() {} };
  }

  const controller = new AbortController();
  const onAbort = (event) => controller.abort(event.target.reason);
  for (const signal of activeSignals) {
    if (signal.aborted) {
      controller.abort(signal.reason);
      break;
    }
    signal.addEventListener('abort', onAbort, { once: true });
  }
  return {
    signal: controller.signal,
    dispose() {
      for (const signal of activeSignals) {
        signal.removeEventListener('abort', onAbort);
      }
    },
  };
}

export async function readResponseBody(response, maxResponseBytes) {
  if (response.body === null) {
    return new ArrayBuffer(0);
  }

  const reader = response.body.getReader();
  const chunks = [];
  let totalBytes = 0;
  let totalBytesBigInt = 0n;
  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    const chunk = value instanceof Uint8Array ? value : new Uint8Array(value);
    const nextTotalBytes = totalBytes + chunk.byteLength;
    const nextTotalBytesBigInt = totalBytesBigInt + BigInt(chunk.byteLength);
    if (
      maxResponseBytes !== undefined &&
      maxResponseBytes !== null &&
      nextTotalBytesBigInt > maxResponseBytes
    ) {
      try {
        await reader.cancel();
      } catch {
        // Preserve the stable, sanitized limit error even if cancellation fails.
      }
      throw new Error(RESPONSE_LIMIT_ERROR);
    }
    chunks.push(chunk);
    totalBytes = nextTotalBytes;
    totalBytesBigInt = nextTotalBytesBigInt;
  }

  const body = new Uint8Array(totalBytes);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body.buffer;
}

export async function executeFetchRequest(request, callerSignal, fetchImplementation = fetch) {
  const timeout =
    request.timeoutSecs !== undefined && request.timeoutSecs !== null
      ? createTimeoutSignal(Number(request.timeoutSecs) * 1_000)
      : undefined;
  const combinedSignal = composeAbortSignals([callerSignal, timeout?.signal]);
  try {
    const response = await fetchImplementation(request.url, {
      method: request.method,
      headers: request.headers.map(({ name, value }) => [name, value]),
      body: request.body,
      signal: combinedSignal.signal,
      redirect: 'manual',
    });
    return {
      status: response.status,
      headers: [...response.headers].map(([name, value]) => ({ name, value })),
      body: await readResponseBody(response, request.maxResponseBytes),
    };
  } finally {
    combinedSignal.dispose();
    timeout?.dispose();
  }
}

const sharedExports = `
export * from './cyclops_sdk_schema'
export * from './fleet_sdk'

import * as cyclops_sdk_schema from './cyclops_sdk_schema'
import * as fleet_sdk from './fleet_sdk'
import initAsync from './wasm-bindgen/index.js'
import wasmPath from './wasm-bindgen/index_bg.wasm'

let initialization: Promise<void> | undefined

function initializeBindings(): void {
  cyclops_sdk_schema.default.initialize()
  fleet_sdk.default.initialize()
}

export default {
  cyclops_sdk_schema,
  fleet_sdk,
}
`;

const browserSource = `${sharedExports}
export function uniffiInitAsync(): Promise<void> {
  initialization ??= (async () => {
    await initAsync({ module_or_path: new URL(wasmPath as unknown as string, import.meta.url) })
    initializeBindings()
  })()
  return initialization
}
`;

const nodeSource = `
import { readFile } from 'node:fs/promises'
import type { HttpClient, HttpRequest, HttpResponse } from './fleet_sdk'
${sharedExports}

const RESPONSE_LIMIT_ERROR = ${JSON.stringify(RESPONSE_LIMIT_ERROR)}

${createTimeoutSignal.toString()}

${composeAbortSignals.toString()}

${readResponseBody.toString()}

${executeFetchRequest.toString()}

export class FetchHttpClient implements HttpClient {
  async execute(
    request: HttpRequest,
    asyncOpts_?: { signal: AbortSignal },
  ): Promise<HttpResponse> {
    return executeFetchRequest(
      request as HttpRequest & { maxResponseBytes?: bigint },
      asyncOpts_?.signal,
    )
  }
}

export function uniffiInitAsync(): Promise<void> {
  initialization ??= (async () => {
    const wasm = await readFile(new URL(wasmPath as unknown as string, import.meta.url))
    await initAsync({ module_or_path: wasm })
    initializeBindings()
  })()
  return initialization
}

export async function createFleetClient(
  configuration: fleet_sdk.CyclopsTokenProviderConfiguration,
  accessToken: string,
): Promise<fleet_sdk.CyclopsClient> {
  await uniffiInitAsync()
  return fleet_sdk.CyclopsClient.connectWithAccessToken(
    configuration,
    accessToken,
    new FetchHttpClient(),
  ) as fleet_sdk.CyclopsClient
}
`;

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

function normalizeDeclarationImports(source) {
  return source
    .replaceAll('.//', './')
    .replace(/(from\s+['"])(\.\/[^'"]+?)(['"])/g, (_match, prefix, specifier, suffix) => {
      return `${prefix}${specifier.endsWith('.js') ? specifier : `${specifier}.js`}${suffix}`;
    });
}

async function main() {
  const { build } = await import('esbuild');

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
  writeFileSync(browserEntry, browserSource);
  writeFileSync(nodeEntry, nodeSource);

  try {
    await build({
      entryPoints: { browser: browserEntry },
      bundle: true,
      format: 'esm',
      platform: 'browser',
      target: ['es2022'],
      outdir: distributionDirectory,
      assetNames: 'fleet',
      loader: {
        '.wasm': 'file',
      },
    });

    await build({
      entryPoints: { node: nodeEntry },
      bundle: true,
      format: 'esm',
      platform: 'node',
      target: ['node20'],
      outdir: distributionDirectory,
      assetNames: 'fleet',
      loader: {
        '.wasm': 'file',
      },
    });

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
      '--types',
      'node',
      '--skipLibCheck',
      '--allowArbitraryExtensions',
      'true',
      browserEntry,
      nodeEntry,
    ]);

    const declarationFiles = [
      'cyclops_sdk_schema-ffi.d.ts',
      'cyclops_sdk_schema.d.ts',
      'fleet_sdk-ffi.d.ts',
      'fleet_sdk.d.ts',
    ];
    for (const file of declarationFiles) {
      cpSync(join(declarationsDirectory, file), join(distributionDirectory, file));
    }
    renameSync(
      join(declarationsDirectory, 'package.browser.d.ts'),
      join(distributionDirectory, 'browser.d.ts')
    );
    renameSync(
      join(declarationsDirectory, 'package.node.d.ts'),
      join(distributionDirectory, 'node.d.ts')
    );
    for (const file of [...declarationFiles, 'browser.d.ts', 'node.d.ts']) {
      const declarationPath = join(distributionDirectory, file);
      writeFileSync(
        declarationPath,
        normalizeDeclarationImports(readFileSync(declarationPath, 'utf8'))
      );
    }
  } finally {
    rmSync(browserEntry, { force: true });
    rmSync(nodeEntry, { force: true });
    rmSync(declarationsDirectory, { force: true, recursive: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
