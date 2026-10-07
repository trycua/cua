import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import test from 'node:test';
import {
  DESCRIPTIONS_FILE,
  OBJECTS,
  SOURCES,
  applyOverrides,
  daemonMapping,
  messageTemplate,
  neutralType,
  notFoundVariants,
  parseClientOps,
  parseHandle,
  parseRoutes,
  parseTfDocs,
  parseTfSchema,
} from './fleet';
import { enumVariants, firstSentence, structFields } from './lib/rust-source';
import { cleanDescription, fieldTable, flattenSchema, typeOf, type Schema } from './lib/schema-fields';

const read = (p: string) => fs.readFileSync(p, 'utf-8');

test('routes evaluate to path templates', () => {
  const routes = parseRoutes(`
const PREFIX: &str = "api/k8s/apis/x/v1/namespaces/";
const KEYS: &str = "api/user-keys";
pub fn pool_item(base: &Url, namespace: &str, name: &str) -> Result<Url, SdkError> {
    route(base, format!("{PREFIX}{namespace}/pools/{name}"))
}
pub fn key_collection(base: &Url) -> Result<Url, SdkError> {
    route(base, KEYS.into())
}
pub fn key_item(base: &Url, id: &str) -> Result<Url, SdkError> {
    let mut url = key_collection(base)?;
    url.path_segments_mut().unwrap().push(id);
    Ok(url)
}
`);
  assert.equal(routes.get('pool_item'), '/api/k8s/apis/x/v1/namespaces/{namespace}/pools/{name}');
  assert.equal(routes.get('key_item'), '/api/user-keys/{id}');
});

test('every REST operation the objects name resolves in the Fleet client', () => {
  const routes = parseRoutes(read(path.join(SOURCES.sdkDir, 'routes.rs')));
  const ops = parseClientOps(SOURCES.sdkDir, routes);
  const pool = ops.get('create_pool');
  assert.equal(pool?.method, 'POST');
  // The request goes to the collection even though the item route is validated first.
  assert.match(pool!.path, /\/osgymsandboxwarmpools$/);
  assert.deepEqual(pool!.calls.sort(), ['create_namespace_if_missing', 'delete_namespace']);
  assert.equal(ops.get('renew_claim')?.method, 'PATCH');
  assert.deepEqual(ops.get('delete_claim')?.statuses, [200, 202, 204, 404]);
  for (const def of OBJECTS)
    for (const ref of def.rest)
      if ('client' in ref) assert.ok(ops.has(ref.client), `${def.slug}: ${ref.client}`);
});

test('every SDK method the objects name exists on the UniFFI handle', () => {
  const handle = parseHandle(read(SOURCES.handle), new Map());
  for (const def of OBJECTS) for (const key of def.sdk) assert.ok(handle.has(key), `${def.slug}: ${key}`);
  assert.deepEqual(handle.get('Fleet.release')?.params, ['namespace', 'name']);
});

test('the Terraform schema parses from the generated provider source', () => {
  const dir = path.join(SOURCES.tfDir, 'internal', 'provider');
  const tf = parseTfSchema(read(path.join(dir, 'pool_generated.go')), 'poolResourceSchema');
  const name = tf.attrs.find((a) => a.name === 'name')!;
  assert.ok(name.required && name.requiresReplace);
  assert.deepEqual(name.length, [1, 63]);
  assert.ok(name.pattern?.startsWith('^[a-z0-9]'));
  assert.deepEqual(tf.attrs.find((a) => a.name === 'runtime')?.oneOf, ['gvisor', 'kubevirt', 'macos']);
  assert.deepEqual(
    tf.blocks.map((b) => [b.name, b.nesting]),
    [
      ['service', 'set'],
      ['autoscaling', 'single'],
    ]
  );
  const docs = parseTfDocs(read(path.join(SOURCES.tfDir, 'docs', 'resources', 'pool.md')));
  assert.ok(docs.args.get('liveness_probe_json'));
  assert.match(docs.importCode ?? '', /terraform import fleets_pool\./);
});

