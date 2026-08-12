/** Run a Codex SDK desktop task through Cua Driver's MCP server. */

import process from 'node:process';

import { Codex } from '@openai/codex-sdk';

function taskFromArgs(): string {
  const task = process.argv.slice(2).join(' ').trim();
  if (!task) {
    throw new Error('usage: npm run codex -- "<trusted desktop task>"');
  }
  return task;
}

function agentPrompt(task: string): string {
  return `Complete this desktop task through the cua_driver MCP server:

${task}

This MCP transport owns one implicit lifecycle session. Omit the session field
for ordinary calls so the runtime creates and reuses it. Select an exact window
or desktop target on every action; session identity never selects capture
modality or permission authority. Use only cua_driver MCP tools for desktop
observation and interaction. Do not substitute shell, filesystem, web, or code
execution. Inspect state before each action and verify state after it. Do not
blindly retry a mutation after a timeout or disconnect; observe first. Do not
perform purchases, send messages, delete data, expose credentials, or take
another irreversible action unless the task explicitly requests that exact
action. Return a concise result and mention any step you could not verify.`;
}

async function main(): Promise<void> {
  const task = taskFromArgs();
  const binary = process.env.CUA_DRIVER_BIN ?? 'cua-driver';
  const codex = new Codex({
    config: {
      mcp_servers: {
        cua_driver: {
          command: binary,
          args: ['mcp'],
          startup_timeout_sec: 30,
          required: true,
        },
      },
    },
  });
  const model = process.env.CODEX_MODEL;
  const thread = codex.startThread({
    ...(model ? { model } : {}),
    sandboxMode: 'read-only',
    approvalPolicy: 'never',
    skipGitRepoCheck: true,
  });

  const result = await thread.run(agentPrompt(task));
  console.log(result.finalResponse);
}

await main();
