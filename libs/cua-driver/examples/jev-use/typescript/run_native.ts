/**
 * Run one native jev-use task against a desktop window (RFC #4268, Phase 1).
 * Mirrors python/run_native.py: attach to --pid, find the task window, and
 * loop oracle -> one get_window_state (tree and screenshot) -> closed
 * candidates -> validated cua.jev_choice_request_v2 -> one element-bound
 * action -> oracle.
 */
import { appendFile, writeFile } from 'node:fs/promises';
import process from 'node:process';
import { randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { TypeSafeClient } from '@typesafe-ai/sdk';

import { providerObservation, validateRequest } from './choose_action.js';
import { parseVisualRegions, validateChoice, VisualObservationError, type Candidate } from './core.js';
import { driverEnvironment } from './driver_env.js';
import { chooseBoundedWithTypeSafe, chooseMockForTask } from './jev_adapter.js';
import { parseWindowState, type NativeObservation } from './native.js';
import type { Platform } from './native_roles.js';
import {
  NATIVE_TASK_IDS,
  nativeTask,
  nativeChoiceRequest,
  visualFallbackReason,
  windowStateArguments,
  type NativeTask,
} from './native_tasks.js';
import { chooseS1Service, S1ServiceError, s1ServiceUrl } from './s1_service.js';
import { backgroundRefusalCode, Driver, DriverToolError, supportsCaptureBoundClick } from './run.js';
import { NativeAccessibilitySource, VisualRegionSource } from './sources.js';
import type { HistoryEntry, Outcome, TaskSources } from './tasks.js';

const STALE_TOKEN_CODES = new Set(['stale_element_token']);
const REOBSERVE_TIMEOUT_MS = 5_000;

type Arguments = {
  task: string;
  provider: 'mock' | 'live' | 's1';
  pid: number;
  stateFile: string;
  noteText?: string;
  allowForeground: boolean;
  platform?: Platform;
  log?: string;
};

export function hostPlatform(): Platform {
  if (process.platform === 'darwin') return 'macos';
  if (process.platform === 'win32') return 'windows';
  return 'linux';
}

export function parseArgs(argv: string[]): Arguments {
  const result: Partial<Arguments> = { provider: 'mock', allowForeground: false };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === '--task') result.task = argv[++index];
    else if (value === '--provider') result.provider = argv[++index] as Arguments['provider'];
    else if (value === '--pid') result.pid = Number(argv[++index]);
    else if (value === '--state-file') result.stateFile = argv[++index];
    else if (value === '--note-text') result.noteText = argv[++index];
    else if (value === '--allow-foreground') result.allowForeground = true;
    else if (value === '--platform') result.platform = argv[++index] as Platform;
    else if (value === '--log') result.log = argv[++index];
    else throw new Error(`unknown argument: ${value}`);
  }
  if (!result.task || !NATIVE_TASK_IDS.includes(result.task)) {
    throw new Error(`--task must be one of ${NATIVE_TASK_IDS.join(', ')}`);
  }
  if (result.provider !== 'mock' && result.provider !== 'live' && result.provider !== 's1') {
    throw new Error('--provider must be mock, live, or s1');
  }
  if (!Number.isInteger(result.pid) || (result.pid as number) <= 0) throw new Error('--pid is required');
  if (!result.stateFile) throw new Error('--state-file is required');
  if (result.platform && !['macos', 'windows', 'linux'].includes(result.platform)) {
    throw new Error('--platform must be macos, windows, or linux');
  }
  return result as Arguments;
}

export function isStaleTokenError(error: unknown): boolean {
  if (error instanceof DriverToolError && error.code && STALE_TOKEN_CODES.has(error.code)) return true;
  return String(error instanceof Error ? error.message : error).includes('element_token is stale');
}

/** Refuse any action addressed outside the task's one window. */
export function assertInScope(candidate: Candidate, pid: number, windowId: number): void {
  if (candidate.tool === null) return;
  if (candidate.arguments.pid !== pid || candidate.arguments.window_id !== windowId) {
    throw new Error('candidate addresses a window outside the task scope');
  }
}

