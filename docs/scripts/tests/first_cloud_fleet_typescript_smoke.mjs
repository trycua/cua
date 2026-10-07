import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import test from 'node:test';

const source = process.env.FIRST_CLOUD_FLEET_TS;
if (!source) throw new Error('Set FIRST_CLOUD_FLEET_TS to the copied tutorial source');
const { runTutorial } = await import(pathToFileURL(source));

const IMAGE_REF = `registry.example/cua-image@sha256:${'1'.repeat(64)}`;
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47]).buffer;

function fakeCua({ failShell = false, wrongDigest = false } = {}) {
  const calls = [];
  const files = new Map();
  const env = {
    upload: async (path, data) => {
      calls.push(['upload', path]);
      files.set(path, new TextDecoder().decode(data));
      return { size: BigInt(data.byteLength), sha256: '', resumes: 0 };
    },
    sh: async (line) => {
      calls.push(['sh', line]);
      if (failShell) throw new Error('synthetic shell failure');
      const text = files.get('/tmp/first-cloud-fleet.txt');
      const digest = wrongDigest ? 'wrong' : createHash('sha256').update(text).digest('hex');
      return {
        exit: { success: true, timedOut: false },
        stdout: new TextEncoder().encode(`${digest}  /tmp/first-cloud-fleet.txt\n`).buffer,
        stderr: new ArrayBuffer(0),
        pty: new ArrayBuffer(0),
      };
    },
    screenshot: async () => (calls.push(['screenshot']), { image: PNG }),
  };
  const sandbox = {
    name: () => 'cua-auto-test-claim',
    spacesd: async () => (calls.push(['spacesd']), env),
    delete_: async () => calls.push(['delete']),
  };
  return {
    calls,
    sandboxes: () => ({
      create: async (options) => (calls.push(['create', options]), sandbox),
    }),
  };
}

async function inTempDir(fn) {
  const cwd = process.cwd();
  process.chdir(await mkdtemp(join(tmpdir(), 'first-fleet-')));
  try {
    return await fn();
  } finally {
    process.chdir(cwd);
  }
}

test('TypeScript example uploads, hashes in the guest, verifies locally, and releases', async () => {
  const cua = fakeCua();
  await inTempDir(() => runTutorial(cua, IMAGE_REF));
  const [, options] = cua.calls.find(([name]) => name === 'create');
  assert.equal(options.image, IMAGE_REF);
  assert.equal(options.on, 'cloud');
  assert.equal(options.name, undefined);
  assert.deepEqual(
    cua.calls.map(([name]) => name),
    ['create', 'spacesd', 'upload', 'sh', 'screenshot', 'delete']
  );
});

test('TypeScript example releases the claim when verification fails', async () => {
  const cua = fakeCua({ wrongDigest: true });
  await assert.rejects(inTempDir(() => runTutorial(cua, IMAGE_REF)), /Verification failed/);
  assert.equal(cua.calls.at(-1)[0], 'delete');
});

test('TypeScript example releases the claim when a guest command fails', async () => {
  const cua = fakeCua({ failShell: true });
  await assert.rejects(inTempDir(() => runTutorial(cua, IMAGE_REF)), /synthetic shell failure/);
  assert.equal(cua.calls.at(-1)[0], 'delete');
});
