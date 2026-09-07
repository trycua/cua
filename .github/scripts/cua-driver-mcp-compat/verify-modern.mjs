import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { mkdtemp, mkdir } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Keep the real v2 SDK isolated from the existing pinned legacy-client suite.
const HERE = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(HERE, 'modern-client', 'package.json'));
const { Client } = require('@modelcontextprotocol/client');
const { StdioClientTransport } = require('@modelcontextprotocol/client/stdio');
const { ResultSchema } = require('@modelcontextprotocol/core');
const Ajv2020 = require('ajv/dist/2020.js');
const { parse: parseYaml } = require('yaml');
const VERSION = '2026-07-28';
const VERSION_KEY = 'io.modelcontextprotocol/protocolVersion';
const CAPABILITIES_KEY = 'io.modelcontextprotocol/clientCapabilities';
const SCHEMA = JSON.parse(readFileSync(join(HERE, 'modern-client/schema-2026-07-28.json')));
const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
ajv.addSchema(SCHEMA, 'mcp');
const DEFINITIONS = {
  'server/discover': 'DiscoverResult',
  'tools/list': 'ListToolsResult',
  'tools/call': 'CallToolResult',
  'resources/list': 'ListResourcesResult',
  'resources/read': 'ReadResourceResult',
  'skills/list': 'Result',
  'skills/get': 'Result',
};
const validators = Object.fromEntries(
  Object.entries(DEFINITIONS).map(([method, name]) => [
    method,
    ajv.compile({ $ref: `mcp#/$defs/${name}` }),
  ])
);
const expectedFiles = [
  'BROWSER.md',
  'EMBEDDING.md',
  'LINUX.md',
  'MACOS.md',
  'README.md',
  'RECORDING.md',
  'SKILL.md',
  'WINDOWS.md',
];
const sourcePack = resolve(HERE, '../../../libs/cua-driver/rust/Skills/cua-driver');
const driver = process.env.CUA_DRIVER_BINARY && resolve(process.env.CUA_DRIVER_BINARY);
assert(driver, 'CUA_DRIVER_BINARY must name the source-built candidate');
const args = process.env.CUA_DRIVER_MCP_ARGS
  ? JSON.parse(process.env.CUA_DRIVER_MCP_ARGS)
  : ['mcp', '--direct'];
assert(Array.isArray(args) && args.every((arg) => typeof arg === 'string'));
// An externally managed proxy must name its isolated socket explicitly.
assert(
  args.includes('--direct') || args.includes('--socket'),
  'use --direct or an explicit isolated --socket'
);
const state = await mkdtemp(join(tmpdir(), 'cua-modern-mcp-'));
const env = {};
for (const name of [
  'PATH',
  'SystemRoot',
  'WINDIR',
  'COMSPEC',
  'PATHEXT',
  'TMPDIR',
  'TEMP',
  'TMP',
]) {
  if (process.env[name]) env[name] = process.env[name];
}
Object.assign(env, {
  HOME: state,
  USERPROFILE: state,
  APPDATA: join(state, 'config'),
  LOCALAPPDATA: join(state, 'local'),
  XDG_CONFIG_HOME: join(state, 'config'),
  XDG_DATA_HOME: join(state, 'data'),
  XDG_STATE_HOME: join(state, 'state'),
  XDG_CACHE_HOME: join(state, 'cache'),
  CUA_DRIVER_HOME: join(state, 'driver'),
  CUA_DRIVER_RS_HOME: join(state, 'driver'),
  CUA_DRIVER_PERMISSION_MODE: 'standard',
  CUA_DRIVER_RS_TELEMETRY_ENABLED: 'false',
  NO_COLOR: '1',
});
for (const subdir of ['config', 'local', 'data', 'state', 'cache', 'driver']) {
  await mkdir(join(state, subdir));
}

const requests = new Map();
const methods = [];
const wireErrors = [];
let omitCapabilities = false;
let responseCount = 0;
let sentRequestCount = 0;
let errorResponseCount = 0;
const observedMessages = new WeakSet();
// Subclassing also keeps negotiation on this observed transport, without a
// separate unobserved disposable probe process in SDK auto-negotiation.
class ObservedTransport extends StdioClientTransport {
  constructor(options) {
    super(options);
    let handler;
    // Negotiation replaces onmessage after discovery. Wrap every assignment;
    // an SDK handler may chain its predecessor, so validate each message once.
    Object.defineProperty(this, 'onmessage', {
      configurable: true,
      get: () => handler,
      set: (deliver) => {
        handler =
          deliver &&
          ((message, ...rest) => {
            this.observe(message);
            deliver(message, ...rest);
          });
      },
    });
  }
  observe(message) {
    if (observedMessages.has(message)) return;
    observedMessages.add(message);
    try {
      if (message.error) errorResponseCount += 1;
      if (message.result) {
        const method = requests.get(message.id);
        const validate = validators[method];
        assert(validate, `unexpected result for ${method}`);
        assert(validate(message.result), `${method}: ${ajv.errorsText(validate.errors)}`);
        assert.equal(message.result.resultType, 'complete');
        assert.equal(
          message.result._meta?.['io.modelcontextprotocol/serverInfo']?.name,
          'cua-driver'
        );
        if (method === 'server/discover' || method.endsWith('/list') || method.endsWith('/read')) {
          assert.equal(message.result.ttlMs, 0);
          assert.equal(message.result.cacheScope, 'private');
        }
        responseCount += 1;
      }
    } catch (error) {
      wireErrors.push(error);
    }
  }
  async send(message, options) {
    if (message.method) {
      methods.push(message.method);
      assert.notEqual(
        message.method,
        'initialize',
        'modern probe must never fall back to initialize'
      );
      assert.notEqual(message.method, 'notifications/initialized');
      if (message.id !== undefined) {
        sentRequestCount += 1;
        requests.set(message.id, message.method);
        assert.equal(message.params?._meta?.[VERSION_KEY], VERSION);
        assert.equal(typeof message.params?._meta?.[CAPABILITIES_KEY], 'object');
        if (omitCapabilities) {
          message = structuredClone(message);
          delete message.params._meta[CAPABILITIES_KEY];
        }
      }
    }
    return super.send(message, options);
  }
}

