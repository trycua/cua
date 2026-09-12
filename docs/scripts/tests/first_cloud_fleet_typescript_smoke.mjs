import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import test from 'node:test';

const source = process.env.FIRST_CLOUD_FLEET_TS;
if (!source) throw new Error('Set FIRST_CLOUD_FLEET_TS to the copied tutorial source');
const { runTutorial } = await import(pathToFileURL(source));

const SOURCE_TEXT = 'pear\napple\npear\nbanana\n';
const POOL_NAME = 'first-fleet-0123456789abcdef';
const IMAGE_REF = `registry.example/cua-image@sha256:${'1'.repeat(64)}`;

function response(payload) {
  return {
    status: 200,
    headers: [],
    body: new TextEncoder().encode(`data: ${JSON.stringify(payload)}\n\n`).buffer,
  };
}

function fakeClient({ failCommand, failDeleteClaim = false } = {}) {
  const calls = [];
  const files = new Map();
  const pool = { metadata: { namespace: POOL_NAME, name: POOL_NAME } };
  const template = { metadata: { namespace: POOL_NAME, name: `${POOL_NAME}-template` } };
  const claim = { metadata: { namespace: POOL_NAME, name: 'first-task' } };
  const sandbox = { metadata: { namespace: POOL_NAME, name: 'guest' } };
  const client = {
    calls,
    createPool: async (request) => (calls.push(['createPool', request]), pool),
    createTemplate: async (request) => (calls.push(['createTemplate', request]), template),
    createClaim: async (request) => (calls.push(['createClaim', request]), claim),
    waitClaim: async (value) => (calls.push(['waitClaim', value]), sandbox),
    deleteClaim: async (value) => {
      calls.push(['deleteClaim', value]);
      if (failDeleteClaim) throw new Error('synthetic claim cleanup failure');
    },
    deleteTemplate: async (value) => calls.push(['deleteTemplate', value]),
    deletePool: async (value) => calls.push(['deletePool', value]),
    serviceRequest: async (_sandbox, service, path, request) => {
      if (path === '/status') {
        calls.push(['serviceRequest', service, path, 'status']);
        return { status: 200, headers: [], body: new ArrayBuffer(0) };
      }
      const { command, params } = JSON.parse(new TextDecoder().decode(request.body));
      calls.push(['serviceRequest', service, path, command, params]);
      if (command === failCommand) throw new Error(`synthetic ${command} failure`);
      if (command === 'write_text') files.set(params.path, params.content);
      if (command === 'run_command') {
        const digest = createHash('sha256')
          .update(files.get('/tmp/first-cloud-fleet.txt'))
          .digest('hex');
        files.set('/tmp/first-cloud-fleet.sha256', `${digest}\n`);
      }
      if (command === 'read_text')
        return response({ success: true, content: files.get(params.path) });
      return response({ success: true });
    },
  };
  return client;
}

test('TypeScript example writes, transforms, independently verifies, and cleans up', async () => {
  const client = fakeClient();
  await runTutorial(client, IMAGE_REF, POOL_NAME);

  const poolRequest = client.calls.find(([name]) => name === 'createPool')[1];
  const templateRequest = client.calls.find(([name]) => name === 'createTemplate')[1];
  assert.equal(poolRequest.spec.replicas, 1);
  assert.deepEqual(templateRequest.spec.vmTemplate.services, [
    { name: 'server', targetPort: 8000 },
  ]);
  assert.deepEqual(
    client.calls.filter(([name]) => name === 'serviceRequest').map((call) => call[3]),
    ['status', 'write_text', 'run_command', 'read_text']
  );
  assert.deepEqual(
    client.calls.slice(-3).map(([name]) => name),
    ['deleteClaim', 'deleteTemplate', 'deletePool']
  );
});

test('TypeScript example cleans up after a guest command fails', async () => {
  const client = fakeClient({ failCommand: 'run_command' });
  await assert.rejects(runTutorial(client, IMAGE_REF, POOL_NAME), /synthetic run_command failure/);
  assert.deepEqual(
    client.calls.slice(-3).map(([name]) => name),
    ['deleteClaim', 'deleteTemplate', 'deletePool']
  );
});

test('TypeScript example continues pool cleanup after claim cleanup fails', async () => {
  const client = fakeClient({ failDeleteClaim: true });
  await assert.rejects(runTutorial(client, IMAGE_REF, POOL_NAME), /Cleanup failed/);
  assert.deepEqual(
    client.calls.slice(-3).map(([name]) => name),
    ['deleteClaim', 'deleteTemplate', 'deletePool']
  );
});
