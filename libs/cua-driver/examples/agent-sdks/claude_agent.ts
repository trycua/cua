/** Run a Claude Agent SDK desktop task through native Cua tools or MCP. */

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

function taskPrompt(task: string, route: Route): string {
  const lifecycle =
    route === 'mcp'
      ? 'The MCP transport owns one implicit lifecycle session. Omit the session field for ordinary calls so the runtime creates and reuses it.'
      : 'The native SDK client owns one implicit lifecycle session; custom tools do not expose lifecycle.';
  return `Complete this trusted desktop task through Cua Driver:

${task}

Route: ${route}. ${lifecycle} Select an exact window or desktop target for every
action; session identity never selects capture modality or permission authority.
Use only supplied Cua tools for desktop observation and interaction. Inspect
before each action and verify afterward. If a mutation times out, observe before
any retry; never blindly replay an action with an unknown outcome. Do not
purchase, send, delete, expose credentials, or perform another irreversible
action unless the task explicitly requests it. Name anything unverified.`;
}

async function runNative(task: string): Promise<void> {
  const runtime = new NativeDesktopTools();
  try {
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
    taskPrompt(task, 'mcp')
  );
}

const args = argumentsFromCli();
if (args.route === 'native') {
  await runNative(args.task);
} else {
  await runMcp(args.task);
}
