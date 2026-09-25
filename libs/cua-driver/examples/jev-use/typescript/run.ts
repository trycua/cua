import { appendFile, writeFile } from 'node:fs/promises';
import process from 'node:process';
import { randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import {
  buildCandidates,
  classify,
  parseVisualRegions,
  validateChoice,
  VisualObservationError,
  type BrowserSnapshot,
  type Outcome,
  type VisualObservation,
} from './core.js';
import { driverEnvironment } from './driver_env.js';
import { chooseLive, chooseMockAdapter } from './jev_adapter.js';

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
  result.fixtureUrl = validateFixtureUrl(result.fixtureUrl);
  return result;
}

export function validateFixtureUrl(value: string): string {
  const parsed = new URL(value);
  const loopback = new Set(['127.0.0.1', 'localhost', '[::1]']);
  if (
    parsed.protocol !== 'http:' ||
    !loopback.has(parsed.hostname) ||
    parsed.username ||
    parsed.password ||
    (parsed.pathname !== '' && parsed.pathname !== '/') ||
    parsed.search ||
    parsed.hash
  ) {
    throw new Error('fixture URL must be an HTTP loopback origin such as http://127.0.0.1:8765/');
  }
  parsed.pathname = '/';
  return parsed.toString();
}

export function selectTabId(tabs: Record<string, any>[]): string {
  if (!tabs.length) throw new Error('isolated browser has no tabs');
  return String(tabs.find((tab) => tab.active)?.tab_id ?? tabs[0].tab_id);
}

export class DriverToolError extends Error {
  constructor(
    message: string,
    readonly code?: string
  ) {
    super(message);
    this.name = 'DriverToolError';
  }
}

export class Driver {
  constructor(
    private readonly client: Client,
    private readonly session: string
  ) {}

  async call(name: string, args: Record<string, unknown>): Promise<Record<string, any>> {
    const result = await this.client.callTool({
      name,
      arguments: { ...args, session: this.session },
    });
    if (result.isError) {
      const structured = result.structuredContent as Record<string, unknown> | undefined;
      const code = typeof structured?.code === 'string' && structured.code ? structured.code : undefined;
      throw new DriverToolError(`${name} failed: ${JSON.stringify(result.content)}`, code);
    }
    const data = result.structuredContent as Record<string, any> | undefined;
    if (!data) throw new Error(`${name} returned no structured result`);
    if (data.status === 'refused' || data.refusal) {
      throw new Error(`${name} refused: ${JSON.stringify(data.refusal ?? data)}`);
    }
    return data;
  }
}

export function supportsCaptureBoundClick(
  tools: readonly { name: string; inputSchema?: Record<string, any> }[]
): boolean {
  const click = tools.find((tool) => tool.name === 'click');
  return Boolean(click?.inputSchema?.properties?.capture_id);
}

export type VisualStatus = {
  status: 'ok' | 'not_installed' | 'error' | 'unavailable';
  error_code?: string;
  capture_id?: string;
  region_count?: number;
};

/**
 * Build the redacted per-step visual record written to the JSONL log. It never
 * contains screenshots, screenshot references, region text, or secrets.
 */
export function visualStatus(
  status: VisualStatus['status'],
  errorCode?: string,
  visual?: VisualObservation
): VisualStatus {
  const result: VisualStatus = { status };
  if (errorCode) result.error_code = errorCode;
  if (visual) {
    result.capture_id = visual.captureId;
    result.region_count = visual.regions.length;
  }
  return result;
}

/**
 * Take an optional visual observation and report what happened. Failures never
 * stop the run: the caller continues on the page-structure path, but the
 * returned status makes the fallback observable.
 */
