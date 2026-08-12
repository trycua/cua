/** Run a Claude Agent SDK desktop task through native Cua tools or MCP. */

import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { promisify } from 'node:util';

import {
  createSdkMcpServer,
  query,
  tool,
  type Options,
  type SDKResultMessage,
} from '@anthropic-ai/claude-agent-sdk';
import { z } from 'zod';

import { NativeDesktopTools } from './native-tools.js';

type Route = 'native' | 'mcp';
type CaptureScope = 'auto' | 'window' | 'desktop';

const execFileAsync = promisify(execFile);

function argumentsFromCli(): { route: Route; task: string } {
  const args = process.argv.slice(2);
  const routeIndex = args.indexOf('--route');
  let route: Route = 'mcp';
  if (routeIndex >= 0) {
    const value = args[routeIndex + 1];
    if (value !== 'native' && value !== 'mcp') {
      throw new Error('--route must be native or mcp');
    }
    route = value;
    args.splice(routeIndex, 2);
  }
  const task = args.join(' ').trim();
  if (!task) throw new Error('usage: npm run claude -- [--route native|mcp] "<task>"');
  return { route, task };
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
  name: string,
  args: Record<string, string>
): Promise<void> {
  await execFileAsync(binary, ['call', name, JSON.stringify(args)], { timeout: 30_000 });
}

async function runAgent(options: Options, prompt: string): Promise<void> {
  let result: SDKResultMessage | undefined;
  for await (const message of query({ prompt, options })) {
    if (message.type === 'result') result = message;
  }
  if (!result) throw new Error('Claude Agent SDK returned no final result');
  if (result.subtype !== 'success') {
    throw new Error(result.errors.join(', ') || result.subtype);
  }
  console.log(result.result);
}

function nativeServer(runtime: NativeDesktopTools) {
  return createSdkMcpServer({
    name: 'cua_native',
    version: '1.0.0',
    tools: [
      tool(
        'observe_desktop',
        'Capture the whole desktop. Use before every action and to verify its result.',
        {},
        async () => await runtime.observe()
      ),
      tool(
        'click_desktop',
        'Click an absolute desktop coordinate grounded in the latest screenshot.',
        { x: z.number(), y: z.number() },
        async ({ x, y }) => await runtime.click(x, y)
      ),
      tool(
        'type_text',
        'Type text into the focused desktop control, then re-observe.',
        { text: z.string() },
        async ({ text }) => await runtime.typeText(text)
      ),
      tool(
        'press_key',
        'Press one named key in the desktop session, then re-observe.',
        { key: z.string() },
        async ({ key }) => await runtime.pressKey(key)
      ),
    ],
  });
}

function taskPrompt(task: string, route: Route, session?: string): string {
  const lifecycle = session
    ? `The host already started session ${JSON.stringify(session)}. Pass it to every Cua tool that accepts a session; do not call start_session or end_session.`
    : 'The host owns the native driver session; custom tools do not expose lifecycle.';
  return `Complete this trusted desktop task through Cua Driver:

${task}

Route: ${route}. ${lifecycle}
Use only supplied Cua tools for desktop observation and interaction. Inspect
before each action and verify afterward. If a mutation times out, observe before
any retry; never blindly replay an action with an unknown outcome. Do not
purchase, send, delete, expose credentials, or perform another irreversible
action unless the task explicitly requests it. Name anything unverified.`;
}

async function runNative(task: string): Promise<void> {
  const runtime = new NativeDesktopTools();
  try {
    await runtime.start();
    await runAgent(
      {
        model: process.env.CLAUDE_MODEL,
        tools: [],
        mcpServers: { cua_native: nativeServer(runtime) },
        strictMcpConfig: true,
        allowedTools: [
          'mcp__cua_native__observe_desktop',
          'mcp__cua_native__click_desktop',
          'mcp__cua_native__type_text',
          'mcp__cua_native__press_key',
        ],
        permissionMode: 'dontAsk',
        maxTurns: 40,
      },
      taskPrompt(task, 'native')
    );
  } finally {
    await runtime.close();
  }
}

async function runMcp(task: string): Promise<void> {
  const binary = process.env.CUA_DRIVER_BIN ?? 'cua-driver';
  const scope = captureScope();
  const session = `claude-mcp-${randomUUID().slice(0, 12)}`;
  let started = false;
  try {
    await driverCall(binary, 'start_session', { session, capture_scope: scope });
    started = true;
    await runAgent(
      {
        model: process.env.CLAUDE_MODEL,
        tools: [],
        mcpServers: {
          cua_driver: { type: 'stdio', command: binary, args: ['mcp'] },
        },
        strictMcpConfig: true,
        // A bare MCP server name enables all of its tools. Claude does not
        // support wildcard tool-name globs in this allowlist.
        allowedTools: ['mcp__cua_driver'],
        permissionMode: 'dontAsk',
        maxTurns: 40,
      },
      taskPrompt(task, 'mcp', session)
    );
  } finally {
    if (started) await driverCall(binary, 'end_session', { session });
  }
}

const args = argumentsFromCli();
if (args.route === 'native') {
  await runNative(args.task);
} else {
  await runMcp(args.task);
}
