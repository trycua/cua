import { appendFile, writeFile } from 'node:fs/promises';
import process from 'node:process';
import { randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { choice, TypeSafeClient } from '@typesafe-ai/sdk';

import {
  buildCandidates,
  chooseMock,
  classify,
  validateChoice,
  type BrowserSnapshot,
  type Candidate,
  type Outcome,
} from './core.js';

type Arguments = {
  provider: 'mock' | 'live';
  fixtureUrl: string;
  token?: string;
  maxSteps: number;
  dryRun: boolean;
  log?: string;
};

function parseArgs(argv: string[]): Arguments {
  const result: Arguments = {
    provider: 'mock',
    fixtureUrl: 'http://127.0.0.1:8765/',
    maxSteps: 4,
    dryRun: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === '--provider') result.provider = argv[++index] as Arguments['provider'];
    else if (value === '--fixture-url') result.fixtureUrl = argv[++index];
    else if (value === '--token') result.token = argv[++index];
    else if (value === '--max-steps') result.maxSteps = Number(argv[++index]);
    else if (value === '--dry-run') result.dryRun = true;
    else if (value === '--log') result.log = argv[++index];
    else throw new Error(`unknown argument: ${value}`);
  }
  if (!Number.isInteger(result.maxSteps) || result.maxSteps < 1) {
    throw new Error('--max-steps must be a positive integer');
  }
  return result;
}

class Driver {
  constructor(
    private readonly client: Client,
    private readonly session: string
  ) {}

  async call(name: string, args: Record<string, unknown>): Promise<Record<string, any>> {
    const result = await this.client.callTool({
      name,
      arguments: { ...args, session: this.session },
    });
    if (result.isError) throw new Error(`${name} failed: ${JSON.stringify(result.content)}`);
    const data = result.structuredContent as Record<string, any> | undefined;
    if (!data) throw new Error(`${name} returned no structured result`);
    if (data.status === 'refused' || data.refusal) {
      throw new Error(`${name} refused: ${JSON.stringify(data.refusal ?? data)}`);
    }
    return data;
  }
}

async function fixtureState(fixtureUrl: string): Promise<{ submitted: string | null }> {
  const response = await fetch(new URL('state', fixtureUrl));
  if (!response.ok) throw new Error(`fixture state failed: HTTP ${response.status}`);
  return (await response.json()) as { submitted: string | null };
}

async function resetFixture(fixtureUrl: string): Promise<void> {
  const response = await fetch(new URL('reset', fixtureUrl), { method: 'POST' });
  if (response.status !== 204) throw new Error(`fixture reset failed: HTTP ${response.status}`);
}

async function waitForWindow(driver: Driver, pid: number) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const response = await driver.call('list_windows', { pid });
    const visible = (response.windows as Record<string, any>[]).filter(
      (window) => window.is_on_screen
    );
    if (visible.length) {
      return visible.sort(
        (left, right) =>
          right.bounds.width * right.bounds.height - left.bounds.width * left.bounds.height
      )[0];
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error('isolated browser window did not become ready');
}

export async function chooseWithTypeSafe(
  client: Pick<TypeSafeClient, 'systemOne'>,
  candidates: Candidate[],
  snapshot: BrowserSnapshot,
  history: Record<string, unknown>[]
) {
  const criteria = Object.fromEntries(
    candidates.map((candidate) => [candidate.id, candidate.description])
  );
  const response = await client.systemOne({
    state: {
      goal: 'Enter the verification token, then submit the form.',
      observation: {
        page: JSON.stringify(snapshot.page ?? null),
        outline: snapshot.outline ?? '',
      },
      history: JSON.stringify(history),
    },
    questions: {
      driver_action: choice(
        'Which complete executable action should Cua Driver run next?',
        criteria
      ),
    },
  });
  const answer = response.answers.driver_action;
  if (answer.type !== 'choice') throw new Error('Jev returned the wrong answer type');
  return answer;
}

async function chooseLive(
  candidates: Candidate[],
  snapshot: BrowserSnapshot,
  history: Record<string, unknown>[]
) {
  return chooseWithTypeSafe(new TypeSafeClient(), candidates, snapshot, history);
}

async function writeEvent(path: string | undefined, event: Record<string, unknown>) {
  const line = JSON.stringify(event);
  console.log(line);
  if (path) await appendFile(path, `${line}\n`, 'utf8');
}