async function writeEvent(path: string | undefined, event: Record<string, unknown>) {
  const line = JSON.stringify(event);
  console.log(line);
  if (path) await appendFile(path, `${line}\n`, 'utf8');
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function findWindow(driver: Driver, pid: number, title: string) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const windows = ((await driver.call('list_windows', { pid })).windows ?? []) as Record<string, any>[];
    const match = windows.find((window) => window.title === title && window.is_on_screen !== false);
    if (match) return match;
    await sleep(250);
  }
  throw new Error(`window ${JSON.stringify(title)} of pid ${pid} did not appear`);
}

async function observe(
  driver: Driver,
  task: NativeTask,
  pid: number,
  windowId: number,
  timeoutMs?: number
): Promise<NativeObservation> {
  const payload = await driver.call('get_window_state', {
    pid,
    window_id: windowId,
    include_accessibility_tree: true,
    include_screenshot: true,
    ...windowStateArguments(task.scope),
    ...(timeoutMs !== undefined ? { timeout_ms: timeoutMs } : {}),
  });
  return parseWindowState(payload, pid, windowId);
}

function nativeCount(task: NativeTask, sources: TaskSources): number {
  return task.plan(sources).candidates.filter((candidate) => candidate.source === 'ax').length;
}

async function observeStep(
  driver: Driver,
  task: NativeTask,
  pid: number,
  windowId: number,
  platform: Platform,
  foregroundIds: ReadonlySet<string>
): Promise<{ sources: TaskSources; record: Record<string, unknown> }> {
  const started = performance.now();
  const build = (observation: NativeObservation): TaskSources => ({
    ax: NativeAccessibilitySource.fromObservation(observation, platform, {
      redact: task.redactText,
      textMethod: task.textMethod,
    }),
    visualPath: false,
    foregroundIds,
  });
  let sources = build(await observe(driver, task, pid, windowId));
  let record: Record<string, unknown> = { reobserved: false };
  const first = sources.ax!.observation;
  if (first.truncated || (!first.complete && nativeCount(task, sources) === 0)) {
    record = { reobserved: true, reason: first.truncated ? 'truncated' : 'partial_empty' };
    sources = build(await observe(driver, task, pid, windowId, REOBSERVE_TIMEOUT_MS));
  }
  const observation = sources.ax!.observation;
  Object.assign(record, {
    snapshot_id: observation.snapshotId ?? null,
    capture_id: observation.captureId ?? null,
    elements_complete: observation.complete,
    truncated: observation.truncated,
    controls: sources.ax!.controls.length,
    excluded: sources.ax!.native.excluded,
    observe_ms: Math.round((performance.now() - started) * 100) / 100,
  });
  return { sources, record };
}

async function maybeVisual(
  driver: Driver,
  task: NativeTask,
  sources: TaskSources,
  availableTools: ReadonlySet<string>,
  captureBoundClick: boolean
): Promise<{ sources: TaskSources; record: Record<string, unknown> }> {
  const reason = visualFallbackReason(sources, task, nativeCount(task, sources));
  if (!reason) return { sources, record: { status: 'skipped' } };
  if (!captureBoundClick || !availableTools.has('parse_visual_regions')) {
    return { sources, record: { status: 'unavailable', reason } };
  }
  const observation = sources.ax!.observation;
  const started = performance.now();
  try {
    const result = await driver.call('parse_visual_regions', {
      capture_id: observation.captureId,
      options: { kinds: ['text', 'icon'], min_confidence: task.visualMinConfidence, max_regions: 100 },
    });
    const visual = parseVisualRegions(result, observation.captureId!, observation.pid, observation.windowId);
    return {
      sources: {
        ...sources,
        visual: new VisualRegionSource(visual, 'background', captureBoundClick, task.visualMinConfidence),
        visualPath: true,
      },
      record: {
        status: 'ok',
        reason,
        region_count: visual.regions.length,
        parse_ms: Math.round((performance.now() - started) * 100) / 100,
      },
    };
  } catch (error) {
    if (error instanceof DriverToolError) {
      return { sources, record: { status: 'error', reason, error_code: error.code ?? 'driver_error' } };
    }
    if (error instanceof VisualObservationError) {
      return { sources, record: { status: 'error', reason, error_code: error.code } };
    }
    throw error;
  }
}

