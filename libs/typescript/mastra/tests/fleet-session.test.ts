import assert from 'node:assert/strict';
import { test } from 'node:test';
import { CyclopsClient, SdkError } from '@trycua/fleet/node';
import { CloudFleetSession, type SessionOptions } from '../src/fleet-session.js';

const options: SessionOptions = {
  poolName: 'fake-pool',
  clientId: 'fake-client',
  clientSecret: 'fake-secret',
  claimTtlSeconds: 3600,
  startupTimeoutMs: 1000,
  requestTimeoutMs: 5,
};

function fakeClient(createError: unknown = new Error('response lost')) {
  let name = '';
  let present = false;
  let lists = 0;
  const deleted: string[] = [];
  const client = {
    async getPool() {
      return {
        metadata: { namespace: 'owned-namespace' },
        spec: { sandboxTemplateRef: { name: 'template' } },
      };
    },
    async createClaim(request: { name: string }) {
      name = request.name;
      throw createError;
    },
    async listClaims(namespace: string) {
      assert.equal(namespace, 'owned-namespace');
      lists++;
      return present ? [{ metadata: { name, namespace } }] : [];
    },
    async deleteClaim(claim: { metadata: { name: string } }) {
      assert.equal(claim.metadata.name, name);
      deleted.push(claim.metadata.name);
      present = false;
    },
  };
  return {
    client: client as unknown as CyclopsClient,
    appear: () => {
      present = true;
    },
    get lists() {
      return lists;
    },
    get name() {
      return name;
    },
    deleted,
  };
}

test('ambiguous creation retains ownership and retries cleanup when the late claim appears', async (t) => {
  const fake = fakeClient();
  t.mock.method(CyclopsClient, 'connect', () => fake.client);
  const session = new CloudFleetSession(options);
  await assert.rejects(session.start(), /response lost/);
  assert.match(fake.name, /^mastra-[a-f0-9-]+$/);
  await assert.rejects(session.close(), /CLAIM_CREATION_OUTCOME_UNKNOWN/);
  assert.equal(fake.deleted.length, 0);
  assert.ok(fake.lists >= 2, 'absence is rechecked during recovery');
  fake.appear();
  await session.close();
  assert.deepEqual(fake.deleted, [fake.name]);
  const completedLists = fake.lists;
  await session.close();
  assert.equal(fake.lists, completedLists, 'completed cleanup is idempotent');
});

test('claim appearing during recovery is deleted and its absence verified', async (t) => {
  const fake = fakeClient();
  const originalList = fake.client.listClaims.bind(fake.client);
  let reads = 0;
  t.mock.method(fake.client, 'listClaims', async (namespace: string) => {
    if (++reads === 2) fake.appear();
    return originalList(namespace);
  });
  t.mock.method(CyclopsClient, 'connect', () => fake.client);
  const session = new CloudFleetSession({ ...options, requestTimeoutMs: 20 });
  await assert.rejects(session.start(), /response lost/);
  await session.close();
  assert.deepEqual(fake.deleted, [fake.name]);
  assert.equal(reads, 3, 'one absent read, one recovered claim, one deletion verification');
});

test('definitive create rejection does not search for or delete claims', async (t) => {
  const rejection = new SdkError.Status({
    operation: 'createClaim',
    status: 403,
    body: 'withheld',
  });
  const fake = fakeClient(rejection);
  t.mock.method(CyclopsClient, 'connect', () => fake.client);
  const session = new CloudFleetSession(options);
  await assert.rejects(session.start(), (error) => error === rejection);
  await session.close();
  assert.equal(fake.lists, 0);
  assert.deepEqual(fake.deleted, []);
});

test('failed deletion verification keeps confirmed identity for a safe retry', async (t) => {
  const fake = fakeClient();
  const originalList = fake.client.listClaims.bind(fake.client);
  let failVerification = true;
  t.mock.method(fake.client, 'listClaims', async (namespace: string) => {
    if (fake.deleted.length && failVerification) throw new Error('inventory unavailable');
    return originalList(namespace);
  });
  t.mock.method(CyclopsClient, 'connect', () => fake.client);
  const session = new CloudFleetSession(options);
  await assert.rejects(session.start(), /response lost/);
  fake.appear();
  await assert.rejects(session.close(), /inventory unavailable/);
  failVerification = false;
  await session.close();
  assert.deepEqual(fake.deleted, [fake.name], 'confirmed deleted claim is not deleted again');
});
