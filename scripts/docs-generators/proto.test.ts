import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import test from 'node:test';

import {
  type FileDescriptorSet,
  type PageSpec,
  PAGES,
  commentMarkdown,
  generateProtoReference,
  parseContractFacts,
} from './proto';

// A two-file cua.demo.v1 contract in the shape `buf build -#format=json` emits.
const FIXTURE: FileDescriptorSet = {
  file: [
    {
      name: 'google/protobuf/duration.proto',
      package: 'google.protobuf',
      messageType: [{ name: 'Duration' }],
    },
    {
      name: 'cua/demo/v1/common.proto',
      package: 'cua.demo.v1',
      enumType: [
        {
          name: 'Color',
          value: [
            { name: 'COLOR_UNSPECIFIED', number: 0 },
            { name: 'COLOR_RED', number: 1, options: { deprecated: true } },
          ],
        },
      ],
      sourceCodeInfo: {
        location: [
          { path: [5, 0], leadingComments: ' A color <with> {braces}.\n' },
          { path: [5, 0, 2, 1], leadingComments: ' Red.\n' },
        ],
      },
    },
    {
      name: 'cua/demo/v1/demo.proto',
      package: 'cua.demo.v1',
      service: [
        {
          name: 'DemoService',
          method: [
            {
              name: 'Upload',
              inputType: '.cua.demo.v1.Chunk',
              outputType: '.cua.demo.v1.Chunk',
              clientStreaming: true,
            },
            {
              name: 'UploadOne',
              inputType: '.cua.demo.v1.Chunk',
              outputType: '.cua.demo.v1.Chunk',
            },
          ],
        },
      ],
      messageType: [
        {
          name: 'Chunk',
          field: [
            { name: 'data', number: 1, label: 'LABEL_OPTIONAL', type: 'TYPE_BYTES' },
            { name: 'tags', number: 2, label: 'LABEL_REPEATED', type: 'TYPE_STRING' },
            {
              name: 'color',
              number: 3,
              label: 'LABEL_OPTIONAL',
              type: 'TYPE_ENUM',
              typeName: '.cua.demo.v1.Color',
            },
            {
              name: 'labels',
              number: 4,
              label: 'LABEL_REPEATED',
              type: 'TYPE_MESSAGE',
              typeName: '.cua.demo.v1.Chunk.LabelsEntry',
            },
            {
              name: 'ttl',
              number: 5,
              label: 'LABEL_OPTIONAL',
              type: 'TYPE_MESSAGE',
              typeName: '.google.protobuf.Duration',
            },
            {
              name: 'size',
              number: 6,
              label: 'LABEL_OPTIONAL',
              type: 'TYPE_UINT64',
              oneofIndex: 1,
              proto3Optional: true,
            },
            {
              name: 'path',
              number: 7,
              label: 'LABEL_OPTIONAL',
              type: 'TYPE_STRING',
              oneofIndex: 0,
            },
          ],
          oneofDecl: [{ name: 'target' }, { name: '_size' }],
          nestedType: [
            {
              name: 'LabelsEntry',
              field: [
                { name: 'key', number: 1, type: 'TYPE_STRING' },
                { name: 'value', number: 2, type: 'TYPE_STRING' },
              ],
              options: { mapEntry: true },
            },
          ],
        },
      ],
      sourceCodeInfo: {
        location: [
          { path: [6, 0], leadingComments: ' Demo service.\n\n # Not a heading.\n' },
          { path: [6, 0, 2, 0], leadingComments: ' Streams chunks.\n' },
          { path: [4, 0], leadingComments: ' One chunk.\n' },
          { path: [4, 0, 2, 0], leadingComments: ' Bytes,\n split | piped.\n' },
        ],
      },
    },
  ],
};

const FACTS_RS = `
pub mod metadata {
    /// Bearer token header.
    pub const AUTHORIZATION: &str = "authorization";
    /// Health route.
    pub const HEALTH_PATH: &str = "/health";
}
pub const ENV_PROTOCOL_VERSION: u32 = 1;
pub const ENV_PROTOCOL_REVISION: u32 = 7;
pub const CLIENT_STREAM_FALLBACKS: &[(&str, &str)] = &[
    (
        "/cua.demo.v1.DemoService/Upload",
        "/cua.demo.v1.DemoService/UploadOne",
    ),
];
`;

