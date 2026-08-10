/** Run a Codex SDK desktop task through Cua Driver's MCP server. */

import { execFile } from 'node:child_process';
import process from 'node:process';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';

import { Codex } from '@openai/codex-sdk';

type CaptureScope = 'auto' | 'window' | 'desktop';

const execFileAsync = promisify(execFile);

function taskFromArgs(): string {
  const task = process.argv.slice(2).join(' ').trim();
  if (!task) {
    throw new Error('usage: npm run codex -- "<trusted desktop task>"');
  }
  return task;
}

function captureScope(): CaptureScope {
  const value = process.env.CUA_CAPTURE_SCOPE ?? 'auto';
  if (value !== 'auto' && value !== 'window' && value !== 'desktop') {
    throw new Error('CUA_CAPTURE_SCOPE must be auto, window, or desktop');
  }
  return value;
}

async function driverCall(
  binary: string,
  tool: string,
  args: Record<string, string>
): Promise<void> {
  await execFileAsync(binary, ['call', tool, JSON.stringify(args)], {
    timeout: 30_000,
  });
}

function agentPrompt(task: string, session: string, scope: CaptureScope): string {
  return `Complete this desktop task through the cua_driver MCP server:

${task}

The host has already started session ${JSON.stringify(session)} with capture
scope ${JSON.stringify(scope)}. Use only cua_driver MCP tools for desktop
observation and interaction. Do not substitute shell, filesystem, web, or code
execution. Pass session ${JSON.stringify(session)} to every Cua tool that
accepts a session. Inspect state before each action and verify state after it.
Do not call start_session or end_session; the host owns lifecycle cleanup. Do
not blindly retry a mutation after a timeout or disconnect; observe first. Do
not perform purchases, send messages, delete data, expose credentials, or take
another irreversible action unless the task explicitly requests that exact
action. Return a concise result and mention any step you could not verify.`;
}

async function main(): Promise<void> {
  const task = taskFromArgs();
  const binary = process.env.CUA_DRIVER_BIN ?? 'cua-driver';
  const scope = captureScope();
  const session = `codex-agent-${randomUUID().slice(0, 12)}`;
  let started = false;

  try {
    await driverCall(binary, 'start_session', {
      session,
      capture_scope: scope,
    });
    started = true;

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

    const result = await thread.run(agentPrompt(task, session, scope));
    console.log(result.finalResponse);
  } finally {
    if (started) {
      await driverCall(binary, 'end_session', { session });
    }
  }
}

await main();
