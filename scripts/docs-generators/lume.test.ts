import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join, sep } from 'node:path';
import test from 'node:test';
import { renderAll, type CLIDocumentation } from './lume';

const cli: CLIDocumentation = JSON.parse(readFileSync(join(__dirname, 'cli-specs', 'lume.json'), 'utf-8'));
const api = {
  base_path: '/lume',
  version: cli.version,
  description: 'Lume API',
  endpoints: [
    {
      method: 'GET',
      path: '/lume/vms/:name',
      description: 'Get VM {name}',
      category: 'VMs',
      path_parameters: [{ name: 'name', type: 'string', required: true, description: 'VM | name' }],
      query_parameters: [],
      response_body: { content_type: 'application/json', description: 'The VM' },
      status_codes: [{ code: 200, description: 'OK' }],
    },
  ],
};

test('lume: CLI groups, HTTP API and MCP render from one dump', () => {
  const files = renderAll({
    cli,
    api,
    mcp: {
      tools: [
        { name: 'lume_list_vms', description: 'List VMs.', input_schema: { type: 'object', properties: {} }, annotations: { read_only: true, idempotent: true } },
        { name: 'check_for_update', description: 'Check.', input_schema: { type: 'object', properties: {} } },
      ],
    },
  });
  const byName = (end: string) => [...files].find(([p]) => p.endsWith(end))![1];
  // The whole CLI is one page, a section per group.
  const cliPage = byName(join('reference', 'cli.mdx'));
  assert.ok(cliPage.includes('## VMs') && cliPage.includes('### `lume create`'));
  assert.ok(![...files.keys()].some((p) => p.includes(join('reference', 'cli') + sep)), 'no CLI folder');
  // The HTTP API, then the MCP tools, on one page.
  const page = byName('http-api.mdx');
  const [http, mcp] = page.split('\n## MCP tools\n');
  assert.ok(http.includes('Get VM &#123;name&#125;') && http.includes('VM \\| name'));
  assert.ok(!/^#### /m.test(http), 'endpoint sub-sections must not repeat headings');
  assert.ok(mcp.includes('### VM tools') && mcp.includes('#### `lume_list_vms`') && mcp.includes('### Maintenance tools'));
  assert.deepEqual(JSON.parse(byName(join('reference', 'meta.json'))).pages, ['index', 'cli', 'http-api']);
});
