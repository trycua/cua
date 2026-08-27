import assert from 'node:assert/strict';
import test from 'node:test';

import { CyclopsClient, CyclopsConfiguration, CyclopsCredentials } from '../dist/node.js';

const encoder = new TextEncoder();

class ScriptedHttpClient {
  requests = [];

  async execute(request) {
    this.requests.push(request);
    if (request.url === 'https://identity.example/oauth/token') {
      return {
        status: 200,
        headers: [],
        body: encoder.encode(JSON.stringify({ access_token: 'token-a', expires_in: 3600 })).buffer,
      };
    }
    return { status: 200, headers: [], body: encoder.encode('[]').buffer };
  }
}

test('initializes synchronous FFI calls and caches client-credentials tokens', async () => {
  const httpClient = new ScriptedHttpClient();
  const credentials = new CyclopsCredentials('client-id', 'client-secret');
  const configuration = CyclopsConfiguration.create({
    baseUrl: 'https://run.example',
    tokenUrl: 'https://identity.example/oauth/token',
    credentials,
    poolPollIntervalMs: 1n,
    poolPollLimit: 1,
    claimPollIntervalMs: 1n,
    claimPollLimit: 1,
  });
  const client = CyclopsClient.connect(configuration, httpClient);

  await client.listNamespaces();
  await client.listNamespaces();

  const tokenRequests = httpClient.requests.filter(
    ({ url }) => url === 'https://identity.example/oauth/token'
  );
  assert.equal(tokenRequests.length, 1);
  assert.equal(tokenRequests[0].method, 'POST');

  const controlPlaneRequests = httpClient.requests.filter(
    ({ url }) => url === 'https://run.example/api/namespaces'
  );
  assert.equal(controlPlaneRequests.length, 2);
  for (const request of controlPlaneRequests) {
    assert.equal(
      request.headers.find(({ name }) => name.toLowerCase() === 'authorization')?.value,
      'Bearer token-a'
    );
  }
});
