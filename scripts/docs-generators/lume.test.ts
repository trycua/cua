import assert from 'node:assert/strict';
import test from 'node:test';

import { generateCommandDoc, type CommandDoc } from './lume';

test('escapes MDX expressions and table delimiters in generated CLI tables', () => {
  const command: CommandDoc = {
    name: 'run',
    abstract: 'Run a virtual machine',
    arguments: [],
    options: [
      {
        name: 'log-file',
        help: 'Log path for {vm} | detached runs',
        type: 'String',
        default_value: '~/Library/Logs/lume/{vm}.log',
        is_optional: true,
      },
    ],
    flags: [],
    subcommands: [],
  };

  const mdx = generateCommandDoc(command, '###').join('\n');

  assert.match(mdx, /~\/Library\/Logs\/lume\/&#123;vm&#125;\.log/);
  assert.match(mdx, /Log path for &#123;vm&#125; \\| detached runs/);
  assert.doesNotMatch(mdx, /\{vm\}/);
});