const DEMO_PAGES: PageSpec[] = [
  {
    slug: 'demo',
    title: 'Demo',
    description: 'Demo.',
    intro: 'Intro.',
    files: ['cua/demo/v1/demo.proto'],
  },
  {
    slug: 'common',
    title: 'Common',
    description: 'Common.',
    intro: 'Shared.',
    files: ['cua/demo/v1/common.proto'],
  },
];

function render() {
  const facts = parseContractFacts(FACTS_RS);
  const files = generateProtoReference(FIXTURE, facts, '9.9.9', DEMO_PAGES);
  const byName = new Map([...files].map(([file, content]) => [path.basename(file), content]));
  return { facts, byName };
}

test('parses fallbacks, metadata and the protocol revision from lib.rs', () => {
  const { facts } = render();
  assert.deepEqual(facts.fallbacks, [
    ['/cua.demo.v1.DemoService/Upload', '/cua.demo.v1.DemoService/UploadOne'],
  ]);
  assert.deepEqual(
    facts.metadata.map((m) => [m.name, m.value]),
    [
      ['AUTHORIZATION', 'authorization'],
      ['HEALTH_PATH', '/health'],
    ]
  );
  assert.equal(facts.protocolRevision, '7');
});

test('renders services, fields, enums, links and fallbacks', () => {
  const { byName } = render();
  const demo = byName.get('demo.mdx')!;
  assert.match(demo, /AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY/);
  assert.match(
    demo,
    /\| \[`Upload`\]\(#demoserviceupload\) \| \[`Chunk`\]\(#chunk\) \| \[`Chunk`\]\(#chunk\) \| client stream \|/
  );
  assert.match(demo, /over gRPC-Web, call the unary \[`UploadOne`\]\(#demoserviceuploadone\)/i);
  assert.match(demo, /import \{ Callout \} from 'fumadocs-ui\/components\/callout';/);
  assert.match(
    demo,
    /\| `color` \| 3 \| \[`Color`\]\(\/cua-sdk\/reference\/protocol\/common#color\) \|/
  );
  assert.match(demo, /\| `labels` \| 4 \| map of `string` to `string` \|/);
  assert.match(demo, /\| `ttl` \| 5 \| `google.protobuf.Duration` \|/);
  assert.match(demo, /\| `size` \| 6 \| optional `uint64` \|/);
  assert.match(demo, /\| `path` \| 7 \| `string` \(oneof `target`\) \|/);
  assert.match(demo, /\| `data` \| 1 \| `bytes` \| Bytes, split \\\| piped\. \|/);
  assert.doesNotMatch(demo, /### Chunk\.LabelsEntry/);
  assert.match(demo, /^\\# Not a heading\.$/m);
  const common = byName.get('common.mdx')!;
  assert.match(common, /A color &lt;with&gt; &#123;braces&#125;\./);
  assert.match(common, /\| `COLOR_RED` \| 1 \| Deprecated\. Red\. \|/);
  const index = byName.get('index.mdx')!;
  assert.match(index, /revision 7/);
  assert.match(index, /\| \[`DemoService`\]\(\/cua-sdk\/reference\/protocol\/demo#demoservice\)/);
  assert.equal(
    byName.get('meta.json'),
    JSON.stringify({ title: 'Protocol', pages: ['demo', 'common'] }, null, 2) + '\n'
  );
});

test('rendering is deterministic', () => {
  assert.deepEqual([...render().byName], [...render().byName]);
});

test('a contract file without a page fails generation', () => {
  const facts = parseContractFacts(FACTS_RS);
  assert.throws(
    () => generateProtoReference(FIXTURE, facts, '9.9.9', DEMO_PAGES.slice(0, 1)),
    /cua\/demo\/v1\/common\.proto is not assigned to a reference page/
  );
});

test('comment markdown strips the proto comment indent', () => {
  assert.equal(commentMarkdown(' a\n  b\n'), 'a\n b');
});

test('every contract proto in libs/cua/proto has a page', () => {
  const root = path.resolve(__dirname, '../../libs/cua/proto');
  const found: string[] = [];
  const walk = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.endsWith('.proto'))
        found.push(path.relative(root, full).split(path.sep).join('/'));
    }
  };
  walk(path.join(root, 'cua'));
  const assigned = PAGES.flatMap((p) => p.files);
  assert.deepEqual(found.sort(), [...assigned].sort());
});