export async function observeVisual(
  driver: Driver,
  pid: number,
  windowId: number,
  availableTools: ReadonlySet<string>,
  captureBoundClick: boolean
): Promise<{ visual?: VisualObservation; status: VisualStatus }> {
  if (!captureBoundClick) {
    return { status: visualStatus('unavailable', 'capture_bound_click_unsupported') };
  }
  if (!availableTools.has('get_window_state') || !availableTools.has('parse_visual_regions')) {
    return { status: visualStatus('unavailable', 'tool_not_advertised') };
  }
  try {
    const capture = await driver.call('get_window_state', {
      pid,
      window_id: windowId,
      include_accessibility_tree: false,
    });
    if (typeof capture.capture_id !== 'string') {
      return { status: visualStatus('error', 'capture_missing') };
    }
    const result = await driver.call('parse_visual_regions', {
      capture_id: capture.capture_id,
      options: { kinds: ['text', 'icon'], min_confidence: 0.8, max_regions: 100 },
    });
    const visual = parseVisualRegions(result, capture.capture_id, pid, windowId);
    return { visual, status: visualStatus('ok', undefined, visual) };
  } catch (error: unknown) {
    if (error instanceof DriverToolError) {
      if (error.code === 'not_installed') {
        return { status: visualStatus('not_installed', 'not_installed') };
      }
      return { status: visualStatus('error', error.code ?? 'driver_error') };
    }
    if (error instanceof VisualObservationError) {
      return { status: visualStatus('error', error.code) };
    }
    // parseVisualRegions reports other contract violations as plain errors.
    if (error instanceof Error && /^(visual (result|region)|unsupported visual)/.test(error.message)) {
      return { status: visualStatus('error', 'invalid_visual_result') };
    }
    return { status: visualStatus('error', 'driver_error') };
  }
}

export async function optionalVisualObservation(
  driver: Driver,
  pid: number,
  windowId: number,
  availableTools: ReadonlySet<string>,
  captureBoundClick: boolean
): Promise<VisualObservation | undefined> {
  return (await observeVisual(driver, pid, windowId, availableTools, captureBoundClick)).visual;
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
    env: driverEnvironment(),
  });
  const client = new Client({ name: 'cua-driver-jev-use-example', version: '0.1.0' });
  const history: Record<string, unknown>[] = [];
  if (args.log) await writeFile(args.log, '', 'utf8');
  await resetFixture(args.fixtureUrl);

  try {
    await client.connect(transport);
    const advertisedTools = (await client.listTools()).tools;
    const availableTools = new Set(advertisedTools.map((tool) => tool.name));
    const captureBoundClick = supportsCaptureBoundClick(advertisedTools);
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
    const tabId = selectTabId(bound.tabs as Record<string, any>[]);
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
      const { visual, status: visualRecord } = await observeVisual(
        driver,
        pid,
        Number(window.window_id),
        availableTools,
        captureBoundClick
      );
      const candidates = buildCandidates(snapshot, token, visual, captureBoundClick);
      if (!candidates.length) {
        await writeEvent(args.log, {
          event: 'outcome',
          outcome: 'abstained',
          step,
          visual: visualRecord,
        });
        return 'abstained';
      }
      const answer =
        args.provider === 'mock'
          ? chooseMockAdapter(candidates, snapshot, visual, history)
          : await chooseLive(candidates, snapshot, visual, history);
      if (!answer.choice) return 'abstained';
      const candidate = validateChoice(answer.choice, candidates, visual?.captureId);
      const decisionMs = Math.round((performance.now() - decisionStarted) * 100) / 100;

      if (candidate.id === 'reobserve') {
        const event = {
          event: 'step',
          step,
          candidate: candidate.id,
          confidence: answer.confidence,
          probabilities: answer.probabilities,
          decision_ms: decisionMs,
          action_ms: 0,
          dry_run: args.dryRun,
          tool: null,
          visual: visualRecord,
        };
        history.push(event);
        await writeEvent(args.log, event);
        continue;
      }

      if (candidate.id === 'abstain') {
        await writeEvent(args.log, {
          event: 'outcome',
          outcome: 'abstained',
          step,
          confidence: answer.confidence,
          probabilities: answer.probabilities,
          visual: visualRecord,
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
            tool: candidate.tool,
            visual: visualRecord,
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
        tool: candidate.tool,
        visual: visualRecord,
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