test('errors: variants, messages and their SDK cases', () => {
  const variants = enumVariants(
    `pub enum E {
    /// Bad input.
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error(transparent)]
    Sdk(#[from] X),
    /// Timed out.
    #[error("sandbox {sandbox} has no {service:?}")]
    Missing {
        sandbox: String,
        service: String,
    },
}`,
    'E'
  );
  assert.deepEqual(
    variants.map((v) => [v.name, v.message]),
    [
      ['Invalid', 'invalid argument: {0}'],
      ['Sdk', 'transparent'],
      ['Missing', 'sandbox {sandbox} has no {service:?}'],
    ]
  );
  assert.equal(messageTemplate('sandbox {sandbox} has no {service:?}: {}'), 'sandbox <sandbox> has no <service>: <detail>');
  const daemon = daemonMapping(read(SOURCES.daemonLib));
  assert.equal(daemon.map.get('MissingCredentials'), 'ProviderNotConfigured');
  assert.ok(daemon.guest.includes('SpacesdNotAvailable'));
  assert.deepEqual([...notFoundVariants(read(SOURCES.fleetLib))].sort(), ['Sdk', 'UnknownService']);
});

test('schema fields flatten with types, defaults and constraints', () => {
  const schema: Schema = {
    type: 'object',
    required: ['spec'],
    properties: {
      spec: {
        type: 'object',
        required: ['replicas'],
        properties: {
          replicas: { type: 'integer', minimum: 0, description: 'Warm\nsandboxes.' },
          runtime: { type: 'string', enum: ['kubevirt', 'gvisor'], default: 'kubevirt' },
          services: { type: 'array', items: { type: 'object', properties: { port: { type: 'integer' } } } },
        },
      },
    },
  };
  const fields = flattenSchema(schema);
  assert.deepEqual(
    fields.map((f) => [f.path, f.type, f.required]),
    [
      ['spec', 'object', true],
      ['spec.replicas', 'integer', true],
      ['spec.runtime', '"kubevirt" | "gvisor"', false],
      ['spec.services', 'object[]', false],
      ['spec.services[].port', 'integer', false],
    ]
  );
  const table = fieldTable(fields).join('\n');
  assert.match(table, /\| `spec.replicas` \| `integer` \| required \| Warm sandboxes\. At least 0\. \|/);
  assert.match(table, /\| `spec.runtime` \| `"kubevirt" \\\| "gvisor"` \| `"kubevirt"` \|/);
  assert.equal(typeOf({ type: 'object', additionalProperties: { type: 'string' } }), 'map of string');
  assert.equal(
    cleanDescription('Grows the pool.\nSee docs/decisions/2026-06-15-x.md. Done (trycua/cloud#7887).'),
    'Grows the pool. Done.'
  );
});

test('description overrides must name existing fields', () => {
  const fields = [{ path: 'spec.a', description: 'internal' }];
  assert.equal(applyOverrides(fields, { 'spec.a': 'public' }, 'X')[0].description, 'public');
  assert.throws(() => applyOverrides(fields, { 'spec.b': 'x' }, 'X'), /has no field spec\.b/);
  const file = JSON.parse(read(DESCRIPTIONS_FILE));
  assert.ok(file.crd.OSGymSandboxClaim['spec.lifecycle.autoRenew']);
});

test('small helpers', () => {
  assert.equal(firstSentence('Deletes pools idle (default 0, i.e. now). More.'), 'Deletes pools idle (default 0, i.e. now).');
  assert.equal(neutralType('Vec<FleetPool>'), '`FleetPool[]`');
  assert.equal(neutralType('()'), 'none');
  assert.deepEqual(
    structFields(
      `#[serde(rename_all = "camelCase")]
pub struct R {
    pub size_bytes: u64,
    pub name: String,
}`,
      'R'
    ).map((f) => f.name),
    ['sizeBytes', 'name']
  );
});