const transport = new ObservedTransport({ command: driver, args, env, cwd: state, stderr: 'pipe' });
let stderr = '';
transport.stderr.on('data', (chunk) => {
  stderr = `${stderr}${chunk}`.slice(-8192);
});
const client = new Client(
  { name: 'cua-modern-mcp-compat', version: '1.0.0' },
  {
    capabilities: { extensions: { 'io.modelcontextprotocol/skills': {} } },
    versionNegotiation: { mode: { pin: VERSION }, probe: { timeoutMs: 15000, maxRetries: 0 } },
  }
);
const timeout = setTimeout(() => {
  console.error('modern MCP probe exceeded 90 seconds');
  void client.close().finally(() => process.exit(1));
}, 90000);
try {
  await client.connect(transport);
  assert.equal(client.getProtocolEra(), 'modern');
  assert.equal(client.getNegotiatedProtocolVersion(), VERSION);
  assert.equal(methods[0], 'server/discover');
  const discovery = client.getDiscoverResult();
  assert(discovery.supportedVersions.includes(VERSION));
  assert.deepEqual(discovery.capabilities.extensions['io.modelcontextprotocol/skills'], {});
  const { tools } = await client.listTools();
  assert(tools.some((tool) => tool.name === 'get_config'));
  const config = await client.callTool({ name: 'get_config', arguments: {} });
  assert.notEqual(config.isError, true, 'read-only get_config failed');
  assert(config.content.length > 0);

  const catalog = await client.request({ method: 'skills/list', params: {} }, ResultSchema);
  assert.equal(catalog.skills.length, 1);
  assert.equal(catalog.nextCursor, undefined);
  const skill = catalog.skills[0];
  assert.equal(skill.uri, 'skill://cua-driver/SKILL.md');
  const fetched = await client.request(
    { method: 'skills/get', params: { uri: skill.uri } },
    ResultSchema
  );
  assert.deepEqual(fetched.skill, skill);
  const resources = await client.listResources();
  assert.deepEqual(
    resources.resources.map((item) => item.uri).sort(),
    expectedFiles.map((name) => `skill://cua-driver/${name}`)
  );
  assert.deepEqual(
    skill.resources.map((item) => item.uri).sort(),
    expectedFiles.map((name) => `skill://cua-driver/${name}`)
  );
  let bytes = 0;
  for (const name of expectedFiles) {
    const uri = `skill://cua-driver/${name}`;
    const manifest = skill.resources.find((item) => item.uri === uri);
    const result = await client.readResource({ uri });
    assert.equal(result.contents.length, 1);
    const content = result.contents[0];
    assert.equal(content.uri, uri);
    assert.equal(content.mimeType, 'text/markdown');
    const data = Buffer.from(content.text, 'utf8');
    assert.deepEqual(
      data,
      readFileSync(join(sourcePack, name)),
      `${name}: embedded source mismatch`
    );
    assert.equal(manifest.size, data.length);
    assert.equal(manifest.digest, `sha256:${createHash('sha256').update(data).digest('hex')}`);
    bytes += data.length;
    if (name === 'SKILL.md') {
      const frontmatter = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(content.text);
      assert(frontmatter, 'missing skill frontmatter');
      assert.deepEqual(skill.frontmatter, parseYaml(frontmatter[1]));
    }
  }
  await assert.rejects(
    client.readResource({ uri: 'skill://cua-driver/UNKNOWN.md' }),
    (error) => error.code === -32602
  );
  omitCapabilities = true;
  await assert.rejects(
    client.request({ method: 'skills/list', params: {} }, ResultSchema),
    (error) => error.code === -32602
  );
  omitCapabilities = false;
  // A valid request after rejection proves capabilities are per-request.
  await client.request({ method: 'skills/list', params: {} }, ResultSchema);
  assert.deepEqual(wireErrors, [], wireErrors.map((error) => error.message).join('\n'));
  assert(responseCount >= 15, 'every successful endpoint response must be schema-validated');
  assert.equal(errorResponseCount, 2, 'both expected protocol errors must be observed');
  assert.equal(
    responseCount + errorResponseCount,
    sentRequestCount,
    'wire validation missed a response'
  );
  console.log(
    JSON.stringify(
      {
        sdk: '@modelcontextprotocol/client@2.0.0',
        protocol: VERSION,
        runtimeArgs: args,
        tools: tools.length,
        skills: 1,
        resources: expectedFiles.length,
        bytes,
        schemaValidatedResponses: responseCount,
        initialized: false,
        nativeSkillActivation: 'not tested',
      },
      null,
      2
    )
  );
} catch (error) {
  if (stderr) console.error(stderr);
  throw error;
} finally {
  clearTimeout(timeout);
  await client.close();
}
