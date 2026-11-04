import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { type CbDocs, resultsPage, rst } from './cua-bench';
import type { CLIDocumentation } from './lib/cli-mdx';

const cli: CLIDocumentation = JSON.parse(readFileSync(join(__dirname, 'cli-specs', 'cb.json'), 'utf-8'));
const cls = (name: string, fields: Array<[string, string]>) => ({
  name,
  doc: `The ${name}.`,
  fields: fields.map(([n, d]) => ({ name: n, type: 'int', default: null, description: d })),
});

function docs(summaryKeys: Array<[string, string]>): CbDocs {
  return {
    cli,
    task: {} as CbDocs['task'],
    results: {
      job_result: cls('JobResult', [['task', 'Task ``name``.']]),
      harbor: cls('HarborTrialFields', [['id', 'Trial id.']]),
      span: cls('Span', [['started_at', 's'], ['finished_at', 'f']]),
      summary: cls('Summary', summaryKeys),
      summary_result: cls('SummaryResult', [['task', '']]),
      summary_stats: cls('SummaryStats', [['n_completed', 'Completed.']]),
      summary_target: cls('SummaryTarget', [['count', 'Count.']]),
      golden_result: { schema_version: 'int', task: 'str', id: 'str' },
      golden_summary: { schema_version: 'int', results: [{ task: 'str' }], stats: { n_completed: 'int' }, targets: [{ count: 'int' }] },
    },
  };
}

const summary: Array<[string, string]> = [
  ['schema_version', 'Layout version.'],
  ['results', 'Rows.'],
  ['stats', 'Stats.'],
  ['targets', 'Targets.'],
];

test('results page documents every golden key, with #: descriptions', () => {
  const page = resultsPage(docs(summary), '1.0.0');
  assert.ok(page.includes('| `task` | `int` | Task `name`. |'));
  // A summary row without its own description falls back to the JobResult field.
  assert.ok(page.includes('| `results[].task` | `int` | Task `name`. |'));
});

test('a key missing from the docs or the golden fails generation', () => {
  assert.throws(() => resultsPage(docs(summary.slice(0, 3)), '1.0.0'), /Undocumented: targets/);
  assert.throws(() => resultsPage(docs([...summary, ['extra', 'x']]), '1.0.0'), /not in the golden: extra/);
});

test('reST markup becomes Markdown', () => {
  assert.equal(rst('See :func:`pass_at_k` and ``x``\n  more'), 'See `pass_at_k` and `x` more');
});