async function run(args: Arguments): Promise<Outcome> {
  const token = args.token ?? `jev-${randomUUID().replaceAll('-', '').slice(0, 10)}`;
  const transport = new StdioClientTransport({
    command: process.env.CUA_DRIVER_BIN ?? 'cua-driver',
    args: ['mcp'],
  });
  const client = new Client({ name: 'cua-driver-typesafe-jev-example', version: '0.1.0' });
  const history: Record<string, unknown>[] = [];
  if (args.log) await writeFile(args.log, '', 'utf8');
  await resetFixture(args.fixtureUrl);

  try {
    await client.connect(transport);
    const driver = new Driver(client, `jev-typescript-${randomUUID().slice(0, 8)}`);
    const prepared = await driver.call('browser_prepare', {
      allow_launch: true,
      profile: { mode: 'isolated_new' },
    });
    const pid = Number(prepared.prepared_pid);
    const window = await waitForWindow(driver, pid);
    const bound = await driver.call('get_browser_state', {
      pid,
      window_id: window.window_id,
    });
    const targetId = String(bound.target_id);
    const activeTab = (bound.tabs as Record<string, any>[]).find((tab) => tab.active);
    if (!activeTab) throw new Error('isolated browser has no active tab');
    const tabId = String(activeTab.tab_id);
    await driver.call('browser_navigate', {
      target_id: targetId,
      tab_id: tabId,
      url: args.fixtureUrl,
    });

    for (let step = 1; step <= args.maxSteps; step += 1) {
      const oracle = await fixtureState(args.fixtureUrl);
      const current = classify(oracle.submitted, token, step - 1, args.maxSteps);
      if (current === 'verified' || current === 'refuted') {
        await writeEvent(args.log, { event: 'outcome', outcome: current, token });
        return current;
      }

      const decisionStarted = performance.now();
      const snapshot = (await driver.call('get_browser_state', {
        target_id: targetId,
        tab_id: tabId,
        snapshot_format: 'semantic_v2',
      })) as BrowserSnapshot;
      const candidates = buildCandidates(snapshot, token);
      if (!candidates.length) {
        await writeEvent(args.log, { event: 'outcome', outcome: 'abstained', step });
        return 'abstained';
      }
      const answer =
        args.provider === 'mock'
          ? chooseMock(candidates)
          : await chooseLive(candidates, snapshot, history);
      if (!answer.choice) return 'abstained';
      const candidate = validateChoice(answer.choice, candidates);
      const decisionMs = Math.round((performance.now() - decisionStarted) * 100) / 100;

      if (candidate.id === 'abstain') {
        await writeEvent(args.log, {
          event: 'outcome',
          outcome: 'abstained',
          step,
          confidence: answer.confidence,
          probabilities: answer.probabilities,
        });
        return 'abstained';
      }

      let actionMs = 0;
      if (!args.dryRun) {
        const actionStarted = performance.now();
        try {
          if (!candidate.tool) throw new Error('selected candidate has no executable tool');
          await driver.call(candidate.tool, candidate.arguments);
        } catch (error: unknown) {
          await writeEvent(args.log, {
            event: 'outcome',
            outcome: 'unknown',
            step,
            phase: 'action',
            error: error instanceof Error ? error.name : 'UnknownError',
          });
          return 'unknown';
        }
        actionMs = Math.round((performance.now() - actionStarted) * 100) / 100;
      }
      const event = {
        event: 'step',
        step,
        candidate: candidate.id,
        confidence: answer.confidence,
        probabilities: answer.probabilities,
        decision_ms: decisionMs,
        action_ms: actionMs,
        dry_run: args.dryRun,
      };
      history.push(event);
      await writeEvent(args.log, event);
      if (args.dryRun) return 'unknown';
      if (candidate.id === 'submit-form') {
        for (let attempt = 0; attempt < 20; attempt += 1) {
          const oracle = await fixtureState(args.fixtureUrl);
          const outcome = classify(oracle.submitted, token, step, args.maxSteps);
          if (outcome === 'verified' || outcome === 'refuted') {
            await writeEvent(args.log, { event: 'outcome', outcome, token });
            return outcome;
          }
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }
    }

    const oracle = await fixtureState(args.fixtureUrl);
    const outcome = classify(oracle.submitted, token, args.maxSteps, args.maxSteps);
    await writeEvent(args.log, { event: 'outcome', outcome, token });
    return outcome;
  } finally {
    await client.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = parseArgs(process.argv.slice(2));
  run(args)
    .then((outcome) => {
      process.exitCode = outcome === 'verified' || args.dryRun ? 0 : 1;
    })
    .catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : error);
      process.exitCode = 1;
    });
}