async function chooseLive(request: Record<string, unknown>) {
  const validated = validateRequest(request);
  const criteria = Object.fromEntries(validated.candidates.map(({ id, description }) => [id, description]));
  const result = await chooseBoundedWithTypeSafe(
    new TypeSafeClient(),
    validated.goal,
    providerObservation(validated),
    criteria
  );
  return { choice: result.selectedId, confidence: result.confidence, probabilities: result.probabilities };
}

async function pollOracle(task: NativeTask, steps: number): Promise<Outcome> {
  let outcome: Outcome = 'unknown';
  for (let attempt = 0; attempt < 20; attempt += 1) {
    outcome = task.classify(await task.readOracle(), steps);
    if (outcome === 'verified' || outcome === 'refuted') return outcome;
    await sleep(100);
  }
  return outcome;
}

export async function runTask(args: Arguments, task: NativeTask): Promise<Outcome> {
  const platform = args.platform ?? hostPlatform();
  if (args.log) await writeFile(args.log, '', 'utf8');
  const history: HistoryEntry[] = [];
  const foregroundIds = new Set<string>();
  const transport = new StdioClientTransport({
    command: process.env.CUA_DRIVER_BIN ?? 'cua-driver',
    args: ['mcp'],
    env: driverEnvironment(),
  });
  const client = new Client({ name: 'cua-driver-jev-use-native', version: '0.1.0' });
  try {
    await client.connect(transport);
    const advertised = (await client.listTools()).tools;
    const availableTools = new Set(advertised.map((tool) => tool.name));
    const captureBoundClick = supportsCaptureBoundClick(advertised);
    const driver = new Driver(client, `jev-native-typescript-${randomUUID().slice(0, 8)}`);
    const window = await findWindow(driver, args.pid, task.scope.windowTitle);
    const windowId = Number(window.window_id);
    await writeEvent(args.log, {
      event: 'start', task: task.id, language: 'typescript', provider: args.provider,
      platform, pid: args.pid, window_id: windowId,
    });
    for (let step = 1; step <= task.maxSteps; step += 1) {
      const current = task.classify(await task.readOracle(), step - 1);
      if (current === 'verified' || current === 'refuted') {
        await writeEvent(args.log, { event: 'outcome', outcome: current, step: step - 1 });
        return current;
      }
      const observed = await observeStep(driver, task, args.pid, windowId, platform, new Set(foregroundIds));
      const visual = await maybeVisual(driver, task, observed.sources, availableTools, captureBoundClick);
      const sources = visual.sources;
      const plan = task.plan(sources);
      const request = nativeChoiceRequest(task, sources, plan, history);
      validateRequest(request);

      const decideStarted = performance.now();
      let decision: { choice: string | null; confidence: number; probabilities: Readonly<Record<string, number>> };
      try {
        decision =
          args.provider === 'mock'
            ? chooseMockForTask(task, sources, plan.candidates, history)
            : args.provider === 's1'
              ? await chooseS1Service(request)
              : await chooseLive(request);
      } catch (error) {
        if (!(error instanceof S1ServiceError)) throw error;
        // A tied or malformed score is not an action; fail closed with a
        // logged outcome instead of a stack trace.
        await writeEvent(args.log, {
          event: 'outcome', outcome: 'unknown', phase: 'decide', step,
          candidate_count: plan.candidates.length, expected_ids: task.expectedNext(history),
          error: 'S1ServiceError', reason: error.message.slice(0, 128),
        });
        return 'unknown';
      }
      // Measurement (#4312): the declared steps due now, and whether the
      // candidate set offered one. IDs only; no values.
      const expectedIds = task.expectedNext(history);
      const offered = new Set(plan.candidates.map((candidate) => candidate.id.replace(/:foreground$/, '')));
      const baseEvent: Record<string, unknown> = {
        event: 'step',
        step,
        observation: observed.record,
        visual: visual.record,
        compose: plan.stats,
        candidate_count: plan.candidates.length,
        schema: request.schema,
        confidence: decision.confidence,
        probabilities: decision.probabilities,
        decide_ms: Math.round((performance.now() - decideStarted) * 100) / 100,
        expected_ids: expectedIds,
        expected_offered: expectedIds.some((id) => offered.has(id)),
      };
      if (!decision.choice) {
        await writeEvent(args.log, { ...baseEvent, event: 'outcome', outcome: 'abstained' });
        return 'abstained';
      }
      const candidate = validateChoice(decision.choice, plan.candidates, sources.visual?.observation.captureId);
      assertInScope(candidate, args.pid, windowId);
      baseEvent.candidate = candidate.id;
      baseEvent.source = candidate.source ?? null;

      if (candidate.id === 'reobserve') {
        history.push(task.historyEntry(step, candidate.id));
        await writeEvent(args.log, { ...baseEvent, tool: null, act_ms: 0 });
        continue;
      }
      if (candidate.id === 'abstain') {
        await writeEvent(args.log, { ...baseEvent, event: 'outcome', outcome: 'abstained' });
        return 'abstained';
      }
      const actStarted = performance.now();
      try {
        await driver.call(candidate.tool!, { ...candidate.arguments });
      } catch (error) {
        const actMs = Math.round((performance.now() - actStarted) * 100) / 100;
        if (isStaleTokenError(error)) {
          history.push(task.historyEntry(step, candidate.id, undefined, { stale: true }));
          await writeEvent(args.log, { ...baseEvent, tool: candidate.tool, act_ms: actMs, action_error: 'stale_element_token' });
          continue;
        }
        const refusal = backgroundRefusalCode(candidate, error);
        if (refusal) {
          foregroundIds.add(candidate.id);
          history.push(task.historyEntry(step, candidate.id, refusal));
          await writeEvent(args.log, {
            ...baseEvent, tool: candidate.tool, act_ms: actMs, action_error: refusal,
            escalation: { from: 'background', to: 'foreground', allowed: task.allowForeground },
          });
          continue;
        }
        await writeEvent(args.log, {
          ...baseEvent, event: 'outcome', outcome: 'unknown', phase: 'action',
          error: error instanceof Error ? error.name : 'Error', tool: candidate.tool,
        });
        return 'unknown';
      }
      const actMs = Math.round((performance.now() - actStarted) * 100) / 100;
      history.push(task.historyEntry(step, candidate.id, undefined, { outcome: plan.outcomes[candidate.id] }));
      await writeEvent(args.log, {
        ...baseEvent, tool: candidate.tool, act_ms: actMs,
        delivery_mode: candidate.arguments.delivery_mode ?? null,
      });
      const outcome = await pollOracle(task, step);
      if (outcome === 'verified' || outcome === 'refuted') {
        await writeEvent(args.log, { event: 'outcome', outcome, step });
        return outcome;
      }
    }
    const outcome = task.classify(await task.readOracle(), task.maxSteps);
    await writeEvent(args.log, { event: 'outcome', outcome, step: task.maxSteps });
    return outcome;
  } finally {
    await client.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = parseArgs(process.argv.slice(2));
  // Fail before touching the app when the S1 service is not configured.
  if (args.provider === 's1') s1ServiceUrl();
  const task = nativeTask(args.task, args.stateFile, {
    pid: args.pid,
    noteText: args.noteText,
    allowForeground: args.allowForeground,
  });
  runTask(args, task)
    .then((outcome) => {
      process.exitCode = outcome === 'verified' ? 0 : 1;
    })
    .catch(async (error: unknown) => {
      await writeEvent(args.log, {
        event: 'outcome', outcome: 'unknown', error: error instanceof Error ? error.name : 'Error',
      });
      process.exitCode = 1;
    });
}
